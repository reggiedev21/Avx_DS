//! Horloge temps réel (Seiko S-3511), pilotée bit à bit par l'ARM7 via
//! 0x04000138 (GBATEK « DS Real-Time Clock »).
//!
//! Bit 0 = données, bit 1 = horloge (/SCK), bit 2 = sélection (CS),
//! bit 4 = sens des données (1 = écriture). Les bits sont échantillonnés sur
//! le front montant de /SCK ; en lecture, le bit suivant est présenté sur le
//! front descendant. Données LSB d'abord ; la commande peut être envoyée
//! LSB ou MSB d'abord (code fixe 0110 reconnu dans les deux sens).

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Rtc {
    pub reg: u8,
    pub status1: u8,
    pub status2: u8,
    /// Décalage (en secondes) entre l'heure de la DS et celle du PC, réglé
    /// quand le logiciel écrit la date.
    pub offset: i64,
    /// Heure fixe (secondes Unix) pour les tests ; `None` = heure du PC.
    pub fixed_time: Option<i64>,
    selected: bool,
    in_byte: u8,
    in_bits: u8,
    /// Commande en cours : (numéro LSB-first, lecture ?).
    cmd: Option<(u8, bool)>,
    /// Octets reçus après la commande (écriture).
    written: Vec<u8>,
    /// Réponse à émettre (lecture) et position du bit courant.
    out: Vec<u8>,
    out_bit: Option<usize>,
}

impl Default for Rtc {
    fn default() -> Self {
        Rtc {
            reg: 0,
            status1: 0x02, // mode 24 h
            status2: 0,
            offset: 0,
            fixed_time: None,
            selected: false,
            in_byte: 0,
            in_bits: 0,
            cmd: None,
            written: Vec::new(),
            out: Vec::new(),
            out_bit: None,
        }
    }
}

fn bcd(v: u32) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}

fn from_bcd(b: u8) -> i64 {
    ((b >> 4) as i64) * 10 + (b & 0xF) as i64
}

/// Jours depuis 1970-01-01 → (année, mois, jour) (algorithme de H. Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// (année, mois, jour) → jours depuis 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl Rtc {
    fn now(&self) -> i64 {
        let host = self.fixed_time.unwrap_or_else(|| {
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
        });
        host + self.offset
    }

    /// Les 7 octets date/heure : année, mois, jour, jour de la semaine
    /// (0 = dimanche), heure, minute, seconde, en BCD.
    pub fn date_time(&self) -> [u8; 7] {
        let t = self.now();
        let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400) as u32);
        let (y, m, d) = civil_from_days(days);
        let weekday = (days + 4).rem_euclid(7) as u32; // 1970-01-01 : jeudi
        let (h, min, s) = (secs / 3600, secs / 60 % 60, secs % 60);
        let hour = if self.status1 & 2 != 0 {
            bcd(h) | if h >= 12 { 0x40 } else { 0 }
        } else {
            bcd(h % 12) | if h >= 12 { 0x40 } else { 0 }
        };
        [bcd((y.rem_euclid(100)) as u32), bcd(m), bcd(d), weekday as u8, hour, bcd(min), bcd(s)]
    }

    fn set_date_time(&mut self, b: &[u8]) {
        let current = self.date_time();
        let mut v = current;
        v[..b.len().min(7)].copy_from_slice(&b[..b.len().min(7)]);
        let year = 2000 + from_bcd(v[0]);
        let days = days_from_civil(year, from_bcd(v[1]).max(1) as u32, from_bcd(v[2]).max(1) as u32);
        let mut hour = from_bcd(v[4] & 0x3F);
        if self.status1 & 2 == 0 && v[4] & 0x40 != 0 {
            hour += 12;
        }
        let wanted = days * 86_400 + hour * 3600 + from_bcd(v[5]) * 60 + from_bcd(v[6]);
        self.offset += wanted - self.now();
    }

    pub fn read(&self) -> u32 {
        let mut v = self.reg as u32 & !1;
        if self.reg & 0x10 == 0 {
            // Lecture : bit 0 = bit présenté par l'horloge.
            if let Some(pos) = self.out_bit {
                let byte = self.out.get(pos / 8).copied().unwrap_or(0);
                v |= ((byte >> (pos % 8)) & 1) as u32;
            }
        } else {
            v |= self.reg as u32 & 1;
        }
        v
    }

    pub fn write(&mut self, val: u32, mask: u32) {
        if mask & 0xFF == 0 {
            return;
        }
        let old = self.reg;
        self.reg = val as u8;
        let cs = self.reg & 4 != 0;
        if !cs {
            self.end();
            return;
        }
        if !self.selected {
            self.selected = true;
            self.in_byte = 0;
            self.in_bits = 0;
            return;
        }
        let (sck_old, sck) = (old & 2 != 0, self.reg & 2 != 0);
        if sck_old && !sck && self.reg & 0x10 == 0 {
            // Front descendant en lecture : bit suivant.
            self.out_bit = Some(self.out_bit.map_or(0, |p| p + 1));
        }
        if !sck_old && sck && self.reg & 0x10 != 0 {
            // Front montant en écriture : on échantillonne (LSB d'abord).
            self.in_byte |= (self.reg & 1) << self.in_bits;
            self.in_bits += 1;
            if self.in_bits == 8 {
                let b = self.in_byte;
                self.in_byte = 0;
                self.in_bits = 0;
                self.byte_in(b);
            }
        }
    }

    fn byte_in(&mut self, b: u8) {
        let Some((cmd, read)) = self.cmd else {
            // LSB d'abord : code 0110 dans les bits 0–3. Envoyée MSB
            // d'abord, la commande arrive renversée (0110 dans les bits 4–7).
            let b = if b & 0x0F == 0x06 { b } else if b & 0xF0 == 0x60 { b.reverse_bits() } else { return };
            let (cmd, read) = ((b >> 4) & 7, b & 0x80 != 0);
            self.cmd = Some((cmd, read));
            self.written.clear();
            self.out.clear();
            self.out_bit = None;
            if read {
                self.out = match cmd {
                    0 => vec![self.status1],
                    4 => vec![self.status2],
                    2 => self.date_time().to_vec(),
                    6 => self.date_time()[4..].to_vec(),
                    _ => vec![0; 3],
                };
            }
            return;
        };
        if !read {
            self.written.push(b);
            match cmd {
                0 => self.status1 = (self.status1 & !0x0E) | (b & 0x0E),
                4 => self.status2 = b,
                _ => {}
            }
        }
    }

    fn end(&mut self) {
        if let Some((cmd, false)) = self.cmd {
            match cmd {
                2 if !self.written.is_empty() => {
                    let w = self.written.clone();
                    self.set_date_time(&w);
                }
                6 if !self.written.is_empty() => {
                    let mut full = self.date_time();
                    for (i, b) in self.written.iter().take(3).enumerate() {
                        full[4 + i] = *b;
                    }
                    self.set_date_time(&full);
                }
                _ => {}
            }
        }
        self.selected = false;
        self.cmd = None;
        self.out_bit = None;
        self.in_bits = 0;
        self.in_byte = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round_trip() {
        for days in [-1000i64, 0, 10_957, 19_000, 20_774, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(20_774), (2026, 11, 17));
    }
}
