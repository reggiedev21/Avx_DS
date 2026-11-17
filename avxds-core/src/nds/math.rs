//! Unité de division et de racine carrée de l'ARM9 (0x04000280–0x040002BF).
//!
//! Les résultats sont disponibles immédiatement : le drapeau « occupé »
//! reste à 0. Le vrai matériel met 18 à 34 cycles ; certains jeux pourraient
//! un jour dépendre de ce délai (à faire avec le scheduler).

#[derive(Clone, Debug, Default)]
pub struct MathUnit {
    pub divcnt: u32,
    pub numer: u64,
    pub denom: u64,
    pub quot: u64,
    pub rem: u64,
    pub sqrtcnt: u32,
    pub sqrt_param: u64,
    pub sqrt_result: u32,
}

impl MathUnit {
    pub fn read(&self, addr: u32) -> u32 {
        match addr & 0xFF {
            0x80 => self.divcnt,
            0x90 => self.numer as u32,
            0x94 => (self.numer >> 32) as u32,
            0x98 => self.denom as u32,
            0x9C => (self.denom >> 32) as u32,
            0xA0 => self.quot as u32,
            0xA4 => (self.quot >> 32) as u32,
            0xA8 => self.rem as u32,
            0xAC => (self.rem >> 32) as u32,
            0xB0 => self.sqrtcnt,
            0xB4 => self.sqrt_result,
            0xB8 => self.sqrt_param as u32,
            0xBC => (self.sqrt_param >> 32) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, val: u32, mask: u32) {
        fn lo(r: &mut u64, v: u32, m: u32) {
            *r = (*r & !(m as u64)) | (v & m) as u64;
        }
        fn hi(r: &mut u64, v: u32, m: u32) {
            *r = (*r & !((m as u64) << 32)) | (((v & m) as u64) << 32);
        }
        match addr & 0xFF {
            0x80 => self.divcnt = (self.divcnt & !(mask & 3)) | (val & mask & 3),
            0x90 => lo(&mut self.numer, val, mask),
            0x94 => hi(&mut self.numer, val, mask),
            0x98 => lo(&mut self.denom, val, mask),
            0x9C => hi(&mut self.denom, val, mask),
            0xB0 => self.sqrtcnt = (self.sqrtcnt & !(mask & 1)) | (val & mask & 1),
            0xB8 => lo(&mut self.sqrt_param, val, mask),
            0xBC => hi(&mut self.sqrt_param, val, mask),
            _ => return,
        }
        if (0x80..=0x9F).contains(&(addr & 0xFF)) {
            self.divide();
        } else {
            self.square_root();
        }
    }

    fn divide(&mut self) {
        // Le drapeau « division par zéro » teste les 64 bits du diviseur,
        // quel que soit le mode.
        if self.denom == 0 {
            self.divcnt |= 1 << 14;
        } else {
            self.divcnt &= !(1 << 14);
        }
        let mode = self.divcnt & 3;
        let (n, d) = match mode {
            0 => (self.numer as i32 as i64, self.denom as i32 as i64),
            2 => (self.numer as i64, self.denom as i64),
            _ => (self.numer as i64, self.denom as i32 as i64), // 1, et 3 = 1
        };
        if d == 0 {
            self.rem = n as u64;
            let q: i64 = if n < 0 { 1 } else { -1 };
            // Mode 32 bits : les 32 bits hauts du quotient sont inversés.
            self.quot = if mode == 0 { (q as u64) ^ 0xFFFF_FFFF_0000_0000 } else { q as u64 };
        } else {
            self.quot = n.wrapping_div(d) as u64;
            self.rem = n.wrapping_rem(d) as u64;
        }
    }

    fn square_root(&mut self) {
        let v = if self.sqrtcnt & 1 != 0 { self.sqrt_param } else { self.sqrt_param & 0xFFFF_FFFF };
        self.sqrt_result = isqrt(v);
    }
}

/// Racine carrée entière exacte (partie entière).
fn isqrt(v: u64) -> u32 {
    let mut x = (v as f64).sqrt() as u64;
    // Corrige l'arrondi des flottants pour les grandes valeurs.
    while x.checked_mul(x).is_none_or(|s| s > v) {
        x -= 1;
    }
    while (x + 1).checked_mul(x + 1).is_some_and(|s| s <= v) {
        x += 1;
    }
    x as u32
}
