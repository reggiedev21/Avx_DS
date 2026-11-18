//! Les 9 banques de VRAM (A à I) et leur placement par VRAMCNT.
//!
//! Chaque banque peut être placée dans une zone visible du CPU (fonds ou
//! sprites d'un moteur 2D, LCDC, WRAM de l'ARM7) ou réservée au GPU
//! (textures, palettes étendues). Plusieurs banques peuvent se chevaucher :
//! une lecture renvoie alors le OU de leurs contenus et une écriture va à
//! toutes.

const SIZES: [usize; 9] = [
    128 << 10, 128 << 10, 128 << 10, 128 << 10, // A B C D
    64 << 10,                                   // E
    16 << 10, 16 << 10,                         // F G
    32 << 10,                                   // H
    16 << 10,                                   // I
];

/// Position de la banque en mode LCDC (offset dans 0x06800000).
const LCDC_BASE: [u32; 9] = [
    0x00000, 0x20000, 0x40000, 0x60000, 0x80000, 0x90000, 0x94000, 0x98000, 0xA0000,
];

/// Zones de l'espace VRAM vues par les CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// Fonds du moteur A : 0x06000000, 512 Ko.
    BgA = 0,
    /// Fonds du moteur B : 0x06200000, 128 Ko.
    BgB = 1,
    /// Sprites du moteur A : 0x06400000, 256 Ko.
    ObjA = 2,
    /// Sprites du moteur B : 0x06600000, 128 Ko.
    ObjB = 3,
    /// Accès direct « LCDC » : 0x06800000, 656 Ko.
    Lcdc = 4,
    /// WRAM de l'ARM7 : 0x06000000 côté ARM7, 256 Ko.
    Arm7 = 5,
}

const REGIONS: usize = 6;
const PAGE_SHIFT: u32 = 14; // pages de 16 Ko
const PAGES: usize = 64; // 1 Mo par zone au maximum

/// Usage d'une banque réservé au GPU (pas d'accès CPU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuRole {
    Texture(u8),
    TexturePalette(u8),
    BgExtPaletteA(u8),
    ObjExtPaletteA,
    BgExtPaletteB,
    ObjExtPaletteB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    Disabled,
    /// (zone, offset de la banque dans la zone)
    Cpu(Region, u32),
    Gpu(GpuRole),
}

pub struct Vram {
    pub banks: [Box<[u8]>; 9],
    /// Valeurs brutes de VRAMCNT_A à VRAMCNT_I.
    pub cnt: [u8; 9],
    pub mapping: [Mapping; 9],
    /// Pour chaque zone et chaque page de 16 Ko : masque des banques présentes.
    pages: [[u16; PAGES]; REGIONS],
}

impl Default for Vram {
    fn default() -> Self {
        Vram {
            banks: SIZES.map(|s| vec![0u8; s].into_boxed_slice()),
            cnt: [0; 9],
            mapping: [Mapping::Disabled; 9],
            pages: [[0; PAGES]; REGIONS],
        }
    }
}

fn decode(bank: usize, cnt: u8) -> Mapping {
    if cnt & 0x80 == 0 {
        return Mapping::Disabled;
    }
    let mst = cnt & 7;
    let ofs = ((cnt >> 3) & 3) as u32;
    use GpuRole::*;
    use Mapping::{Cpu, Disabled, Gpu};
    use Region::*;
    let lcdc = Cpu(Lcdc, LCDC_BASE[bank]);
    match bank {
        0 | 1 => match mst & 3 {
            0 => lcdc,
            1 => Cpu(BgA, 0x20000 * ofs),
            2 => Cpu(ObjA, 0x20000 * (ofs & 1)),
            _ => Gpu(Texture(ofs as u8)),
        },
        2 | 3 => match mst {
            0 => lcdc,
            1 => Cpu(BgA, 0x20000 * ofs),
            2 => Cpu(Arm7, 0x20000 * (ofs & 1)),
            3 => Gpu(Texture(ofs as u8)),
            4 if bank == 2 => Cpu(BgB, 0),
            4 => Cpu(ObjB, 0),
            _ => Disabled,
        },
        4 => match mst {
            0 => lcdc,
            1 => Cpu(BgA, 0),
            2 => Cpu(ObjA, 0),
            3 => Gpu(TexturePalette(0)),
            4 => Gpu(BgExtPaletteA(0)),
            _ => Disabled,
        },
        5 | 6 => {
            let off = 0x4000 * (ofs & 1) + 0x10000 * (ofs >> 1);
            match mst {
                0 => lcdc,
                1 => Cpu(BgA, off),
                2 => Cpu(ObjA, off),
                3 => Gpu(TexturePalette(ofs as u8)),
                4 => Gpu(BgExtPaletteA(ofs as u8 & 1)),
                5 => Gpu(ObjExtPaletteA),
                _ => Disabled,
            }
        }
        7 => match mst & 3 {
            0 => lcdc,
            1 => Cpu(BgB, 0),
            2 => Gpu(BgExtPaletteB),
            _ => Disabled,
        },
        _ => match mst & 3 {
            0 => lcdc,
            1 => Cpu(BgB, 0x8000),
            2 => Cpu(ObjB, 0),
            _ => Gpu(ObjExtPaletteB),
        },
    }
}

impl Vram {
    /// Écrit VRAMCNT_x (x = 0 pour A … 8 pour I) et recalcule le placement.
    pub fn set_cnt(&mut self, bank: usize, val: u8) {
        self.cnt[bank] = val;
        self.mapping[bank] = decode(bank, val);
        self.pages = [[0; PAGES]; REGIONS];
        for (b, mapping) in self.mapping.iter().enumerate() {
            if let Mapping::Cpu(region, base) = *mapping {
                let first = (base >> PAGE_SHIFT) as usize;
                let count = SIZES[b] >> PAGE_SHIFT;
                for p in first..(first + count).min(PAGES) {
                    self.pages[region as usize][p] |= 1 << b;
                }
            }
        }
    }

    /// VRAMSTAT (lu par l'ARM7) : bit 0 = C, bit 1 = D placée chez l'ARM7.
    pub fn vramstat(&self) -> u8 {
        let arm7 = |b: usize| matches!(self.mapping[b], Mapping::Cpu(Region::Arm7, _)) as u8;
        arm7(2) | (arm7(3) << 1)
    }

    /// Zone et offset d'une adresse 0x06xxxxxx vue par l'ARM9.
    #[inline]
    pub fn locate_arm9(addr: u32) -> (Region, u32) {
        match (addr >> 21) & 7 {
            0 => (Region::BgA, addr & 0x7FFFF),
            1 => (Region::BgB, addr & 0x1FFFF),
            2 => (Region::ObjA, addr & 0x3FFFF),
            3 => (Region::ObjB, addr & 0x1FFFF),
            _ => (Region::Lcdc, addr & 0xFFFFF),
        }
    }

    #[inline]
    pub fn locate_arm7(addr: u32) -> (Region, u32) {
        (Region::Arm7, addr & 0x3FFFF)
    }

    #[inline]
    fn banks_at(&self, region: Region, off: u32) -> u16 {
        self.pages[region as usize][(off >> PAGE_SHIFT) as usize & (PAGES - 1)]
    }

    #[inline]
    fn bank_offset(&self, b: usize, off: u32) -> usize {
        let Mapping::Cpu(_, base) = self.mapping[b] else { unreachable!() };
        (off - base) as usize & (SIZES[b] - 1)
    }

    /// Lecture 16 bits (offset pair). Renvoie 0 si aucune banque n'est placée.
    pub fn read16(&self, region: Region, off: u32) -> u16 {
        let mut mask = self.banks_at(region, off);
        let mut v = 0;
        while mask != 0 {
            let b = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let o = self.bank_offset(b, off);
            v |= u16::from_le_bytes([self.banks[b][o], self.banks[b][o + 1]]);
        }
        v
    }

    pub fn read8(&self, region: Region, off: u32) -> u8 {
        (self.read16(region, off & !1) >> ((off & 1) * 8)) as u8
    }

    pub fn read32(&self, region: Region, off: u32) -> u32 {
        self.read16(region, off) as u32 | (self.read16(region, off + 2) as u32) << 16
    }

    pub fn write16(&mut self, region: Region, off: u32, val: u16) {
        let mut mask = self.banks_at(region, off);
        while mask != 0 {
            let b = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let o = self.bank_offset(b, off);
            self.banks[b][o..o + 2].copy_from_slice(&val.to_le_bytes());
        }
    }

    pub fn write8(&mut self, region: Region, off: u32, val: u8) {
        let mut mask = self.banks_at(region, off);
        while mask != 0 {
            let b = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let o = self.bank_offset(b, off);
            self.banks[b][o] = val;
        }
    }

    pub fn write32(&mut self, region: Region, off: u32, val: u32) {
        self.write16(region, off, val as u16);
        self.write16(region, off + 2, (val >> 16) as u16);
    }

    /// Palettes étendues de fonds d'un moteur : 4 slots de 8 Ko (GBATEK
    /// « DS Video Extended Palettes »). Moteur A : E (slots 0–3, 32 Ko
    /// du bas), F ou G (slots 0–1 si OFS = 0, 2–3 si OFS = 1) ; moteur B :
    /// H (slots 0–3) (GBATEK « DS Memory Control - VRAM »).
    pub fn bg_ext_palette(&self, engine_a: bool) -> ExtPalette<'_> {
        let mut pal = ExtPalette::default();
        for (b, mapping) in self.mapping.iter().enumerate() {
            let first = match (*mapping, engine_a, b) {
                (Mapping::Gpu(GpuRole::BgExtPaletteA(_)), true, 4) => 0,
                (Mapping::Gpu(GpuRole::BgExtPaletteA(ofs)), true, _) => 2 * ofs as usize,
                (Mapping::Gpu(GpuRole::BgExtPaletteB), false, _) => 0,
                _ => continue,
            };
            pal.add(&self.banks[b], first);
        }
        pal
    }

    /// Palette étendue des sprites d'un moteur : un seul slot, les 8 Ko du
    /// bas de F ou G (moteur A) ou de I (moteur B).
    pub fn obj_ext_palette(&self, engine_a: bool) -> ExtPalette<'_> {
        let mut pal = ExtPalette::default();
        for (b, mapping) in self.mapping.iter().enumerate() {
            match (*mapping, engine_a) {
                (Mapping::Gpu(GpuRole::ObjExtPaletteA), true) | (Mapping::Gpu(GpuRole::ObjExtPaletteB), false) => {
                    pal.add(&self.banks[b][..SLOT], 0);
                }
                _ => {}
            }
        }
        pal
    }
}

/// Taille d'un slot de palette étendue : 16 palettes de 256 couleurs.
const SLOT: usize = 8 << 10;

/// Vue en lecture des palettes étendues d'un moteur. Pour chaque slot, les
/// banques qui le couvrent (au plus E, F et G) ; si plusieurs se
/// chevauchent, leurs contenus sont combinés par OU, comme pour la VRAM vue
/// du CPU.
#[derive(Default)]
pub struct ExtPalette<'a> {
    slots: [[Option<&'a [u8]>; 3]; 4],
}

impl<'a> ExtPalette<'a> {
    /// Place les slots couverts par `bank`, à partir de `first`.
    fn add(&mut self, bank: &'a [u8], first: usize) {
        for (k, chunk) in bank.chunks_exact(SLOT).enumerate() {
            let Some(slot) = self.slots.get_mut(first + k) else { break };
            if let Some(free) = slot.iter_mut().find(|s| s.is_none()) {
                *free = Some(chunk);
            }
        }
    }

    /// Couleur `index` (0..4095 : palette × 256 + couleur) du slot `slot`,
    /// en 15 bits ; 0 si aucune banque n'y est placée.
    pub fn color(&self, slot: usize, index: usize) -> u16 {
        let o = (index & 0xFFF) * 2;
        self.slots[slot & 3]
            .iter()
            .flatten()
            .fold(0, |c, s| c | u16::from_le_bytes([s[o], s[o + 1]]))
            & 0x7FFF
    }
}
