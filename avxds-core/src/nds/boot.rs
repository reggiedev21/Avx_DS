//! Direct boot : démarrer une ROM `.nds` sans BIOS ni firmware Nintendo.
//!
//! On reproduit l'état dans lequel le firmware laisse la machine au moment
//! de sauter dans le jeu. Les valeurs viennent de `NDS::SetupDirectBoot` de
//! melonDS (consulté le 17 novembre 2026), sauf mention contraire.

use super::cart::{crc16, CartError, Header};
use super::hle_bios;
use super::{Nds, CpuId};
use crate::bus::{Bus, Cp15Op};
use crate::cpu::Mode;
use std::fmt;

#[derive(Debug)]
pub enum BootError {
    Cart(CartError),
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BootError::Cart(e) => write!(f, "ROM invalide : {e}"),
        }
    }
}

impl std::error::Error for BootError {}

impl From<CartError> for BootError {
    fn from(e: CartError) -> Self {
        BootError::Cart(e)
    }
}

fn cp15(crn: u8, crm: u8, opc2: u8) -> Cp15Op {
    Cp15Op { opc1: 0, crn, crm, opc2 }
}

/// Réglages utilisateur du firmware recopiés en 0x027FFC80 (0x70 octets).
/// Valeurs par défaut choisies par AvxDS, d'après le format décrit par
/// GBATEK (« DS Firmware User Settings ») ; à vérifier le jour où un jeu
/// s'en sert (calibrage de l'écran tactile notamment).
/// Réglages utilisateur d'une image de firmware : la copie valide (CRC
/// correct) au compteur le plus élevé ; à défaut, ceux d'AvxDS.
pub(crate) fn firmware_user_settings(fw: &[u8]) -> [u8; 0x70] {
    let ptr = fw.get(0x20..0x22).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize * 8);
    let mut best: Option<(u16, [u8; 0x70])> = None;
    for copy in 0..2 {
        let base = ptr + copy * 0x100;
        let Some(block) = fw.get(base..base + 0x74) else { continue };
        let data: [u8; 0x70] = block[..0x70].try_into().unwrap();
        let counter = u16::from_le_bytes([block[0x70], block[0x71]]) & 0x7F;
        let crc = u16::from_le_bytes([block[0x72], block[0x73]]);
        if crc != crc16(0xFFFF, data) {
            continue;
        }
        // Compteur sur 7 bits : 0 suit 0x7F.
        let newer = |b: u16| (counter.wrapping_sub(b) & 0x7F) < 0x40;
        if best.as_ref().is_none_or(|(c, _)| newer(*c)) {
            best = Some((counter, data));
        }
    }
    best.map_or_else(user_settings, |(_, d)| d)
}

pub(crate) fn user_settings() -> [u8; 0x70] {
    let mut s = [0u8; 0x70];
    s[0x00] = 5; // version
    s[0x02] = 11; // couleur préférée
    s[0x03] = 1; // anniversaire : 1er janvier
    s[0x04] = 1;
    for (i, c) in "AvxDS".encode_utf16().enumerate() {
        s[0x06 + 2 * i..0x08 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    s[0x1A] = 5; // longueur du pseudo
    // Calibrage tactile : deux points (ADC 12 bits → pixel).
    // X : 16 unités ADC par pixel, Y : 20, pour que tout l'écran tienne
    // dans les 12 bits du TSC (0x010–0xFF0 en X, 0x094–0xF6C en Y).
    let calib: [(u16, u16, u8, u8); 2] = [(0x0200, 0x0300, 0x20, 0x20), (0x0E00, 0x0D00, 0xE0, 0xA0)];
    for (k, (ax, ay, px, py)) in calib.iter().enumerate() {
        let o = 0x58 + 6 * k;
        s[o..o + 2].copy_from_slice(&ax.to_le_bytes());
        s[o + 2..o + 4].copy_from_slice(&ay.to_le_bytes());
        s[o + 4] = *px;
        s[o + 5] = *py;
    }
    s[0x64] = 1; // langue : anglais
    s
}

impl Nds {
    /// Remet la machine à neuf et démarre `rom` en direct boot, avec le
    /// mini-BIOS d'AvxDS et l'émulation des appels BIOS en Rust.
    pub fn direct_boot(&mut self, rom: Vec<u8>) -> Result<Header, BootError> {
        let header = Header::parse(&rom)?;
        // Ce qui vit hors de la console survit au redémarrage : la flash du
        // firmware, l'horloge (pile), le stylet et les touches tenus.
        let firmware = std::mem::take(&mut self.hw.spi.firmware.data);
        let (rtc_offset, rtc_fixed) = (self.hw.rtc.offset, self.hw.rtc.fixed_time);
        let (pen, keys, ext) = (self.hw.spi.touch.pen, self.hw.keyinput, self.hw.extkeyin);
        let save = std::mem::take(&mut self.hw.slot1.save);
        *self = Nds::new();
        if firmware.len() == super::FIRMWARE_SIZE {
            self.hw.spi.firmware.data = firmware;
        }
        self.hw.rtc.offset = rtc_offset;
        self.hw.rtc.fixed_time = rtc_fixed;
        self.hw.spi.touch.pen = pen;
        self.hw.keyinput = keys;
        self.hw.extkeyin = ext;
        self.hw.slot1.save = save; // la puce de sauvegarde est sur la cartouche

        self.load_bios9(&hle_bios::bytes(hle_bios::BIOS9));
        self.load_bios7(&hle_bios::bytes(hle_bios::BIOS7));
        self.hw.hle = true;

        // CP15 tel que le laisse le firmware : DTCM en 0x03000000 (16 Ko),
        // ITCM de 32 Mo virtuels, TCM activées, vecteurs hauts.
        {
            let c = &mut self.hw.cp15;
            c.write(cp15(1, 0, 0), 0x0005_2078);
            c.write(cp15(2, 0, 0), 0x42);
            c.write(cp15(2, 0, 1), 0x42);
            c.write(cp15(3, 0, 0), 0x02);
            c.write(cp15(5, 0, 2), 0x1511_1011);
            c.write(cp15(5, 0, 3), 0x0510_0011);
            let regions = [0x0400_0033, 0x0200_002B, 0, 0x0800_0035, 0x0300_001B, 0, 0xFFFF_001D, 0x027F_F017];
            for (n, v) in regions.into_iter().enumerate() {
                c.write(cp15(6, n as u8, 0), v);
            }
            c.write(cp15(9, 1, 0), 0x0300_000A);
            c.write(cp15(9, 1, 1), 0x0000_0020);
        }

        self.hw.wramcnt = 3;
        self.hw.postflg9 = 1;
        self.hw.postflg7 = 1;
        self.hw.powcnt1 = 0x820F;
        self.hw.powcnt2 = 1;
        // Slot 1 et slot 2 à l'ARM9 (EXMEMCNT bit 11 = 0, bit 7 = 0).
        // Valeur supposée, à confirmer.
        self.hw.exmemcnt = 0x6000;
        // Biais du son à son niveau habituel (200h, GBATEK), comme le laisse
        // le firmware, qui joue son propre son avant de lancer le jeu.
        // Pokémon N2 ne le règle pas lui-même : sans ça, la moitié
        // négative de l'onde est coupée (SOUND_SPEC §H8).
        self.hw.sound.bias = 0x200;

        // Identifiant de puce : fabricant 0xC2, taille en Mo - 1 (approché).
        let chip_id = super::cart::chip_id(&header, rom.len());
        self.hw.slot1.chip_id = chip_id;
        let settings = firmware_user_settings(&self.hw.spi.firmware.data);
        let header_crc = header.header_crc as u32;
        let secure_crc = header.secure_area_crc as u32;
        {
            let mut bus = self.bus9();
            for (i, chunk) in rom[..0x170].chunks(4).enumerate() {
                bus.write32(0x027F_FE00 + 4 * i as u32, u32::from_le_bytes(chunk.try_into().unwrap()));
            }
            for base in [0x027F_F800, 0x027F_FC00] {
                bus.write32(base, chip_id);
                bus.write32(base + 4, chip_id);
                bus.write32(base + 8, header_crc | secure_crc << 16);
            }
            bus.write16(0x027F_F850, 0x5835);
            bus.write16(0x027F_FC10, 0x5835);
            bus.write16(0x027F_FC30, 0xFFFF);
            bus.write16(0x027F_FC40, 0x0001); // démarrage depuis la cartouche
            for (i, b) in settings.into_iter().enumerate() {
                bus.write8(0x027F_FC80 + i as u32, b);
            }
        }

        copy(&mut self.bus9(), &rom, header.arm9_rom_offset, header.arm9_size, header.arm9_ram_address);
        copy(&mut self.bus7(), &rom, header.arm7_rom_offset, header.arm7_size, header.arm7_ram_address);

        // Registres : piles de chaque mode, R12 = LR = point d'entrée.
        // Mode System, IRQ masquées : supposition (melonDS ne montre pas le
        // CPSR) ; le crt0 des jeux règle de toute façon ses propres modes.
        for cpu in [CpuId::Arm9, CpuId::Arm7] {
            let (entry, svc, irq, sys) = match cpu {
                CpuId::Arm9 => (header.arm9_entry, 0x0300_3FC0, 0x0300_3F80, 0x0300_2F7C),
                CpuId::Arm7 => (header.arm7_entry, 0x0380_FFC0, 0x0380_FF80, 0x0380_FD80),
            };
            macro_rules! setup {
                ($c:expr) => {{
                    let c = $c;
                    c.set_cpsr(Mode::System as u32 | crate::cpu::FLAG_I | crate::cpu::FLAG_F);
                    c.set_banked_sp(Mode::Supervisor, svc);
                    c.set_banked_sp(Mode::Irq, irq);
                    c.set_reg(13, sys);
                    c.set_reg(12, entry);
                    c.set_reg(14, entry);
                    c.jump(entry, false);
                }};
            }
            match cpu {
                CpuId::Arm9 => setup!(&mut self.arm9),
                CpuId::Arm7 => setup!(&mut self.arm7),
            }
        }

        self.hw.rom = rom;
        Ok(header)
    }
}

/// Copie un binaire de la ROM vers la mémoire vue par un CPU.
fn copy(bus: &mut impl Bus, rom: &[u8], offset: u32, size: u32, dest: u32) {
    let data = &rom[offset as usize..(offset + size) as usize];
    for (i, b) in data.iter().enumerate() {
        bus.write8(dest + i as u32, *b);
    }
}
