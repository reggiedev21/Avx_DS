//! Cœur de l'émulateur Nintendo DS AvxDS.
//!
//! Pour l'instant : les deux processeurs (ARM946E-S et ARM7TDMI) via un
//! interpréteur unique, générique sur la version d'architecture.
//!
//! ```
//! use avxds_core::cpu::{Arm9, Arm7};
//! let arm9 = Arm9::new();
//! let arm7 = Arm7::new();
//! assert_eq!(arm9.pc(), 0);
//! # let _ = arm7;
//! ```

#![forbid(unsafe_code)]

pub mod bus;
pub mod cpu;

pub use bus::{Bus, Cp15Op};
pub use cpu::{Arch, Arm7, Arm9, Armv4t, Armv5te, Cpu, Mode};
