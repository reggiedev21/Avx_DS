//! Additions, soustractions et barrel shifter, communs à ARM et Thumb.

/// `a + b + carry` → (résultat, C, V).
#[inline]
pub fn adc(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
    let (r1, c1) = a.overflowing_add(b);
    let (r, c2) = r1.overflowing_add(carry as u32);
    let v = (!(a ^ b) & (a ^ r)) >> 31 != 0;
    (r, c1 | c2, v)
}

/// `a - b` → (résultat, C = pas de retenue, V).
#[inline]
pub fn sub(a: u32, b: u32) -> (u32, bool, bool) {
    adc(a, !b, true)
}

/// `a - b - !carry` (SBC).
#[inline]
pub fn sbc(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
    adc(a, !b, carry)
}

pub const LSL: u32 = 0;
pub const LSR: u32 = 1;
pub const ASR: u32 = 2;
pub const ROR: u32 = 3;

/// Décalage par immédiat (champ de 5 bits), avec les cas spéciaux :
/// `LSR #0` = `LSR #32`, `ASR #0` = `ASR #32`, `ROR #0` = `RRX`.
#[inline]
pub fn shift_imm(kind: u32, val: u32, amount: u32, carry: bool) -> (u32, bool) {
    match kind {
        LSL => {
            if amount == 0 {
                (val, carry)
            } else {
                (val << amount, (val >> (32 - amount)) & 1 != 0)
            }
        }
        LSR => {
            if amount == 0 {
                (0, val >> 31 != 0)
            } else {
                (val >> amount, (val >> (amount - 1)) & 1 != 0)
            }
        }
        ASR => {
            if amount == 0 {
                let r = ((val as i32) >> 31) as u32;
                (r, r & 1 != 0)
            } else {
                (((val as i32) >> amount) as u32, (val >> (amount - 1)) & 1 != 0)
            }
        }
        _ => {
            if amount == 0 {
                (((carry as u32) << 31) | (val >> 1), val & 1 != 0)
            } else {
                (val.rotate_right(amount), (val >> (amount - 1)) & 1 != 0)
            }
        }
    }
}

/// Décalage par registre (octet bas de Rs, 0 à 255).
#[inline]
pub fn shift_reg(kind: u32, val: u32, amount: u32, carry: bool) -> (u32, bool) {
    if amount == 0 {
        return (val, carry);
    }
    match kind {
        LSL => match amount {
            1..=31 => (val << amount, (val >> (32 - amount)) & 1 != 0),
            32 => (0, val & 1 != 0),
            _ => (0, false),
        },
        LSR => match amount {
            1..=31 => (val >> amount, (val >> (amount - 1)) & 1 != 0),
            32 => (0, val >> 31 != 0),
            _ => (0, false),
        },
        ASR => {
            if amount < 32 {
                (((val as i32) >> amount) as u32, (val >> (amount - 1)) & 1 != 0)
            } else {
                let r = ((val as i32) >> 31) as u32;
                (r, r & 1 != 0)
            }
        }
        _ => {
            let a = amount & 31;
            if a == 0 {
                (val, val >> 31 != 0)
            } else {
                (val.rotate_right(a), (val >> (a - 1)) & 1 != 0)
            }
        }
    }
}

/// Addition signée saturée (QADD) → (résultat, saturé ?).
#[inline]
pub fn qadd(a: u32, b: u32) -> (u32, bool) {
    match (a as i32).checked_add(b as i32) {
        Some(r) => (r as u32, false),
        None => ((a as i32).saturating_add(b as i32) as u32, true),
    }
}

/// Soustraction signée saturée (QSUB).
#[inline]
pub fn qsub(a: u32, b: u32) -> (u32, bool) {
    match (a as i32).checked_sub(b as i32) {
        Some(r) => (r as u32, false),
        None => ((a as i32).saturating_sub(b as i32) as u32, true),
    }
}

/// Évalue un code de condition sur les drapeaux NZCV du CPSR.
#[inline]
pub fn condition(cond: u32, cpsr: u32) -> bool {
    let n = cpsr >> 31 & 1 != 0;
    let z = cpsr >> 30 & 1 != 0;
    let c = cpsr >> 29 & 1 != 0;
    let v = cpsr >> 28 & 1 != 0;
    match cond {
        0x0 => z,
        0x1 => !z,
        0x2 => c,
        0x3 => !c,
        0x4 => n,
        0x5 => !n,
        0x6 => v,
        0x7 => !v,
        0x8 => c && !z,
        0x9 => !c || z,
        0xA => n == v,
        0xB => n != v,
        0xC => !z && n == v,
        0xD => z || n != v,
        0xE => true,
        _ => false, // 0xF : traité à part par le décodeur ARM
    }
}
