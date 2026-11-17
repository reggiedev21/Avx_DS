//! En-tête d'une ROM `.nds` (les 0x200 premiers octets).

use std::fmt;

/// CRC16 utilisé par le BIOS (SWI GetCRC16) et par l'en-tête des ROM :
/// polynôme 0x8005 réfléchi (0xA001), octets de poids faible en premier.
pub fn crc16(mut crc: u16, data: impl IntoIterator<Item = u8>) -> u16 {
    for b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub game_code: [u8; 4],
    pub maker_code: [u8; 2],
    pub unit_code: u8,
    /// Taille de la puce : 128 Ko << n.
    pub device_capacity: u8,
    pub arm9_rom_offset: u32,
    pub arm9_entry: u32,
    pub arm9_ram_address: u32,
    pub arm9_size: u32,
    pub arm7_rom_offset: u32,
    pub arm7_entry: u32,
    pub arm7_ram_address: u32,
    pub arm7_size: u32,
    pub fnt_offset: u32,
    pub fnt_size: u32,
    pub fat_offset: u32,
    pub fat_size: u32,
    pub secure_area_crc: u16,
    pub logo_crc: u16,
    pub header_crc: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CartError {
    TooSmall(usize),
    /// Binaire hors de la ROM : (nom, offset, taille, taille de la ROM).
    OutOfRom(&'static str, u32, u32, usize),
    /// Adresse de chargement impossible : (nom, adresse, taille).
    BadLoadAddress(&'static str, u32, u32),
}

impl fmt::Display for CartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CartError::TooSmall(n) => write!(f, "ROM trop petite ({n} octets, en-tête de 0x200 attendu)"),
            CartError::OutOfRom(name, off, size, len) => write!(
                f, "binaire {name} hors de la ROM : offset {off:#X} + {size:#X} > {len:#X}"),
            CartError::BadLoadAddress(name, addr, size) => write!(
                f, "binaire {name} : chargement en {addr:#010X} ({size:#X} octets) hors des zones permises"),
        }
    }
}

impl std::error::Error for CartError {}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Le BIOS limite chaque binaire à 0x3BFE00 octets.
const MAX_BINARY: u32 = 0x3B_FE00;

impl Header {
    pub fn parse(rom: &[u8]) -> Result<Header, CartError> {
        if rom.len() < 0x200 {
            return Err(CartError::TooSmall(rom.len()));
        }
        let title = rom[..12].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let h = Header {
            title,
            game_code: rom[0x0C..0x10].try_into().unwrap(),
            maker_code: rom[0x10..0x12].try_into().unwrap(),
            unit_code: rom[0x12],
            device_capacity: rom[0x14],
            arm9_rom_offset: u32_at(rom, 0x20),
            arm9_entry: u32_at(rom, 0x24),
            arm9_ram_address: u32_at(rom, 0x28),
            arm9_size: u32_at(rom, 0x2C),
            arm7_rom_offset: u32_at(rom, 0x30),
            arm7_entry: u32_at(rom, 0x34),
            arm7_ram_address: u32_at(rom, 0x38),
            arm7_size: u32_at(rom, 0x3C),
            fnt_offset: u32_at(rom, 0x40),
            fnt_size: u32_at(rom, 0x44),
            fat_offset: u32_at(rom, 0x48),
            fat_size: u32_at(rom, 0x4C),
            secure_area_crc: u16_at(rom, 0x6C),
            logo_crc: u16_at(rom, 0x15C),
            header_crc: u16_at(rom, 0x15E),
        };
        for (name, off, size) in [("ARM9", h.arm9_rom_offset, h.arm9_size), ("ARM7", h.arm7_rom_offset, h.arm7_size)] {
            if off as u64 + size as u64 > rom.len() as u64 {
                return Err(CartError::OutOfRom(name, off, size, rom.len()));
            }
        }
        let in_main_ram = |addr: u32, size: u32| {
            addr >= 0x0200_0000 && (addr as u64 + size as u64) <= 0x0240_0000 && size <= MAX_BINARY
        };
        if !in_main_ram(h.arm9_ram_address, h.arm9_size) {
            return Err(CartError::BadLoadAddress("ARM9", h.arm9_ram_address, h.arm9_size));
        }
        // ARM7 : Main RAM, ou WRAM partagée + WRAM ARM7 (0x037F8000–0x0380FFFF).
        let in_arm7_wram = h.arm7_ram_address >= 0x037F_8000
            && (h.arm7_ram_address as u64 + h.arm7_size as u64) <= 0x0381_0000;
        if !in_main_ram(h.arm7_ram_address, h.arm7_size) && !in_arm7_wram {
            return Err(CartError::BadLoadAddress("ARM7", h.arm7_ram_address, h.arm7_size));
        }
        Ok(h)
    }

    /// CRC de l'en-tête recalculé (à comparer à `header_crc`).
    pub fn compute_header_crc(rom: &[u8]) -> u16 {
        crc16(0xFFFF, rom[..0x15E].iter().copied())
    }

    pub fn game_code_str(&self) -> String {
        self.game_code.iter().map(|&c| c as char).collect()
    }
}
