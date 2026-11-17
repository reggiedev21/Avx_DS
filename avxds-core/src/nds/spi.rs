//! Bus SPI de l'ARM7 (SPICNT 0x040001C0, SPIDATA 0x040001C2) et ses trois
//! périphériques : contrôleur d'alimentation, flash du firmware, contrôleur
//! de l'écran tactile (TSC). Transferts instantanés (Busy toujours à 0).
//!
//! Sources : GBATEK « DS Serial Peripheral Interface Bus », « DS Power
//! Management Device », « DS Firmware Serial Flash Memory », « DS Touch
//! Screen Controller ».

use super::boot::user_settings;
use super::cart::crc16;
use super::spimem::{Family, SpiMem};
use std::collections::VecDeque;

pub const FIRMWARE_SIZE: usize = 256 << 10;
const USER_SETTINGS: usize = 0x3FE00;

/// Firmware de remplacement d'AvxDS (aucun octet Nintendo) : en-tête
/// minimal et deux copies valides des réglages utilisateur.
pub fn synthetic_firmware() -> Vec<u8> {
    let mut fw = vec![0xFF; FIRMWARE_SIZE];
    fw[..0x200].fill(0);
    fw[0x1D] = 0xFF; // type de console : DS
    fw[0x20..0x22].copy_from_slice(&((USER_SETTINGS / 8) as u16).to_le_bytes());
    fw[0x36..0x3C].copy_from_slice(&[0x00, 0x09, 0xBF, 0x41, 0x56, 0x58]); // adresse MAC fictive
    let settings = user_settings();
    for (copy, counter) in [(0usize, 1u16), (1, 0)] {
        let base = USER_SETTINGS + copy * 0x100;
        fw[base..base + 0x70].copy_from_slice(&settings);
        fw[base + 0x70..base + 0x72].copy_from_slice(&counter.to_le_bytes());
        let crc = crc16(0xFFFF, settings);
        fw[base + 0x72..base + 0x74].copy_from_slice(&crc.to_le_bytes());
    }
    fw
}

// ----------------------------------------------------------------------
// Contrôleur d'alimentation
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PowerManager {
    /// Registres 0 à 4. Valeurs initiales : celles qu'indique GBATEK pour
    /// une DSi en mode DS (seules valeurs documentées).
    pub regs: [u8; 5],
    index: Option<u8>,
    /// Le logiciel a demandé l'extinction (registre 0, bit 6).
    pub shutdown_requested: bool,
}

impl Default for PowerManager {
    fn default() -> Self {
        PowerManager { regs: [0x0F, 0x00, 0x00, 0x01, 0x41], index: None, shutdown_requested: false }
    }
}

impl PowerManager {
    fn transfer(&mut self, v: u8) -> u8 {
        match self.index {
            None => {
                self.index = Some(v);
                0
            }
            Some(idx) => {
                let reg = (idx & 0x7F) as usize;
                if idx & 0x80 != 0 {
                    return self.regs.get(reg).copied().unwrap_or(0);
                }
                match reg {
                    0 => {
                        self.regs[0] = v & 0x7F;
                        if v & 0x40 != 0 {
                            self.shutdown_requested = true;
                        }
                    }
                    2 => self.regs[2] = v & 1,
                    3 => self.regs[3] = v & 3,
                    4 => self.regs[4] = (self.regs[4] & 0xF8) | (v & 7),
                    _ => {} // 1 : lecture seule
                }
                0
            }
        }
    }

    fn deselect(&mut self) {
        self.index = None;
    }
}

// ----------------------------------------------------------------------
// Écran tactile
// ----------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct TouchScreen {
    /// Point touché, en pixels de l'écran du bas (None = stylet levé).
    pub pen: Option<(u8, u8)>,
    out: VecDeque<u8>,
}

/// Calibrage lu dans les réglages utilisateur : (adc1, scr1, adc2, scr2)
/// pour X et Y.
fn calibration() -> [(i32, i32, i32, i32); 2] {
    let s = user_settings();
    let u16_at = |o: usize| u16::from_le_bytes([s[o], s[o + 1]]) as i32;
    [
        (u16_at(0x58), s[0x5C] as i32, u16_at(0x5E), s[0x62] as i32),
        (u16_at(0x5A), s[0x5D] as i32, u16_at(0x60), s[0x63] as i32),
    ]
}

impl TouchScreen {
    /// Valeur ADC correspondant au pixel, inverse de la formule de GBATEK :
    /// scr = (adc - adc1) × (scr2 - scr1) / (adc2 - adc1) + (scr1 - 1).
    pub fn adc_for(axis: usize, pixel: u8) -> u16 {
        let (adc1, scr1, adc2, scr2) = calibration()[axis];
        let adc = (pixel as i32 - (scr1 - 1)) * (adc2 - adc1) / (scr2 - scr1) + adc1;
        adc.clamp(0, 0xFFF) as u16
    }

    fn sample(&self, channel: u8) -> u16 {
        match (channel, self.pen) {
            (5, Some((x, _))) => Self::adc_for(0, x),
            (1, Some((_, y))) => Self::adc_for(1, y),
            (5, None) => 0x000,
            (1, None) => 0xFFF,
            (3, Some(_)) => 0x200, // Z1 : pression plausible
            (4, Some(_)) => 0xE00, // Z2
            (3, None) => 0,
            (4, None) => 0xFFF,
            (2, _) => 0, // batterie : reliée à la masse sur DS
            (6, _) => 0x800, // micro : silence (milieu de l'échelle)
            _ => 0x300, // températures
        }
    }

    fn transfer(&mut self, v: u8) -> u8 {
        // Le premier octet en attente sort pendant qu'on reçoit celui-ci :
        // le logiciel peut enchaîner la commande suivante sur le dernier
        // octet de la réponse précédente.
        let out = self.out.pop_front().unwrap_or(0);
        if v & 0x80 != 0 {
            let channel = (v >> 4) & 7;
            let value = self.sample(channel);
            self.out.clear();
            if v & 0x08 != 0 {
                let v8 = value >> 4; // mode 8 bits
                self.out.extend([(v8 >> 1) as u8, ((v8 << 7) & 0xFF) as u8]);
            } else {
                self.out.extend([(value >> 5) as u8, ((value << 3) & 0xFF) as u8]);
            }
        }
        out
    }

    fn deselect(&mut self) {
        self.out.clear();
    }
}

// ----------------------------------------------------------------------
// Le bus
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Spi {
    pub cnt: u16,
    pub data: u8,
    pub power: PowerManager,
    /// Flash du firmware : M45PE20 de 256 Ko (identifiant 20 40 12).
    pub firmware: SpiMem,
    pub touch: TouchScreen,
}

impl Default for Spi {
    fn default() -> Self {
        Spi {
            cnt: 0,
            data: 0,
            power: PowerManager::default(),
            firmware: SpiMem::fixed(Family::Flash, synthetic_firmware(), [0x20, 0x40, 0x12]),
            touch: TouchScreen::default(),
        }
    }
}

impl Spi {
    pub fn read(&self) -> u32 {
        (self.cnt & !0x80) as u32 | (self.data as u32) << 16
    }

    /// Écriture de SPICNT (16 bits bas) et/ou SPIDATA (16 bits hauts).
    /// Renvoie `true` s'il faut lever l'IRQ SPI.
    pub fn write(&mut self, val: u32, mask: u32) -> bool {
        if mask & 0xFFFF != 0 {
            let m = (mask as u16) & 0xCF03;
            self.cnt = (self.cnt & !m) | (val as u16 & m);
        }
        if mask & 0x00FF_0000 == 0 || self.cnt & 0x8000 == 0 {
            return false;
        }
        let v = (val >> 16) as u8;
        let device = (self.cnt >> 8) & 3;
        self.data = match device {
            0 => self.power.transfer(v),
            1 => self.firmware.transfer(v),
            2 => self.touch.transfer(v),
            _ => 0,
        };
        if self.cnt & (1 << 11) == 0 {
            // Fin de commande : chip select relâché après cet octet.
            match device {
                0 => self.power.deselect(),
                1 => self.firmware.deselect(),
                2 => self.touch.deselect(),
                _ => {}
            }
        }
        self.cnt & (1 << 14) != 0
    }
}
