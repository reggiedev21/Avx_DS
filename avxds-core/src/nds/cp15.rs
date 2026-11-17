//! Coprocesseur système CP15 de l'ARM946E-S : contrôle, ITCM, DTCM.
//!
//! La protection mémoire (MPU) et les caches sont mémorisés mais sans effet :
//! la DS n'en a pas besoin pour fonctionner, seulement pour le timing.

use crate::bus::Cp15Op;

const CTRL_HIGH_VECTORS: u32 = 1 << 13;
const CTRL_DTCM_ENABLE: u32 = 1 << 16;
const CTRL_DTCM_LOAD: u32 = 1 << 17;
const CTRL_ITCM_ENABLE: u32 = 1 << 18;
const CTRL_ITCM_LOAD: u32 = 1 << 19;
/// Bits du registre de contrôle modifiables par logiciel.
const CTRL_WRITABLE: u32 = 0x000F_F085;
/// Bits 3–6 toujours à 1.
const CTRL_FIXED: u32 = 0x0000_0078;

#[derive(Clone, Debug)]
pub struct Cp15 {
    pub control: u32,
    /// c9,c1,0 : base (bits 31–12) et taille virtuelle (bits 5–1) de la DTCM.
    pub dtcm_region: u32,
    /// c9,c1,1 : taille virtuelle de l'ITCM (la base est toujours 0).
    pub itcm_region: u32,
    /// Registres sans effet ici, gardés pour être relus à l'identique.
    pub cache_bits: [u32; 2],
    pub write_buffer: u32,
    pub access_perms: [u32; 4],
    pub protection_regions: [u32; 8],
    pub cache_lockdown: [u32; 2],
    pub trace_process_id: u32,

    // Valeurs dérivées, recalculées à chaque écriture.
    pub(crate) dtcm_base: u32,
    pub(crate) dtcm_size: u64,
    pub(crate) itcm_size: u64,
}

impl Default for Cp15 {
    fn default() -> Self {
        let mut c = Cp15 {
            // Valeur au reset : vecteurs hauts (0xFFFF0000), TCM désactivées.
            control: CTRL_FIXED | CTRL_HIGH_VECTORS,
            dtcm_region: 0,
            itcm_region: 0,
            cache_bits: [0; 2],
            write_buffer: 0,
            access_perms: [0; 4],
            protection_regions: [0; 8],
            cache_lockdown: [0; 2],
            trace_process_id: 0,
            dtcm_base: 0,
            dtcm_size: 0,
            itcm_size: 0,
        };
        c.update();
        c
    }
}

/// Taille codée sur 5 bits : 512 << n, au minimum 4 Ko (en u64 : peut
/// atteindre 1 To, c'est-à-dire « tout l'espace d'adressage »).
fn region_size(reg: u32) -> u64 {
    let n = (reg >> 1) & 0x1F;
    (512u64 << n).max(0x1000)
}

impl Cp15 {
    fn update(&mut self) {
        self.dtcm_base = self.dtcm_region & 0xFFFF_F000;
        self.dtcm_size = region_size(self.dtcm_region);
        self.itcm_size = region_size(self.itcm_region);
    }

    pub fn high_vectors(&self) -> bool {
        self.control & CTRL_HIGH_VECTORS != 0
    }

    /// L'ITCM répond-elle à cette adresse ? En mode « load », seulement aux
    /// écritures (les lectures vont à la mémoire normale).
    #[inline]
    pub(crate) fn itcm_hit(&self, addr: u32, write: bool) -> bool {
        self.control & CTRL_ITCM_ENABLE != 0
            && (addr as u64) < self.itcm_size
            && (write || self.control & CTRL_ITCM_LOAD == 0)
    }

    #[inline]
    pub(crate) fn dtcm_hit(&self, addr: u32, write: bool) -> bool {
        self.control & CTRL_DTCM_ENABLE != 0
            && (addr.wrapping_sub(self.dtcm_base) as u64) < self.dtcm_size
            && (write || self.control & CTRL_DTCM_LOAD == 0)
    }

    pub fn read(&self, op: Cp15Op) -> u32 {
        match (op.opc1, op.crn, op.crm, op.opc2) {
            (0, 0, 0, 0) => 0x4105_9461, // ID principal : ARM946E-S
            (0, 0, 0, 1) => 0x0F0D_2112, // type de cache
            (0, 0, 0, 2) => 0x0014_0180, // tailles des TCM
            (0, 1, 0, 0) => self.control,
            (0, 2, 0, i @ 0..=1) => self.cache_bits[i as usize],
            (0, 3, 0, 0) => self.write_buffer,
            (0, 5, 0, i @ 0..=3) => self.access_perms[i as usize],
            (0, 6, r, 0 | 1) => self.protection_regions[r as usize & 7],
            (0, 9, 0, i @ 0..=1) => self.cache_lockdown[i as usize],
            (0, 9, 1, 0) => self.dtcm_region,
            (0, 9, 1, 1) => self.itcm_region,
            (0, 13, _, _) => self.trace_process_id,
            _ => 0,
        }
    }

    pub fn write(&mut self, op: Cp15Op, val: u32) {
        match (op.opc1, op.crn, op.crm, op.opc2) {
            (0, 1, 0, 0) => self.control = (val & CTRL_WRITABLE) | CTRL_FIXED,
            (0, 2, 0, i @ 0..=1) => self.cache_bits[i as usize] = val,
            (0, 3, 0, 0) => self.write_buffer = val,
            (0, 5, 0, i @ 0..=3) => self.access_perms[i as usize] = val,
            (0, 6, r, 0 | 1) => self.protection_regions[r as usize & 7] = val,
            (0, 9, 0, i @ 0..=1) => self.cache_lockdown[i as usize] = val,
            (0, 9, 1, 0) => self.dtcm_region = val,
            (0, 9, 1, 1) => self.itcm_region = val & 0x3E, // base fixée à 0
            (0, 13, _, _) => self.trace_process_id = val,
            _ => {} // c7 (caches, wait for interrupt) : rien à mémoriser
        }
        self.update();
    }
}
