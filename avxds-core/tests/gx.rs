//! ROM `tests/roms/gx_test` : du code ARM envoie chaque commande de
//! géométrie par les ports, par GXFIFO (non compact puis compact) et par
//! DMA en mode « FIFO géométrie », et relève GXSTAT autour d'un
//! SwapBuffers (étape 3D-1).

use avxds_core::nds::gpu3d::commands;
use avxds_core::{Bus, Nds};

const ROM: &[u8] = include_bytes!("roms/gx_test.nds");
const RESULTS: u32 = 0x0230_0000;

fn run() -> Nds {
    let mut nds = Nds::new();
    nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    for _ in 0..20 {
        if nds.bus9().read32(RESULTS + 60) == 0x600D_B007 {
            assert!(nds.hw.unhandled_io.is_empty(), "I/O non gérées : {:x?}", nds.hw.unhandled_io);
            return nds;
        }
        nds.run_frame();
    }
    panic!("ne termine pas\nARM9 {:?}", nds.arm9);
}

fn r(nds: &mut Nds, n: u32) -> u32 {
    nds.bus9().read32(RESULTS + 4 * n)
}

#[test]
fn every_command_is_executed_once_per_path() {
    let nds = run();
    let t = nds.hw.gpu3d.totals();
    for cmd in 0x10..0x80u8 {
        let expected = match cmd {
            0x50 => 2,         // SwapBuffers seul, puis en tête de la DMA
            0x20 => 3 + 200,   // COLOR : 3 chemins + 200 par DMA
            c if commands::info(c).is_some() => 3,
            _ => 0,
        };
        assert_eq!(t.commands[cmd as usize], expected, "{} ({cmd:#04x})", commands::name(cmd));
    }
    assert_eq!(t.broken, 0, "aucune commande interrompue");
    assert_eq!(t.swaps, 2);
    assert_eq!(nds.hw.gpu3d.unpack.invalid, 0);
}

#[test]
fn gxstat_seen_from_arm_code() {
    let mut nds = run();
    let idle = r(&mut nds, 0);
    assert_eq!(idle & (0x1FF << 16 | 1 << 27), 0, "file vide, moteur libre : {idle:#010x}");
    assert_ne!(idle & (1 << 25 | 1 << 26), 0);
    assert_ne!(r(&mut nds, 1) & (1 << 27), 0, "occupé après SwapBuffers");
    assert!(r(&mut nds, 2) < 192, "SwapBuffers envoyé avant le VBlank");
    assert_eq!(r(&mut nds, 3), 192, "le moteur repart au VBlank (ligne 192)");
    let during = r(&mut nds, 4);
    assert!((during >> 16) & 0x1FF >= 128, "DMA arrêtée au-delà de la moitié : {during:#010x}");
    assert_eq!(during & (1 << 25), 0);
    assert_eq!(r(&mut nds, 5), 1, "DMA encore active");
    let after = r(&mut nds, 6);
    assert_eq!(after & (0x1FF << 16 | 1 << 27), 0, "tout exécuté : {after:#010x}");
}
