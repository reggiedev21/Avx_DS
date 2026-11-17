//! Bus de la cartouche DS (slot 1) : ROMCTRL, commandes, données.
//!
//! La cartouche est supposée déjà en mode « main data » (après KEY2), état
//! laissé par le firmware au direct boot. Le chiffrement KEY2 est fait par
//! le matériel de façon transparente : le logiciel envoie des commandes et
//! reçoit des données en clair, et l'émulation travaille donc en clair.
//!
//! Commandes (GBATEK « DS Cartridge Protocol ») : B7 (lecture), B8 et 90
//! (identifiant), 00 (en-tête), 9F (factice). Transferts instantanés.

use super::save::Save;
use std::collections::VecDeque;

const CTRL_DATA_READY: u32 = 1 << 23;
const CTRL_BUSY: u32 = 1 << 31;
/// Bits de ROMCTRL que le logiciel peut écrire (23 est en lecture seule).
const CTRL_WRITABLE: u32 = !CTRL_DATA_READY;

#[derive(Clone, Debug)]
pub struct Slot1 {
    pub romctrl: u32,
    pub auxspicnt: u16,
    pub auxspidata: u8,
    /// Octets de commande, dans l'ordre d'envoi (0x040001A8 = premier).
    pub command: [u8; 8],
    /// Identifiant de la puce (4 octets, petit-boutiste).
    pub chip_id: u32,
    /// Données du transfert en cours.
    pending: VecDeque<u8>,
    /// Puce de sauvegarde (bus SPI de la cartouche).
    pub save: Save,
    /// Aiguillage de la transaction SPI en cours (None = puce désélectionnée).
    spi_route: Option<SpiRoute>,
    /// La cartouche a une puce infrarouge devant sa sauvegarde (vu un
    /// préfixe 00h). Ses commandes ne vont alors plus à la sauvegarde.
    pub infrared: bool,
}

/// Destination des octets d'une transaction SPI de la cartouche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpiRoute {
    /// Directement à la puce de sauvegarde.
    Save,
    /// Préfixe 00h vu : la suite va à la sauvegarde, à travers la puce
    /// infrarouge (GBATEK : « FLASH access needs a 00h prefix byte »).
    PassThrough,
    /// Commande destinée à la puce infrarouge elle-même (non émulée).
    Infrared,
}

impl Default for Slot1 {
    fn default() -> Self {
        Slot1 {
            romctrl: 1 << 29, // reset relâché
            auxspicnt: 0,
            auxspidata: 0,
            command: [0; 8],
            chip_id: 0x0000_00C2,
            pending: VecDeque::new(),
            save: Save::default(),
            spi_route: None,
            infrared: false,
        }
    }
}

/// Fin de transfert : faut-il lever l'IRQ « cartouche » ?
pub type EndIrq = bool;

impl Slot1 {
    pub fn transfer_active(&self) -> bool {
        self.romctrl & CTRL_BUSY != 0
    }

    pub fn words_ready(&self) -> bool {
        self.romctrl & CTRL_DATA_READY != 0
    }

    pub fn read_reg(&self, addr: u32) -> u32 {
        match addr {
            0x0400_01A0 => self.auxspicnt as u32 | (self.auxspidata as u32) << 16,
            0x0400_01A4 => self.romctrl,
            _ => 0, // commandes et graines : écriture seule
        }
    }

    /// Écriture d'un registre. Renvoie `Some(fin)` si un transfert vient de
    /// démarrer : `fin` = déjà terminé (taille nulle) avec IRQ à lever.
    pub fn write_reg(&mut self, addr: u32, val: u32, mask: u32, rom: &[u8]) -> Option<EndIrq> {
        let merge = |r: &mut u32, writable: u32| *r = (*r & !(mask & writable)) | (val & mask & writable);
        match addr {
            0x0400_01A0 => {
                let mut v = self.auxspicnt as u32;
                merge(&mut v, 0xE043);
                self.auxspicnt = v as u16;
                // Puce sélectionnée tant que le slot est activé en mode SPI
                // (bits 15 et 13) ; libnds la relâche en écrivant AUXSPICNT.
                let spi = self.auxspicnt & 0xA000 == 0xA000;
                if mask & 0xFFFF != 0 && !spi {
                    self.spi_deselect();
                }
                if mask & 0x00FF_0000 != 0 && spi {
                    self.auxspidata = self.spi_transfer((val >> 16) as u8);
                    if self.auxspicnt & 0x40 == 0 {
                        self.spi_deselect(); // pas de « hold » : fin après cet octet
                    }
                }
                None
            }
            0x0400_01A4 => {
                let was_busy = self.transfer_active();
                merge(&mut self.romctrl, CTRL_WRITABLE);
                if self.transfer_active() && !was_busy {
                    return Some(self.start(rom));
                }
                None
            }
            0x0400_01A8 | 0x0400_01AC => {
                let base = (addr - 0x0400_01A8) as usize;
                for k in 0..4 {
                    if mask & (0xFF << (8 * k)) != 0 {
                        self.command[base + k] = (val >> (8 * k)) as u8;
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Un octet sur le SPI de la cartouche, aiguillé selon le premier octet
    /// de la transaction.
    fn spi_transfer(&mut self, v: u8) -> u8 {
        let route = match self.spi_route {
            Some(r) => r,
            None => {
                // Début de transaction. Aucune commande EEPROM/FLASH ne vaut
                // 00h : c'est le préfixe des cartouches à infrarouge.
                let r = if v == 0x00 {
                    self.infrared = true;
                    SpiRoute::PassThrough
                } else if self.infrared {
                    SpiRoute::Infrared
                } else {
                    SpiRoute::Save
                };
                self.spi_route = Some(r);
                if r != SpiRoute::Save {
                    // Le préfixe (ou la commande infrarouge) ne va pas à la
                    // sauvegarde. Réponse inconnue : bus au repos.
                    return 0xFF;
                }
                r
            }
        };
        match route {
            SpiRoute::Save | SpiRoute::PassThrough => self.save.transfer(v),
            SpiRoute::Infrared => 0xFF, // puce infrarouge non émulée
        }
    }

    fn spi_deselect(&mut self) {
        if matches!(self.spi_route.take(), Some(SpiRoute::Save | SpiRoute::PassThrough)) {
            self.save.deselect();
        }
    }

    fn block_size(&self) -> usize {
        match (self.romctrl >> 24) & 7 {
            0 => 0,
            7 => 4,
            n => 0x100 << n,
        }
    }

    fn start(&mut self, rom: &[u8]) -> EndIrq {
        let len = self.block_size();
        let c = self.command;
        let id = self.chip_id.to_le_bytes();
        self.pending.clear();
        match c[0] {
            0xB7 => {
                let mut addr = u32::from_be_bytes([c[1], c[2], c[3], c[4]]);
                if addr < 0x8000 {
                    addr = 0x8000 + (addr & 0x1FF);
                }
                // Miroir au-delà de la taille de la puce (puissance de 2).
                let chip = rom.len().next_power_of_two().max(1) as u32;
                for i in 0..len as u32 {
                    let a = ((addr & !0xFFF) | (addr.wrapping_add(i) & 0xFFF)) & (chip - 1);
                    self.pending.push_back(rom.get(a as usize).copied().unwrap_or(0xFF));
                }
            }
            0xB8 | 0x90 => self.pending.extend((0..len).map(|i| id[i & 3])),
            0x00 => self.pending.extend((0..len).map(|i| rom.get(i & 0xFFF).copied().unwrap_or(0xFF))),
            _ => self.pending.extend(std::iter::repeat_n(0xFF, len)),
        }
        if self.pending.is_empty() {
            self.romctrl &= !(CTRL_BUSY | CTRL_DATA_READY);
            return self.auxspicnt & (1 << 14) != 0;
        }
        self.romctrl |= CTRL_DATA_READY;
        false
    }

    /// Lecture de 0x04100010. Renvoie le mot et, si c'était le dernier,
    /// s'il faut lever l'IRQ de fin de transfert.
    pub fn read_data(&mut self) -> (u32, EndIrq) {
        if self.pending.is_empty() {
            return (0xFFFF_FFFF, false);
        }
        let mut w = [0xFF; 4];
        for b in &mut w {
            if let Some(x) = self.pending.pop_front() {
                *b = x;
            }
        }
        let mut irq = false;
        if self.pending.is_empty() {
            self.romctrl &= !(CTRL_BUSY | CTRL_DATA_READY);
            irq = self.auxspicnt & (1 << 14) != 0;
        }
        (u32::from_le_bytes(w), irq)
    }
}
