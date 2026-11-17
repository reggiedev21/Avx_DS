//! Timing de l'écran : lignes, HBlank, VBlank, VCOUNT, DISPSTAT.
//!
//! 263 lignes de 355 points, 6 cycles de bus par point : 2130 cycles de
//! bus par ligne, soit 4260 cycles ARM9. Lignes 0–191 visibles, VBlank de
//! 192 à 262 (le drapeau VBlank est déjà effacé à la ligne 262).

use super::irq_bits as irq;
use super::scheduler::BUS_CYCLE;

pub const DOTS_PER_LINE: u64 = 355;
pub const LINE_CYCLES: u64 = DOTS_PER_LINE * 6 * BUS_CYCLE;
/// Début du HBlank : après les 256 points visibles (256 × 6 cycles de bus).
/// Valeur déduite, pas mesurée : à confirmer sur matériel ou dans un
/// émulateur de référence.
pub const HBLANK_START: u64 = 256 * 6 * BUS_CYCLE;
pub const LINES: u16 = 263;
pub const VISIBLE_LINES: u16 = 192;
pub const FRAME_CYCLES: u64 = LINE_CYCLES * LINES as u64;

const DS_VBLANK: u16 = 1 << 0;
const DS_HBLANK: u16 = 1 << 1;
const DS_VCOUNT: u16 = 1 << 2;
const DS_VBLANK_IRQ: u16 = 1 << 3;
const DS_HBLANK_IRQ: u16 = 1 << 4;
const DS_VCOUNT_IRQ: u16 = 1 << 5;
/// Bits de DISPSTAT modifiables : IRQ (3–5) et ligne de comparaison (7–15).
const DS_WRITABLE: u16 = 0xFFB8;

#[derive(Clone, Debug, Default)]
pub struct Lcd {
    pub vcount: u16,
    /// DISPSTAT de l'ARM9 (indice 0) et de l'ARM7 (indice 1) : chaque CPU a
    /// ses propres autorisations d'IRQ et sa propre ligne de comparaison.
    pub dispstat: [u16; 2],
    /// Nombre d'images commencées (incrémenté au début du VBlank).
    pub frames: u64,
    /// Instant de début de la ligne courante.
    pub line_start: u64,
}

impl Lcd {
    pub fn read_dispstat(&self, cpu: usize) -> u16 {
        self.dispstat[cpu]
    }

    pub fn write_dispstat(&mut self, cpu: usize, val: u16, mask: u16) {
        let m = mask & DS_WRITABLE;
        self.dispstat[cpu] = (self.dispstat[cpu] & !m) | (val & m);
    }

    /// Ligne de comparaison sur 9 bits : bits 8–15 = bits 0–7, bit 7 = bit 8.
    fn lyc(ds: u16) -> u16 {
        (ds >> 8) | ((ds & 0x80) << 1)
    }

    /// Début du HBlank. Renvoie les IRQ à lever pour [ARM9, ARM7].
    pub fn hblank(&mut self) -> [u32; 2] {
        let mut out = [0; 2];
        for (cpu, ds) in self.dispstat.iter_mut().enumerate() {
            *ds |= DS_HBLANK;
            if *ds & DS_HBLANK_IRQ != 0 {
                out[cpu] |= irq::HBLANK;
            }
        }
        out
    }

    /// Passage à la ligne suivante, à l'instant `time`.
    pub fn next_line(&mut self, time: u64) -> [u32; 2] {
        self.line_start = time;
        self.vcount = (self.vcount + 1) % LINES;
        let mut out = [0; 2];
        if self.vcount == VISIBLE_LINES {
            self.frames += 1;
        }
        for (cpu, ds) in self.dispstat.iter_mut().enumerate() {
            *ds &= !DS_HBLANK;
            if self.vcount == VISIBLE_LINES {
                *ds |= DS_VBLANK;
                if *ds & DS_VBLANK_IRQ != 0 {
                    out[cpu] |= irq::VBLANK;
                }
            } else if self.vcount == LINES - 1 {
                *ds &= !DS_VBLANK;
            }
            if self.vcount == Self::lyc(*ds) {
                *ds |= DS_VCOUNT;
                if *ds & DS_VCOUNT_IRQ != 0 {
                    out[cpu] |= irq::VCOUNT;
                }
            } else {
                *ds &= !DS_VCOUNT;
            }
        }
        out
    }
}
