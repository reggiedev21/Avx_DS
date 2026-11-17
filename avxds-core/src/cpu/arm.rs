//! Jeu d'instructions ARM (32 bits).
//!
//! Le décodage passe par une table de 4096 entrées calculée à la
//! compilation, indexée par les bits 27–20 et 7–4 de l'instruction.

use super::alu::{self, adc, sbc, shift_imm, shift_reg, sub};
use super::{Arch, Cpu, Exception, FLAG_C, FLAG_Q, FLAG_T};
use crate::bus::{Bus, Cp15Op};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArmOp {
    DataProc,
    DataProcImm,
    Multiply,
    MultiplyLong,
    Swap,
    Halfword,
    Mrs,
    MsrReg,
    MsrImm,
    Bx,
    BlxReg,
    Clz,
    QArith,
    DspMultiply,
    Bkpt,
    SingleTransfer,
    BlockTransfer,
    Branch,
    CoprocTransfer,
    CoprocData,
    CoprocRegister,
    Swi,
    Undefined,
}

const fn decode(hi: u32, lo: u32) -> ArmOp {
    use ArmOp::*;
    match hi >> 5 {
        0b000 => {
            if lo == 0b1001 {
                if hi & 0xFC == 0x00 {
                    Multiply
                } else if hi & 0xF8 == 0x08 {
                    MultiplyLong
                } else if hi & 0xFB == 0x10 {
                    Swap
                } else {
                    Undefined
                }
            } else if lo & 0b1001 == 0b1001 {
                Halfword
            } else if hi & 0x19 == 0x10 {
                // Opcodes TST/TEQ/CMP/CMN sans S : instructions diverses.
                match (hi, lo) {
                    (0x10 | 0x14, 0b0000) => Mrs,
                    (0x12 | 0x16, 0b0000) => MsrReg,
                    (0x12, 0b0001) => Bx,
                    (0x16, 0b0001) => Clz,
                    (0x12, 0b0011) => BlxReg,
                    (_, 0b0101) => QArith,
                    (0x12, 0b0111) => Bkpt,
                    (_, l) if l & 0b1001 == 0b1000 => DspMultiply,
                    _ => Undefined,
                }
            } else {
                DataProc
            }
        }
        0b001 => {
            if hi & 0x19 == 0x10 {
                if hi & 0x02 != 0 {
                    MsrImm
                } else {
                    Undefined
                }
            } else {
                DataProcImm
            }
        }
        0b010 => SingleTransfer,
        0b011 => {
            if lo & 1 != 0 {
                Undefined
            } else {
                SingleTransfer
            }
        }
        0b100 => BlockTransfer,
        0b101 => Branch,
        0b110 => CoprocTransfer,
        _ => {
            if hi & 0x10 != 0 {
                Swi
            } else if lo & 1 != 0 {
                CoprocRegister
            } else {
                CoprocData
            }
        }
    }
}

const fn build_lut() -> [ArmOp; 4096] {
    let mut lut = [ArmOp::Undefined; 4096];
    let mut i = 0;
    while i < 4096 {
        lut[i] = decode((i >> 4) as u32, (i & 0xF) as u32);
        i += 1;
    }
    lut
}

pub(crate) static ARM_LUT: [ArmOp; 4096] = build_lut();

#[inline]
pub(crate) fn lut_index(op: u32) -> usize {
    (((op >> 16) & 0xFF0) | ((op >> 4) & 0xF)) as usize
}

#[inline]
fn reg(op: u32, shift: u32) -> usize {
    ((op >> shift) & 0xF) as usize
}

#[inline]
fn bit(op: u32, n: u32) -> bool {
    op & (1 << n) != 0
}

/// Demi-mot signé haut ou bas (multiplications DSP).
#[inline]
fn half(v: u32, top: bool) -> i32 {
    if top { (v >> 16) as i16 as i32 } else { v as i16 as i32 }
}

impl<A: Arch> Cpu<A> {
    pub(super) fn exec_arm<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        let cond = op >> 28;
        if cond == 0xF {
            return self.arm_unconditional(bus, op);
        }
        if !alu::condition(cond, self.cpsr) {
            return 1;
        }
        use ArmOp::*;
        match ARM_LUT[lut_index(op)] {
            DataProc => self.arm_data_proc(op, false),
            DataProcImm => self.arm_data_proc(op, true),
            Multiply => self.arm_multiply(op),
            MultiplyLong => self.arm_multiply_long(op),
            Swap => self.arm_swap(bus, op),
            Halfword => self.arm_halfword(bus, op),
            Mrs => self.arm_mrs(op),
            MsrReg | MsrImm => self.arm_msr(op),
            Bx => {
                let target = self.r[reg(op, 0)];
                self.branch_exchange(target);
                3
            }
            BlxReg if A::V5 => {
                let target = self.r[reg(op, 0)];
                self.r[14] = self.cur_pc.wrapping_add(4);
                self.branch_exchange(target);
                3
            }
            Clz if A::V5 => {
                let v = self.r[reg(op, 0)];
                self.r[reg(op, 12)] = v.leading_zeros();
                1
            }
            QArith if A::V5 => self.arm_qarith(op),
            DspMultiply if A::V5 => self.arm_dsp_multiply(op),
            Bkpt if A::V5 => {
                let lr = self.cur_pc.wrapping_add(4);
                self.enter_exception(bus, Exception::PrefetchAbort, lr);
                3
            }
            SingleTransfer => self.arm_single_transfer(bus, op),
            BlockTransfer => self.block_transfer(
                bus, reg(op, 16), op & 0xFFFF, bit(op, 20), bit(op, 23), bit(op, 24),
                bit(op, 21), bit(op, 22),
            ),
            Branch => {
                let offset = ((op << 8) as i32 >> 6) as u32;
                if bit(op, 24) {
                    self.r[14] = self.cur_pc.wrapping_add(4);
                }
                let target = self.r[15].wrapping_add(offset);
                self.write_pc(target);
                3
            }
            CoprocRegister => self.arm_coproc_register(bus, op),
            Swi => self.software_interrupt(bus, (op >> 16) as u8),
            // Pas de coprocesseur à transferts sur DS ; instructions ARMv5
            // sur ARMv4 ; encodages réellement indéfinis.
            CoprocTransfer | CoprocData | BlxReg | Clz | QArith | DspMultiply | Bkpt
            | Undefined => self.undefined(bus),
        }
    }

    /// Condition 0xF : BLX immédiat et PLD sur ARMv5, « jamais » sur ARMv4.
    fn arm_unconditional<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        if !A::V5 {
            return 1;
        }
        if (op >> 25) & 7 == 0b101 {
            // BLX <imm> : passe en Thumb, H (bit 24) ajoute un demi-mot.
            let offset = (((op << 8) as i32 >> 6) as u32) | ((op >> 23) & 2);
            self.r[14] = self.cur_pc.wrapping_add(4);
            let target = self.r[15].wrapping_add(offset);
            self.cpsr |= FLAG_T;
            self.write_pc(target);
            3
        } else if op & 0x0D70_F000 == 0x0550_F000 {
            1 // PLD : indice de préchargement, sans effet ici.
        } else {
            self.undefined(bus)
        }
    }

    fn arm_data_proc(&mut self, op: u32, imm: bool) -> u32 {
        let opcode = (op >> 21) & 0xF;
        let s = bit(op, 20);
        let rn = reg(op, 16);
        let rd = reg(op, 12);
        let carry = self.flag(FLAG_C);
        let reg_shift = !imm && bit(op, 4);

        let (op2, shifter_c) = if imm {
            let rot = ((op >> 8) & 0xF) * 2;
            let v = (op & 0xFF).rotate_right(rot);
            (v, if rot == 0 { carry } else { v >> 31 != 0 })
        } else {
            let rm = reg(op, 0);
            let kind = (op >> 5) & 3;
            if reg_shift {
                // Décalage par registre : un cycle de plus, R15 lu à PC+12.
                let amount = self.r[reg(op, 8)] & 0xFF;
                let v = self.r[rm].wrapping_add(if rm == 15 { 4 } else { 0 });
                shift_reg(kind, v, amount, carry)
            } else {
                shift_imm(kind, self.r[rm], (op >> 7) & 0x1F, carry)
            }
        };
        let a = self.r[rn].wrapping_add(if rn == 15 && reg_shift { 4 } else { 0 });

        // (résultat, C, V, écrit Rd ?) ; None pour V = inchangé.
        let (res, c, v, write) = match opcode {
            0x0 => (a & op2, shifter_c, None, true),          // AND
            0x1 => (a ^ op2, shifter_c, None, true),          // EOR
            0x2 => { let (r, c, v) = sub(a, op2); (r, c, Some(v), true) }            // SUB
            0x3 => { let (r, c, v) = sub(op2, a); (r, c, Some(v), true) }            // RSB
            0x4 => { let (r, c, v) = adc(a, op2, false); (r, c, Some(v), true) }     // ADD
            0x5 => { let (r, c, v) = adc(a, op2, carry); (r, c, Some(v), true) }     // ADC
            0x6 => { let (r, c, v) = sbc(a, op2, carry); (r, c, Some(v), true) }     // SBC
            0x7 => { let (r, c, v) = sbc(op2, a, carry); (r, c, Some(v), true) }     // RSC
            0x8 => (a & op2, shifter_c, None, false),         // TST
            0x9 => (a ^ op2, shifter_c, None, false),         // TEQ
            0xA => { let (r, c, v) = sub(a, op2); (r, c, Some(v), false) }           // CMP
            0xB => { let (r, c, v) = adc(a, op2, false); (r, c, Some(v), false) }    // CMN
            0xC => (a | op2, shifter_c, None, true),          // ORR
            0xD => (op2, shifter_c, None, true),              // MOV
            0xE => (a & !op2, shifter_c, None, true),         // BIC
            _ => (!op2, shifter_c, None, true),               // MVN
        };

        if write && rd == 15 {
            if s {
                // MOVS PC, LR et consorts : retour d'exception.
                self.restore_cpsr_from_spsr();
            }
            self.write_pc(res);
            return if reg_shift { 4 } else { 3 };
        }
        if s {
            match v {
                Some(v) => self.set_nzcv(res, c, v),
                None => self.set_nzc(res, c),
            }
        }
        if write {
            self.r[rd] = res;
        }
        if reg_shift { 2 } else { 1 }
    }

    fn arm_multiply(&mut self, op: u32) -> u32 {
        let rd = reg(op, 16);
        let mut res = self.r[reg(op, 0)].wrapping_mul(self.r[reg(op, 8)]);
        if bit(op, 21) {
            res = res.wrapping_add(self.r[reg(op, 12)]); // MLA
        }
        self.r[rd] = res;
        if bit(op, 20) {
            // ARMv5 : C inchangé. ARMv4 : C « sans signification », on le garde.
            self.set_nz(res);
        }
        if bit(op, 21) { 3 } else { 2 }
    }

    fn arm_multiply_long(&mut self, op: u32) -> u32 {
        let (rdhi, rdlo) = (reg(op, 16), reg(op, 12));
        let (rm, rs) = (self.r[reg(op, 0)], self.r[reg(op, 8)]);
        let mut res = if bit(op, 22) {
            (rm as i32 as i64).wrapping_mul(rs as i32 as i64) as u64
        } else {
            (rm as u64) * (rs as u64)
        };
        if bit(op, 21) {
            let acc = ((self.r[rdhi] as u64) << 32) | self.r[rdlo] as u64;
            res = res.wrapping_add(acc);
        }
        self.r[rdlo] = res as u32;
        self.r[rdhi] = (res >> 32) as u32;
        if bit(op, 20) {
            let n = (res >> 32) as u32 & super::FLAG_N;
            let z = if res == 0 { super::FLAG_Z } else { 0 };
            self.cpsr = (self.cpsr & !(super::FLAG_N | super::FLAG_Z)) | n | z;
        }
        4
    }

    fn arm_swap<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        let addr = self.r[reg(op, 16)];
        let src = self.r[reg(op, 0)];
        let rd = reg(op, 12);
        if bit(op, 22) {
            let old = bus.read8(addr) as u32;
            bus.write8(addr, src as u8);
            self.r[rd] = old;
        } else {
            let old = Self::load32_rotated(bus, addr);
            bus.write32(addr & !3, src);
            self.r[rd] = old;
        }
        4
    }

    /// LDRH/STRH/LDRSB/LDRSH et, sur ARMv5, LDRD/STRD.
    fn arm_halfword<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        let (pre, up, imm, wb, load) = (bit(op, 24), bit(op, 23), bit(op, 22), bit(op, 21), bit(op, 20));
        let rn = reg(op, 16);
        let rd = reg(op, 12);
        let offset = if imm { ((op >> 4) & 0xF0) | (op & 0xF) } else { self.r[reg(op, 0)] };
        let base = self.r[rn];
        let offset_addr = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let addr = if pre { offset_addr } else { base };
        let writeback = (!pre || wb) && rn != 15;
        let sh = (op >> 5) & 3;

        match (load, sh) {
            (false, 1) => {
                // STRH
                let v = self.r[rd].wrapping_add(if rd == 15 { 4 } else { 0 });
                bus.write16(addr & !1, v as u16);
                if writeback {
                    self.r[rn] = offset_addr;
                }
                2
            }
            (true, _) => {
                let v = match sh {
                    1 => Self::load16_unsigned(bus, addr),
                    2 => bus.read8(addr) as i8 as i32 as u32,
                    _ => Self::load16_signed(bus, addr),
                };
                if writeback {
                    self.r[rn] = offset_addr;
                }
                if rd == 15 {
                    self.load_pc(v);
                    5
                } else {
                    self.r[rd] = v;
                    3
                }
            }
            (false, 2) if A::V5 => {
                // LDRD : Rd doit être pair.
                let lo = bus.read32(addr & !3);
                let hi = bus.read32((addr & !3).wrapping_add(4));
                if writeback {
                    self.r[rn] = offset_addr;
                }
                let rd = rd & !1;
                self.r[rd] = lo;
                if rd + 1 == 15 {
                    self.load_pc(hi);
                } else {
                    self.r[rd + 1] = hi;
                }
                4
            }
            (false, 3) if A::V5 => {
                // STRD
                let rd = rd & !1;
                let lo = self.r[rd];
                let hi = self.r[rd + 1].wrapping_add(if rd + 1 == 15 { 4 } else { 0 });
                bus.write32(addr & !3, lo);
                bus.write32((addr & !3).wrapping_add(4), hi);
                if writeback {
                    self.r[rn] = offset_addr;
                }
                3
            }
            _ => 1, // LDRD/STRD sur ARMv4 : sans effet.
        }
    }

    fn arm_single_transfer<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        let (reg_off, pre, up, byte, wb, load) =
            (bit(op, 25), bit(op, 24), bit(op, 23), bit(op, 22), bit(op, 21), bit(op, 20));
        let rn = reg(op, 16);
        let rd = reg(op, 12);
        let offset = if reg_off {
            let carry = self.flag(FLAG_C);
            shift_imm((op >> 5) & 3, self.r[reg(op, 0)], (op >> 7) & 0x1F, carry).0
        } else {
            op & 0xFFF
        };
        let base = self.r[rn];
        let offset_addr = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let addr = if pre { offset_addr } else { base };
        // Post-indexé avec W=1 = LDRT/STRT (accès « utilisateur ») : aucune
        // différence ici, la DS n'a pas de protection mémoire par mode.
        let writeback = (!pre || wb) && rn != 15;

        if load {
            let v = if byte { bus.read8(addr) as u32 } else { Self::load32_rotated(bus, addr) };
            if writeback {
                self.r[rn] = offset_addr;
            }
            if rd == 15 {
                self.load_pc(v);
                5
            } else {
                self.r[rd] = v;
                3
            }
        } else {
            let v = self.r[rd].wrapping_add(if rd == 15 { 4 } else { 0 });
            if byte {
                bus.write8(addr, v as u8);
            } else {
                bus.write32(addr & !3, v);
            }
            if writeback {
                self.r[rn] = offset_addr;
            }
            2
        }
    }

    fn arm_mrs(&mut self, op: u32) -> u32 {
        let v = if bit(op, 22) { self.spsr().unwrap_or(self.cpsr) } else { self.cpsr };
        self.r[reg(op, 12)] = v;
        1
    }

    fn arm_msr(&mut self, op: u32) -> u32 {
        let val = if bit(op, 25) {
            (op & 0xFF).rotate_right(((op >> 8) & 0xF) * 2)
        } else {
            self.r[reg(op, 0)]
        };
        let mut mask = 0u32;
        for (field, byte_mask) in [(16, 0x0000_00FF), (17, 0x0000_FF00), (18, 0x00FF_0000), (19, 0xFF00_0000)] {
            if bit(op, field) {
                mask |= byte_mask;
            }
        }
        // Bits réellement présents : NZCV (+ Q sur ARMv5), I, F, T, mode.
        let valid = if A::V5 { 0xF800_00FF } else { 0xF000_00FF };
        mask &= valid;

        if bit(op, 22) {
            let m = self.mode();
            if m.has_spsr() {
                let s = &mut self.spsr[m.bank()];
                *s = (*s & !mask) | (val & mask);
            }
        } else {
            if self.mode() == super::Mode::User {
                mask &= 0xFF00_0000;
            }
            mask &= !FLAG_T; // MSR ne doit pas changer d'état.
            let new = (self.cpsr & !mask) | (val & mask);
            self.set_cpsr(new);
        }
        1
    }

    fn arm_qarith(&mut self, op: u32) -> u32 {
        let rm = self.r[reg(op, 0)];
        let rn = self.r[reg(op, 16)];
        let (res, sat) = match (op >> 21) & 3 {
            0 => alu::qadd(rm, rn),
            1 => alu::qsub(rm, rn),
            2 => {
                let (d, s1) = alu::qadd(rn, rn);
                let (r, s2) = alu::qadd(rm, d);
                (r, s1 | s2)
            }
            _ => {
                let (d, s1) = alu::qadd(rn, rn);
                let (r, s2) = alu::qsub(rm, d);
                (r, s1 | s2)
            }
        };
        if sat {
            self.cpsr |= FLAG_Q;
        }
        self.r[reg(op, 12)] = res;
        1
    }

    /// SMLAxy, SMLAWy, SMULWy, SMLALxy, SMULxy.
    fn arm_dsp_multiply(&mut self, op: u32) -> u32 {
        let rd = reg(op, 16);
        let rn = reg(op, 12);
        let rm = self.r[reg(op, 0)];
        let rs = self.r[reg(op, 8)];
        let x = bit(op, 5);
        let y = bit(op, 6);
        match (op >> 21) & 3 {
            0 => {
                // SMLAxy
                let p = half(rm, x).wrapping_mul(half(rs, y)) as u32;
                let (res, _, v) = adc(p, self.r[rn], false);
                if v {
                    self.cpsr |= FLAG_Q;
                }
                self.r[rd] = res;
            }
            1 => {
                // SMLAWy (x = 0) / SMULWy (x = 1)
                let p = ((rm as i32 as i64 * half(rs, y) as i64) >> 16) as u32;
                if x {
                    self.r[rd] = p;
                } else {
                    let (res, _, v) = adc(p, self.r[rn], false);
                    if v {
                        self.cpsr |= FLAG_Q;
                    }
                    self.r[rd] = res;
                }
            }
            2 => {
                // SMLALxy : RdHi = rd, RdLo = rn
                let p = half(rm, x).wrapping_mul(half(rs, y)) as i64 as u64;
                let acc = ((self.r[rd] as u64) << 32) | self.r[rn] as u64;
                let res = acc.wrapping_add(p);
                self.r[rn] = res as u32;
                self.r[rd] = (res >> 32) as u32;
                return 2;
            }
            _ => {
                // SMULxy
                self.r[rd] = half(rm, x).wrapping_mul(half(rs, y)) as u32;
            }
        }
        1
    }

    /// MRC/MCR. Seul CP15 existe, et seulement sur l'ARM9.
    fn arm_coproc_register<B: Bus>(&mut self, bus: &mut B, op: u32) -> u32 {
        let cp = (op >> 8) & 0xF;
        if !A::V5 || cp != 15 {
            return self.undefined(bus);
        }
        let cp15 = Cp15Op {
            opc1: ((op >> 21) & 7) as u8,
            crn: ((op >> 16) & 0xF) as u8,
            crm: (op & 0xF) as u8,
            opc2: ((op >> 5) & 7) as u8,
        };
        let rd = reg(op, 12);
        if bit(op, 20) {
            let v = bus.cp15_read(cp15);
            if rd == 15 {
                self.cpsr = (self.cpsr & 0x0FFF_FFFF) | (v & 0xF000_0000);
            } else {
                self.r[rd] = v;
            }
        } else {
            let v = self.r[rd].wrapping_add(if rd == 15 { 4 } else { 0 });
            let wfi = cp15.crn == 7
                && ((cp15.crm == 0 && cp15.opc2 == 4) || (cp15.crm == 8 && cp15.opc2 == 2));
            if wfi {
                self.halted = true;
            }
            bus.cp15_write(cp15, v);
        }
        2
    }
}
