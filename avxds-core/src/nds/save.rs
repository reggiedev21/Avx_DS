//! Puce de sauvegarde de la cartouche, avec détection automatique du type.
//!
//! Le jeu sait quelle puce il a, mais rien dans la ROM ne le dit. La
//! détection est une machine à états fondée sur deux observations :
//!
//! 1. **Tant que la mémoire est vierge, le type ne change rien aux
//!    lectures** : tout vaut 0xFF quelle que soit la largeur d'adresse. On
//!    peut donc répondre juste sans avoir choisi, et ne s'engager qu'à la
//!    première écriture, qu'on reçoit en entier avant de l'appliquer (une
//!    vraie puce ne la valide qu'à la désélection).
//! 2. **La forme des transactions trahit la puce.** Une transaction est
//!    `commande + en-tête (adresse) + n octets`. Pour chaque hypothèse :
//!    - une écriture correcte ne franchit jamais une page (elle reboucle et
//!      corromprait la sauvegarde) et reste dans la taille maximale ;
//!    - pendant la phase de données d'une lecture, le jeu envoie un octet
//!      factice constant : si cette suite constante commence après la fin
//!      supposée de l'adresse, l'hypothèse est fausse ;
//!    - n est presque toujours une puissance de 2, et l'adresse alignée ;
//!    - certaines commandes n'existent que dans une famille (effacements,
//!      RDID pour la FLASH).
//!
//! Avec un `.sav` importé, la mémoire n'est plus vierge. L'étendue des
//! données (dernier octet différent de 0xFF) donne une borne basse sur la
//! taille, la taille du fichier une borne haute (certains outils complètent
//! à 512 Ko), et l'a priori favorise la plus petite famille qui contient
//! les données.

use super::spimem::{Family, SpiMem};

const PREFERENCE: [Family; 4] = [Family::Eeprom16, Family::Flash, Family::Eeprom512, Family::Eeprom24];
const MAX_TRACE: usize = 32;

/// Taille de fichier à laquelle on a vu des sauvegardes complétées (les
/// trois `.sav` de test, dont deux n'utilisent que 8 et 32 Ko).
const PADDED_SIZE: usize = 512 << 10;

fn save_size_is_trusted(len: usize) -> bool {
    len != PADDED_SIZE
}

#[derive(Clone, Debug)]
pub struct Save {
    state: State,
    /// Raisons des décisions de la détection, pour le diagnostic.
    pub trace: Vec<String>,
}

#[derive(Clone, Debug)]
enum State {
    Detecting(Box<Detector>),
    Locked(SpiMem),
}

#[derive(Clone, Debug)]
struct Detector {
    /// Contenu importé (vide si aucun `.sav`).
    data: Vec<u8>,
    extent: usize,
    file_size: usize,
    alive: [bool; 4],
    score: [i32; 4],
    /// Octets de la transaction en cours.
    tx: Vec<u8>,
    wel: bool,
    status: u8,
}

fn idx(f: Family) -> usize {
    Family::ALL.iter().position(|&x| x == f).unwrap()
}

impl Default for Save {
    fn default() -> Self {
        Save::load(&[])
    }
}

impl Save {
    /// Sauvegarde importée (ou vide). Le type sera détecté.
    pub fn load(bytes: &[u8]) -> Self {
        let extent = bytes.iter().rposition(|&b| b != 0xFF).map_or(0, |p| p + 1);
        let mut d = Detector {
            data: bytes.to_vec(),
            extent,
            file_size: bytes.len(),
            alive: [true; 4],
            score: [0; 4],
            tx: Vec::new(),
            wel: false,
            status: 0,
        };
        // L'EEPROM de 128 Ko est rare (et son protocole non confirmé) : elle
        // ne gagne que sur preuve.
        d.score[idx(Family::Eeprom24)] -= 2;
        let mut trace = Vec::new();
        for f in Family::ALL {
            let i = idx(f);
            if f.max_size() < extent {
                d.alive[i] = false;
                trace.push(format!("{f:?} écartée : données jusqu'à 0x{extent:X}"));
            } else if d.file_size > 0 && f.min_size() > d.file_size {
                d.alive[i] = false;
                trace.push(format!("{f:?} écartée : fichier de {} octets", d.file_size));
            }
            if d.file_size > 0 && f.sizes().contains(&d.file_size) {
                d.score[i] += 1;
            }
        }
        if extent > 0 {
            // A priori : la plus petite famille qui contient les données.
            if let Some(f) = Family::ALL.into_iter().filter(|&f| d.alive[idx(f)]).min_by_key(|f| f.max_size()) {
                d.score[idx(f)] += 2;
            }
        }
        // Taille exacte d'une famille : c'est la vraie taille, sauf 512 Ko,
        // taille à laquelle certains outils complètent tous les fichiers.
        let exact: Vec<Family> =
            Family::ALL.into_iter().filter(|&f| d.alive[idx(f)] && f.sizes().contains(&d.file_size)).collect();
        let mut save = Save { state: State::Detecting(Box::new(d)), trace };
        match exact[..] {
            [f] if save_size_is_trusted(bytes.len()) => save.lock(f, None, "taille exacte du fichier"),
            _ => save.lock_if_unique(),
        }
        save
    }

    /// Type imposé (option `--save-type`), contenu éventuel.
    pub fn forced(family: Family, size: usize, bytes: &[u8]) -> Self {
        let mut mem = SpiMem::new(family, size.max(bytes.len().min(family.max_size())));
        let n = bytes.len().min(mem.data.len());
        mem.data[..n].copy_from_slice(&bytes[..n]);
        Save { state: State::Locked(mem), trace: vec![format!("{family:?} imposée")] }
    }

    /// Famille et taille, une fois la détection terminée.
    pub fn locked(&self) -> Option<(Family, usize)> {
        match &self.state {
            State::Locked(m) => Some((m.family, m.size)),
            State::Detecting(_) => None,
        }
    }

    /// Hypothèse en tête pendant la détection (ou famille verrouillée).
    pub fn leading(&self) -> Family {
        match &self.state {
            State::Locked(m) => m.family,
            State::Detecting(d) => d.leading(),
        }
    }

    pub fn dirty(&self) -> bool {
        matches!(&self.state, State::Locked(m) if m.dirty)
    }

    pub fn clear_dirty(&mut self) {
        if let State::Locked(m) = &mut self.state {
            m.dirty = false;
        }
    }

    /// Contenu à écrire dans le `.sav`, à la taille standard de la puce.
    /// `None` tant que le jeu n'a rien écrit et que rien n'a été importé.
    pub fn export(&self) -> Option<Vec<u8>> {
        match &self.state {
            State::Locked(m) => Some(m.export()),
            State::Detecting(d) if !d.data.is_empty() => Some(d.data.clone()),
            State::Detecting(_) => None,
        }
    }

    fn note(&mut self, s: String) {
        if self.trace.len() < MAX_TRACE {
            self.trace.push(s);
        }
    }

    pub fn transfer(&mut self, v: u8) -> u8 {
        match &mut self.state {
            State::Locked(m) => m.transfer(v),
            State::Detecting(d) => {
                d.tx.push(v);
                if d.tx.len() == 1 {
                    match v {
                        0x06 => d.wel = true,
                        0x04 => d.wel = false,
                        0x9F if d.alive[idx(Family::Flash)] => d.score[idx(Family::Flash)] += 2,
                        _ => {}
                    }
                }
                d.respond()
            }
        }
    }

    pub fn deselect(&mut self) {
        let State::Detecting(d) = &mut self.state else {
            if let State::Locked(m) = &mut self.state {
                m.deselect();
            }
            return;
        };
        let tx = std::mem::take(&mut d.tx);
        let Some(&cmd) = tx.first() else { return };
        if cmd == 0x9F {
            let s = "RDID : indice pour la FLASH".to_string();
            self.note(s);
        }
        let State::Detecting(d) = &mut self.state else { unreachable!() };
        match cmd {
            0x01 => {
                if d.wel && tx.len() > 1 {
                    d.status = tx[1] & 0x8C;
                }
                d.wel = false;
            }
            0x03 | 0x0B => {
                let notes = d.read_evidence(&tx);
                for n in notes {
                    self.note(n);
                }
            }
            0x02 | 0x0A | 0xDB | 0xD8 if d.wel => {
                let notes = d.write_evidence(&tx);
                let best = d.leading();
                for n in notes {
                    self.note(n);
                }
                self.lock(best, Some(&tx), "première écriture");
                return;
            }
            0x02 | 0x0A | 0xDB | 0xD8 => d.wel = false, // sans WREN : ignorée
            _ => {}
        }
        self.lock_if_unique();
    }

    fn lock_if_unique(&mut self) {
        let State::Detecting(d) = &self.state else { return };
        let alive: Vec<Family> = Family::ALL.into_iter().filter(|&f| d.alive[idx(f)]).collect();
        if let [only] = alive[..] {
            self.lock(only, None, "seule famille possible");
        }
    }

    fn lock(&mut self, family: Family, replay: Option<&[u8]>, why: &str) {
        let State::Detecting(d) = &self.state else { return };
        let size = if family.sizes().contains(&d.file_size) {
            d.file_size
        } else {
            family.size_for(d.extent)
        };
        let mut mem = SpiMem::new(family, size);
        let n = d.data.len().min(mem.data.len());
        mem.data[..n].copy_from_slice(&d.data[..n]);
        mem.wel = d.wel;
        mem.status = d.status;
        let scores = Family::ALL.map(|f| (d.alive[idx(f)], d.score[idx(f)]));
        if let Some(tx) = replay {
            for &b in tx {
                mem.transfer(b);
            }
            mem.deselect();
        }
        let msg = format!("verrouillée : {family:?} {} Ko ({why}) ; scores {scores:?}", mem.size >> 10);
        self.state = State::Locked(mem);
        self.note(msg);
    }
}

impl Detector {
    fn leading(&self) -> Family {
        let rank = |f: Family| PREFERENCE.iter().position(|&p| p == f).unwrap() as i32;
        let pick = |only_alive: bool| {
            Family::ALL
                .into_iter()
                .filter(|&f| !only_alive || self.alive[idx(f)])
                .max_by_key(|&f| (self.score[idx(f)], -rank(f)))
        };
        pick(true).or_else(|| pick(false)).unwrap()
    }

    fn byte(&self, addr: u32) -> u8 {
        self.data.get(addr as usize).copied().unwrap_or(0xFF)
    }

    /// Réponse au dernier octet reçu, selon l'hypothèse en tête.
    fn respond(&self) -> u8 {
        let f = self.leading();
        let cmd = self.tx[0];
        let k = self.tx.len() - 1;
        let hl = f.header_len(cmd);
        if k == 0 || k <= hl {
            return 0xFF;
        }
        let i = (k - hl - 1) as u32;
        match cmd {
            0x05 => {
                let base = if f == Family::Eeprom512 { 0xF0 } else { 0 };
                base | self.status | (self.wel as u8) << 1
            }
            0x9F if f != Family::Flash => 0xFF, // EEPROM/FRAM : pas de RDID
            0x9F => {
                let est = Family::Flash.size_for(self.extent.max(self.file_size));
                SpiMem::new(Family::Flash, est).jedec().map_or(0xFF, |id| id.get(i as usize).copied().unwrap_or(0xFF))
            }
            c if f.is_read(c) => self.byte(f.address(c, &self.tx[1..=hl]).wrapping_add(i)),
            _ => 0xFF,
        }
    }

    fn read_evidence(&mut self, tx: &[u8]) -> Vec<String> {
        let (cmd, len) = (tx[0], tx.len());
        // Début de la suite constante finale (les octets factices).
        let v = tx[len - 1];
        let mut run = len - 1;
        while run > 1 && tx[run - 1] == v {
            run -= 1;
        }
        let mut notes = Vec::new();
        for f in Family::ALL {
            let i = idx(f);
            if !self.alive[i] {
                continue;
            }
            if !f.is_read(cmd) {
                self.score[i] -= 4;
                continue;
            }
            let hl = f.header_len(cmd);
            let Some(n) = (len - 1).checked_sub(hl) else {
                self.score[i] -= 4;
                continue;
            };
            if n >= 2 && run > hl + 1 {
                self.score[i] -= 4;
                notes.push(format!("lecture {cmd:02X} de {len} octets : en-tête plus long que {hl} pour {f:?}"));
            }
            if n > 0 && n.is_power_of_two() {
                self.score[i] += 1;
            }
        }
        notes
    }

    fn write_evidence(&mut self, tx: &[u8]) -> Vec<String> {
        let (cmd, len) = (tx[0], tx.len());
        let before = self.alive;
        let mut notes = Vec::new();
        for f in Family::ALL {
            let i = idx(f);
            if !self.alive[i] {
                continue;
            }
            let hl = f.header_len(cmd);
            let n = (len - 1).checked_sub(hl);
            let reason = match n {
                _ if !f.is_write(cmd) => Some(format!("commande {cmd:02X} inconnue")),
                None => Some("transaction trop courte".to_string()),
                Some(n) => {
                    let addr = f.address(cmd, &tx[1..=hl]) as usize;
                    let page = f.page();
                    let is_erase = matches!(cmd, 0xDB | 0xD8);
                    if addr + n > f.max_size() {
                        Some(format!("0x{addr:X} + {n} hors de la puce"))
                    } else if !is_erase && addr % page + n > page {
                        Some(format!("0x{addr:X} + {n} franchit une page de {page}"))
                    } else {
                        if n > 0 && n.is_power_of_two() {
                            self.score[i] += 1;
                        }
                        if n > 0 && addr.is_multiple_of(n) {
                            self.score[i] += 1;
                        }
                        if f == Family::Flash && cmd == 0x02 {
                            // GBATEK : la FLASH écrit en 0A, ou efface (DB/D8)
                            // avant un 02. Un effacement verrouille la FLASH
                            // aussitôt, donc un 02 en détection n'a pas eu
                            // d'effacement : il désigne plutôt une EEPROM. Sur
                            // des données non effacées, PP les corromprait.
                            self.score[i] -= 2;
                            if (addr..addr + n).any(|a| self.byte(a as u32) != 0xFF) {
                                self.score[i] -= 3;
                            }
                        }
                        None
                    }
                }
            };
            if let Some(r) = reason {
                self.alive[i] = false;
                notes.push(format!("{f:?} écartée : {r}"));
            }
        }
        if !self.alive.iter().any(|&a| a) {
            self.alive = before; // aucune hypothèse cohérente : on garde la meilleure
            notes.push("écriture incohérente avec toutes les familles".to_string());
        }
        notes
    }
}
