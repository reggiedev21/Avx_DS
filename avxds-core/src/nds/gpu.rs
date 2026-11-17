//! Les deux moteurs 2D (A et B) et la composition des deux écrans.
//!
//! Rendu par ligne, au début du HBlank de chaque ligne visible. Gérés :
//! modes d'affichage (éteint, graphique, VRAM directe, FIFO non géré), fonds
//! texte 4 et 8 bits, fonds affines, fonds étendus (tuiles 16 bits, bitmap
//! 256 couleurs, bitmap direct), grand bitmap (mode 6), priorités, couleur
//! de fond, master brightness, échange des écrans (POWCNT1 bit 15).
//!
//! Sprites (normaux, affines, bitmap), fenêtres (WIN0, WIN1, OBJ) et
//! mélange (alpha, éclaircir, assombrir, sprites semi-transparents).
//!
//! Pas encore : mosaïque, palettes étendues, 3D, capture. Leur emploi est noté dans
//! [`Gpu::unsupported`].

use super::vram::{Region, Vram};
use std::collections::BTreeSet;

pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 192;

/// Une couleur BGR555 de la DS vers 0x00RRGGBB.
#[inline]
pub fn rgb555_to_rgb888(c: u16) -> u32 {
    let ch = |v: u16| {
        let v = (v & 0x1F) as u32;
        (v << 3) | (v >> 2)
    };
    (ch(c) << 16) | (ch(c >> 5) << 8) | ch(c >> 10)
}

/// Paramètres d'un fond affine (BG2 ou BG3).
#[derive(Clone, Copy, Debug, Default)]
pub struct Affine {
    pub pa: i16,
    pub pb: i16,
    pub pc: i16,
    pub pd: i16,
    /// Points de référence BGxX/BGxY (28 bits signés, 8 bits de fraction).
    pub x: i32,
    pub y: i32,
    /// Copies internes, avancées de PB/PD à chaque ligne.
    pub ix: i32,
    pub iy: i32,
}

/// Un moteur 2D. Les registres sont gardés tels qu'écrits (mots de 32 bits
/// à partir de 0x04000000 ou 0x04001000) et décodés au rendu.
#[derive(Clone, Debug)]
pub struct Engine {
    pub is_a: bool,
    pub regs: [u32; 28],
    pub affine: [Affine; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BgKind {
    Text,
    Affine,
    Extended,
    LargeBitmap,
}

impl Engine {
    fn new(is_a: bool) -> Self {
        Engine { is_a, regs: [0; 28], affine: [Affine::default(); 2] }
    }

    pub fn dispcnt(&self) -> u32 {
        self.regs[0]
    }

    pub fn bgcnt(&self, bg: usize) -> u16 {
        (self.regs[2 + bg / 2] >> ((bg & 1) * 16)) as u16
    }

    fn scroll(&self, bg: usize) -> (u32, u32) {
        let r = self.regs[4 + bg];
        (r & 0x1FF, (r >> 16) & 0x1FF)
    }

    fn master_bright(&self) -> u32 {
        self.regs[0x6C / 4]
    }

    fn bg_kind(&self, bg: usize) -> Option<BgKind> {
        use BgKind::*;
        let mode = self.dispcnt() & 7;
        let table: [[Option<BgKind>; 4]; 8] = [
            [Some(Text), Some(Text), Some(Text), Some(Text)],
            [Some(Text), Some(Text), Some(Text), Some(Affine)],
            [Some(Text), Some(Text), Some(Affine), Some(Affine)],
            [Some(Text), Some(Text), Some(Text), Some(Extended)],
            [Some(Text), Some(Text), Some(Affine), Some(Extended)],
            [Some(Text), Some(Text), Some(Extended), Some(Extended)],
            [Some(Text), None, Some(LargeBitmap), None],
            [None, None, None, None],
        ];
        table[mode as usize][bg]
    }

    /// Lecture d'un registre (seuls certains sont lisibles sur la DS).
    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x00 | 0x08 | 0x0C | 0x48 | 0x50 | 0x60 | 0x64 | 0x6C => self.regs[(off / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, val: u32, mask: u32) {
        let i = (off / 4) as usize;
        if i >= self.regs.len() {
            return;
        }
        self.regs[i] = (self.regs[i] & !mask) | (val & mask);
        let r = self.regs[i];
        let sext28 = |v: u32| ((v << 4) as i32) >> 4;
        match off {
            0x20 | 0x30 => {
                let a = &mut self.affine[((off - 0x20) / 0x10) as usize];
                a.pa = r as i16;
                a.pb = (r >> 16) as i16;
            }
            0x24 | 0x34 => {
                let a = &mut self.affine[((off - 0x24) / 0x10) as usize];
                a.pc = r as i16;
                a.pd = (r >> 16) as i16;
            }
            // Écrire un point de référence recharge aussi la copie interne.
            0x28 | 0x38 => {
                let a = &mut self.affine[((off - 0x28) / 0x10) as usize];
                a.x = sext28(r);
                a.ix = a.x;
            }
            0x2C | 0x3C => {
                let a = &mut self.affine[((off - 0x2C) / 0x10) as usize];
                a.y = sext28(r);
                a.iy = a.y;
            }
            _ => {}
        }
    }

    /// Début d'image : les copies internes repartent des registres.
    fn reload_affine(&mut self) {
        for a in &mut self.affine {
            a.ix = a.x;
            a.iy = a.y;
        }
    }
}

pub struct Gpu {
    pub engines: [Engine; 2],
    /// Image de chaque écran, 0x00RRGGBB : indice 0 = écran du haut.
    pub screens: [Box<[u32]>; 2],
    /// Fonctions demandées par les jeux mais pas encore rendues.
    pub unsupported: BTreeSet<&'static str>,
}

impl Default for Gpu {
    fn default() -> Self {
        Gpu {
            engines: [Engine::new(true), Engine::new(false)],
            screens: [vec![0; WIDTH * HEIGHT].into_boxed_slice(), vec![0; WIDTH * HEIGHT].into_boxed_slice()],
            unsupported: BTreeSet::new(),
        }
    }
}

/// Ce dont le rendu a besoin dans le reste de la machine.
pub struct RenderCtx<'a> {
    pub vram: &'a Vram,
    pub palette: &'a [u8],
    pub oam: &'a [u8],
    pub powcnt1: u16,
}

impl Gpu {
    /// Accès I/O de l'ARM9 entre 0x04000000 et 0x0400106F (hors DISPSTAT
    /// et VCOUNT). Renvoie `None` si l'adresse n'est pas au GPU.
    pub fn io_read(&self, addr: u32) -> Option<u32> {
        let (e, off) = Self::locate(addr)?;
        Some(self.engines[e].read(off))
    }

    pub fn io_write(&mut self, addr: u32, val: u32, mask: u32) -> bool {
        let Some((e, off)) = Self::locate(addr) else { return false };
        self.engines[e].write(off, val, mask);
        true
    }

    fn locate(addr: u32) -> Option<(usize, u32)> {
        match addr {
            0x0400_0000..=0x0400_006F => Some((0, addr - 0x0400_0000)),
            0x0400_1000..=0x0400_106F if !(0x0400_1060..=0x0400_1068).contains(&addr) => {
                Some((1, addr - 0x0400_1000))
            }
            _ => None,
        }
    }

    pub fn start_frame(&mut self) {
        for e in &mut self.engines {
            e.reload_affine();
        }
    }

    /// Dessine la ligne `y` des deux écrans.
    pub fn render_line(&mut self, y: usize, ctx: &RenderCtx) {
        let mut line = [0u16; WIDTH];
        for e in 0..2 {
            let screen = if (ctx.powcnt1 & 0x8000 != 0) == (e == 0) { 0 } else { 1 };
            if ctx.powcnt1 & 1 == 0 {
                line = [0; WIDTH]; // écrans éteints : noir
            } else {
                self.render_engine_line(e, y, ctx, &mut line);
            }
            let out = &mut self.screens[screen][y * WIDTH..(y + 1) * WIDTH];
            let mb = self.engines[e].master_bright();
            for (o, &c) in out.iter_mut().zip(line.iter()) {
                *o = rgb555_to_rgb888(master_brightness(c, mb));
            }
        }
        for e in &mut self.engines {
            for a in &mut e.affine {
                a.ix = a.ix.wrapping_add(a.pb as i32);
                a.iy = a.iy.wrapping_add(a.pd as i32);
            }
        }
    }

    fn render_engine_line(&mut self, e: usize, y: usize, ctx: &RenderCtx, line: &mut [u16; WIDTH]) {
        let eng = &self.engines[e];
        let dispcnt = eng.dispcnt();
        let display_mode = if eng.is_a { (dispcnt >> 16) & 3 } else { (dispcnt >> 16) & 1 };
        const WHITE: u16 = 0x7FFF;
        if dispcnt & 0x80 != 0 {
            *line = [WHITE; WIDTH]; // « forced blank »
            return;
        }
        match display_mode {
            0 => *line = [WHITE; WIDTH],
            2 => {
                // Affichage direct d'une banque VRAM (A à D) en 256×192×16 bits.
                let bank = &ctx.vram.banks[((dispcnt >> 18) & 3) as usize];
                for (x, px) in line.iter_mut().enumerate() {
                    let o = (y * WIDTH + x) * 2;
                    *px = u16::from_le_bytes([bank[o], bank[o + 1]]) & 0x7FFF;
                }
            }
            3 => {
                self.unsupported.insert("affichage depuis la FIFO de la Main RAM");
                *line = [0; WIDTH];
            }
            _ => self.render_graphics(e, y, ctx, line),
        }
    }

    fn render_graphics(&mut self, e: usize, y: usize, ctx: &RenderCtx, line: &mut [u16; WIDTH]) {
        let eng = &self.engines[e];
        let dispcnt = eng.dispcnt();
        let pal_base = if eng.is_a { 0 } else { 0x400 };
        let region = if eng.is_a { Region::BgA } else { Region::BgB };
        let pal = |i: usize| -> u16 {
            let o = pal_base + i * 2;
            u16::from_le_bytes([ctx.palette[o], ctx.palette[o + 1]]) & 0x7FFF
        };
        let mut notes: Vec<&'static str> = Vec::new();
        if dispcnt & (1 << 30) != 0 {
            notes.push("palettes étendues de fonds");
        }

        // 1. Chaque fond actif dessine sa ligne.
        let mut bgs = [[None::<u16>; WIDTH]; 4];
        let mut bg_on = [false; 4];
        for bg in 0..4 {
            if dispcnt & (1 << (8 + bg)) == 0 {
                continue;
            }
            let Some(kind) = eng.bg_kind(bg) else { continue };
            if bg == 0 && eng.is_a && dispcnt & 8 != 0 {
                notes.push("3D (BG0)");
                continue;
            }
            if eng.bgcnt(bg) & 0x40 != 0 {
                notes.push("mosaïque");
            }
            match kind {
                BgKind::Text => eng.text_line(bg, y, ctx.vram, region, &pal, &mut bgs[bg]),
                _ => eng.affine_line(bg, kind, ctx.vram, region, &pal, &mut bgs[bg]),
            }
            bg_on[bg] = true;
        }

        // 2. Les sprites.
        let mut obj = [ObjPixel::NONE; WIDTH];
        if dispcnt & (1 << 12) != 0 {
            eng.obj_line(y, ctx, &mut obj, &mut notes);
        }

        // 3. Composition : pour chaque pixel, les deux couches du dessus.
        let backdrop = pal(0);
        let r = |off: u32| eng.regs[(off / 4) as usize];
        let (win0h, win1h) = (r(0x40) & 0xFFFF, r(0x40) >> 16);
        let (win0v, win1v) = (r(0x44) & 0xFFFF, r(0x44) >> 16);
        let (winin, winout) = (r(0x48) & 0xFFFF, r(0x48) >> 16);
        let bldcnt = r(0x50) & 0xFFFF;
        let bldalpha = r(0x50) >> 16;
        let effect = (bldcnt >> 6) & 3;
        let (first, second) = (bldcnt & 0x3F, (bldcnt >> 8) & 0x3F);
        let eva = (bldalpha & 0x1F).min(16) as u16;
        let evb = ((bldalpha >> 8) & 0x1F).min(16) as u16;
        let evy = (r(0x54) & 0x1F).min(16) as u16;
        // Fin exclusive. X1 > X2 (ou Y1 > Y2) : la fenêtre va jusqu'au bord
        // (GBATEK, page GBA « Window Feature », transposé à 256×192).
        let inside = |v: u32, reg: u32| {
            let (lo, hi) = ((reg >> 8) & 0xFF, reg & 0xFF);
            if lo <= hi { lo <= v && v < hi } else { v >= lo }
        };
        let y32 = y as u32;
        let in_win0 = dispcnt & (1 << 13) != 0 && inside(y32, win0v);
        let in_win1 = dispcnt & (1 << 14) != 0 && inside(y32, win1v);
        let windows = dispcnt & 0xE000 != 0;

        for x in 0..WIDTH {
            let x32 = x as u32;
            // Couches visibles (bits 0–4) et effets autorisés (bit 5) ici.
            let mask = if !windows {
                0x3F
            } else if in_win0 && inside(x32, win0h) {
                winin & 0x3F
            } else if in_win1 && inside(x32, win1h) {
                (winin >> 8) & 0x3F
            } else if dispcnt & (1 << 15) != 0 && obj[x].window {
                (winout >> 8) & 0x3F
            } else {
                winout & 0x3F
            };

            // (clé de tri, bit de la couche, couleur) ; plus petite clé devant.
            const BACKDROP: (u8, u32, u16) = (u8::MAX, 1 << 5, 0);
            let mut top = (BACKDROP.0, BACKDROP.1, backdrop);
            let mut below = top;
            let mut offer = |key: u8, bit: u32, c: u16| {
                if key < top.0 {
                    below = top;
                    top = (key, bit, c);
                } else if key < below.0 {
                    below = (key, bit, c);
                }
            };
            let o = obj[x];
            if o.prio < 4 && mask & 0x10 != 0 {
                offer(o.prio * 8, 1 << 4, o.color); // à priorité égale, l'OBJ passe devant
            }
            for bg in 0..4 {
                if bg_on[bg] && mask & (1 << bg) != 0 {
                    if let Some(c) = bgs[bg][x] {
                        offer((eng.bgcnt(bg) & 3) as u8 * 8 + 1 + bg as u8, 1 << bg, c);
                    }
                }
            }

            let semi_obj = top.1 == 1 << 4 && o.semi;
            line[x] = if semi_obj && second & below.1 != 0 {
                blend_alpha(top.2, below.2, eva, evb)
            } else if mask & 0x20 != 0 && first & top.1 != 0 {
                match effect {
                    1 if second & below.1 != 0 => blend_alpha(top.2, below.2, eva, evb),
                    2 => brightness(top.2, evy, true),
                    3 => brightness(top.2, evy, false),
                    _ => top.2,
                }
            } else {
                top.2
            };
        }
        for n in notes {
            self.unsupported.insert(n);
        }
    }

    /// Image complète d'un écran (pour les captures et le frontend).
    pub fn screen(&self, n: usize) -> &[u32] {
        &self.screens[n]
    }
}

/// Mélange alpha : (A × EVA + B × EVB) / 16, saturé à 31 par composante.
fn blend_alpha(a: u16, b: u16, eva: u16, evb: u16) -> u16 {
    let ch = |s: u32| -> u16 {
        let (ca, cb) = ((a >> s) & 0x1F, (b >> s) & 0x1F);
        ((ca * eva + cb * evb) / 16).min(31) << s
    };
    ch(0) | ch(5) | ch(10)
}

/// Éclaircissement (vers le blanc) ou assombrissement (vers le noir).
fn brightness(c: u16, evy: u16, up: bool) -> u16 {
    let ch = |s: u32| -> u16 {
        let v = (c >> s) & 0x1F;
        (if up { v + (31 - v) * evy / 16 } else { v - v * evy / 16 }) << s
    };
    ch(0) | ch(5) | ch(10)
}

fn master_brightness(c: u16, reg: u32) -> u16 {
    let factor = (reg & 0x1F).min(16) as u16;
    let mode = (reg >> 14) & 3;
    if factor == 0 || mode == 0 || mode == 3 {
        return c;
    }
    let ch = |v: u16| -> u16 {
        let v = v & 0x1F;
        if mode == 1 { v + (31 - v) * factor / 16 } else { v - v * factor / 16 }
    };
    ch(c) | ch(c >> 5) << 5 | ch(c >> 10) << 10
}

/// Pixel de la couche des sprites.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ObjPixel {
    color: u16,
    /// 0–3, ou 4 = aucun sprite.
    prio: u8,
    /// Sprite semi-transparent (mode 1) : mélangé avec la couche dessous.
    semi: bool,
    /// Couvert par un sprite de fenêtre (mode 2).
    window: bool,
}

impl ObjPixel {
    const NONE: ObjPixel = ObjPixel { color: 0, prio: 4, semi: false, window: false };
}

/// (largeur, hauteur) selon la forme (carré, horizontal, vertical) et la taille.
const OBJ_SIZES: [[(i32, i32); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

impl Engine {
    /// Dessine les sprites qui coupent la ligne `y`. Ordre OAM : un sprite
    /// ne remplace un pixel déjà posé que s'il a une priorité plus forte.
    fn obj_line(&self, y: usize, ctx: &RenderCtx, out: &mut [ObjPixel; WIDTH], notes: &mut Vec<&'static str>) {
        let oam = if self.is_a { &ctx.oam[..0x400] } else { &ctx.oam[0x400..0x800] };
        let rd16 = |o: usize| u16::from_le_bytes([oam[o], oam[o + 1]]);
        let pal_base = if self.is_a { 0x200 } else { 0x600 };
        let pal = |i: usize| {
            let o = pal_base + i * 2;
            u16::from_le_bytes([ctx.palette[o], ctx.palette[o + 1]]) & 0x7FFF
        };
        let region = if self.is_a { Region::ObjA } else { Region::ObjB };
        let dispcnt = self.dispcnt();
        let one_d = dispcnt & (1 << 4) != 0;
        let tile_step = 32u32 << ((dispcnt >> 20) & 3);
        if dispcnt & (1 << 31) != 0 {
            notes.push("palettes étendues de sprites");
        }

        for i in 0..128 {
            let a0 = rd16(i * 8) as u32;
            let a1 = rd16(i * 8 + 2) as u32;
            let a2 = rd16(i * 8 + 4) as u32;
            let affine = a0 & 0x100 != 0;
            if !affine && a0 & 0x200 != 0 {
                continue; // sprite masqué
            }
            let shape = (a0 >> 14) as usize;
            if shape == 3 {
                continue;
            }
            let (w, h) = OBJ_SIZES[shape][(a1 >> 14) as usize];
            let double = affine && a0 & 0x200 != 0;
            let (bw, bh) = if double { (2 * w, 2 * h) } else { (w, h) };
            let line = ((y as u32).wrapping_sub(a0 & 0xFF) & 0xFF) as i32;
            if line >= bh {
                continue;
            }
            let mut sx = (a1 & 0x1FF) as i32;
            if sx >= 256 {
                sx -= 512;
            }
            let mode = (a0 >> 10) & 3;
            if a0 & 0x1000 != 0 {
                notes.push("mosaïque");
            }
            let prio = ((a2 >> 10) & 3) as u8;
            let tile = a2 & 0x3FF;
            let color256 = a0 & 0x2000 != 0;
            let (pa, pb, pc, pd) = if affine {
                let p = ((a1 >> 9) & 0x1F) as usize * 32;
                let g = |k: usize| rd16(p + 6 + 8 * k) as i16 as i32;
                (g(0), g(1), g(2), g(3))
            } else {
                (0x100, 0, 0, 0x100)
            };
            if mode == 3 && a2 >> 12 != 15 {
                notes.push("alpha des sprites bitmap");
            }

            for bx in 0..bw {
                let screen_x = sx + bx;
                if !(0..WIDTH as i32).contains(&screen_x) {
                    continue;
                }
                let (tx, ty) = if affine {
                    let (dx, dy) = (bx - bw / 2, line - bh / 2);
                    let tx = ((pa * dx + pb * dy) >> 8) + w / 2;
                    let ty = ((pc * dx + pd * dy) >> 8) + h / 2;
                    if tx < 0 || ty < 0 || tx >= w || ty >= h {
                        continue;
                    }
                    (tx as u32, ty as u32)
                } else {
                    let tx = if a1 & 0x1000 != 0 { w - 1 - bx } else { bx };
                    let ty = if a1 & 0x2000 != 0 { h - 1 - line } else { line };
                    (tx as u32, ty as u32)
                };
                let w = w as u32;
                let color = if mode == 3 {
                    // Sprite bitmap : 16 bits par pixel, bit 15 = opaque.
                    let (base, stride) = if dispcnt & (1 << 6) != 0 {
                        (tile * (128 << ((dispcnt >> 22) & 1)), w)
                    } else if dispcnt & (1 << 5) != 0 {
                        ((tile & 0x1F) * 0x10 + (tile & 0x3E0) * 0x80, 256)
                    } else {
                        ((tile & 0x0F) * 0x10 + (tile & 0x3F0) * 0x80, 128)
                    };
                    let c = ctx.vram.read16(region, base + (ty * stride + tx) * 2);
                    (c & 0x8000 != 0).then_some(c & 0x7FFF)
                } else if color256 {
                    let off = if one_d {
                        tile * tile_step + ((ty / 8) * (w / 8) + tx / 8) * 64
                    } else {
                        (tile & !1) * 32 + (ty / 8) * 1024 + (tx / 8) * 64
                    } + (ty % 8) * 8 + tx % 8;
                    let idx = ctx.vram.read8(region, off) as usize;
                    (idx != 0).then(|| pal(idx))
                } else {
                    let off = if one_d {
                        tile * tile_step + ((ty / 8) * (w / 8) + tx / 8) * 32
                    } else {
                        tile * 32 + (ty / 8) * 1024 + (tx / 8) * 32
                    } + (ty % 8) * 4 + (tx % 8) / 2;
                    let b = ctx.vram.read8(region, off);
                    let idx = ((b >> ((tx & 1) * 4)) & 0xF) as usize;
                    (idx != 0).then(|| pal((a2 >> 12) as usize * 16 + idx))
                };
                let Some(c) = color else { continue };
                let px = &mut out[screen_x as usize];
                if mode == 2 {
                    px.window = true;
                } else if prio < px.prio {
                    *px = ObjPixel { color: c, prio, semi: mode == 1, window: px.window };
                }
            }
        }
    }

    fn char_and_screen_base(&self, bg: usize) -> (u32, u32) {
        let cnt = self.bgcnt(bg) as u32;
        let mut cb = ((cnt >> 2) & 0xF) * 0x4000;
        let mut sb = ((cnt >> 8) & 0x1F) * 0x800;
        if self.is_a {
            cb += ((self.dispcnt() >> 24) & 7) * 0x1_0000;
            sb += ((self.dispcnt() >> 27) & 7) * 0x1_0000;
        }
        (cb, sb)
    }

    fn text_line(&self, bg: usize, y: usize, vram: &Vram, region: Region, pal: &impl Fn(usize) -> u16, out: &mut [Option<u16>; WIDTH]) {
        let cnt = self.bgcnt(bg) as u32;
        let (cb, sb) = self.char_and_screen_base(bg);
        let color256 = cnt & 0x80 != 0;
        let size = cnt >> 14;
        let width = if size & 1 != 0 { 512 } else { 256 };
        let height = if size & 2 != 0 { 512 } else { 256 };
        let (hofs, vofs) = self.scroll(bg);
        let py = (y as u32 + vofs) % height;
        for (x, px) in out.iter_mut().enumerate() {
            let tx_all = (x as u32 + hofs) % width;
            let block = tx_all / 256 + (py / 256) * (width / 256);
            let map = sb + block * 0x800 + ((py % 256) / 8 * 32 + (tx_all % 256) / 8) * 2;
            let entry = vram.read16(region, map) as u32;
            let tile = entry & 0x3FF;
            let mut tx = tx_all % 8;
            let mut ty = py % 8;
            if entry & 0x400 != 0 {
                tx = 7 - tx;
            }
            if entry & 0x800 != 0 {
                ty = 7 - ty;
            }
            let color = if color256 {
                let idx = vram.read8(region, cb + tile * 64 + ty * 8 + tx) as usize;
                (idx != 0).then(|| pal(idx))
            } else {
                let b = vram.read8(region, cb + tile * 32 + ty * 4 + tx / 2);
                let idx = ((b >> ((tx & 1) * 4)) & 0xF) as usize;
                (idx != 0).then(|| pal((entry >> 12) as usize * 16 + idx))
            };
            *px = color;
        }
    }

    fn affine_line(&self, bg: usize, kind: BgKind, vram: &Vram, region: Region, pal: &impl Fn(usize) -> u16, out: &mut [Option<u16>; WIDTH]) {
        let cnt = self.bgcnt(bg) as u32;
        let a = self.affine[bg - 2];
        let (cb, sb) = self.char_and_screen_base(bg);
        let wrap = cnt & 0x2000 != 0;
        let size = (cnt >> 14) as usize;

        // Dimensions et nature du fond.
        enum Src {
            Tiles8,
            Tiles16,
            Bitmap256(u32),
            Direct(u32),
        }
        let (w, h, src) = match kind {
            BgKind::Affine => (128 << size, 128 << size, Src::Tiles8),
            BgKind::Extended if cnt & 0x80 == 0 => (128 << size, 128 << size, Src::Tiles16),
            BgKind::Extended => {
                let (w, h) = [(128, 128), (256, 256), (512, 256), (512, 512)][size];
                let base = ((cnt >> 8) & 0x1F) * 0x4000;
                (w, h, if cnt & 4 != 0 { Src::Direct(base) } else { Src::Bitmap256(base) })
            }
            _ => {
                let (w, h) = if size & 1 == 0 { (512, 1024) } else { (1024, 512) };
                (w, h, Src::Bitmap256(0))
            }
        };
        for (x, px) in out.iter_mut().enumerate() {
            let fx = a.ix.wrapping_add(a.pa as i32 * x as i32) >> 8;
            let fy = a.iy.wrapping_add(a.pc as i32 * x as i32) >> 8;
            let (tx, ty) = if wrap {
                (fx.rem_euclid(w) as u32, fy.rem_euclid(h) as u32)
            } else if fx < 0 || fy < 0 || fx >= w || fy >= h {
                *px = None;
                continue;
            } else {
                (fx as u32, fy as u32)
            };
            *px = match src {
                Src::Tiles8 => {
                    let tile = vram.read8(region, sb + (ty / 8) * (w as u32 / 8) + tx / 8) as u32;
                    let idx = vram.read8(region, cb + tile * 64 + (ty % 8) * 8 + tx % 8) as usize;
                    (idx != 0).then(|| pal(idx))
                }
                Src::Tiles16 => {
                    let entry = vram.read16(region, sb + ((ty / 8) * (w as u32 / 8) + tx / 8) * 2) as u32;
                    let (mut ux, mut uy) = (tx % 8, ty % 8);
                    if entry & 0x400 != 0 {
                        ux = 7 - ux;
                    }
                    if entry & 0x800 != 0 {
                        uy = 7 - uy;
                    }
                    let idx = vram.read8(region, cb + (entry & 0x3FF) * 64 + uy * 8 + ux) as usize;
                    (idx != 0).then(|| pal(idx))
                }
                Src::Bitmap256(base) => {
                    let idx = vram.read8(region, base + ty * w as u32 + tx) as usize;
                    (idx != 0).then(|| pal(idx))
                }
                Src::Direct(base) => {
                    let c = vram.read16(region, base + (ty * w as u32 + tx) * 2);
                    (c & 0x8000 != 0).then_some(c & 0x7FFF)
                }
            };
        }
    }
}
