//! File de commandes de géométrie : FIFO de 256 entrées + PIPE de 4, et
//! décodage du format compact de GXFIFO.
//!
//! Source : GBATEK « DS 3D Geometry Commands » (`docs/3D_SPEC.md`,
//! section 2).

use std::collections::VecDeque;

use super::commands;

pub const FIFO_CAP: usize = 256;
pub const PIPE_CAP: usize = 4;
/// Seuil du bit 25 de GXSTAT et du déclenchement de la DMA.
pub const HALF: usize = FIFO_CAP / 2;

/// Une entrée de 40 bits : commande 8 bits + paramètre 32 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub cmd: u8,
    pub param: u32,
}

/// FIFO + PIPE. Le moteur lit dans le PIPE ; GXSTAT ne compte que la FIFO.
///
/// `reserve` n'existe pas sur la console : elle garde les entrées reçues
/// quand la file est pleine et que le moteur ne peut pas avancer (attente
/// d'un SwapBuffers). Sur la console, le CPU resterait bloqué à la place.
#[derive(Clone, Debug, Default)]
pub struct CommandFifo {
    fifo: VecDeque<Entry>,
    pipe: VecDeque<Entry>,
    reserve: VecDeque<Entry>,
}

impl CommandFifo {
    /// Entrées dans la FIFO (bits 16–24 de GXSTAT), 0 à 256.
    pub fn fifo_len(&self) -> usize {
        self.fifo.len()
    }

    pub fn pipe_len(&self) -> usize {
        self.pipe.len()
    }

    pub fn reserve_len(&self) -> usize {
        self.reserve.len()
    }

    pub fn is_full(&self) -> bool {
        self.fifo.len() >= FIFO_CAP
    }

    /// Plus rien à exécuter nulle part.
    pub fn is_drained(&self) -> bool {
        self.fifo.is_empty() && self.pipe.is_empty() && self.reserve.is_empty()
    }

    /// Ajoute une entrée. Elle va dans le PIPE si la FIFO est vide et le
    /// PIPE non plein, sinon dans la FIFO. Renvoie `false` si la FIFO est
    /// pleine (rien n'est ajouté : à l'appelant de faire de la place).
    pub fn push(&mut self, e: Entry) -> bool {
        if self.fifo.is_empty() && self.pipe.len() < PIPE_CAP {
            self.pipe.push_back(e);
        } else if self.fifo.len() < FIFO_CAP {
            self.fifo.push_back(e);
        } else {
            return false;
        }
        true
    }

    /// Range une entrée hors file (file pleine, moteur à l'arrêt).
    pub fn push_reserve(&mut self, e: Entry) {
        self.reserve.push_back(e);
    }

    /// Retire l'entrée suivante du PIPE. Quand le PIPE passe sous 3
    /// entrées, 2 entrées viennent de la FIFO ; la réserve comble la FIFO.
    pub fn pop(&mut self) -> Option<Entry> {
        self.refill();
        let e = self.pipe.pop_front();
        self.refill();
        e
    }

    fn refill(&mut self) {
        if self.pipe.len() < 3 {
            for _ in 0..2 {
                match self.fifo.pop_front() {
                    Some(e) => self.pipe.push_back(e),
                    None => break,
                }
            }
        }
        while self.fifo.len() < FIFO_CAP {
            match self.reserve.pop_front() {
                Some(e) => self.fifo.push_back(e),
                None => break,
            }
        }
    }
}

/// Décodeur du format compact de GXFIFO : un mot de commandes (jusqu'à 4
/// codes), puis les paramètres de chacune, dans l'ordre.
#[derive(Clone, Debug, Default)]
pub struct Unpacker {
    cmds: [u8; 4],
    n: usize,
    i: usize,
    /// Paramètres encore attendus pour `cmds[i]`.
    left: u8,
    /// Mots dont le 4e emplacement porte une commande sans paramètre :
    /// GBATEK l'interdit sans dire ce qui se passe (point ouvert).
    pub top_slot_without_params: u32,
    /// Codes invalides rencontrés (ignorés).
    pub invalid: u32,
}

impl Unpacker {
    /// Traite un mot écrit dans GXFIFO ; `out` reçoit les entrées produites.
    pub fn feed(&mut self, word: u32, out: &mut impl FnMut(Entry)) {
        if self.left > 0 {
            out(Entry { cmd: self.cmds[self.i], param: word });
            self.left -= 1;
            if self.left == 0 {
                self.i += 1;
                self.advance(out);
            }
            return;
        }
        // Nouveau mot de commandes. 00h = pas de commande ; codes invalides
        // ignorés sans consommer de paramètres.
        self.n = 0;
        self.i = 0;
        for slot in 0..4 {
            let cmd = (word >> (slot * 8)) as u8;
            if cmd == 0 {
                continue;
            }
            match commands::info(cmd) {
                Some(info) => {
                    if slot == 3 && info.params == 0 {
                        self.top_slot_without_params += 1;
                    }
                    self.cmds[self.n] = cmd;
                    self.n += 1;
                }
                None => self.invalid += 1,
            }
        }
        self.advance(out);
    }

    /// Émet les commandes sans paramètre jusqu'à la prochaine qui en attend.
    fn advance(&mut self, out: &mut impl FnMut(Entry)) {
        while self.i < self.n {
            let cmd = self.cmds[self.i];
            let params = commands::info(cmd).map_or(0, |i| i.params);
            if params > 0 {
                self.left = params;
                return;
            }
            out(Entry { cmd, param: 0 });
            self.i += 1;
        }
        self.n = 0;
        self.i = 0;
    }

    /// Un paramètre est-il attendu (le mot suivant n'est pas un mot de
    /// commandes) ?
    pub fn expects_param(&self) -> bool {
        self.left > 0
    }
}
