//! Mémoires SPI : EEPROM, FRAM et FLASH, comme celles des cartouches et
//! comme la flash du firmware.
//!
//! Source : GBATEK « DS Cartridge Backup » et « DS Firmware Serial Flash
//! Memory ». Les écritures sont instantanées : le bit WIP vaut toujours 0.

/// Famille de puce, c'est-à-dire la façon de la piloter. La taille exacte
/// est un paramètre à part : elle peut grandir pendant la détection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// EEPROM de 0,5 Ko : 1 octet d'adresse, le 9e bit est dans la commande.
    Eeprom512,
    /// EEPROM de 8 et 64 Ko, FRAM de 32 Ko : 2 octets d'adresse.
    Eeprom16,
    /// EEPROM de 128 Ko : 3 octets d'adresse (non confirmé par GBATEK).
    Eeprom24,
    /// FLASH de 256 Ko à 8 Mo : 3 octets d'adresse, effacement séparé.
    Flash,
}

impl Family {
    pub const ALL: [Family; 4] = [Family::Eeprom512, Family::Eeprom16, Family::Eeprom24, Family::Flash];

    /// Tailles standard, croissantes.
    pub fn sizes(self) -> &'static [usize] {
        match self {
            Family::Eeprom512 => &[512],
            Family::Eeprom16 => &[8 << 10, 32 << 10, 64 << 10],
            Family::Eeprom24 => &[128 << 10],
            Family::Flash => &[256 << 10, 512 << 10, 1 << 20, 8 << 20],
        }
    }

    pub fn min_size(self) -> usize {
        self.sizes()[0]
    }

    pub fn max_size(self) -> usize {
        *self.sizes().last().unwrap()
    }

    /// Plus petite taille standard qui contient `n` octets.
    pub fn size_for(self, n: usize) -> usize {
        self.sizes().iter().copied().find(|&s| s >= n).unwrap_or(self.max_size())
    }

    /// Octets envoyés après la commande avant les données (adresse, plus
    /// l'octet factice de FAST READ).
    pub fn header_len(self, cmd: u8) -> usize {
        match (self, cmd) {
            (_, 0x06 | 0x04 | 0x05 | 0x01 | 0x9F) => 0,
            (Family::Eeprom512, _) => 1,
            (Family::Eeprom16, _) => 2,
            (Family::Flash, 0x0B) => 4,
            _ => 3,
        }
    }

    /// Taille de page pour les écritures (bouclage dans la page). Pour la
    /// famille 16 bits, celle de l'EEPROM de 64 Ko : la plus grande, une
    /// écriture correcte d'une 8 Ko ne la franchit jamais non plus.
    pub fn page(self) -> usize {
        match self {
            Family::Eeprom512 => 16,
            Family::Eeprom16 => 128,
            Family::Eeprom24 | Family::Flash => 256,
        }
    }

    pub fn is_read(self, cmd: u8) -> bool {
        match self {
            Family::Eeprom512 => matches!(cmd, 0x03 | 0x0B),
            Family::Flash => matches!(cmd, 0x03 | 0x0B),
            _ => cmd == 0x03,
        }
    }

    /// Commandes qui modifient la mémoire (écriture ou effacement).
    pub fn is_write(self, cmd: u8) -> bool {
        match self {
            Family::Eeprom512 => matches!(cmd, 0x02 | 0x0A),
            Family::Flash => matches!(cmd, 0x02 | 0x0A | 0xDB | 0xD8),
            _ => cmd == 0x02,
        }
    }

    /// Adresse d'un accès d'après les octets d'adresse reçus.
    pub fn address(self, cmd: u8, header: &[u8]) -> u32 {
        let bytes = &header[..header.len().min(3)];
        let a = bytes.iter().fold(0u32, |a, &b| (a << 8) | b as u32);
        if self == Family::Eeprom512 && matches!(cmd, 0x0B | 0x0A) {
            a | 0x100
        } else {
            a
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpiMem {
    pub family: Family,
    /// Contenu. Pour une sauvegarde, toujours de la taille maximale de la
    /// famille, pour que la taille puisse grandir sans copie ni miroir.
    pub data: Vec<u8>,
    /// Taille standard actuelle (protection, identifiant, export).
    pub size: usize,
    /// La taille ne grandit pas (flash du firmware).
    fixed: bool,
    /// Identifiant JEDEC imposé ; sinon déduit de la taille.
    jedec: Option<[u8; 3]>,
    cmd: Option<u8>,
    header: Vec<u8>,
    /// Octets de données échangés depuis la fin de l'en-tête.
    index: u32,
    pub wel: bool,
    /// Bits WP (2–3) et SRWD (7) du registre d'état.
    pub status: u8,
    /// Modifiée depuis le dernier export.
    pub dirty: bool,
}

impl SpiMem {
    /// Puce de sauvegarde vide (0xFF) de la famille donnée.
    pub fn new(family: Family, size: usize) -> Self {
        let mut m = SpiMem::with_data(family, vec![0xFF; family.max_size()], None);
        m.size = family.size_for(size);
        m
    }

    /// Puce dont le contenu et la taille sont imposés.
    pub fn fixed(family: Family, data: Vec<u8>, jedec: [u8; 3]) -> Self {
        let mut m = SpiMem::with_data(family, data, Some(jedec));
        m.fixed = true;
        m
    }

    fn with_data(family: Family, data: Vec<u8>, jedec: Option<[u8; 3]>) -> Self {
        let size = data.len();
        SpiMem {
            family,
            data,
            size,
            fixed: false,
            jedec,
            cmd: None,
            header: Vec::new(),
            index: 0,
            wel: false,
            status: 0,
            dirty: false,
        }
    }

    /// Contenu à la taille actuelle (ce qu'on écrit dans le `.sav`).
    pub fn export(&self) -> Vec<u8> {
        self.data[..self.size].to_vec()
    }

    pub fn jedec(&self) -> Option<[u8; 3]> {
        if self.family != Family::Flash {
            return None;
        }
        self.jedec.or(Some(match self.size {
            s if s <= 256 << 10 => [0x20, 0x40, 0x12], // M45PE20 (GBATEK)
            s if s <= 512 << 10 => [0x20, 0x40, 0x13], // M45PE40 (fiche technique)
            s if s <= 1 << 20 => [0x20, 0x40, 0x14],   // M45PE80 (fiche technique)
            _ => [0xC2, 0x20, 0x17],                   // MX25L6445E : non vérifié
        }))
    }

    pub fn read_status(&self) -> u8 {
        let base = if self.family == Family::Eeprom512 { 0xF0 } else { 0 };
        base | self.status | (self.wel as u8) << 1
    }

    fn grow(&mut self, end: usize) {
        if !self.fixed && end > self.size {
            self.size = self.family.size_for(end);
        }
    }

    fn protected(&self, addr: usize) -> bool {
        if self.family == Family::Flash {
            return false;
        }
        let from = match (self.status >> 2) & 3 {
            0 => return false,
            1 => self.size - self.size / 4,
            2 => self.size / 2,
            _ => 0,
        };
        addr % self.size >= from
    }

    fn cell(&self, addr: u32) -> usize {
        addr as usize & (self.data.len() - 1)
    }

    pub fn transfer(&mut self, v: u8) -> u8 {
        let Some(cmd) = self.cmd else {
            self.cmd = Some(v);
            self.header.clear();
            self.index = 0;
            match v {
                0x06 => self.wel = true,
                0x04 => self.wel = false,
                _ => {}
            }
            return 0xFF;
        };
        let f = self.family;
        if self.header.len() < f.header_len(cmd) {
            self.header.push(v);
            return 0xFF;
        }
        let i = self.index;
        self.index += 1;
        let base = f.address(cmd, &self.header);
        match cmd {
            0x05 => self.read_status(),
            0x9F => self.jedec().and_then(|id| id.get(i as usize).copied()).unwrap_or(0xFF),
            0x01 => {
                if i == 0 && self.wel && f != Family::Flash {
                    let mask = if f == Family::Eeprom512 { 0x0C } else { 0x8C };
                    self.status = v & mask;
                }
                0xFF
            }
            c if f.is_read(c) => {
                // Lecture continue : l'adresse avance sans bouclage de page.
                let limit = if f == Family::Eeprom512 { 0x1FF } else { u32::MAX };
                self.data[self.cell(base.wrapping_add(i) & limit)]
            }
            c if f.is_write(c) && matches!(c, 0x02 | 0x0A) && self.wel => {
                let page = f.page() as u32;
                let addr = (base & !(page - 1)) | (base.wrapping_add(i) & (page - 1));
                let a = self.cell(addr);
                if !self.protected(a) {
                    self.grow(a + 1);
                    let cell = &mut self.data[a];
                    // FLASH : PP (02) ne fait passer que des 1 à 0.
                    *cell = if f == Family::Flash && c == 0x02 { *cell & v } else { v };
                    self.dirty = true;
                }
                0xFF
            }
            _ => 0xFF,
        }
    }

    /// Fin de la transaction (puce désélectionnée).
    pub fn deselect(&mut self) {
        if let Some(cmd) = self.cmd.take() {
            let f = self.family;
            let complete = self.header.len() >= f.header_len(cmd);
            if f == Family::Flash && self.wel && complete && matches!(cmd, 0xDB | 0xD8) {
                let len = if cmd == 0xDB { 0x100 } else { 0x1_0000 };
                let start = self.cell(f.address(cmd, &self.header)) & !(len - 1);
                self.grow(start + len);
                self.data[start..start + len].fill(0xFF);
                self.dirty = true;
            }
            if matches!(cmd, 0x01) || f.is_write(cmd) {
                self.wel = false;
            }
        }
        self.header.clear();
    }
}
