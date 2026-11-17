//! Émulation haut niveau (HLE) des appels BIOS (SWI).
//!
//! Numéros d'après GBATEK (« BIOS Functions »), identiques sur ARM9 et ARM7
//! pour tout ce qui est traité ici.

use super::cart::crc16;
use super::CpuId;
use crate::bus::Bus;

/// Résultat d'un SWI vu par le HLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swi {
    /// Traité en Rust : le CPU continue à l'instruction suivante.
    Handled,
    /// Doit passer par le mini-BIOS (attente : IntrWait, Halt…).
    Bios,
    /// Pas encore émulé : ignoré, et noté pour le débogage.
    Unhandled,
}

pub fn swi<B: Bus>(bus: &mut B, cpu: CpuId, num: u8, r: &mut [u32; 4]) -> Swi {
    match (num, cpu) {
        (0x03, _) => Swi::Handled, // WaitByLoop : le temps n'est pas consommé
        (0x04..=0x06, _) | (0x07, CpuId::Arm7) => Swi::Bios, // IntrWait, VBlankIntrWait, Halt, Sleep
        (0x08, CpuId::Arm7) => Swi::Handled, // SoundBias
        (0x09, _) => div(r),
        (0x0B, _) => {
            cpu_set(bus, r[0], r[1], r[2]);
            Swi::Handled
        }
        (0x0C, _) => {
            cpu_fast_set(bus, r[0], r[1], r[2]);
            Swi::Handled
        }
        (0x0D, _) => {
            r[0] = isqrt(r[0]);
            Swi::Handled
        }
        (0x0E, _) => {
            // GetCRC16(crc, adresse, longueur en octets)
            let (start, len) = (r[1], r[2]);
            let bytes = (0..len).map(|i| bus.read8(start.wrapping_add(i))).collect::<Vec<_>>();
            r[0] = crc16(r[0] as u16, bytes) as u32;
            Swi::Handled
        }
        (0x0F, _) => {
            r[0] = 0; // IsDebugger : console de vente
            Swi::Handled
        }
        (0x11, _) => {
            lz77(bus, r[0], r[1]);
            Swi::Handled
        }
        (0x14, _) => {
            run_length(bus, r[0], r[1]);
            Swi::Handled
        }
        _ => Swi::Unhandled,
    }
}

/// Div : R0 = R0 / R1, R1 = R0 % R1, R3 = |quotient| (division signée).
fn div(r: &mut [u32; 4]) -> Swi {
    let (n, d) = (r[0] as i32, r[1] as i32);
    if d == 0 {
        // Le vrai BIOS renvoie des valeurs sans intérêt ; on le signale.
        return Swi::Unhandled;
    }
    let q = n.wrapping_div(d);
    r[0] = q as u32;
    r[1] = n.wrapping_rem(d) as u32;
    r[3] = q.unsigned_abs();
    Swi::Handled
}

fn isqrt(v: u32) -> u32 {
    let mut x = (v as f64).sqrt() as u32;
    while (x as u64) * (x as u64) > v as u64 {
        x -= 1;
    }
    while ((x + 1) as u64) * ((x + 1) as u64) <= v as u64 {
        x += 1;
    }
    x
}

/// CpuSet : bits 0–20 = nombre d'unités, bit 24 = remplissage (source
/// fixe), bit 26 = mots de 32 bits (sinon 16 bits).
fn cpu_set<B: Bus>(bus: &mut B, src: u32, dst: u32, ctrl: u32) {
    let count = ctrl & 0x1F_FFFF;
    let fill = ctrl & (1 << 24) != 0;
    if ctrl & (1 << 26) != 0 {
        let (src, dst) = (src & !3, dst & !3);
        let value = bus.read32(src);
        for i in 0..count {
            let v = if fill { value } else { bus.read32(src.wrapping_add(4 * i)) };
            bus.write32(dst.wrapping_add(4 * i), v);
        }
    } else {
        let (src, dst) = (src & !1, dst & !1);
        let value = bus.read16(src);
        for i in 0..count {
            let v = if fill { value } else { bus.read16(src.wrapping_add(2 * i)) };
            bus.write16(dst.wrapping_add(2 * i), v);
        }
    }
}

/// CpuFastSet : toujours en 32 bits, par blocs de 8 mots.
fn cpu_fast_set<B: Bus>(bus: &mut B, src: u32, dst: u32, ctrl: u32) {
    let count = ((ctrl & 0x1F_FFFF) + 7) & !7;
    cpu_set(bus, src, dst, count | (ctrl & (1 << 24)) | (1 << 26));
}

/// LZ77UnCompReadNormalWrite8bit (SWI 0x11).
fn lz77<B: Bus>(bus: &mut B, mut src: u32, mut dst: u32) {
    let header = bus.read32(src & !3);
    let mut remaining = header >> 8;
    let start = dst;
    src = (src & !3) + 4;
    while remaining > 0 {
        let flags = bus.read8(src);
        src += 1;
        for bit in (0..8).rev() {
            if remaining == 0 {
                break;
            }
            if flags & (1 << bit) == 0 {
                let b = bus.read8(src);
                src += 1;
                bus.write8(dst, b);
                dst += 1;
                remaining -= 1;
            } else {
                let b1 = bus.read8(src) as u32;
                let b2 = bus.read8(src + 1) as u32;
                src += 2;
                let len = (b1 >> 4) + 3;
                let disp = (((b1 & 0xF) << 8) | b2) + 1;
                for _ in 0..len {
                    if remaining == 0 {
                        break;
                    }
                    // Lecture dans ce qui vient d'être écrit (fenêtre glissante).
                    let from = dst.wrapping_sub(disp);
                    let b = if from >= start { bus.read8(from) } else { 0 };
                    bus.write8(dst, b);
                    dst += 1;
                    remaining -= 1;
                }
            }
        }
    }
}

/// RLUnCompReadNormalWrite8bit (SWI 0x14).
fn run_length<B: Bus>(bus: &mut B, mut src: u32, mut dst: u32) {
    let header = bus.read32(src & !3);
    let mut remaining = header >> 8;
    src = (src & !3) + 4;
    while remaining > 0 {
        let flag = bus.read8(src) as u32;
        src += 1;
        if flag & 0x80 != 0 {
            let len = (flag & 0x7F) + 3;
            let b = bus.read8(src);
            src += 1;
            for _ in 0..len.min(remaining) {
                bus.write8(dst, b);
                dst += 1;
            }
            remaining -= len.min(remaining);
        } else {
            let len = (flag & 0x7F) + 1;
            for _ in 0..len.min(remaining) {
                let b = bus.read8(src);
                src += 1;
                bus.write8(dst, b);
                dst += 1;
            }
            remaining -= len.min(remaining);
        }
    }
}
