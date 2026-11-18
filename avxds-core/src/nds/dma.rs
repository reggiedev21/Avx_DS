//! DMA : 4 canaux par CPU (0x040000B0–0x040000DF) et, sur l'ARM9, les
//! registres « DMA fill » (0x040000E0–0x040000EF).
//!
//! Les transferts sont instantanés : le temps de bus et le blocage du CPU
//! ne sont pas encore modélisés.

/// Déclencheurs gérés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Immediate,
    VBlank,
    HBlank,
    /// Mot prêt sur le bus de la cartouche (ARM9 : 5, ARM7 : 2).
    Cartridge,
    /// FIFO de géométrie à moins de moitié pleine (ARM9 : 7). Rafales d'au
    /// plus 112 mots (GBATEK « DS 3D Geometry Commands »).
    GeometryFifo,
    /// Tout autre déclencheur (cartouche, FIFO géométrique…), pas encore géré.
    Other(u8),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Channel {
    pub sad: u32,
    pub dad: u32,
    /// CNT complet : nombre d'unités dans les bits bas, contrôle en haut.
    pub cnt: u32,
    /// Adresses internes, latchées à l'activation.
    pub src: u32,
    pub dst: u32,
    /// Unités restant à transférer (mode FIFO géométrie, par rafales).
    pub left: u32,
}

impl Channel {
    pub fn enabled(&self) -> bool {
        self.cnt & (1 << 31) != 0
    }
    pub fn repeat(&self) -> bool {
        self.cnt & (1 << 25) != 0
    }
    pub fn word(&self) -> bool {
        self.cnt & (1 << 26) != 0
    }
    pub fn irq(&self) -> bool {
        self.cnt & (1 << 30) != 0
    }
    pub fn dst_mode(&self) -> u32 {
        (self.cnt >> 21) & 3
    }
    pub fn src_mode(&self) -> u32 {
        (self.cnt >> 23) & 3
    }
}

#[derive(Clone, Debug)]
pub struct Dma {
    pub is_arm9: bool,
    pub ch: [Channel; 4],
    /// DMA fill (ARM9) : 4 mots de données sources fixes.
    pub fill: [u32; 4],
}

impl Dma {
    pub fn new(is_arm9: bool) -> Self {
        Dma { is_arm9, ch: [Channel::default(); 4], fill: [0; 4] }
    }

    pub fn trigger_of(&self, n: usize) -> Trigger {
        let cnt = self.ch[n].cnt;
        if self.is_arm9 {
            match (cnt >> 27) & 7 {
                0 => Trigger::Immediate,
                1 => Trigger::VBlank,
                2 => Trigger::HBlank,
                5 => Trigger::Cartridge,
                7 => Trigger::GeometryFifo,
                t => Trigger::Other(t as u8),
            }
        } else {
            match (cnt >> 28) & 3 {
                0 => Trigger::Immediate,
                1 => Trigger::VBlank,
                2 => Trigger::Cartridge,
                t => Trigger::Other(t as u8 + 8), // 8+ : déclencheurs ARM7
            }
        }
    }

    /// Nombre d'unités à transférer (0 = maximum).
    pub fn count(&self, n: usize) -> u32 {
        let (mask, max) = match (self.is_arm9, n) {
            (true, _) => (0x1F_FFFF, 0x20_0000),
            (false, 3) => (0xFFFF, 0x1_0000),
            (false, _) => (0x3FFF, 0x4000),
        };
        match self.ch[n].cnt & mask {
            0 => max,
            c => c,
        }
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        match addr {
            0x0400_00B0..=0x0400_00DF => {
                let n = ((addr - 0x0400_00B0) / 12) as usize;
                // SAD et DAD sont en écriture seule ; CNT se relit.
                Some(if (addr - 0x0400_00B0) % 12 == 8 { self.ch[n].cnt } else { 0 })
            }
            0x0400_00E0..=0x0400_00EC if self.is_arm9 => Some(self.fill[((addr - 0x0400_00E0) / 4) as usize]),
            _ => None,
        }
    }

    /// Écriture d'un registre. Renvoie le canal à lancer tout de suite
    /// (activation d'un transfert immédiat), si besoin.
    pub fn write(&mut self, addr: u32, val: u32, mask: u32) -> Option<Option<usize>> {
        match addr {
            0x0400_00B0..=0x0400_00DF => {
                let n = ((addr - 0x0400_00B0) / 12) as usize;
                let c = &mut self.ch[n];
                let merge = |r: &mut u32| *r = (*r & !mask) | (val & mask);
                match (addr - 0x0400_00B0) % 12 {
                    0 => merge(&mut c.sad),
                    4 => merge(&mut c.dad),
                    _ => {
                        let was = c.enabled();
                        merge(&mut c.cnt);
                        if c.enabled() && !was {
                            c.src = c.sad & 0x0FFF_FFFF;
                            c.dst = c.dad & 0x0FFF_FFFF;
                            self.ch[n].left = self.count(n);
                            if self.trigger_of(n) == Trigger::Immediate {
                                return Some(Some(n));
                            }
                        }
                    }
                }
                Some(None)
            }
            0x0400_00E0..=0x0400_00EC if self.is_arm9 => {
                let r = &mut self.fill[((addr - 0x0400_00E0) / 4) as usize];
                *r = (*r & !mask) | (val & mask);
                Some(None)
            }
            _ => None,
        }
    }
}
