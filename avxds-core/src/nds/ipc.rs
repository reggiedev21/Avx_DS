//! FIFO IPC : deux files de 16 mots entre l'ARM9 et l'ARM7.
//!
//! IPCFIFOCNT (0x04000184), IPCFIFOSEND (0x04000188), IPCFIFORECV
//! (0x04100000). Indice 0 = ARM9, 1 = ARM7. `queue[c]` est la file que le
//! CPU `c` remplit (et que l'autre vide).

use super::irq_bits as irq;
use std::collections::VecDeque;

const SEND_EMPTY: u16 = 1 << 0;
const SEND_FULL: u16 = 1 << 1;
const SEND_EMPTY_IRQ: u16 = 1 << 2;
const SEND_CLEAR: u16 = 1 << 3;
const RECV_EMPTY: u16 = 1 << 8;
const RECV_FULL: u16 = 1 << 9;
const RECV_IRQ: u16 = 1 << 10;
const ERROR: u16 = 1 << 14;
const ENABLE: u16 = 1 << 15;

pub const DEPTH: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct IpcFifo {
    pub queue: [VecDeque<u32>; 2],
    /// Bits mémorisés de IPCFIFOCNT par CPU (2, 10, 14, 15).
    pub cnt: [u16; 2],
    /// Dernier mot reçu par chaque CPU (relu si la file est vide).
    pub last: [u32; 2],
}

/// IRQ à lever, pour [ARM9, ARM7].
pub type Irqs = [u32; 2];

impl IpcFifo {
    pub fn read_cnt(&self, cpu: usize) -> u16 {
        let send = &self.queue[cpu];
        let recv = &self.queue[1 - cpu];
        let mut v = self.cnt[cpu] & (SEND_EMPTY_IRQ | RECV_IRQ | ERROR | ENABLE);
        if send.is_empty() {
            v |= SEND_EMPTY;
        }
        if send.len() == DEPTH {
            v |= SEND_FULL;
        }
        if recv.is_empty() {
            v |= RECV_EMPTY;
        }
        if recv.len() == DEPTH {
            v |= RECV_FULL;
        }
        v
    }

    pub fn write_cnt(&mut self, cpu: usize, val: u16, mask: u16) -> Irqs {
        let mut out = [0; 2];
        let val = val & mask;
        let old = self.cnt[cpu];
        if val & SEND_CLEAR != 0 {
            self.queue[cpu].clear();
        }
        if val & ERROR != 0 {
            self.cnt[cpu] &= !ERROR; // 1 = acquitter
        }
        let m = mask & (SEND_EMPTY_IRQ | RECV_IRQ | ENABLE);
        self.cnt[cpu] = (self.cnt[cpu] & !m) | (val & m);
        let new = self.cnt[cpu];
        // Les IRQ sont des fronts : autoriser l'IRQ alors que la condition
        // est déjà vraie en déclenche une.
        let rose = |bit: u16| old & bit == 0 && new & bit != 0;
        if (rose(SEND_EMPTY_IRQ) || val & SEND_CLEAR != 0)
            && new & SEND_EMPTY_IRQ != 0
            && self.queue[cpu].is_empty()
        {
            out[cpu] |= irq::IPC_SEND_EMPTY;
        }
        if rose(RECV_IRQ) && !self.queue[1 - cpu].is_empty() {
            out[cpu] |= irq::IPC_RECV_NOT_EMPTY;
        }
        out
    }

    pub fn send(&mut self, cpu: usize, val: u32) -> Irqs {
        let mut out = [0; 2];
        if self.cnt[cpu] & ENABLE == 0 {
            return out;
        }
        let q = &mut self.queue[cpu];
        if q.len() == DEPTH {
            self.cnt[cpu] |= ERROR;
            return out;
        }
        let was_empty = q.is_empty();
        q.push_back(val);
        let other = 1 - cpu;
        if was_empty && self.cnt[other] & RECV_IRQ != 0 {
            out[other] |= irq::IPC_RECV_NOT_EMPTY;
        }
        out
    }

    pub fn receive(&mut self, cpu: usize) -> (u32, Irqs) {
        let mut out = [0; 2];
        let other = 1 - cpu;
        if self.cnt[cpu] & ENABLE == 0 {
            // FIFO désactivée : on relit le premier mot sans le retirer.
            let v = self.queue[other].front().copied().unwrap_or(self.last[cpu]);
            return (v, out);
        }
        match self.queue[other].pop_front() {
            None => {
                self.cnt[cpu] |= ERROR;
                (self.last[cpu], out)
            }
            Some(v) => {
                self.last[cpu] = v;
                if self.queue[other].is_empty() && self.cnt[other] & SEND_EMPTY_IRQ != 0 {
                    out[other] |= irq::IPC_SEND_EMPTY;
                }
                (v, out)
            }
        }
    }
}
