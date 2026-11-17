//! Interpréteur ARM/Thumb générique sur la version d'architecture.
//!
//! * [`Arm9`] = `Cpu<Armv5te>` : ARM946E-S (BLX, CLZ, QADD, LDRD, CP15…)
//! * [`Arm7`] = `Cpu<Armv4t>`  : ARM7TDMI
//!
//! ## Le PC
//!
//! Entre deux instructions, `r[15]` contient l'adresse de la **prochaine
//! instruction à exécuter**. Pendant l'exécution, `r[15]` vaut cette
//! adresse + 8 (ARM) ou + 4 (Thumb), exactement ce que voit le code ARM
//! qui lit R15. Une instruction qui écrit R15 lève le drapeau `branched` ;
//! sinon le PC avance de la taille de l'instruction.

mod alu;
mod arm;
mod thumb;

#[cfg(test)]
mod tests;

use crate::bus::Bus;
use core::marker::PhantomData;

/// Version d'architecture.
pub trait Arch: 'static {
    /// `true` pour ARMv5TE (ARM9), `false` pour ARMv4T (ARM7).
    const V5: bool;
    const NAME: &'static str;
}

/// ARMv4T : ARM7TDMI.
#[derive(Debug)]
pub enum Armv4t {}
/// ARMv5TE : ARM946E-S.
#[derive(Debug)]
pub enum Armv5te {}

impl Arch for Armv4t {
    const V5: bool = false;
    const NAME: &'static str = "ARMv4T";
}
impl Arch for Armv5te {
    const V5: bool = true;
    const NAME: &'static str = "ARMv5TE";
}

pub type Arm9 = Cpu<Armv5te>;
pub type Arm7 = Cpu<Armv4t>;

// Bits du CPSR.
pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_Q: u32 = 1 << 27;
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;

/// Modes du processeur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Mode {
    User = 0x10,
    Fiq = 0x11,
    Irq = 0x12,
    Supervisor = 0x13,
    Abort = 0x17,
    Undefined = 0x1B,
    System = 0x1F,
}

impl Mode {
    /// Mode correspondant aux 5 bits bas du CPSR. Une valeur invalide est
    /// traitée comme User (comportement imprévisible sur le vrai matériel).
    pub fn from_bits(bits: u32) -> Mode {
        match bits & 0x1F {
            0x11 => Mode::Fiq,
            0x12 => Mode::Irq,
            0x13 => Mode::Supervisor,
            0x17 => Mode::Abort,
            0x1B => Mode::Undefined,
            0x1F => Mode::System,
            _ => Mode::User,
        }
    }

    /// Index de banque : User et System partagent la banque 0.
    fn bank(self) -> usize {
        match self {
            Mode::User | Mode::System => 0,
            Mode::Fiq => 1,
            Mode::Irq => 2,
            Mode::Supervisor => 3,
            Mode::Abort => 4,
            Mode::Undefined => 5,
        }
    }

    fn has_spsr(self) -> bool {
        self.bank() != 0
    }
}

/// Exceptions ARM. La DS n'utilise ni FIQ ni abort de données (pas de MMU) ;
/// les variantes existent pour la complétude du modèle.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exception {
    Reset,
    Undefined,
    Swi,
    PrefetchAbort,
    DataAbort,
    Irq,
    Fiq,
}

impl Exception {
    fn mode(self) -> Mode {
        match self {
            Exception::Reset | Exception::Swi => Mode::Supervisor,
            Exception::Undefined => Mode::Undefined,
            Exception::PrefetchAbort | Exception::DataAbort => Mode::Abort,
            Exception::Irq => Mode::Irq,
            Exception::Fiq => Mode::Fiq,
        }
    }

    fn vector(self) -> u32 {
        match self {
            Exception::Reset => 0x00,
            Exception::Undefined => 0x04,
            Exception::Swi => 0x08,
            Exception::PrefetchAbort => 0x0C,
            Exception::DataAbort => 0x10,
            Exception::Irq => 0x18,
            Exception::Fiq => 0x1C,
        }
    }
}

pub struct Cpu<A: Arch> {
    /// Registres visibles dans le mode courant.
    r: [u32; 16],
    cpsr: u32,
    /// SPSR par banque (l'entrée 0, User/System, n'est jamais utilisée).
    spsr: [u32; 6],
    /// R13/R14 sauvegardés, par banque.
    bank_sp_lr: [[u32; 2]; 6],
    /// R8–R12 du mode FIQ et de tous les autres modes.
    bank_fiq_r8_12: [u32; 5],
    bank_usr_r8_12: [u32; 5],

    /// Adresse de l'instruction en cours d'exécution.
    cur_pc: u32,
    branched: bool,

    /// Ligne IRQ : à mettre à jour par la machine (IME && (IE & IF) != 0).
    pub irq_line: bool,
    /// CPU en pause (Halt BIOS, CP15 « wait for interrupt »). La machine le
    /// remet à `false` quand `IE & IF != 0`, même si IME vaut 0.
    pub halted: bool,

    /// Nombre d'instructions exécutées depuis le reset.
    pub instructions: u64,

    _arch: PhantomData<A>,
}

impl<A: Arch> Default for Cpu<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Arch> core::fmt::Debug for Cpu<A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{} pc={:08X} cpsr={:08X} ({:?}{})", A::NAME, self.r[15], self.cpsr,
            self.mode(), if self.thumb() { ", Thumb" } else { "" })?;
        for row in 0..4 {
            for col in 0..4 {
                let i = row * 4 + col;
                write!(f, "r{:<2}={:08X} ", i, self.r[i])?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl<A: Arch> Cpu<A> {
    /// CPU dans l'état qui suit un reset matériel : mode Superviseur,
    /// IRQ et FIQ masquées, ARM, PC = 0. Appeler [`Cpu::reset`] pour
    /// démarrer au vecteur de reset d'un bus donné.
    pub fn new() -> Self {
        Cpu {
            r: [0; 16],
            cpsr: Mode::Supervisor as u32 | FLAG_I | FLAG_F,
            spsr: [0; 6],
            bank_sp_lr: [[0; 2]; 6],
            bank_fiq_r8_12: [0; 5],
            bank_usr_r8_12: [0; 5],
            cur_pc: 0,
            branched: false,
            irq_line: false,
            halted: false,
            instructions: 0,
            _arch: PhantomData,
        }
    }

    /// Reset : mode Superviseur, PC = vecteur de reset du bus.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        *self = Self::new();
        self.r[15] = bus.exception_base();
    }

    // ------------------------------------------------------------------
    // Accès publics (débogueur, direct boot, tests)
    // ------------------------------------------------------------------

    /// Adresse de la prochaine instruction exécutée.
    pub fn pc(&self) -> u32 {
        self.r[15]
    }

    /// Positionne le PC et l'état ARM/Thumb.
    pub fn jump(&mut self, addr: u32, thumb: bool) {
        if thumb {
            self.cpsr |= FLAG_T;
            self.r[15] = addr & !1;
        } else {
            self.cpsr &= !FLAG_T;
            self.r[15] = addr & !3;
        }
        self.halted = false;
    }

    pub fn reg(&self, i: usize) -> u32 {
        self.r[i]
    }

    /// Écrit un registre du mode courant. Pour R15, utiliser [`Cpu::jump`].
    pub fn set_reg(&mut self, i: usize, val: u32) {
        assert!(i < 15, "utiliser jump() pour R15");
        self.r[i] = val;
    }

    pub fn regs(&self) -> &[u32; 16] {
        &self.r
    }

    pub fn cpsr(&self) -> u32 {
        self.cpsr
    }

    /// Écrit le CPSR complet, en rebanquant les registres si le mode change.
    pub fn set_cpsr(&mut self, val: u32) {
        let new_mode = Mode::from_bits(val);
        self.switch_bank(new_mode);
        self.cpsr = val | 0x10;
    }

    pub fn spsr(&self) -> Option<u32> {
        let m = self.mode();
        m.has_spsr().then(|| self.spsr[m.bank()])
    }

    pub fn mode(&self) -> Mode {
        Mode::from_bits(self.cpsr)
    }

    pub fn thumb(&self) -> bool {
        self.cpsr & FLAG_T != 0
    }

    /// Écrit SP (R13) d'un mode donné, quel que soit le mode courant.
    /// Sert au direct boot pour préparer les piles IRQ/SVC/SYS.
    pub fn set_banked_sp(&mut self, mode: Mode, val: u32) {
        if mode.bank() == self.mode().bank() {
            self.r[13] = val;
        } else {
            self.bank_sp_lr[mode.bank()][0] = val;
        }
    }

    /// Lit SP (R13) d'un mode donné.
    pub fn banked_sp(&self, mode: Mode) -> u32 {
        if mode.bank() == self.mode().bank() {
            self.r[13]
        } else {
            self.bank_sp_lr[mode.bank()][0]
        }
    }

    // ------------------------------------------------------------------
    // Exécution
    // ------------------------------------------------------------------

    /// Exécute une instruction (ou prend une IRQ). Renvoie un nombre de
    /// cycles approximatif ; le timing précis viendra avec le scheduler.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if self.halted {
            return 1;
        }
        if self.irq_line && self.cpsr & FLAG_I == 0 {
            // LR_irq = adresse de l'instruction interrompue + 4, en ARM comme
            // en Thumb : le gestionnaire revient avec `SUBS PC, LR, #4`.
            let ret = self.r[15].wrapping_add(4);
            self.enter_exception(bus, Exception::Irq, ret);
            return 3;
        }

        let pc = self.r[15];
        self.cur_pc = pc;
        self.branched = false;
        self.instructions += 1;

        if self.thumb() {
            let op = bus.fetch16(pc);
            self.r[15] = pc.wrapping_add(4);
            let c = self.exec_thumb(bus, op);
            if !self.branched {
                self.r[15] = pc.wrapping_add(2);
            }
            c
        } else {
            let op = bus.fetch32(pc);
            self.r[15] = pc.wrapping_add(8);
            let c = self.exec_arm(bus, op);
            if !self.branched {
                self.r[15] = pc.wrapping_add(4);
            }
            c
        }
    }

    /// Exécute jusqu'à `max` instructions, ou jusqu'à ce que `stop` renvoie
    /// `true` (testé avant chaque instruction). Renvoie le nombre exécuté.
    pub fn run_until<B: Bus>(&mut self, bus: &mut B, max: u64, mut stop: impl FnMut(&Self) -> bool) -> u64 {
        let start = self.instructions;
        while self.instructions - start < max && !stop(self) {
            self.step(bus);
        }
        self.instructions - start
    }

    // ------------------------------------------------------------------
    // Interne
    // ------------------------------------------------------------------

    #[inline]
    fn flag(&self, f: u32) -> bool {
        self.cpsr & f != 0
    }

    #[inline]
    fn set_nz(&mut self, res: u32) {
        self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z)) | (res & FLAG_N) | if res == 0 { FLAG_Z } else { 0 };
    }

    #[inline]
    fn set_nzc(&mut self, res: u32, c: bool) {
        self.set_nz(res);
        self.cpsr = (self.cpsr & !FLAG_C) | if c { FLAG_C } else { 0 };
    }

    #[inline]
    fn set_nzcv(&mut self, res: u32, c: bool, v: bool) {
        self.set_nzc(res, c);
        self.cpsr = (self.cpsr & !FLAG_V) | if v { FLAG_V } else { 0 };
    }

    /// Écrit R15 sans changer d'état (ARM : aligné sur 4, Thumb : sur 2).
    #[inline]
    fn write_pc(&mut self, val: u32) {
        self.r[15] = if self.thumb() { val & !1 } else { val & !3 };
        self.branched = true;
    }

    /// Branchement avec changement d'état selon le bit 0 (BX).
    #[inline]
    fn branch_exchange(&mut self, val: u32) {
        if val & 1 != 0 {
            self.cpsr |= FLAG_T;
            self.r[15] = val & !1;
        } else {
            self.cpsr &= !FLAG_T;
            self.r[15] = val & !3;
        }
        self.branched = true;
    }

    /// Chargement dans R15 depuis la mémoire (LDR, LDM, POP) : change
    /// d'état sur ARMv5, pas sur ARMv4.
    #[inline]
    fn load_pc(&mut self, val: u32) {
        if A::V5 {
            self.branch_exchange(val);
        } else {
            self.write_pc(val);
        }
    }

    /// Restaure le CPSR depuis le SPSR (retour d'exception).
    fn restore_cpsr_from_spsr(&mut self) {
        let m = self.mode();
        if m.has_spsr() {
            let s = self.spsr[m.bank()];
            self.set_cpsr(s);
        }
    }

    /// Sauvegarde les registres banqués du mode courant et charge ceux de
    /// `new`. Ne touche pas au CPSR.
    fn switch_bank(&mut self, new: Mode) {
        let old = self.mode();
        let (ob, nb) = (old.bank(), new.bank());
        if ob == nb {
            return;
        }
        self.bank_sp_lr[ob] = [self.r[13], self.r[14]];
        [self.r[13], self.r[14]] = self.bank_sp_lr[nb];

        let old_fiq = old == Mode::Fiq;
        let new_fiq = new == Mode::Fiq;
        if old_fiq != new_fiq {
            let (save, load) = if old_fiq {
                (&mut self.bank_fiq_r8_12, &self.bank_usr_r8_12)
            } else {
                (&mut self.bank_usr_r8_12, &self.bank_fiq_r8_12)
            };
            save.copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(load);
        }
    }

    /// Registre de la banque User (LDM/STM avec ^).
    fn user_reg(&self, i: usize) -> u32 {
        match (i, self.mode()) {
            (_, Mode::User | Mode::System) => self.r[i],
            (8..=12, Mode::Fiq) => self.bank_usr_r8_12[i - 8],
            (13 | 14, _) => self.bank_sp_lr[0][i - 13],
            _ => self.r[i],
        }
    }

    fn set_user_reg(&mut self, i: usize, val: u32) {
        match (i, self.mode()) {
            (_, Mode::User | Mode::System) => self.r[i] = val,
            (8..=12, Mode::Fiq) => self.bank_usr_r8_12[i - 8] = val,
            (13 | 14, _) => self.bank_sp_lr[0][i - 13] = val,
            _ => self.r[i] = val,
        }
    }

    pub(crate) fn enter_exception<B: Bus>(&mut self, bus: &mut B, ex: Exception, lr: u32) {
        let old_cpsr = self.cpsr;
        let mode = ex.mode();
        self.switch_bank(mode);
        self.cpsr = (old_cpsr & !(0x1F | FLAG_T)) | mode as u32 | FLAG_I;
        if matches!(ex, Exception::Reset | Exception::Fiq) {
            self.cpsr |= FLAG_F;
        }
        self.spsr[mode.bank()] = old_cpsr;
        self.r[14] = lr;
        self.r[15] = bus.exception_base().wrapping_add(ex.vector());
        self.branched = true;
    }

    /// Instruction indéfinie : LR = instruction suivante.
    fn undefined<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let size = if self.thumb() { 2 } else { 4 };
        let lr = self.cur_pc.wrapping_add(size);
        self.enter_exception(bus, Exception::Undefined, lr);
        3
    }

    /// SWI : tente d'abord le HLE, sinon exception.
    fn software_interrupt<B: Bus>(&mut self, bus: &mut B, num: u8) -> u32 {
        let mut regs = [self.r[0], self.r[1], self.r[2], self.r[3]];
        if bus.hle_swi(num, &mut regs) {
            self.r[..4].copy_from_slice(&regs);
            return 3;
        }
        let size = if self.thumb() { 2 } else { 4 };
        let lr = self.cur_pc.wrapping_add(size);
        self.enter_exception(bus, Exception::Swi, lr);
        3
    }

    // --- Accès mémoire avec la sémantique des lectures non alignées ---

    /// LDR : lecture alignée puis rotation (ARMv4 comme ARMv5).
    #[inline]
    fn load32_rotated<B: Bus>(bus: &mut B, addr: u32) -> u32 {
        bus.read32(addr & !3).rotate_right((addr & 3) * 8)
    }

    /// LDRH : ARMv4 fait une rotation de 8 bits sur une adresse impaire,
    /// ARMv5 lit simplement le demi-mot aligné.
    #[inline]
    fn load16_unsigned<B: Bus>(bus: &mut B, addr: u32) -> u32 {
        let v = bus.read16(addr & !1) as u32;
        if !A::V5 && addr & 1 != 0 {
            v.rotate_right(8)
        } else {
            v
        }
    }

    /// LDRSH : sur ARMv4, une adresse impaire se comporte comme LDRSB.
    #[inline]
    fn load16_signed<B: Bus>(bus: &mut B, addr: u32) -> u32 {
        if !A::V5 && addr & 1 != 0 {
            bus.read8(addr) as i8 as i32 as u32
        } else {
            bus.read16(addr & !1) as i16 as i32 as u32
        }
    }

    /// Calcule le résultat d'un transfert multiple (LDM/STM, PUSH/POP…)
    /// commun à ARM et Thumb. `rn` = registre de base, `list` = liste
    /// (16 bits), `up`/`pre` = sens et pré-incrément, `writeback`.
    /// `user` : variante `^` sans PC (banque User).
    #[allow(clippy::too_many_arguments)]
    fn block_transfer<B: Bus>(
        &mut self, bus: &mut B, rn: usize, mut list: u32, load: bool,
        up: bool, pre: bool, writeback: bool, psr: bool,
    ) -> u32 {
        let base = self.r[rn];
        let empty = list == 0;
        if empty && !A::V5 {
            // ARMv4 : une liste vide transfère R15.
            list = 1 << 15;
        }
        let bytes = if empty { 0x40 } else { list.count_ones() * 4 };
        let new_base = if up { base.wrapping_add(bytes) } else { base.wrapping_sub(bytes) };
        let mut addr = match (up, pre) {
            (true, false) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => base.wrapping_sub(bytes).wrapping_add(4),
            (false, true) => base.wrapping_sub(bytes),
        };
        addr &= !3;

        let has_pc = list & (1 << 15) != 0;
        let user_bank = psr && !(load && has_pc);
        let rn_in_list = list & (1 << rn) != 0;
        let first = list.trailing_zeros() as usize;
        let last = 31 - list.leading_zeros() as usize;

        if load {
            let mut pc_val = 0;
            for i in 0..16 {
                if list & (1 << i) == 0 {
                    continue;
                }
                let v = bus.read32(addr);
                addr = addr.wrapping_add(4);
                if i == 15 {
                    pc_val = v; // traité après la writeback
                    continue;
                }
                if user_bank {
                    self.set_user_reg(i, v);
                } else {
                    self.r[i] = v;
                }
            }
            // Writeback : ARMv4 jamais si Rn est dans la liste ;
            // ARMv5 si Rn est seul ou n'est pas le dernier.
            let wb_allowed = !rn_in_list || (A::V5 && (list.count_ones() == 1 || rn != last));
            if writeback && wb_allowed {
                self.r[rn] = new_base;
            }
            if has_pc {
                let v = pc_val;
                if psr {
                    self.restore_cpsr_from_spsr();
                    self.write_pc(v);
                } else {
                    self.load_pc(v);
                }
            }
            if empty { 3 } else { 2 + list.count_ones() }
        } else {
            for i in 0..16 {
                if list & (1 << i) == 0 {
                    continue;
                }
                let v = if i == 15 {
                    self.r[15].wrapping_add(4) // PC + 12
                } else if i == rn && !A::V5 && i != first && writeback {
                    new_base // ARMv4 : nouvelle base si Rn n'est pas le premier
                } else if user_bank {
                    self.user_reg(i)
                } else {
                    self.r[i]
                };
                bus.write32(addr, v);
                addr = addr.wrapping_add(4);
            }
            if writeback {
                self.r[rn] = new_base;
            }
            1 + list.count_ones()
        }
    }
}
