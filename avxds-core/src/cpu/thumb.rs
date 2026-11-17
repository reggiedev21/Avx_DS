//! Jeu d'instructions Thumb (16 bits).
//!
//! Décodage par table de 1024 entrées indexée par les bits 15–6.

use super::alu::{self, adc, sbc, shift_imm, shift_reg, sub};
use super::{Arch, Cpu, Exception, FLAG_C, FLAG_T};
use crate::bus::Bus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThumbOp {
    ShiftImm,
    AddSub,
    Imm8,
    Alu,
    HiReg,
    LdrPc,
    LoadStoreReg,
    LoadStoreImm,
    LoadStoreHalf,
    LoadStoreSp,
    AddPcSp,
    AdjustSp,
    PushPop,
    LoadStoreMultiple,
    CondBranch,
    Swi,
    Branch,
    BlxSuffix,
    BlPrefix,
    BlSuffix,
    Bkpt,
    Undefined,
}

const fn decode(i: u32) -> ThumbOp {
    use ThumbOp::*;
    let op = i << 6; // reconstitue les bits 15–6
    match op >> 13 {
        0b000 => {
            if (op >> 11) & 3 == 3 {
                AddSub
            } else {
                ShiftImm
            }
        }
        0b001 => Imm8,
        0b010 => {
            if op >> 10 == 0b010000 {
                Alu
            } else if op >> 10 == 0b010001 {
                HiReg
            } else if op >> 11 == 0b01001 {
                LdrPc
            } else {
                LoadStoreReg
            }
        }
        0b011 => LoadStoreImm,
        0b100 => {
            if op & 0x1000 == 0 {
                LoadStoreHalf
            } else {
                LoadStoreSp
            }
        }
        0b101 => {
            if op & 0x1000 == 0 {
                AddPcSp
            } else if (op >> 8) & 0xF == 0b0000 {
                AdjustSp
            } else if (op >> 8) & 0x6 == 0x4 {
                PushPop
            } else if (op >> 8) & 0xF == 0b1110 {
                Bkpt
            } else {
                Undefined
            }
        }
        0b110 => {
            if op & 0x1000 == 0 {
                LoadStoreMultiple
            } else if (op >> 8) & 0xF == 0xF {
                Swi
            } else if (op >> 8) & 0xF == 0xE {
                Undefined
            } else {
                CondBranch
            }
        }
        _ => match (op >> 11) & 3 {
            0 => Branch,
            1 => BlxSuffix,
            2 => BlPrefix,
            _ => BlSuffix,
        },
    }
}

const fn build_lut() -> [ThumbOp; 1024] {
    let mut lut = [ThumbOp::Undefined; 1024];
    let mut i = 0;
    while i < 1024 {
        lut[i] = decode(i as u32);
        i += 1;
    }
    lut
}

pub(crate) static THUMB_LUT: [ThumbOp; 1024] = build_lut();

#[inline]
fn lo(op: u16, shift: u32) -> usize {
    ((op >> shift) & 7) as usize
}

impl<A: Arch> Cpu<A> {
    pub(super) fn exec_thumb<B: Bus>(&mut self, bus: &mut B, op: u16) -> u32 {
        use ThumbOp::*;
        let o = op as u32;
        match THUMB_LUT[(op >> 6) as usize] {
            ShiftImm => {
                let (rd, rs) = (lo(op, 0), lo(op, 3));
                let carry = self.flag(FLAG_C);
                let (res, c) = shift_imm((o >> 11) & 3, self.r[rs], (o >> 6) & 0x1F, carry);
                self.r[rd] = res;
                self.set_nzc(res, c);
                1
            }
            AddSub => {
                let (rd, rs) = (lo(op, 0), lo(op, 3));
                let operand = if o & 0x400 != 0 { (o >> 6) & 7 } else { self.r[lo(op, 6)] };
                let a = self.r[rs];
                let (res, c, v) = if o & 0x200 != 0 { sub(a, operand) } else { adc(a, operand, false) };
                self.r[rd] = res;
                self.set_nzcv(res, c, v);
                1
            }
            Imm8 => {
                let rd = lo(op, 8);
                let imm = o & 0xFF;
                let a = self.r[rd];
                match (o >> 11) & 3 {
                    0 => {
                        self.r[rd] = imm;
                        self.set_nz(imm);
                    }
                    1 => {
                        let (r, c, v) = sub(a, imm);
                        self.set_nzcv(r, c, v);
                    }
                    2 => {
                        let (r, c, v) = adc(a, imm, false);
                        self.r[rd] = r;
                        self.set_nzcv(r, c, v);
                    }
                    _ => {
                        let (r, c, v) = sub(a, imm);
                        self.r[rd] = r;
                        self.set_nzcv(r, c, v);
                    }
                }
                1
            }
            Alu => self.thumb_alu(op),
            HiReg => self.thumb_hi_reg(op),
            LdrPc => {
                let addr = (self.r[15] & !3).wrapping_add((o & 0xFF) * 4);
                self.r[lo(op, 8)] = bus.read32(addr);
                3
            }
            LoadStoreReg => {
                let addr = self.r[lo(op, 3)].wrapping_add(self.r[lo(op, 6)]);
                let rd = lo(op, 0);
                match (o >> 9) & 7 {
                    0 => bus.write32(addr & !3, self.r[rd]),                                // STR
                    1 => bus.write16(addr & !1, self.r[rd] as u16),                         // STRH
                    2 => bus.write8(addr, self.r[rd] as u8),                                // STRB
                    3 => self.r[rd] = bus.read8(addr) as i8 as i32 as u32,                  // LDRSB
                    4 => self.r[rd] = Self::load32_rotated(bus, addr),                      // LDR
                    5 => self.r[rd] = Self::load16_unsigned(bus, addr),                     // LDRH
                    6 => self.r[rd] = bus.read8(addr) as u32,                               // LDRB
                    _ => self.r[rd] = Self::load16_signed(bus, addr),                       // LDRSH
                }
                2
            }
            LoadStoreImm => {
                let (rd, rb) = (lo(op, 0), lo(op, 3));
                let imm = (o >> 6) & 0x1F;
                let byte = o & 0x1000 != 0;
                let addr = self.r[rb].wrapping_add(if byte { imm } else { imm * 4 });
                match (byte, o & 0x800 != 0) {
                    (false, false) => bus.write32(addr & !3, self.r[rd]),
                    (false, true) => self.r[rd] = Self::load32_rotated(bus, addr),
                    (true, false) => bus.write8(addr, self.r[rd] as u8),
                    (true, true) => self.r[rd] = bus.read8(addr) as u32,
                }
                2
            }
            LoadStoreHalf => {
                let (rd, rb) = (lo(op, 0), lo(op, 3));
                let addr = self.r[rb].wrapping_add(((o >> 6) & 0x1F) * 2);
                if o & 0x800 != 0 {
                    self.r[rd] = Self::load16_unsigned(bus, addr);
                } else {
                    bus.write16(addr & !1, self.r[rd] as u16);
                }
                2
            }
            LoadStoreSp => {
                let rd = lo(op, 8);
                let addr = self.r[13].wrapping_add((o & 0xFF) * 4);
                if o & 0x800 != 0 {
                    self.r[rd] = Self::load32_rotated(bus, addr);
                } else {
                    bus.write32(addr & !3, self.r[rd]);
                }
                2
            }
            AddPcSp => {
                let base = if o & 0x800 != 0 { self.r[13] } else { self.r[15] & !3 };
                self.r[lo(op, 8)] = base.wrapping_add((o & 0xFF) * 4);
                1
            }
            AdjustSp => {
                let imm = (o & 0x7F) * 4;
                self.r[13] = if o & 0x80 != 0 {
                    self.r[13].wrapping_sub(imm)
                } else {
                    self.r[13].wrapping_add(imm)
                };
                1
            }
            PushPop => {
                let pop = o & 0x800 != 0;
                let extra = o & 0x100 != 0; // LR pour PUSH, PC pour POP
                let mut list = o & 0xFF;
                if extra {
                    list |= if pop { 1 << 15 } else { 1 << 14 };
                }
                // PUSH = STMDB SP!, POP = LDMIA SP!
                self.block_transfer(bus, 13, list, pop, pop, !pop, true, false)
            }
            LoadStoreMultiple => {
                let rb = lo(op, 8);
                let load = o & 0x800 != 0;
                self.block_transfer(bus, rb, o & 0xFF, load, true, false, true, false)
            }
            CondBranch => {
                if alu::condition((o >> 8) & 0xF, self.cpsr) {
                    let offset = (o as u8 as i8 as i32 * 2) as u32;
                    let target = self.r[15].wrapping_add(offset);
                    self.write_pc(target);
                    3
                } else {
                    1
                }
            }
            Swi => self.software_interrupt(bus, op as u8),
            Branch => {
                let offset = (((o << 21) as i32) >> 20) as u32;
                let target = self.r[15].wrapping_add(offset);
                self.write_pc(target);
                3
            }
            BlPrefix => {
                let offset = (((o << 21) as i32) >> 9) as u32;
                self.r[14] = self.r[15].wrapping_add(offset);
                1
            }
            BlSuffix => {
                let target = self.r[14].wrapping_add((o & 0x7FF) << 1);
                self.r[14] = self.cur_pc.wrapping_add(2) | 1;
                self.write_pc(target);
                3
            }
            BlxSuffix if A::V5 => {
                let target = self.r[14].wrapping_add((o & 0x7FF) << 1) & !3;
                self.r[14] = self.cur_pc.wrapping_add(2) | 1;
                self.cpsr &= !FLAG_T;
                self.write_pc(target);
                3
            }
            Bkpt if A::V5 => {
                let lr = self.cur_pc.wrapping_add(4);
                self.enter_exception(bus, Exception::PrefetchAbort, lr);
                3
            }
            BlxSuffix | Bkpt | Undefined => self.undefined(bus),
        }
    }

    fn thumb_alu(&mut self, op: u16) -> u32 {
        let (rd, rs) = (lo(op, 0), lo(op, 3));
        let a = self.r[rd];
        let b = self.r[rs];
        let carry = self.flag(FLAG_C);
        match (op >> 6) & 0xF {
            0x0 => { let r = a & b; self.r[rd] = r; self.set_nz(r); }                       // AND
            0x1 => { let r = a ^ b; self.r[rd] = r; self.set_nz(r); }                       // EOR
            k @ (0x2 | 0x3 | 0x4 | 0x7) => {                                                // LSL LSR ASR ROR
                let kind = match k { 0x2 => alu::LSL, 0x3 => alu::LSR, 0x4 => alu::ASR, _ => alu::ROR };
                let (r, c) = shift_reg(kind, a, b & 0xFF, carry);
                self.r[rd] = r;
                self.set_nzc(r, c);
                return 2;
            }
            0x5 => { let (r, c, v) = adc(a, b, carry); self.r[rd] = r; self.set_nzcv(r, c, v); } // ADC
            0x6 => { let (r, c, v) = sbc(a, b, carry); self.r[rd] = r; self.set_nzcv(r, c, v); } // SBC
            0x8 => { self.set_nz(a & b); }                                                  // TST
            0x9 => { let (r, c, v) = sub(0, b); self.r[rd] = r; self.set_nzcv(r, c, v); }    // NEG
            0xA => { let (r, c, v) = sub(a, b); self.set_nzcv(r, c, v); }                    // CMP
            0xB => { let (r, c, v) = adc(a, b, false); self.set_nzcv(r, c, v); }             // CMN
            0xC => { let r = a | b; self.r[rd] = r; self.set_nz(r); }                       // ORR
            0xD => { let r = a.wrapping_mul(b); self.r[rd] = r; self.set_nz(r); return 3; } // MUL
            0xE => { let r = a & !b; self.r[rd] = r; self.set_nz(r); }                      // BIC
            _ => { let r = !b; self.r[rd] = r; self.set_nz(r); }                            // MVN
        }
        1
    }

    fn thumb_hi_reg(&mut self, op: u16) -> u32 {
        let o = op as u32;
        let rd = ((o & 7) | ((o >> 4) & 8)) as usize;
        let rs = ((o >> 3) & 0xF) as usize;
        let b = self.r[rs];
        match (o >> 8) & 3 {
            0 => {
                let r = self.r[rd].wrapping_add(b);
                if rd == 15 { self.write_pc(r); return 3; }
                self.r[rd] = r;
            }
            1 => {
                let (r, c, v) = sub(self.r[rd], b);
                self.set_nzcv(r, c, v);
            }
            2 => {
                if rd == 15 { self.write_pc(b); return 3; }
                self.r[rd] = b;
            }
            _ => {
                // BX Rs / BLX Rs (bit 7, ARMv5)
                if o & 0x80 != 0 {
                    if !A::V5 {
                        return 1;
                    }
                    self.r[14] = self.cur_pc.wrapping_add(2) | 1;
                }
                self.branch_exchange(b);
                return 3;
            }
        }
        1
    }
}
