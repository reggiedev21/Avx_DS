//! La machine Nintendo DS : mémoire, registres I/O et les deux CPU.
//!
//! [`Nds`] possède tout. Pour exécuter un CPU, on lui prête une *vue* du
//! matériel ([`Arm9Bus`] ou [`Arm7Bus`]) qui implémente [`Bus`] : pas de
//! pointeurs partagés, pas de `RefCell`, le borrow checker est satisfait par
//! des emprunts disjoints.

mod boot;
mod cart;
mod cp15;
mod dma;
pub mod gpu;
pub mod gpu3d;
pub mod sound;
mod hle_bios;
mod hle_swi;
mod ipc;
mod irq;
mod lcd;
mod math;
mod rtc;
pub mod save;
mod scheduler;
mod slot1;
mod spi;
pub mod spimem;
mod timers;
mod vram;

#[cfg(test)]
mod tests;

pub use boot::BootError;
pub use cart::{crc16, CartError, Header};
pub use cp15::Cp15;
pub use dma::{Dma, Trigger};
pub use gpu::Gpu;
pub use hle_bios::{BIOS7 as HLE_BIOS7, BIOS9 as HLE_BIOS9};
pub use irq::{bits as irq_bits, IrqController};
pub use ipc::IpcFifo;
pub use lcd::{Lcd, FRAME_CYCLES, LINE_CYCLES};
pub use math::MathUnit;
pub use rtc::Rtc;
pub use scheduler::{Event, Scheduler, BUS_CYCLE};
pub use slot1::Slot1;
pub use spi::{synthetic_firmware, Spi, FIRMWARE_SIZE};
pub use timers::{Timer, Timers};
pub use vram::{GpuRole, Mapping, Region, Vram};

use crate::bus::{Bus, Cp15Op};
use crate::cpu::{Arm7, Arm9};
use std::collections::{BTreeMap, BTreeSet};

pub const MAIN_RAM_SIZE: usize = 4 << 20;
pub const SHARED_WRAM_SIZE: usize = 32 << 10;
pub const ARM7_WRAM_SIZE: usize = 64 << 10;
pub const ITCM_SIZE: usize = 32 << 10;
pub const DTCM_SIZE: usize = 16 << 10;
pub const BIOS9_SIZE: usize = 4 << 10;
pub const BIOS7_SIZE: usize = 16 << 10;

/// Quel CPU a fait un accès (pour le suivi des I/O non gérées).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuId {
    Arm9,
    Arm7,
}

/// Accès I/O comptés par (CPU, instruction, registre, écriture ?) : le
/// diagnostic « où le jeu attend-il ? ».
#[derive(Clone, Debug, Default)]
pub struct IoTrace {
    /// (nombre d'accès, dernière valeur lue ou écrite)
    pub counts: BTreeMap<(CpuId, u32, u32, bool), (u64, u32)>,
    /// Les derniers accès, dans l'ordre : (CPU, instruction, registre,
    /// écriture ?, valeur).
    pub recent: std::collections::VecDeque<(CpuId, u32, u32, bool, u32)>,
    /// Les premiers accès à IPCSYNC, à l'IPCFIFO, au SPI de la cartouche
    /// (AUXSPICNT/AUXSPIDATA) et à EXMEMCNT, dans l'ordre : instant
    /// (cycles ARM9), CPU, instruction, registre, écriture ?, valeur, et
    /// nombre de répétitions (un accès identique à l'un des 4 derniers est
    /// compté sur celui-ci : les boucles de scrutation tiennent en une ligne).
    pub ipc: Vec<(u64, CpuId, u32, u32, bool, u32, u64)>,
    /// Les derniers accès IPC (même format), une fois `ipc` plein.
    pub ipc_tail: std::collections::VecDeque<(u64, CpuId, u32, u32, bool, u32, u64)>,
    /// Accès IPC ni dans `ipc` ni dans `ipc_tail` (sortis de la fenêtre).
    pub ipc_dropped: u64,
}

impl IoTrace {
    pub const RECENT: usize = 64;
    pub const IPC: usize = 256;

    fn record(&mut self, now: u64, cpu: CpuId, pc: u32, addr: u32, write: bool, value: u32) {
        if matches!(addr, 0x0400_0180 | 0x0400_0184 | 0x0400_0188 | 0x0410_0000 | 0x0400_01A0 | 0x0400_0204) {
            // Une scrutation répète les mêmes accès (souvent « écrire, lire ») :
            // un accès identique à l'un des 4 derniers n'est que compté.
            let key = (cpu, pc, addr, write, value);
            let entry = (now, cpu, pc, addr, write, value, 1);
            if self.ipc.len() < Self::IPC {
                match self.ipc.iter_mut().rev().take(4).find(|e| (e.1, e.2, e.3, e.4, e.5) == key) {
                    Some(e) => e.6 += 1,
                    None => self.ipc.push(entry),
                }
            } else {
                match self.ipc_tail.iter_mut().rev().take(4).find(|e| (e.1, e.2, e.3, e.4, e.5) == key) {
                    Some(e) => e.6 += 1,
                    None => {
                        if self.ipc_tail.len() == Self::IPC {
                            self.ipc_tail.pop_front();
                            self.ipc_dropped += 1;
                        }
                        self.ipc_tail.push_back(entry);
                    }
                }
            }
        }
        let e = self.counts.entry((cpu, pc, addr, write)).or_insert((0, 0));
        e.0 += 1;
        e.1 = value;
        if self.recent.len() == Self::RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back((cpu, pc, addr, write, value));
    }
}

/// Surveillance des écritures dans une zone mémoire (diagnostic).
#[derive(Clone, Debug, Default)]
pub struct Watch {
    pub start: u32,
    pub end: u32,
    pub total: u64,
    /// Les dernières écritures : (instant, CPU, instruction, adresse,
    /// taille, valeur).
    pub hits: std::collections::VecDeque<(u64, CpuId, u32, u32, u32, u32)>,
}

impl Watch {
    pub const KEEP: usize = 256;

    pub fn new(start: u32, len: u32) -> Self {
        Watch { start, end: start.wrapping_add(len), ..Default::default() }
    }
}

/// Tout le matériel, sans les CPU.
pub struct Hardware {
    pub main_ram: Box<[u8]>,
    pub shared_wram: Box<[u8]>,
    pub arm7_wram: Box<[u8]>,
    pub itcm: Box<[u8]>,
    pub dtcm: Box<[u8]>,
    pub bios9: Box<[u8]>,
    pub bios7: Box<[u8]>,
    pub palette: Box<[u8]>,
    pub oam: Box<[u8]>,
    pub vram: Vram,
    pub cp15: Cp15,

    pub irq9: IrqController,
    pub irq7: IrqController,

    /// WRAMCNT (0x04000247) : répartition des 32 Ko de WRAM partagée.
    pub wramcnt: u8,
    /// IPCSYNC de chaque CPU : bits 8–11 = sortie, bit 14 = IRQ autorisée.
    pub ipcsync9: u16,
    pub ipcsync7: u16,
    pub powcnt1: u16,
    pub powcnt2: u8,
    /// Touches A, B, Select, Start, →, ←, ↑, ↓, R, L (0 = appuyée).
    pub keyinput: u16,
    /// X, Y, stylet, charnière (0 = appuyée / fermée). Lu par l'ARM7.
    pub extkeyin: u16,
    pub postflg9: u8,
    pub postflg7: u8,
    pub math: MathUnit,

    pub sched: Scheduler,
    pub lcd: Lcd,
    /// Timers de l'ARM9 (indice 0) et de l'ARM7 (indice 1).
    pub timers: [Timers; 2],
    pub fifo: IpcFifo,
    pub gpu: Gpu,
    /// DMA de l'ARM9 (indice 0) et de l'ARM7 (indice 1).
    pub dma: [Dma; 2],
    /// Journal des accès I/O (diagnostic, `None` = désactivé).
    pub io_trace: Option<IoTrace>,
    /// Adresse de l'instruction en cours sur chaque CPU (tenue à jour
    /// seulement quand `io_trace` est actif).
    pub cur_pc: [u32; 2],
    /// Zone mémoire surveillée en écriture (diagnostic). Les CPU écrivent
    /// via `cur_pc`, donc activer aussi `io_trace` pour avoir l'instruction.
    pub watch: Option<Watch>,
    /// Moteur 3D (bouchon : commandes ignorées, file toujours vide).
    pub gpu3d: gpu3d::Gpu3d,
    /// Contrôleur son de l'ARM7.
    pub sound: sound::Sound,
    /// Garde : une DMA « FIFO géométrie » est en cours (pas de relance
    /// imbriquée depuis ses propres écritures dans GXFIFO).
    gx_dma_active: bool,
    /// Bus de la cartouche (slot 1).
    pub slot1: Slot1,
    /// Bus SPI de l'ARM7 : alimentation, firmware, écran tactile.
    pub spi: Spi,
    pub rtc: Rtc,
    /// EXMEMCNT (0x04000204) : bit 11 = slot 1 à l'ARM7, bit 7 = slot 2 à l'ARM7.
    pub exmemcnt: u16,
    /// Fonctions hors GPU demandées mais pas encore émulées (DMA spéciales…).
    pub unsupported: BTreeSet<&'static str>,
    /// Instant courant, en cycles ARM9, vu par le CPU qui accède au bus.
    pub now: u64,

    /// HALTCNT écrit par l'ARM7 : la machine met l'ARM7 en pause.
    pub(crate) arm7_halt_request: bool,

    /// Registres I/O accédés mais pas encore émulés (pour le débogage).
    pub unhandled_io: BTreeSet<(CpuId, u32)>,

    /// Appels BIOS émulés en Rust (mini-BIOS chargé). Désactivé, chaque SWI
    /// passe par le BIOS chargé (par exemple un vrai BIOS ou bios9.bin).
    pub hle: bool,
    /// SWI appelés mais pas encore émulés (pour le débogage).
    pub unhandled_swi: BTreeSet<(CpuId, u8)>,
    /// ROM de la cartouche insérée (vide si aucune).
    pub rom: Vec<u8>,
}

fn zeroed(size: usize) -> Box<[u8]> {
    vec![0u8; size].into_boxed_slice()
}

impl Default for Hardware {
    fn default() -> Self {
        Hardware {
            main_ram: zeroed(MAIN_RAM_SIZE),
            shared_wram: zeroed(SHARED_WRAM_SIZE),
            arm7_wram: zeroed(ARM7_WRAM_SIZE),
            itcm: zeroed(ITCM_SIZE),
            dtcm: zeroed(DTCM_SIZE),
            bios9: zeroed(BIOS9_SIZE),
            bios7: zeroed(BIOS7_SIZE),
            palette: zeroed(2 << 10),
            oam: zeroed(2 << 10),
            vram: Vram::default(),
            cp15: Cp15::default(),
            irq9: IrqController::default(),
            irq7: IrqController::default(),
            wramcnt: 3,
            ipcsync9: 0,
            ipcsync7: 0,
            powcnt1: 0,
            powcnt2: 0,
            keyinput: 0x03FF,
            extkeyin: 0x007F,
            postflg9: 0,
            postflg7: 0,
            math: MathUnit::default(),
            sched: Scheduler::default(),
            lcd: Lcd::default(),
            timers: [Timers::default(), Timers::default()],
            fifo: IpcFifo::default(),
            gpu: Gpu::default(),
            dma: [Dma::new(true), Dma::new(false)],
            io_trace: None,
            watch: None,
            cur_pc: [0; 2],
            gpu3d: gpu3d::Gpu3d::default(),
            sound: sound::Sound::default(),
            gx_dma_active: false,
            slot1: Slot1::default(),
            spi: Spi::default(),
            rtc: Rtc::default(),
            exmemcnt: 0,
            unsupported: BTreeSet::new(),
            now: 0,
            arm7_halt_request: false,
            unhandled_io: BTreeSet::new(),
            hle: false,
            unhandled_swi: BTreeSet::new(),
            rom: Vec::new(),
        }
    }
}

impl CpuId {
    #[inline]
    fn index(self) -> usize {
        self as usize
    }
}

impl Hardware {
    fn irq(&mut self, cpu: usize) -> &mut IrqController {
        if cpu == 0 { &mut self.irq9 } else { &mut self.irq7 }
    }

    /// Lève les IRQ renvoyées par un périphérique, pour [ARM9, ARM7].
    fn raise(&mut self, irqs: [u32; 2]) {
        self.irq9.request(irqs[0]);
        self.irq7.request(irqs[1]);
    }

    fn timer_event(cpu: usize, n: usize) -> Event {
        Event::Timer(if cpu == 0 { CpuId::Arm9 } else { CpuId::Arm7 }, n as u8)
    }

    fn timer_read(&self, cpu: usize, n: usize) -> u32 {
        let t = &self.timers[cpu].t[n];
        t.value(n, self.now) as u32 | (t.control as u32) << 16
    }

    fn timer_write(&mut self, cpu: usize, n: usize, val: u32, mask: u32) {
        let now = self.now;
        let t = &mut self.timers[cpu].t[n];
        if mask & 0xFFFF != 0 {
            let m = mask as u16;
            t.reload = (t.reload & !m) | (val as u16 & m);
        }
        if mask & 0x00FF_0000 == 0 {
            return;
        }
        // Fige la valeur courante avant de changer le mode de comptage.
        let was_enabled = t.enabled();
        t.counter = t.value(n, now);
        t.start = now;
        t.control = (val >> 16) as u8 & 0xC7;
        if t.enabled() && !was_enabled {
            t.counter = t.reload;
        }
        let ev = Self::timer_event(cpu, n);
        if t.free_running(n) {
            let at = t.overflow_time();
            self.sched.schedule(ev, at);
        } else {
            self.sched.cancel(ev);
        }
    }

    /// Débordement du timer `n` à l'instant `time` (et cascade éventuelle).
    fn timer_overflow(&mut self, cpu: usize, n: usize, time: u64) {
        let t = &mut self.timers[cpu].t[n];
        t.counter = t.reload;
        t.start = time;
        let irq_on = t.irq_enabled();
        if t.free_running(n) {
            let at = t.overflow_time();
            self.sched.schedule(Self::timer_event(cpu, n), at);
        }
        if irq_on {
            self.irq(cpu).request(irq_bits::TIMER0 << n);
        }
        if n < 3 {
            let next = &mut self.timers[cpu].t[n + 1];
            if next.enabled() && !next.free_running(n + 1) {
                if next.counter == 0xFFFF {
                    self.timer_overflow(cpu, n + 1, time);
                } else {
                    next.counter += 1;
                }
            }
        }
    }

    /// Lance le transfert DMA `n` du CPU `cpu`.
    fn run_dma(&mut self, cpu: usize, n: usize) {
        let ch = self.dma[cpu].ch[n];
        let trigger = self.dma[cpu].trigger_of(n);
        // FIFO géométrie : rafales d'au plus 112 mots (GBATEK).
        let full = self.dma[cpu].count(n);
        let count = if trigger == Trigger::GeometryFifo { ch.left.min(112) } else { full };
        let step: i32 = if ch.word() { 4 } else { 2 };
        let delta = |mode: u32| match mode {
            0 | 3 => step, // 3 : interdit pour la source, traité comme 0
            1 => -step,
            _ => 0,
        };
        let (ds, dd) = (delta(ch.src_mode()), delta(ch.dst_mode()));
        let (s, d) = if cpu == 0 {
            dma_transfer(&mut Arm9Bus { hw: self }, ch.word(), count, ch.src, ch.dst, ds, dd)
        } else {
            dma_transfer(&mut Arm7Bus { hw: self }, ch.word(), count, ch.src, ch.dst, ds, dd)
        };
        let c = &mut self.dma[cpu].ch[n];
        c.src = s;
        c.dst = if ch.dst_mode() == 3 { c.dad & 0x0FFF_FFFF } else { d };
        if trigger == Trigger::GeometryFifo {
            c.left -= count;
            if c.left > 0 {
                return; // rafale suivante quand la file se videra
            }
            if ch.repeat() {
                // GBATEK demande « répétition 0 » ; si elle est mise malgré
                // tout, on recharge le compte (pas de rafale vide sans fin).
                c.left = full;
            }
        }
        if !ch.repeat() || trigger == Trigger::Immediate {
            c.cnt &= !(1 << 31);
        }
        if ch.irq() {
            self.irq(cpu).request(irq_bits::DMA0 << n);
        }
    }

    #[inline]
    fn watch_write(&mut self, cpu: CpuId, addr: u32, size: u32, val: u32) {
        if let Some(w) = &mut self.watch {
            if addr < w.end && addr.wrapping_add(size) > w.start {
                w.total += 1;
                if w.hits.len() == Watch::KEEP {
                    w.hits.pop_front();
                }
                let pc = self.cur_pc[cpu as usize];
                w.hits.push_back((self.now, cpu, pc, addr, size, val));
            }
        }
    }

    /// Moteur 3D : rattrape l'horloge, lance la DMA « FIFO géométrie »
    /// tant que la file est à moins de moitié pleine, pose l'IRQ de la file
    /// (de niveau) si sa condition tient.
    fn gx_service(&mut self) {
        self.gpu3d.catch_up(self.now);
        if !self.gx_dma_active {
            self.gx_dma_active = true;
            while self.gpu3d.wants_dma() {
                let ready = (0..4).find(|&n| {
                    self.dma[0].ch[n].enabled() && self.dma[0].trigger_of(n) == Trigger::GeometryFifo
                });
                let Some(n) = ready else { break };
                self.run_dma(0, n);
            }
            self.gx_dma_active = false;
        }
        if self.gpu3d.irq_pending() {
            self.irq9.request(irq_bits::GEOMETRY_FIFO);
        }
    }

    /// CPU propriétaire du slot 1 (EXMEMCNT bit 11).
    fn slot1_owner(&self) -> usize {
        ((self.exmemcnt >> 11) & 1) as usize
    }

    /// Vide le transfert de cartouche en cours par DMA, si un canal attend
    /// ce déclencheur chez le CPU propriétaire du slot.
    fn cartridge_dma(&mut self) {
        let cpu = self.slot1_owner();
        while self.slot1.words_ready() {
            let ready = (0..4).find(|&n| {
                self.dma[cpu].ch[n].enabled() && self.dma[cpu].trigger_of(n) == Trigger::Cartridge
            });
            let Some(n) = ready else { break };
            self.run_dma(cpu, n);
        }
    }

    /// Lance les canaux DMA en attente de `trigger`.
    fn trigger_dma(&mut self, cpu: usize, trigger: Trigger) {
        for n in 0..4 {
            if self.dma[cpu].ch[n].enabled() && self.dma[cpu].trigger_of(n) == trigger {
                self.run_dma(cpu, n);
            }
        }
    }

    /// Exécute un événement arrivé à échéance.
    fn handle_event(&mut self, ev: Event, time: u64) {
        self.now = time;
        match ev {
            Event::HBlank => {
                let irqs = self.lcd.hblank();
                self.raise(irqs);
                // Le moteur 3D rattrape l'horloge ; DMA et IRQ de sa file.
                self.gx_service();
                let y = self.lcd.vcount as usize;
                if y < gpu::HEIGHT {
                    let ctx = gpu::RenderCtx {
                        vram: &self.vram,
                        palette: &self.palette,
                        oam: &self.oam,
                        powcnt1: self.powcnt1,
                        layer3d: &self.gpu3d.renderer.color,
                    };
                    if let Some(c) = self.gpu.render_line(y, &ctx) {
                        // Écriture de la capture, avec repli à 128 Ko.
                        let bank = &mut self.vram.banks[c.bank];
                        for (i, px) in c.pixels.iter().enumerate() {
                            let o = (c.offset + 2 * i) & 0x1_FFFF;
                            bank[o..o + 2].copy_from_slice(&px.to_le_bytes());
                        }
                    }
                    self.trigger_dma(0, Trigger::HBlank);
                }
            }
            Event::LineEnd => {
                let irqs = self.lcd.next_line(time);
                self.raise(irqs);
                if self.lcd.vcount == lcd::VISIBLE_LINES {
                    // SwapBuffers en attente ; la DMA et l'IRQ de la file
                    // suivent au HBlank de cette même ligne.
                    // DISP3DCNT est rangé avec les registres du moteur 2D A.
                    let disp3dcnt = self.gpu.engines[0].regs[0x60 / 4] & 0xFFFF;
                    let tex = gpu3d::texture::TexMem::from_vram(&self.vram);
                    self.gpu3d.vblank(time, disp3dcnt, &tex);
                    self.gpu.start_frame();
                    self.trigger_dma(0, Trigger::VBlank);
                    self.trigger_dma(1, Trigger::VBlank);
                }
                self.sched.schedule(Event::HBlank, time + lcd::HBLANK_START);
                self.sched.schedule(Event::LineEnd, time + LINE_CYCLES);
            }
            Event::Timer(cpu, n) => self.timer_overflow(cpu.index(), n as usize, time),
            Event::Sound => {
                // Le son lit ses échantillons (et écrit ses captures) sur le
                // bus de l'ARM7 : on le sort le temps de l'appel.
                let mut snd = std::mem::take(&mut self.sound);
                snd.sample(&mut Arm7Bus { hw: self });
                self.sound = snd;
                self.sched.schedule(Event::Sound, time + sound::SAMPLE_CYCLES);
            }
        }
    }

    fn io_common_read(&mut self, cpu: usize, addr: u32) -> Option<u32> {
        if let Some(v) = self.dma[cpu].read(addr) {
            return Some(v);
        }
        Some(match addr {
            0x0400_0004 => self.lcd.read_dispstat(cpu) as u32 | (self.lcd.vcount as u32) << 16,
            0x0400_0100..=0x0400_010C => self.timer_read(cpu, ((addr >> 2) & 3) as usize),
            0x0400_0184 => self.fifo.read_cnt(cpu) as u32,
            0x0410_0000 => {
                let (v, irqs) = self.fifo.receive(cpu);
                self.raise(irqs);
                v
            }
            0x0400_01A0..=0x0400_01BC => self.slot1.read_reg(addr),
            0x0410_0010 => {
                let (v, end) = self.slot1.read_data();
                if end {
                    let owner = self.slot1_owner();
                    self.irq(owner).request(irq_bits::CARD_TRANSFER);
                }
                v
            }
            _ => return None,
        })
    }

    fn io_common_write(&mut self, cpu: usize, addr: u32, val: u32, mask: u32) -> bool {
        if let Some(start) = self.dma[cpu].write(addr, val, mask) {
            let n = ((addr.wrapping_sub(0x0400_00B0)) / 12) as usize;
            if n < 4 && self.dma[cpu].ch[n].enabled() {
                if let Trigger::Other(_) = self.dma[cpu].trigger_of(n) {
                    self.unsupported.insert("DMA sur un déclencheur autre que immédiat/VBlank/HBlank");
                }
            }
            if let Some(n) = start {
                self.run_dma(cpu, n);
            }
            if cpu == 0 && n < 4 && self.dma[0].trigger_of(n) == Trigger::GeometryFifo {
                self.gx_service(); // la file a peut-être déjà de la place
            }
            if n < 4 && self.dma[cpu].trigger_of(n) == Trigger::Cartridge {
                self.cartridge_dma(); // DMA activée après le début du transfert
            }
            return true;
        }
        match addr {
            0x0400_01A0..=0x0400_01BC => {
                if let Some(ended) = self.slot1.write_reg(addr, val, mask, &self.rom) {
                    if ended {
                        let owner = self.slot1_owner();
                        self.irq(owner).request(irq_bits::CARD_TRANSFER);
                    }
                    self.cartridge_dma();
                }
            }
            0x0400_0004 => self.lcd.write_dispstat(cpu, val as u16, mask as u16),
            0x0400_0100..=0x0400_010C => self.timer_write(cpu, ((addr >> 2) & 3) as usize, val, mask),
            0x0400_0184 => {
                let irqs = self.fifo.write_cnt(cpu, val as u16, mask as u16);
                self.raise(irqs);
            }
            0x0400_0188 => {
                let irqs = self.fifo.send(cpu, val);
                self.raise(irqs);
            }
            _ => return false,
        }
        true
    }
}

// ----------------------------------------------------------------------
// Accès mémoire bruts (offsets déjà dans les bornes grâce aux masques)
// ----------------------------------------------------------------------

#[inline]
fn rd16(m: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([m[o], m[o + 1]])
}
#[inline]
fn rd32(m: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]])
}
#[inline]
fn wr16(m: &mut [u8], o: usize, v: u16) {
    m[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
#[inline]
fn wr32(m: &mut [u8], o: usize, v: u32) {
    m[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Où tombe un accès.
enum Target<'a> {
    Mem(&'a mut [u8], usize),
    /// Mémoire en lecture seule (BIOS) : les écritures sont ignorées.
    Rom(&'a [u8], usize),
    Vram(Region, u32),
    Io,
    /// Rien à cette adresse : lecture à 0, écriture ignorée.
    Open,
}

/// Copie DMA élémentaire, à travers la vue du CPU concerné.
fn dma_transfer<B: Bus>(bus: &mut B, word: bool, count: u32, mut s: u32, mut d: u32, ds: i32, dd: i32) -> (u32, u32) {
    for _ in 0..count {
        if word {
            let v = bus.read32(s & !3);
            bus.write32(d & !3, v);
        } else {
            let v = bus.read16(s & !1);
            bus.write16(d & !1, v);
        }
        s = s.wrapping_add(ds as u32);
        d = d.wrapping_add(dd as u32);
    }
    (s, d)
}

/// Masque de 32 bits couvrant un accès de `size` octets à `addr`.
#[inline]
fn lane(addr: u32, size: u32) -> (u32, u32) {
    let shift = (addr & 3) * 8;
    let mask = if size == 4 { u32::MAX } else { ((1u32 << (size * 8)) - 1) << shift };
    (shift, mask)
}

#[inline]
fn merge(reg: &mut u32, val: u32, mask: u32, writable: u32) {
    let m = mask & writable;
    *reg = (*reg & !m) | (val & m);
}

#[inline]
fn merge16(reg: &mut u16, val: u32, mask: u32, writable: u16) {
    let m = mask as u16 & writable;
    *reg = (*reg & !m) | (val as u16 & m);
}

// ----------------------------------------------------------------------
// Vue ARM9
// ----------------------------------------------------------------------

/// Le matériel vu par l'ARM9.
pub struct Arm9Bus<'a> {
    pub hw: &'a mut Hardware,
}

impl Arm9Bus<'_> {
    fn target(&mut self, addr: u32, write: bool) -> Target<'_> {
        let hw = &mut *self.hw;
        if hw.cp15.itcm_hit(addr, write) {
            return Target::Mem(&mut hw.itcm, (addr & 0x7FFF) as usize);
        }
        if hw.cp15.dtcm_hit(addr, write) {
            let off = addr.wrapping_sub(hw.cp15.dtcm_base) & 0x3FFF;
            return Target::Mem(&mut hw.dtcm, off as usize);
        }
        match addr >> 24 {
            0x02 => Target::Mem(&mut hw.main_ram, (addr & 0x3F_FFFF) as usize),
            0x03 => match hw.wramcnt & 3 {
                0 => Target::Mem(&mut hw.shared_wram, (addr & 0x7FFF) as usize),
                1 => Target::Mem(&mut hw.shared_wram, 0x4000 + (addr & 0x3FFF) as usize),
                2 => Target::Mem(&mut hw.shared_wram, (addr & 0x3FFF) as usize),
                _ => Target::Open,
            },
            0x04 => Target::Io,
            0x05 => Target::Mem(&mut hw.palette, (addr & 0x7FF) as usize),
            0x06 => {
                let (r, o) = Vram::locate_arm9(addr);
                Target::Vram(r, o)
            }
            0x07 => Target::Mem(&mut hw.oam, (addr & 0x7FF) as usize),
            0xFF if addr >= 0xFFFF_0000 => Target::Rom(&hw.bios9, (addr & 0xFFF) as usize),
            _ => Target::Open,
        }
    }

    fn io_read(&mut self, addr: u32) -> u32 {
        let v = self.io_read_raw(addr);
        if let Some(t) = &mut self.hw.io_trace {
            t.record(self.hw.now, CpuId::Arm9, self.hw.cur_pc[0], addr, false, v);
        }
        v
    }

    fn io_read_raw(&mut self, addr: u32) -> u32 {
        let hw = &mut *self.hw;
        if let Some(v) = hw.io_common_read(0, addr) {
            return v;
        }
        if let Some(v) = hw.gpu.io_read(addr) {
            return v;
        }
        if gpu3d::Gpu3d::handles(addr) {
            hw.gx_service();
            let now = hw.now;
            return hw.gpu3d.read(addr, now);
        }
        match addr {
            0x0400_0130 => hw.keyinput as u32,
            0x0400_0180 => ((hw.ipcsync9 & 0x4F00) | ((hw.ipcsync7 >> 8) & 0xF)) as u32,
            0x0400_0208 => hw.irq9.ime as u32,
            0x0400_0210 => hw.irq9.ie,
            0x0400_0214 => hw.irq9.if_,
            0x0400_0240 => u32::from_le_bytes([hw.vram.cnt[0], hw.vram.cnt[1], hw.vram.cnt[2], hw.vram.cnt[3]]),
            0x0400_0244 => u32::from_le_bytes([hw.vram.cnt[4], hw.vram.cnt[5], hw.vram.cnt[6], hw.wramcnt]),
            0x0400_0248 => u32::from_le_bytes([hw.vram.cnt[7], hw.vram.cnt[8], 0, 0]),
            0x0400_0280..=0x0400_02BC => hw.math.read(addr),
            0x0400_0204 => hw.exmemcnt as u32,
            0x0400_0300 => hw.postflg9 as u32,
            0x0400_0304 => hw.powcnt1 as u32,
            _ => {
                hw.unhandled_io.insert((CpuId::Arm9, addr));
                0
            }
        }
    }

    fn io_write(&mut self, addr: u32, val: u32, mask: u32) {
        if let Some(t) = &mut self.hw.io_trace {
            t.record(self.hw.now, CpuId::Arm9, self.hw.cur_pc[0], addr, true, val & mask);
        }
        self.io_write_raw(addr, val, mask);
    }

    fn io_write_raw(&mut self, addr: u32, val: u32, mask: u32) {
        let hw = &mut *self.hw;
        if hw.io_common_write(0, addr, val, mask) || hw.gpu.io_write(addr, val, mask) {
            return;
        }
        if gpu3d::Gpu3d::handles(addr) {
            let now = hw.now;
            hw.gpu3d.write(addr, val, mask, now);
            hw.gpu.unsupported.insert("3D : textures, translucides et effets pas encore dessinés");
            hw.gx_service();
            return;
        }
        match addr {
            0x0400_0180 => {
                merge16(&mut hw.ipcsync9, val, mask, 0x4F00);
                if mask & val & (1 << 13) != 0 && hw.ipcsync7 & (1 << 14) != 0 {
                    hw.irq7.request(irq::bits::IPC_SYNC);
                }
            }
            0x0400_0208 => {
                if mask & 1 != 0 {
                    hw.irq9.ime = val & 1 != 0;
                }
            }
            0x0400_0210 => merge(&mut hw.irq9.ie, val, mask, 0x003F_3F7F),
            0x0400_0214 => hw.irq9.if_ &= !(val & mask),
            0x0400_0240 | 0x0400_0244 | 0x0400_0248 => {
                let first = ((addr - 0x0400_0240) as usize) & !3;
                for k in 0..4 {
                    if mask & (0xFF << (k * 8)) == 0 {
                        continue;
                    }
                    let byte = (val >> (k * 8)) as u8;
                    match first + k {
                        7 => hw.wramcnt = byte & 3,
                        b @ 0..=6 => hw.vram.set_cnt(b, byte),
                        b @ 8..=9 => hw.vram.set_cnt(b - 1, byte),
                        _ => {}
                    }
                }
            }
            0x0400_0280..=0x0400_02BC => hw.math.write(addr, val, mask),
            0x0400_0204 => merge16(&mut hw.exmemcnt, val, mask, 0xE8FF),
            0x0400_0300 => {
                if mask & 0xFF != 0 {
                    // Bit 0 : ne peut que passer à 1. Bit 1 : libre.
                    hw.postflg9 = (hw.postflg9 & 1) | (val as u8 & 3);
                }
            }
            0x0400_0304 => merge16(&mut hw.powcnt1, val, mask, 0x820F),
            _ => {
                hw.unhandled_io.insert((CpuId::Arm9, addr));
            }
        }
    }
}

impl Bus for Arm9Bus<'_> {
    fn read8(&mut self, addr: u32) -> u8 {
        match self.target(addr, false) {
            Target::Mem(m, o) => m[o],
            Target::Rom(m, o) => m[o],
            Target::Vram(r, o) => self.hw.vram.read8(r, o),
            Target::Io => (self.io_read(addr & !3) >> ((addr & 3) * 8)) as u8,
            Target::Open => 0,
        }
    }
    fn read16(&mut self, addr: u32) -> u16 {
        match self.target(addr, false) {
            Target::Mem(m, o) => rd16(m, o),
            Target::Rom(m, o) => rd16(m, o),
            Target::Vram(r, o) => self.hw.vram.read16(r, o),
            Target::Io => (self.io_read(addr & !3) >> ((addr & 2) * 8)) as u16,
            Target::Open => 0,
        }
    }
    fn read32(&mut self, addr: u32) -> u32 {
        match self.target(addr, false) {
            Target::Mem(m, o) => rd32(m, o),
            Target::Rom(m, o) => rd32(m, o),
            Target::Vram(r, o) => self.hw.vram.read32(r, o),
            Target::Io => self.io_read(addr),
            Target::Open => 0,
        }
    }
    fn write8(&mut self, addr: u32, val: u8) {
        self.hw.watch_write(CpuId::Arm9, addr, 1, val as u32);
        // Palette, VRAM et OAM ignorent les écritures 8 bits de l'ARM9.
        if matches!(addr >> 24, 0x05..=0x07) && !self.hw.cp15.itcm_hit(addr, true) && !self.hw.cp15.dtcm_hit(addr, true) {
            return;
        }
        match self.target(addr, true) {
            Target::Mem(m, o) => m[o] = val,
            Target::Io => {
                let (shift, mask) = lane(addr, 1);
                self.io_write(addr & !3, (val as u32) << shift, mask);
            }
            _ => {}
        }
    }
    fn write16(&mut self, addr: u32, val: u16) {
        self.hw.watch_write(CpuId::Arm9, addr, 2, val as u32);
        match self.target(addr, true) {
            Target::Mem(m, o) => wr16(m, o, val),
            Target::Vram(r, o) => self.hw.vram.write16(r, o, val),
            Target::Io => {
                let (shift, mask) = lane(addr, 2);
                self.io_write(addr & !3, (val as u32) << shift, mask);
            }
            _ => {}
        }
    }
    fn write32(&mut self, addr: u32, val: u32) {
        self.hw.watch_write(CpuId::Arm9, addr, 4, val);
        match self.target(addr, true) {
            Target::Mem(m, o) => wr32(m, o, val),
            Target::Vram(r, o) => self.hw.vram.write32(r, o, val),
            Target::Io => self.io_write(addr, val, u32::MAX),
            _ => {}
        }
    }
    fn exception_base(&mut self) -> u32 {
        if self.hw.cp15.high_vectors() { 0xFFFF_0000 } else { 0 }
    }
    fn cp15_read(&mut self, op: Cp15Op) -> u32 {
        self.hw.cp15.read(op)
    }
    fn cp15_write(&mut self, op: Cp15Op, val: u32) {
        self.hw.cp15.write(op, val);
    }
    fn hle_swi(&mut self, num: u8, regs: &mut [u32; 4]) -> bool {
        swi_hook(self, CpuId::Arm9, num, regs)
    }
}

/// HLE d'un SWI pour l'une ou l'autre vue du bus.
fn swi_hook<B: Bus + HasHardware>(bus: &mut B, cpu: CpuId, num: u8, regs: &mut [u32; 4]) -> bool {
    if !bus.hardware().hle {
        return false;
    }
    match hle_swi::swi(bus, cpu, num, regs) {
        hle_swi::Swi::Handled => true,
        hle_swi::Swi::Bios => false,
        hle_swi::Swi::Unhandled => {
            bus.hardware().unhandled_swi.insert((cpu, num));
            true
        }
    }
}

trait HasHardware {
    fn hardware(&mut self) -> &mut Hardware;
}
impl HasHardware for Arm9Bus<'_> {
    fn hardware(&mut self) -> &mut Hardware {
        self.hw
    }
}
impl HasHardware for Arm7Bus<'_> {
    fn hardware(&mut self) -> &mut Hardware {
        self.hw
    }
}

// ----------------------------------------------------------------------
// Vue ARM7
// ----------------------------------------------------------------------

/// Le matériel vu par l'ARM7.
pub struct Arm7Bus<'a> {
    pub hw: &'a mut Hardware,
}

impl Arm7Bus<'_> {
    fn target(&mut self, addr: u32) -> Target<'_> {
        let hw = &mut *self.hw;
        match addr >> 24 {
            0x00 if addr < BIOS7_SIZE as u32 => Target::Rom(&hw.bios7, addr as usize),
            0x02 => Target::Mem(&mut hw.main_ram, (addr & 0x3F_FFFF) as usize),
            0x03 if addr < 0x0380_0000 => match hw.wramcnt & 3 {
                // Sans WRAM partagée, la zone reflète la WRAM propre de l'ARM7.
                0 => Target::Mem(&mut hw.arm7_wram, (addr & 0xFFFF) as usize),
                1 => Target::Mem(&mut hw.shared_wram, (addr & 0x3FFF) as usize),
                2 => Target::Mem(&mut hw.shared_wram, 0x4000 + (addr & 0x3FFF) as usize),
                _ => Target::Mem(&mut hw.shared_wram, (addr & 0x7FFF) as usize),
            },
            0x03 => Target::Mem(&mut hw.arm7_wram, (addr & 0xFFFF) as usize),
            0x04 if addr < 0x0480_0000 => Target::Io,
            0x06 => {
                let (r, o) = Vram::locate_arm7(addr);
                Target::Vram(r, o)
            }
            _ => Target::Open,
        }
    }

    fn io_read(&mut self, addr: u32) -> u32 {
        let v = self.io_read_raw(addr);
        if let Some(t) = &mut self.hw.io_trace {
            t.record(self.hw.now, CpuId::Arm7, self.hw.cur_pc[1], addr, false, v);
        }
        v
    }

    fn io_read_raw(&mut self, addr: u32) -> u32 {
        let hw = &mut *self.hw;
        if let Some(v) = hw.io_common_read(1, addr) {
            return v;
        }
        match addr {
            0x0400_0130 => hw.keyinput as u32,
            0x0400_0134 => {
                // Bit 6 : 0 = stylet posé, d'après l'état de l'écran tactile.
                let pen = if hw.spi.touch.pen.is_some() { 0 } else { 1 << 6 };
                // RCNT (moitié basse) : port série, non émulé. Lu à 0 ;
                // valeur réelle non vérifiée.
                (((hw.extkeyin & !(1 << 6)) | pen) as u32) << 16
            }
            0x0400_0138 => hw.rtc.read(),
            0x0400_01C0 => hw.spi.read(),
            0x0400_0204 => hw.exmemcnt as u32,
            0x0400_0180 => ((hw.ipcsync7 & 0x4F00) | ((hw.ipcsync9 >> 8) & 0xF)) as u32,
            0x0400_0208 => hw.irq7.ime as u32,
            0x0400_0210 => hw.irq7.ie,
            0x0400_0214 => hw.irq7.if_,
            0x0400_0240 => hw.vram.vramstat() as u32 | (hw.wramcnt as u32) << 8,
            0x0400_0300 => hw.postflg7 as u32,
            0x0400_0304 => hw.powcnt2 as u32,
            a if sound::Sound::handles(a) => hw.sound.read(a),
            _ => {
                hw.unhandled_io.insert((CpuId::Arm7, addr));
                0
            }
        }
    }

    fn io_write(&mut self, addr: u32, val: u32, mask: u32) {
        if let Some(t) = &mut self.hw.io_trace {
            t.record(self.hw.now, CpuId::Arm7, self.hw.cur_pc[1], addr, true, val & mask);
        }
        self.io_write_raw(addr, val, mask);
    }

    fn io_write_raw(&mut self, addr: u32, val: u32, mask: u32) {
        let hw = &mut *self.hw;
        if hw.io_common_write(1, addr, val, mask) {
            return;
        }
        match addr {
            0x0400_0180 => {
                merge16(&mut hw.ipcsync7, val, mask, 0x4F00);
                if mask & val & (1 << 13) != 0 && hw.ipcsync9 & (1 << 14) != 0 {
                    hw.irq9.request(irq::bits::IPC_SYNC);
                }
            }
            0x0400_0208 => {
                if mask & 1 != 0 {
                    hw.irq7.ime = val & 1 != 0;
                }
            }
            0x0400_0138 => hw.rtc.write(val, mask),
            0x0400_01C0 => {
                if hw.spi.write(val, mask) {
                    hw.irq7.request(irq::bits::SPI);
                }
            }
            0x0400_0204 => {
                // L'ARM7 ne règle que les bits 0–6 (son propre slot 2).
                merge16(&mut hw.exmemcnt, val, mask, 0x007F);
            }
            0x0400_0210 => merge(&mut hw.irq7.ie, val, mask, 0x01FF_3FFF),
            0x0400_0214 => hw.irq7.if_ &= !(val & mask),
            0x0400_0300 => {
                if mask & 0xFF != 0 {
                    hw.postflg7 |= val as u8 & 1;
                }
                // HALTCNT (0x04000301), bits 6–7 : 2 = Halt. GBA et Sleep : à faire.
                if mask & 0xFF00 != 0 && (val >> 14) & 3 == 2 {
                    hw.arm7_halt_request = true;
                }
            }
            0x0400_0304 => {
                if mask & 0xFF != 0 {
                    hw.powcnt2 = val as u8 & 3;
                }
            }
            a if sound::Sound::handles(a) => {
                // Un canal qui démarre lit son en-tête ADPCM sur le bus.
                let mut snd = std::mem::take(&mut hw.sound);
                snd.write(a, val, mask, &mut Arm7Bus { hw: &mut *hw });
                hw.sound = snd;
            }
            _ => {
                hw.unhandled_io.insert((CpuId::Arm7, addr));
            }
        }
    }
}

impl Bus for Arm7Bus<'_> {
    fn read8(&mut self, addr: u32) -> u8 {
        match self.target(addr) {
            Target::Mem(m, o) => m[o],
            Target::Rom(m, o) => m[o],
            Target::Vram(r, o) => self.hw.vram.read8(r, o),
            Target::Io => (self.io_read(addr & !3) >> ((addr & 3) * 8)) as u8,
            Target::Open => 0,
        }
    }
    fn read16(&mut self, addr: u32) -> u16 {
        match self.target(addr) {
            Target::Mem(m, o) => rd16(m, o),
            Target::Rom(m, o) => rd16(m, o),
            Target::Vram(r, o) => self.hw.vram.read16(r, o),
            Target::Io => (self.io_read(addr & !3) >> ((addr & 2) * 8)) as u16,
            Target::Open => 0,
        }
    }
    fn read32(&mut self, addr: u32) -> u32 {
        match self.target(addr) {
            Target::Mem(m, o) => rd32(m, o),
            Target::Rom(m, o) => rd32(m, o),
            Target::Vram(r, o) => self.hw.vram.read32(r, o),
            Target::Io => self.io_read(addr),
            Target::Open => 0,
        }
    }
    fn write8(&mut self, addr: u32, val: u8) {
        self.hw.watch_write(CpuId::Arm7, addr, 1, val as u32);
        match self.target(addr) {
            Target::Mem(m, o) => m[o] = val,
            Target::Vram(r, o) => self.hw.vram.write8(r, o, val),
            Target::Io => {
                let (shift, mask) = lane(addr, 1);
                self.io_write(addr & !3, (val as u32) << shift, mask);
            }
            _ => {}
        }
    }
    fn write16(&mut self, addr: u32, val: u16) {
        self.hw.watch_write(CpuId::Arm7, addr, 2, val as u32);
        match self.target(addr) {
            Target::Mem(m, o) => wr16(m, o, val),
            Target::Vram(r, o) => self.hw.vram.write16(r, o, val),
            Target::Io => {
                let (shift, mask) = lane(addr, 2);
                self.io_write(addr & !3, (val as u32) << shift, mask);
            }
            _ => {}
        }
    }
    fn write32(&mut self, addr: u32, val: u32) {
        self.hw.watch_write(CpuId::Arm7, addr, 4, val);
        match self.target(addr) {
            Target::Mem(m, o) => wr32(m, o, val),
            Target::Vram(r, o) => self.hw.vram.write32(r, o, val),
            Target::Io => self.io_write(addr, val, u32::MAX),
            _ => {}
        }
    }
    fn hle_swi(&mut self, num: u8, regs: &mut [u32; 4]) -> bool {
        swi_hook(self, CpuId::Arm7, num, regs)
    }
}

// ----------------------------------------------------------------------
// La machine
// ----------------------------------------------------------------------

/// Durée maximale d'une tranche d'exécution, en cycles ARM9. Les deux CPU
/// avancent chacun leur tour d'au plus cette durée : plus c'est court, plus
/// leurs échanges par mémoire partagée sont fidèles, plus c'est lent.
pub const MAX_SLICE: u64 = 64;

pub struct Nds {
    pub arm9: Arm9,
    pub arm7: Arm7,
    pub hw: Hardware,
    /// Temps atteint par chaque CPU (cycles ARM9).
    t9: u64,
    t7: u64,
    /// Temps atteint par toute la machine : événements traités jusqu'ici.
    now: u64,
}

impl Default for Nds {
    fn default() -> Self {
        Self::new()
    }
}

impl Nds {
    pub fn new() -> Self {
        let mut nds = Nds { arm9: Arm9::new(), arm7: Arm7::new(), hw: Hardware::default(), t9: 0, t7: 0, now: 0 };
        nds.hw.sched.schedule(Event::HBlank, lcd::HBLANK_START);
        nds.hw.sched.schedule(Event::LineEnd, LINE_CYCLES);
        nds.hw.sched.schedule(Event::Sound, sound::SAMPLE_CYCLES);
        nds
    }

    /// Vue ARM9 du matériel (débogueur, tests, chargement).
    pub fn bus9(&mut self) -> Arm9Bus<'_> {
        self.hw.now = self.now;
        Arm9Bus { hw: &mut self.hw }
    }

    pub fn bus7(&mut self) -> Arm7Bus<'_> {
        self.hw.now = self.now;
        Arm7Bus { hw: &mut self.hw }
    }

    /// Temps écoulé depuis la mise sous tension, en cycles ARM9.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Charge un BIOS ARM9 (4 Ko, complété par des zéros s'il est plus court).
    pub fn load_bios9(&mut self, data: &[u8]) {
        let n = data.len().min(BIOS9_SIZE);
        self.hw.bios9[..n].copy_from_slice(&data[..n]);
    }

    /// Charge un BIOS ARM7 (16 Ko).
    pub fn load_bios7(&mut self, data: &[u8]) {
        let n = data.len().min(BIOS7_SIZE);
        self.hw.bios7[..n].copy_from_slice(&data[..n]);
    }

    /// Pose (Some(x, y), en pixels de l'écran du bas) ou lève (None) le stylet.
    pub fn set_touch(&mut self, pen: Option<(u8, u8)>) {
        let pen = pen.map(|(x, y)| (x, y.min(191)));
        self.hw.spi.touch.pen = pen; // EXTKEYIN bit 6 en découle à la lecture
    }

    /// Remplace le firmware synthétique par une image de 256 Ko (par
    /// exemple un dump de sa propre console).
    pub fn load_firmware(&mut self, data: &[u8]) {
        let fw = &mut self.hw.spi.firmware.data;
        let n = data.len().min(fw.len());
        fw[..n].copy_from_slice(&data[..n]);
    }

    /// Charge une sauvegarde (contenu d'un `.sav`). Le type de puce sera
    /// détecté d'après les accès du jeu.
    pub fn load_save(&mut self, bytes: &[u8]) {
        self.hw.slot1.save = save::Save::load(bytes);
    }

    /// Impose le type de puce (et sa taille), avec un contenu éventuel.
    pub fn force_save_type(&mut self, family: spimem::Family, size: usize, bytes: &[u8]) {
        self.hw.slot1.save = save::Save::forced(family, size, bytes);
    }

    /// Contenu à écrire dans le `.sav` (None : rien à sauvegarder).
    /// Diagnostic du rendu 3D (voir `gpu3d::report`).
    pub fn report_3d(&self) -> String {
        let disp3dcnt = self.hw.gpu.engines[0].regs[0x60 / 4] & 0xFFFF;
        let tex = gpu3d::texture::TexMem::from_vram(&self.hw.vram);
        let mut out = gpu3d::report(&self.hw.gpu3d, disp3dcnt, &self.hw.vram.cnt, &tex);
        // Registres 2D utiles à la composition du calque 3D (BG0 du moteur A).
        out.push_str(&format!("POWCNT1 {:04X}\n", self.hw.powcnt1));
        for (e, name) in [(0, "A"), (1, "B")] {
            let r = &self.hw.gpu.engines[e].regs;
            out.push_str(&format!(
                "moteur {name} : DISPCNT {:08X}  BG0–3CNT {:08X} {:08X}  WIN {:08X} {:08X} {:08X}  BLDCNT {:04X}  BLDALPHA {:04X}  BLDY {:02X}  DISPCAPCNT {:08X}\n",
                r[0],
                r[2],
                r[3],
                r[0x40 / 4],
                r[0x44 / 4],
                r[0x48 / 4],
                r[0x50 / 4] & 0xFFFF,
                r[0x50 / 4] >> 16,
                r[0x54 / 4] & 0x1F,
                r[0x64 / 4],
            ));
        }
        out
    }

    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.hw.slot1.save.export()
    }

    /// Le jeu a écrit dans sa sauvegarde depuis le dernier `clear_save_dirty`.
    pub fn save_dirty(&self) -> bool {
        self.hw.slot1.save.dirty()
    }

    pub fn clear_save_dirty(&mut self) {
        self.hw.slot1.save.clear_dirty();
    }

    /// Reset des deux CPU sur leur vecteur de reset.
    pub fn reset(&mut self) {
        self.arm9.reset(&mut Arm9Bus { hw: &mut self.hw });
        self.arm7.reset(&mut Arm7Bus { hw: &mut self.hw });
    }

    fn sync_irq9(&mut self) {
        self.arm9.irq_line = self.hw.irq9.line();
        if self.arm9.halted && self.hw.irq9.wake() {
            self.arm9.halted = false;
        }
    }

    fn sync_irq7(&mut self) {
        self.arm7.irq_line = self.hw.irq7.line();
        if self.arm7.halted && self.hw.irq7.wake() {
            self.arm7.halted = false;
        }
    }

    /// Une instruction ARM9 (ou un cycle s'il est en pause).
    fn exec9(&mut self) -> u64 {
        self.hw.now = self.t9;
        self.sync_irq9();
        if self.arm9.halted {
            return 1;
        }
        if self.hw.io_trace.is_some() {
            self.hw.cur_pc[0] = self.arm9.pc();
        }
        self.arm9.step(&mut Arm9Bus { hw: &mut self.hw }) as u64
    }

    /// Une instruction ARM7, en cycles ARM9.
    fn exec7(&mut self) -> u64 {
        self.hw.now = self.t7;
        self.sync_irq7();
        if self.arm7.halted {
            return BUS_CYCLE;
        }
        if self.hw.io_trace.is_some() {
            self.hw.cur_pc[1] = self.arm7.pc();
        }
        let c = self.arm7.step(&mut Arm7Bus { hw: &mut self.hw }) as u64 * BUS_CYCLE;
        if self.hw.arm7_halt_request {
            self.hw.arm7_halt_request = false;
            self.arm7.halted = true;
        }
        c
    }

    fn process_events(&mut self) {
        while let Some((ev, t)) = self.hw.sched.pop_due(self.now) {
            self.hw.handle_event(ev, t);
        }
    }

    /// Fait avancer la machine jusqu'à l'instant `end` (en cycles ARM9).
    pub fn run_until(&mut self, end: u64) {
        while self.now < end {
            self.process_events();
            let target = end.min(self.now + MAX_SLICE).min(self.hw.sched.next_time());
            while self.t9 < target {
                if self.arm9.halted && !self.hw.irq9.wake() {
                    self.t9 = target; // rien à faire avant le prochain événement
                    break;
                }
                self.t9 += self.exec9();
                // Une écriture I/O a pu programmer un événement plus tôt.
                if self.hw.sched.next_time() < target {
                    break;
                }
            }
            let target = target.min(self.t9.max(self.now + 1));
            while self.t7 < target {
                if self.arm7.halted && !self.hw.irq7.wake() {
                    self.t7 = target;
                    break;
                }
                self.t7 += self.exec7();
            }
            self.now = target.max(self.now);
        }
        self.process_events();
    }

    /// Avance **exactement** jusqu'au début du prochain VBlank (une image) :
    /// les CPU ne vont pas au-delà, le frontend peut lire l'image.
    pub fn run_frame(&mut self) {
        let line_end = self.hw.sched.time_of(Event::LineEnd).expect("événement de ligne toujours programmé");
        let next_line = (self.hw.lcd.vcount + 1) % lcd::LINES;
        let lines_to_go = (lcd::VISIBLE_LINES + lcd::LINES - next_line) % lcd::LINES;
        self.run_until(line_end + lines_to_go as u64 * LINE_CYCLES);
    }

    /// Exécute une instruction ARM9 et la part correspondante de l'ARM7
    /// (pas à pas pour le débogage et les tests).
    pub fn step(&mut self) {
        self.process_events();
        self.t9 += self.exec9();
        while self.t7 < self.t9 {
            self.t7 += self.exec7();
        }
        self.now = self.now.max(self.t9.min(self.t7));
        self.process_events();
    }
}
