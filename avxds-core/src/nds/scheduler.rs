//! Ordonnanceur d'événements.
//!
//! Le temps est compté en **cycles ARM9** (67,03 MHz). Un cycle de bus
//! (33,51 MHz, horloge de l'ARM7 et des timers) vaut 2 cycles ARM9.
//!
//! Chaque type d'événement a un emplacement fixe : au plus une occurrence
//! en attente par type, ce qui suffit à la DS (une ligne d'écran, un
//! débordement par timer…) et évite toute allocation.

use super::CpuId;

/// Un cycle de bus (ARM7, timers, écran) en cycles ARM9.
pub const BUS_CYCLE: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Début du HBlank de la ligne courante.
    HBlank,
    /// Fin de la ligne courante : passage à la suivante.
    LineEnd,
    /// Débordement d'un timer.
    Timer(CpuId, u8),
}

const N: usize = 10;

impl Event {
    fn index(self) -> usize {
        match self {
            Event::HBlank => 0,
            Event::LineEnd => 1,
            Event::Timer(CpuId::Arm9, n) => 2 + n as usize,
            Event::Timer(CpuId::Arm7, n) => 6 + n as usize,
        }
    }

    fn from_index(i: usize) -> Event {
        match i {
            0 => Event::HBlank,
            1 => Event::LineEnd,
            2..=5 => Event::Timer(CpuId::Arm9, (i - 2) as u8),
            _ => Event::Timer(CpuId::Arm7, (i - 6) as u8),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Scheduler {
    slots: [u64; N],
    next: u64,
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler { slots: [u64::MAX; N], next: u64::MAX }
    }
}

impl Scheduler {
    /// Programme (ou reprogramme) un événement à l'instant `time`.
    pub fn schedule(&mut self, ev: Event, time: u64) {
        self.slots[ev.index()] = time;
        self.next = self.next.min(time);
    }

    pub fn cancel(&mut self, ev: Event) {
        let i = ev.index();
        if self.slots[i] != u64::MAX {
            self.slots[i] = u64::MAX;
            self.recompute();
        }
    }

    /// Instant du prochain événement (`u64::MAX` si aucun).
    #[inline]
    pub fn next_time(&self) -> u64 {
        self.next
    }

    pub fn time_of(&self, ev: Event) -> Option<u64> {
        let t = self.slots[ev.index()];
        (t != u64::MAX).then_some(t)
    }

    /// Retire et renvoie l'événement le plus ancien arrivé à échéance
    /// (`time <= now`). À égalité, l'ordre des emplacements fait foi.
    pub fn pop_due(&mut self, now: u64) -> Option<(Event, u64)> {
        if self.next > now {
            return None;
        }
        let (i, &t) = self.slots.iter().enumerate().min_by_key(|&(_, &t)| t)?;
        self.slots[i] = u64::MAX;
        self.recompute();
        Some((Event::from_index(i), t))
    }

    fn recompute(&mut self) {
        self.next = self.slots.iter().copied().min().unwrap_or(u64::MAX);
    }
}
