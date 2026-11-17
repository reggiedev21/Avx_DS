//! La machine Nintendo DS : mémoire, registres I/O et les deux CPU.
//!
//! [`Nds`] possède tout. Pour exécuter un CPU, on lui prête une *vue* du
//! matériel ([`Arm9Bus`] ou [`Arm7Bus`]) qui implémente [`Bus`] : pas de
//! pointeurs partagés, pas de `RefCell`, le borrow checker est satisfait par
//! des emprunts disjoints.

mod cp15;
mod irq;
mod math;
mod vram;

#[cfg(test)]
mod tests;

pub use cp15::Cp15;
pub use irq::{bits as irq_bits, IrqController};
pub use math::MathUnit;
pub use vram::{GpuRole, Mapping, Region, Vram};

use crate::bus::{Bus, Cp15Op};
use crate::cpu::{Arm7, Arm9};
use std::collections::BTreeSet;

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

    /// HALTCNT écrit par l'ARM7 : la machine met l'ARM7 en pause.
    pub(crate) arm7_halt_request: bool,

    /// Registres I/O accédés mais pas encore émulés (pour le débogage).
    pub unhandled_io: BTreeSet<(CpuId, u32)>,
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
            arm7_halt_request: false,
            unhandled_io: BTreeSet::new(),
        }
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
        let hw = &mut *self.hw;
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
            0x0400_0300 => hw.postflg9 as u32,
            0x0400_0304 => hw.powcnt1 as u32,
            _ => {
                hw.unhandled_io.insert((CpuId::Arm9, addr));
                0
            }
        }
    }

    fn io_write(&mut self, addr: u32, val: u32, mask: u32) {
        let hw = &mut *self.hw;
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
        let hw = &mut *self.hw;
        match addr {
            0x0400_0130 => hw.keyinput as u32,
            0x0400_0134 => (hw.extkeyin as u32) << 16,
            0x0400_0180 => ((hw.ipcsync7 & 0x4F00) | ((hw.ipcsync9 >> 8) & 0xF)) as u32,
            0x0400_0208 => hw.irq7.ime as u32,
            0x0400_0210 => hw.irq7.ie,
            0x0400_0214 => hw.irq7.if_,
            0x0400_0240 => hw.vram.vramstat() as u32 | (hw.wramcnt as u32) << 8,
            0x0400_0300 => hw.postflg7 as u32,
            0x0400_0304 => hw.powcnt2 as u32,
            _ => {
                hw.unhandled_io.insert((CpuId::Arm7, addr));
                0
            }
        }
    }

    fn io_write(&mut self, addr: u32, val: u32, mask: u32) {
        let hw = &mut *self.hw;
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
        match self.target(addr) {
            Target::Mem(m, o) => wr32(m, o, val),
            Target::Vram(r, o) => self.hw.vram.write32(r, o, val),
            Target::Io => self.io_write(addr, val, u32::MAX),
            _ => {}
        }
    }
}

// ----------------------------------------------------------------------
// La machine
// ----------------------------------------------------------------------

pub struct Nds {
    pub arm9: Arm9,
    pub arm7: Arm7,
    pub hw: Hardware,
    /// Cycles ARM9 pas encore « rattrapés » par l'ARM7 (2 pour 1).
    arm7_debt: i64,
}

impl Default for Nds {
    fn default() -> Self {
        Self::new()
    }
}

impl Nds {
    pub fn new() -> Self {
        Nds { arm9: Arm9::new(), arm7: Arm7::new(), hw: Hardware::default(), arm7_debt: 0 }
    }

    /// Vue ARM9 du matériel (débogueur, tests, chargement).
    pub fn bus9(&mut self) -> Arm9Bus<'_> {
        Arm9Bus { hw: &mut self.hw }
    }

    pub fn bus7(&mut self) -> Arm7Bus<'_> {
        Arm7Bus { hw: &mut self.hw }
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

    /// Reset des deux CPU sur leur vecteur de reset.
    pub fn reset(&mut self) {
        self.arm9.reset(&mut Arm9Bus { hw: &mut self.hw });
        self.arm7.reset(&mut Arm7Bus { hw: &mut self.hw });
        self.arm7_debt = 0;
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

    /// Exécute une instruction ARM9 et la part correspondante de l'ARM7
    /// (fréquence moitié). Ordonnancement provisoire : le scheduler par
    /// événements le remplacera.
    pub fn step(&mut self) {
        self.sync_irq9();
        let c9 = self.arm9.step(&mut Arm9Bus { hw: &mut self.hw });
        self.arm7_debt += c9 as i64;
        while self.arm7_debt >= 2 {
            self.sync_irq7();
            let c7 = self.arm7.step(&mut Arm7Bus { hw: &mut self.hw });
            self.arm7_debt -= 2 * c7 as i64;
            if self.hw.arm7_halt_request {
                self.hw.arm7_halt_request = false;
                self.arm7.halted = true;
            }
        }
    }
}
