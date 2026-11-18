//! Textures (étape 3D-4) : mémoire des textures et des palettes vue par le
//! moteur 3D, décodage des 7 formats, répétition et miroir.
//!
//! Sources : GBATEK « Texture Attributes », « Texture Formats », « Memory
//! Control - VRAM » (`docs/3D_SPEC.md`, section 11).

use super::super::vram::{GpuRole, Mapping, Vram};

/// Taille de l'espace des textures : 4 slots de 128 Ko.
pub const TEX_SIZE: usize = 4 * 0x2_0000;
/// Taille de l'espace des palettes : 6 slots de 16 Ko.
pub const PAL_SIZE: usize = 6 * 0x4000;

/// Copie de la mémoire des textures et des palettes, prise au moment du
/// rendu. Les banques en mode texture ne sont pas visibles du CPU : seul le
/// moteur 3D les lit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexMem {
    pub tex: Vec<u8>,
    pub pal: Vec<u8>,
}

impl Default for TexMem {
    fn default() -> Self {
        TexMem { tex: vec![0; TEX_SIZE], pal: vec![0; PAL_SIZE] }
    }
}

impl TexMem {
    /// Assemble les banques attribuées aux textures (A–D, slot = OFS) et aux
    /// palettes (E : slots 0–3 ; F, G : slot = OFS.0 + 4 × OFS.1).
    /// Banques superposées : la dernière l'emporte (hypothèse).
    pub fn from_vram(vram: &Vram) -> TexMem {
        let mut m = TexMem::default();
        for (bank, map) in vram.mapping.iter().enumerate() {
            match *map {
                Mapping::Gpu(GpuRole::Texture(slot)) => {
                    let base = slot as usize * 0x2_0000;
                    m.tex[base..base + 0x2_0000].copy_from_slice(&vram.banks[bank][..0x2_0000]);
                }
                Mapping::Gpu(GpuRole::TexturePalette(ofs)) => {
                    let data = &vram.banks[bank];
                    let base = if bank == 4 { 0 } else { ((ofs & 1) as usize + 4 * (ofs >> 1) as usize) * 0x4000 };
                    let n = data.len().min(PAL_SIZE - base);
                    m.pal[base..base + n].copy_from_slice(&data[..n]);
                }
                _ => {}
            }
        }
        m
    }

    fn tex8(&self, addr: usize) -> u8 {
        self.tex[addr % TEX_SIZE]
    }

    fn tex16(&self, addr: usize) -> u16 {
        u16::from_le_bytes([self.tex8(addr), self.tex8(addr + 1)])
    }

    fn pal16(&self, addr: usize) -> u16 {
        // Adresses paires (couleurs de 2 octets) ; 6 slots de 16 Ko : pas une
        // puissance de 2, d'où une comparaison plutôt qu'un modulo.
        let a = if addr < PAL_SIZE - 1 { addr } else { (addr % PAL_SIZE) & !1 };
        u16::from_le_bytes([self.pal[a], self.pal[a + 1]])
    }
}

/// Texel décodé : couleur 5 bits par composante (R, G, B), alpha 5 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texel {
    pub rgb: [u8; 3],
    pub alpha: u8,
}

fn rgb15(c: u16) -> [u8; 3] {
    [(c & 31) as u8, ((c >> 5) & 31) as u8, ((c >> 10) & 31) as u8]
}

/// Ce qu'il faut savoir d'une texture : TEXIMAGE_PARAM et PLTT_BASE décodés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexParams {
    pub addr: usize,
    pub width: u32,
    pub height: u32,
    pub repeat: [bool; 2],
    pub flip: [bool; 2],
    pub format: u8,
    pub color0_transparent: bool,
    pub pal_base: usize,
}

impl TexParams {
    pub fn new(teximage: u32, pltt_base: u32) -> TexParams {
        let format = ((teximage >> 26) & 7) as u8;
        // PLTT_BASE : pas de 8 octets pour le format 4 couleurs, 16 sinon.
        let step = if format == 2 { 8 } else { 16 };
        TexParams {
            addr: (teximage & 0xFFFF) as usize * 8,
            width: 8 << ((teximage >> 20) & 7),
            height: 8 << ((teximage >> 23) & 7),
            repeat: [teximage & (1 << 16) != 0, teximage & (1 << 17) != 0],
            flip: [teximage & (1 << 18) != 0, teximage & (1 << 19) != 0],
            format,
            color0_transparent: teximage & (1 << 29) != 0,
            pal_base: (pltt_base & 0x1FFF) as usize * step,
        }
    }

    /// Coordonnée de texel le long d'un axe : répétition (avec miroir une
    /// fois sur deux) ou bornage.
    fn wrap(&self, c: i32, axis: usize) -> u32 {
        let size = if axis == 0 { self.width } else { self.height } as i32;
        if !self.repeat[axis] {
            return c.clamp(0, size - 1) as u32;
        }
        // Tailles en puissances de 2 : masque au lieu d'un modulo.
        let m = c & (size - 1);
        let odd = (c >> size.trailing_zeros()) & 1 == 1;
        (if self.flip[axis] && odd { size - 1 - m } else { m }) as u32
    }

    /// Texel en (s, t), coordonnées en texels entiers.
    pub fn sample(&self, mem: &TexMem, s: i32, t: i32) -> Texel {
        let (u, v) = (self.wrap(s, 0) as usize, self.wrap(t, 1) as usize);
        let i = v * self.width as usize + u;
        let indexed = |idx: usize, bytes_per_color: usize| {
            let c = mem.pal16(self.pal_base + idx * bytes_per_color);
            let alpha = if idx == 0 && self.color0_transparent { 0 } else { 31 };
            Texel { rgb: rgb15(c), alpha }
        };
        match self.format {
            1 => {
                // A3I5 : alpha 3 bits étendu par A × 4 + A / 2.
                let b = mem.tex8(self.addr + i);
                let a = b >> 5;
                Texel { rgb: rgb15(mem.pal16(self.pal_base + (b & 31) as usize * 2)), alpha: a * 4 + a / 2 }
            }
            2 => indexed(((mem.tex8(self.addr + i / 4) >> ((i % 4) * 2)) & 3) as usize, 2),
            3 => indexed(((mem.tex8(self.addr + i / 2) >> ((i % 2) * 4)) & 15) as usize, 2),
            4 => indexed(mem.tex8(self.addr + i) as usize, 2),
            5 => self.compressed(mem, u, v),
            6 => {
                // A5I3 : alpha 5 bits tel quel (GBATEK ne donne pas
                // d'extension).
                let b = mem.tex8(self.addr + i);
                Texel { rgb: rgb15(mem.pal16(self.pal_base + (b & 7) as usize * 2)), alpha: b >> 3 }
            }
            7 => {
                let c = mem.tex16(self.addr + i * 2);
                Texel { rgb: rgb15(c), alpha: if c & 0x8000 != 0 { 31 } else { 0 } }
            }
            _ => Texel { rgb: [31; 3], alpha: 31 },
        }
    }

    /// Format 5 : blocs de 4×4 texels (32 bits) en slot 0 ou 2 ; un mot de
    /// 16 bits par bloc en slot 1 donne la palette et le mode.
    fn compressed(&self, mem: &TexMem, u: usize, v: usize) -> Texel {
        let block = (v / 4) * (self.width as usize / 4) + u / 4;
        let block_addr = self.addr + block * 4;
        let texel = (mem.tex8(block_addr + v % 4) >> ((u % 4) * 2)) & 3;
        // Slot 1 : adresse du bloc / 2, partie haute (+ 10000h) pour le slot 2.
        let in_slot = block_addr % 0x2_0000;
        let upper = if block_addr >= 0x4_0000 { 0x1_0000 } else { 0 };
        let info = mem.tex16(0x2_0000 + upper + in_slot / 2);
        let pal = self.pal_base + (info & 0x3FFF) as usize * 4;
        let color = |n: usize| rgb15(mem.pal16(pal + n * 2));
        let mix = |a: u32, b: u32| {
            let (c0, c1) = (color(0), color(1));
            Texel { rgb: [0, 1, 2].map(|k| ((c0[k] as u32 * a + c1[k] as u32 * b) / 8) as u8), alpha: 31 }
        };
        let solid = |n: usize| Texel { rgb: color(n), alpha: 31 };
        let clear = Texel { rgb: [0; 3], alpha: 0 };
        match (info >> 14, texel) {
            (_, 0) => solid(0),
            (_, 1) => solid(1),
            (0 | 2, 2) => solid(2),
            (1, 2) => mix(4, 4), // (Color0 + Color1) / 2
            (3, 2) => mix(5, 3),
            (0 | 1, _) => clear,
            (2, _) => solid(3),
            _ => mix(3, 5),
        }
    }
}
