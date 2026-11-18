//! Moteur 3D. La **file de commandes** (3D-1 : FIFO + PIPE, format compact,
//! ports, GXSTAT, IRQ, durées, arrêt sur SwapBuffers) alimente le **moteur
//! de géométrie** (3D-2, `geometry.rs` : matrices, sommets, polygones,
//! découpage, lumière, tests). Le rendu des polygones (3D-3) n'existe pas
//! encore : le calque 3D reste noir.
//!
//! Spécification et hypothèses : `docs/3D_SPEC.md`.
//!
//! **Temps.** Le moteur a sa propre horloge (`t_engine`, en cycles ARM9) :
//! l'instant où il sera libre. Il rattrape l'horloge de la machine à chaque
//! accès aux registres 3D et à chaque HBlank (`catch_up`). Une commande
//! s'exécute quand tous ses paramètres sont arrivés, puis occupe le moteur
//! le temps indiqué par la table.

pub mod commands;
pub mod fifo;
pub mod geometry;
pub mod math;
pub mod polygon;
pub mod render;
pub mod texture;

use fifo::{CommandFifo, Entry, Unpacker, HALF};
use geometry::Geometry;
use render::{RenderParams, Renderer};

use super::scheduler::BUS_CYCLE;

/// Événement du journal des commandes (diagnostic, désactivé par défaut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GxEvent {
    /// Mot écrit dans GXFIFO, tel quel.
    Word(u32),
    /// Écriture dans le port d'une commande.
    Port(u8, u32),
    /// Commande exécutée, avec son premier paramètre.
    Exec(u8, u32),
    /// Début du VBlank (échange éventuel).
    VBlank { swapped: bool },
}

/// Statistiques d'une image (remises à zéro à chaque VBlank).
#[derive(Clone, Debug)]
pub struct FrameStats {
    /// Commandes exécutées, par code.
    pub commands: [u32; 0x80],
    /// Mots écrits dans GXFIFO.
    pub gxfifo_words: u32,
    /// Écritures dans les ports de commande.
    pub port_writes: u32,
    /// Entrées arrivées file pleine, moteur arrêté (gardées en réserve).
    pub stalls: u32,
    /// Commandes dont les paramètres ont été interrompus par une autre.
    pub broken: u32,
    /// SwapBuffers pris en compte au VBlank.
    pub swaps: u32,
}

impl Default for FrameStats {
    fn default() -> Self {
        FrameStats { commands: [0; 0x80], gxfifo_words: 0, port_writes: 0, stalls: 0, broken: 0, swaps: 0 }
    }
}

impl FrameStats {
    pub fn total_commands(&self) -> u32 {
        self.commands.iter().sum()
    }

    fn add(&mut self, o: &FrameStats) {
        for (a, b) in self.commands.iter_mut().zip(o.commands.iter()) {
            *a += b;
        }
        self.gxfifo_words += o.gxfifo_words;
        self.port_writes += o.port_writes;
        self.stalls += o.stalls;
        self.broken += o.broken;
        self.swaps += o.swaps;
    }
}

#[derive(Clone, Debug)]
pub struct Gpu3d {
    pub fifo: CommandFifo,
    pub unpack: Unpacker,
    /// Commande en cours d'assemblage : code et paramètres reçus.
    cur: u8,
    params: [u32; 32],
    have: usize,
    /// Instant (cycles ARM9) où le moteur sera libre.
    t_engine: u64,
    /// SwapBuffers exécuté : moteur arrêté jusqu'au prochain VBlank.
    swap_pending: bool,
    /// Bits 30–31 de GXSTAT : IRQ de la file.
    pub fifo_irq: u32,
    /// MTX_MODE courant (pour la durée des multiplications).
    mtx_mode: u8,
    /// Registres de rendu 0x04000320–0x040003BF, bruts (étapes 3D-3+).
    regs: [u8; 0xA0],
    /// Matrices, sommets, polygones (étape 3D-2).
    pub geo: Geometry,
    /// Fin du test en cours (BOX/POS/VEC_TEST) : GXSTAT bit 0.
    test_until: u64,
    /// Image en cours et dernière image complète.
    pub stats: FrameStats,
    pub last_frame: FrameStats,
    /// Cumul des images terminées (voir `totals`).
    done: FrameStats,
    /// Journal des commandes, rempli seulement si `Some` (diagnostic).
    pub log: Option<Vec<GxEvent>>,
    /// Image 3D, redessinée à chaque VBlank (étape 3D-3).
    pub renderer: Renderer,
    /// Entrées du dernier rendu : sans échange ni changement de registres
    /// ou de textures, l'image serait identique (jeux à 30 images/s : une
    /// liste pour deux VBlank).
    last_inputs: Option<Box<(RenderParams, texture::TexMem)>>,
    /// Rendus évités ainsi (diagnostic).
    pub renders_skipped: u64,
}

impl Default for Gpu3d {
    fn default() -> Self {
        Gpu3d {
            fifo: CommandFifo::default(),
            unpack: Unpacker::default(),
            cur: 0,
            params: [0; 32],
            have: 0,
            t_engine: 0,
            swap_pending: false,
            fifo_irq: 0,
            mtx_mode: 0,
            regs: [0; 0xA0],
            geo: Geometry::default(),
            test_until: 0,
            stats: FrameStats::default(),
            last_frame: FrameStats::default(),
            done: FrameStats::default(),
            log: None,
            renderer: Renderer::default(),
            last_inputs: None,
            renders_skipped: 0,
        }
    }
}

impl Gpu3d {
    /// Adresses ARM9 du moteur 3D (DISP3DCNT, en 0x04000060, reste au
    /// moteur 2D A qui le relit).
    pub fn handles(addr: u32) -> bool {
        (0x0400_0320..=0x0400_06A4).contains(&addr)
    }

    pub fn read(&mut self, addr: u32, now: u64) -> u32 {
        self.catch_up(now);
        match addr {
            0x0400_0600 => self.gxstat(now),
            0x0400_0604 => self.geo.ram_count(),
            // Rendu supposé toujours à l'heure : cache plein, 48 − 2.
            0x0400_0320 => 46,
            0x0400_0620..=0x0400_062F => self.geo.pos_result[((addr >> 2) & 3) as usize] as u32,
            0x0400_0630..=0x0400_0635 => {
                let r = self.geo.vec_result;
                if addr & 4 == 0 {
                    r[0] as u16 as u32 | (r[1] as u16 as u32) << 16
                } else {
                    r[2] as u16 as u32
                }
            }
            0x0400_0640..=0x0400_067F => self.geo.clip.at(((addr - 0x0400_0640) >> 2) as usize) as u32,
            0x0400_0680..=0x0400_06A3 => self.geo.vecmtx(((addr - 0x0400_0680) >> 2) as usize) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, val: u32, mask: u32, now: u64) {
        match addr {
            0x0400_0400..=0x0400_043F => {
                self.stats.gxfifo_words += 1;
                self.record(GxEvent::Word(val));
                let mut out = Vec::with_capacity(4);
                self.unpack.feed(val, &mut |e| out.push(e));
                for e in out {
                    self.push(e, now);
                }
            }
            0x0400_0440..=0x0400_05FF => {
                let cmd = ((addr - 0x0400_0400) >> 2) as u8;
                if commands::info(cmd).is_some() {
                    self.stats.port_writes += 1;
                    self.record(GxEvent::Port(cmd, val));
                    self.push(Entry { cmd, param: val }, now);
                }
            }
            0x0400_0600 => {
                if mask & 0xC000_0000 != 0 {
                    self.fifo_irq = (val >> 30) & 3;
                }
                if mask & val & (1 << 15) != 0 {
                    self.geo.ack_stack_error();
                }
            }
            0x0400_0610 => {
                if mask & 0xFFFF != 0 {
                    self.geo.disp_1dot_depth = val as u16 & 0x7FFF;
                }
            }
            0x0400_0320..=0x0400_03BF => {
                let base = (addr - 0x0400_0320) as usize;
                for k in 0..4 {
                    if mask & (0xFF << (k * 8)) != 0 && base + k < self.regs.len() {
                        self.regs[base + k] = (val >> (k * 8)) as u8;
                    }
                }
            }
            _ => {}
        }
    }

    /// Registre de rendu brut (0x04000320–0x040003BF), octet par octet.
    pub fn reg8(&self, addr: u32) -> u8 {
        self.regs.get(addr.wrapping_sub(0x0400_0320) as usize).copied().unwrap_or(0)
    }

    fn reg_word(&self, addr: u32) -> [u8; 4] {
        [0, 1, 2, 3].map(|k| self.reg8(addr + k))
    }

    fn push(&mut self, e: Entry, now: u64) {
        self.catch_up(now);
        if self.fifo.is_full() {
            // Sur la console, le CPU attendrait : on fait avancer le moteur
            // en avance sur l'horloge jusqu'à libérer une place.
            self.run_ahead();
        }
        if !self.fifo.push(e) {
            self.stats.stalls += 1;
            self.fifo.push_reserve(e);
        }
    }

    /// Exécute ce qui est exécutable jusqu'à l'instant `now`.
    pub fn catch_up(&mut self, now: u64) {
        while !self.swap_pending && self.t_engine <= now {
            match self.fifo.pop() {
                Some(e) => self.consume(e),
                None => {
                    // Rien à faire : le moteur attend depuis `now`.
                    self.t_engine = self.t_engine.max(now);
                    break;
                }
            }
        }
    }

    /// File pleine : exécute sans attendre l'horloge jusqu'à une place libre
    /// (ou l'arrêt sur SwapBuffers).
    fn run_ahead(&mut self) {
        while self.fifo.is_full() && !self.swap_pending {
            match self.fifo.pop() {
                Some(e) => self.consume(e),
                None => break,
            }
        }
    }

    /// Une entrée lue dans le PIPE.
    fn consume(&mut self, e: Entry) {
        if self.have > 0 && e.cmd != self.cur {
            // Paramètres d'une autre commande au milieu (ports et GXFIFO
            // mélangés) : la commande incomplète est abandonnée.
            self.stats.broken += 1;
            self.have = 0;
        }
        let Some(info) = commands::info(e.cmd) else { return };
        self.cur = e.cmd;
        self.params[self.have] = e.param;
        self.have += 1;
        if self.have >= (info.params as usize).max(1) {
            self.have = 0;
            let cycles = info.cycles + if self.mtx_mode == 2 { commands::mode2_extra(e.cmd) } else { 0 };
            self.execute(e.cmd, (info.params as usize).max(1));
            self.t_engine += cycles as u64 * BUS_CYCLE;
            if matches!(e.cmd, 0x70..=0x72) {
                self.test_until = self.t_engine;
            }
        }
    }

    /// Effets d'une commande complète.
    fn execute(&mut self, cmd: u8, n: usize) {
        self.stats.commands[cmd as usize & 0x7F] += 1;
        self.record(GxEvent::Exec(cmd, self.params[0]));
        match cmd {
            0x10 => self.mtx_mode = (self.params[0] & 3) as u8,
            0x50 => {
                self.swap_pending = true;
                self.geo.request_swap(self.params[0]);
            }
            _ => {}
        }
        self.geo.execute(cmd, &self.params[..n]);
    }

    /// Début du VBlank (ligne 192) : un SwapBuffers en attente a lieu et le
    /// moteur repart. Les statistiques de l'image sont archivées.
    pub fn vblank(&mut self, now: u64, disp3dcnt: u32, tex: &texture::TexMem) {
        self.catch_up(now);
        self.record(GxEvent::VBlank { swapped: self.swap_pending });
        let swapped = self.swap_pending;
        if self.swap_pending {
            self.swap_pending = false;
            self.geo.swap();
            self.stats.swaps += 1;
            self.t_engine = self.t_engine.max(now);
        }
        // Le rendu se fait pendant le VBlank, pour l'image suivante ; sans
        // nouvelle liste, la même image est redessinée (GBATEK, Overview).
        let params = RenderParams {
            clear_color: u32::from_le_bytes(self.reg_word(0x0400_0350)),
            clear_depth: u16::from_le_bytes([self.reg8(0x0400_0354), self.reg8(0x0400_0355)]),
            w_buffer: self.geo.rendering.swap_flags & 2 != 0,
            manual_sort: self.geo.rendering.swap_flags & 1 != 0,
            disp3dcnt,
            alpha_ref: self.reg8(0x0400_0340) & 31,
            toon: std::array::from_fn(|i| {
                u16::from_le_bytes([self.reg8(0x0400_0380 + 2 * i as u32), self.reg8(0x0400_0381 + 2 * i as u32)])
            }),
        };
        let same = !swapped && self.last_inputs.as_deref().is_some_and(|(p, t)| *p == params && t == tex);
        if same {
            self.renders_skipped += 1;
        } else {
            self.renderer.render(&self.geo.rendering, tex, &params);
            self.last_inputs = Some(Box::new((params, tex.clone())));
        }
        self.done.add(&self.stats);
        self.last_frame = std::mem::take(&mut self.stats);
        self.catch_up(now);
    }

    fn record(&mut self, e: GxEvent) {
        if let Some(log) = &mut self.log {
            log.push(e);
        }
    }

    /// Statistiques depuis la mise en route, image en cours comprise.
    pub fn totals(&self) -> FrameStats {
        let mut t = self.done.clone();
        t.add(&self.stats);
        t
    }

    /// Moteur occupé (GXSTAT bit 27) : une commande s'exécute, attend ses
    /// paramètres ou attend le VBlank, ou des entrées restent à traiter.
    pub fn busy(&self, now: u64) -> bool {
        self.swap_pending || self.have > 0 || self.t_engine > now || !self.fifo.is_drained()
    }

    pub fn gxstat(&self, now: u64) -> u32 {
        let n = self.fifo.fifo_len();
        let mut v = (n as u32) << 16 | self.geo.stack_status() | (self.geo.box_result as u32) << 1;
        if self.test_until > now {
            v |= 1;
        }
        if n < HALF {
            v |= 1 << 25;
        }
        if n == 0 {
            v |= 1 << 26;
        }
        if self.busy(now) {
            v |= 1 << 27;
        }
        v | self.fifo_irq << 30
    }

    /// FIFO à moins de moitié pleine : la DMA « FIFO géométrie » démarre.
    pub fn wants_dma(&self) -> bool {
        self.fifo.fifo_len() < HALF
    }

    /// Condition de l'IRQ de la file (de niveau : reposée tant qu'elle tient).
    pub fn irq_pending(&self) -> bool {
        match self.fifo_irq {
            1 => self.fifo.fifo_len() < HALF,
            2 => self.fifo.fifo_len() == 0,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;

/// Diagnostic : état du rendu 3D en texte (touche F11 du frontend,
/// `boot_report`). Polygones de la liste rendue regroupés par texture, avec
/// le décodage de TEXIMAGE_PARAM et l'état de la mémoire des textures.
pub fn report(g: &Gpu3d, disp3dcnt: u32, vramcnt: &[u8; 9], tex: &texture::TexMem) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    let mut out = String::new();
    let list = &g.geo.rendering;
    let _ = writeln!(out, "DISP3DCNT {disp3dcnt:04X} (textures {})", if disp3dcnt & 1 != 0 { "actives" } else { "COUPÉES" });
    let _ = writeln!(out, "VRAMCNT A–I : {}", vramcnt.map(|c| format!("{c:02X}")).join(" "));
    let used = |mem: &[u8]| mem.chunks(0x4000).map(|c| if c.iter().any(|&b| b != 0) { '#' } else { '.' }).collect::<String>();
    let _ = writeln!(out, "mémoire des textures (16 Ko par signe, # = non vide) : {}", used(&tex.tex));
    let _ = writeln!(out, "mémoire des palettes  (16 Ko par signe)               : {}", used(&tex.pal));
    let _ = writeln!(
        out,
        "liste rendue : {} polygones, {} sommets, débordement {}, SWAP {:X} ; translucides dessinés {}",
        list.polygons.len(),
        list.vertex_count,
        list.overflow,
        list.swap_flags,
        g.renderer.translucent
    );
    let mut groups: BTreeMap<(u32, u32, u32), (usize, [i32; 4])> = BTreeMap::new();
    for p in &list.polygons {
        let e = groups.entry((p.teximage, p.pltt_base, p.attr)).or_insert((0, [i32::MAX, i32::MAX, i32::MIN, i32::MIN]));
        e.0 += 1;
        for v in &p.vertices {
            let [x, y] = v.screen;
            e.1 = [e.1[0].min(x), e.1[1].min(y), e.1[2].max(x), e.1[3].max(y)];
        }
    }
    let _ = writeln!(out, "nb    TEXIMAGE PLTT  ATTR      format taille   adresse  texture       zone écran");
    for ((ti, pb, attr), (n, [x0, y0, x1, y1])) in groups {
        let t = texture::TexParams::new(ti, pb);
        let size = (t.width as usize * t.height as usize * [0, 8, 2, 4, 8, 2, 8, 16][t.format as usize]) / 8;
        let data = if t.format == 0 {
            "-".to_string()
        } else if tex.tex[t.addr.min(texture::TEX_SIZE)..(t.addr + size).min(texture::TEX_SIZE)].iter().all(|&b| b == 0) {
            "VIDE".to_string()
        } else {
            "données".to_string()
        };
        let _ = writeln!(
            out,
            "{n:<5} {ti:08X} {pb:04X}  {attr:08X}  {:<6} {:>4}x{:<4} {:06X}   {data:<13} ({x0},{y0})-({x1},{y1})",
            t.format, t.width, t.height, t.addr
        );
    }
    out
}

/// Calque 3D seul, en 0x00RRGGBB ; alpha 0 en magenta sombre.
pub fn layer_rgb(g: &Gpu3d) -> Vec<u32> {
    g.renderer
        .color
        .iter()
        .map(|p| {
            if p.alpha == 0 {
                0x40_0040
            } else {
                let [r, gg, b] = p.rgb.map(|c| (c as u32) << 2 | (c as u32) >> 4);
                r << 16 | gg << 8 | b
            }
        })
        .collect()
}
