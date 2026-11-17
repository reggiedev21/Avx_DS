//! Interface entre un CPU et le reste de la machine.

/// Désignation d'un registre CP15 (`MRC/MCR p15, opc1, Rd, CRn, CRm, opc2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cp15Op {
    pub opc1: u8,
    pub crn: u8,
    pub crm: u8,
    pub opc2: u8,
}

/// Ce que voit un CPU : la mémoire, et quelques points d'accroche système.
///
/// **Contrat d'alignement :** le CPU passe toujours des adresses alignées à
/// `read16`/`write16` (multiple de 2) et à `read32`/`write32` (multiple de 4).
/// Les rotations des lectures non alignées sont faites côté CPU, selon les
/// règles de l'architecture. Le bus n'a donc jamais à s'en soucier.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, val: u8);
    fn write16(&mut self, addr: u32, val: u16);
    fn write32(&mut self, addr: u32, val: u32);

    /// Lecture d'une instruction Thumb. Par défaut, une lecture normale ;
    /// à surcharger pour un chemin rapide (ITCM, cache d'instructions…).
    fn fetch16(&mut self, addr: u32) -> u16 {
        self.read16(addr)
    }

    /// Lecture d'une instruction ARM.
    fn fetch32(&mut self, addr: u32) -> u32 {
        self.read32(addr)
    }

    /// Adresse de la table des vecteurs d'exception.
    /// ARM7 : toujours 0. ARM9 : 0 ou `0xFFFF0000` selon le bit 13 du
    /// registre de contrôle CP15.
    fn exception_base(&mut self) -> u32 {
        0
    }

    /// `MRC p15` (ARMv5TE uniquement ; sur ARMv4T le CPU lève une exception
    /// d'instruction indéfinie sans appeler le bus).
    fn cp15_read(&mut self, _op: Cp15Op) -> u32 {
        0
    }

    /// `MCR p15`. Les écritures « wait for interrupt »
    /// (`c7,c0,4` et `c7,c8,2`) mettent aussi le CPU en pause.
    fn cp15_write(&mut self, _op: Cp15Op, _val: u32) {}

    /// Point d'accroche pour émuler les appels BIOS en haut niveau (HLE).
    ///
    /// `num` est le numéro de SWI (octet de commentaire), `regs` contient
    /// R0 à R3. Renvoyer `true` signifie « traité » : le CPU continue à
    /// l'instruction suivante sans prendre l'exception. Renvoyer `false`
    /// (défaut) laisse le CPU sauter au vecteur SWI du BIOS.
    fn hle_swi(&mut self, _num: u8, _regs: &mut [u32; 4]) -> bool {
        false
    }
}
