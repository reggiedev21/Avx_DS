//! Les 4 timers de chaque CPU (TMxCNT_L / TMxCNT_H, 0x04000100–0x0400010F).
//!
//! Un timer qui compte sur l'horloge n'est pas incrémenté à chaque cycle :
//! on mémorise sa valeur et l'instant de départ, on calcule la valeur
//! courante à la lecture, et on programme un événement pour le débordement.
//! Un timer en cascade (« count-up ») n'avance qu'au débordement du
//! précédent.

use super::scheduler::BUS_CYCLE;

/// Périodes des prescalers (en cycles de bus) : F/1, F/64, F/256, F/1024.
const PRESCALER: [u64; 4] = [1, 64, 256, 1024];

const CNT_CASCADE: u8 = 1 << 2;
const CNT_IRQ: u8 = 1 << 6;
const CNT_ENABLE: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Default)]
pub struct Timer {
    pub reload: u16,
    pub control: u8,
    /// Valeur du compteur à l'instant `start`.
    pub counter: u16,
    pub start: u64,
}

impl Timer {
    pub fn enabled(&self) -> bool {
        self.control & CNT_ENABLE != 0
    }

    /// Timer qui compte tout seul (activé, pas en cascade). Le timer 0 ne
    /// peut pas être en cascade.
    pub fn free_running(&self, n: usize) -> bool {
        self.enabled() && (n == 0 || self.control & CNT_CASCADE == 0)
    }

    /// Durée d'un tic, en cycles ARM9.
    pub fn period(&self) -> u64 {
        PRESCALER[(self.control & 3) as usize] * BUS_CYCLE
    }

    pub fn irq_enabled(&self) -> bool {
        self.control & CNT_IRQ != 0
    }

    /// Valeur courante du compteur.
    pub fn value(&self, n: usize, now: u64) -> u16 {
        if self.free_running(n) {
            let ticks = now.saturating_sub(self.start) / self.period();
            (self.counter as u64 + ticks).min(0xFFFF) as u16
        } else {
            self.counter
        }
    }

    /// Instant du prochain débordement (timer libre uniquement).
    pub fn overflow_time(&self) -> u64 {
        self.start + (0x1_0000 - self.counter as u64) * self.period()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Timers {
    pub t: [Timer; 4],
}
