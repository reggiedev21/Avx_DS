//! Direct boot d'une vraie ROM `.nds` compilée pour ce projet
//! (`tests/roms/boot_test/`, reconstruite par `tools/build_test_roms.sh`).
//!
//! Elle exerce : l'IRQ VBlank relayée par le mini-BIOS vers un gestionnaire
//! en C, VBlankIntrWait, les SWI Div et CpuFastSet, le diviseur matériel,
//! l'IPCFIFO et l'IPCSYNC avec l'ARM7, un timer, et le Halt des deux CPU.

use avxds_core::nds::{CpuId, FRAME_CYCLES};
use avxds_core::{Bus, Nds};

const ROM: &[u8] = include_bytes!("roms/boot_test.nds");
const RESULTS: u32 = 0x0230_0000;
const DONE: u32 = 0x600D_B007;

fn boot() -> Nds {
    let mut nds = Nds::new();
    let header = nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    assert_eq!(header.title, "AVXDS TEST");
    assert_eq!(header.game_code_str(), "AVXT");
    nds
}

fn result(nds: &mut Nds, n: u32) -> u32 {
    nds.bus9().read32(RESULTS + 4 * n)
}

#[test]
fn boot_test_rom_runs_to_completion() {
    let mut nds = boot();
    let mut frames = 0;
    while result(&mut nds, 15) != DONE {
        nds.run_frame();
        frames += 1;
        assert!(frames < 20, "ne termine pas\nARM9 {:?}\nARM7 {:?}", nds.arm9, nds.arm7);
    }

    assert_eq!(result(&mut nds, 0), 0x900D_0001, "main9 atteint");
    assert_eq!(result(&mut nds, 1), 3, "3 VBlank comptés par le gestionnaire d'IRQ");
    assert_eq!(result(&mut nds, 2), 192, "réveil au début du VBlank");
    assert_eq!(result(&mut nds, 3) as i32, -142, "SWI Div : quotient");
    assert_eq!(result(&mut nds, 4) as i32, -6, "SWI Div : reste");
    assert_eq!(result(&mut nds, 5), 142, "SWI Div : |quotient| dans R3");
    assert_eq!(result(&mut nds, 6) as i32, -333_333, "diviseur matériel");
    for k in 0..4 {
        assert_eq!(result(&mut nds, 7 + k), (k + 1) * 100 * 2 + 1, "réponse FIFO de l'ARM7 n°{k}");
    }
    assert_eq!(result(&mut nds, 11), 0xA, "IPCSYNC");
    let expected_sum = (0..16u32).fold(0u32, |s, i| s.wrapping_mul(31).wrapping_add(i * 0x0101_0101));
    assert_eq!(result(&mut nds, 12), expected_sum, "SWI CpuFastSet");
    let ticks = result(&mut nds, 13);
    let expected = FRAME_CYCLES / (64 * 2);
    assert!(ticks.abs_diff(expected as u32) <= 1, "timer F/64 sur une image : {ticks} (≈ {expected})");

    assert!(nds.hw.unhandled_swi.is_empty(), "SWI non gérés : {:?}", nds.hw.unhandled_swi);
    assert!(nds.hw.unhandled_io.is_empty(), "I/O non gérées : {:x?}", nds.hw.unhandled_io);
    assert!(frames <= 6, "{frames} images");
}

#[test]
fn both_cpus_sleep_at_the_end() {
    let mut nds = boot();
    for _ in 0..8 {
        nds.run_frame();
    }
    assert!(nds.arm7.halted, "ARM7 : Halt définitif (IE = 0)");
    let before = (nds.arm9.instructions, nds.arm7.instructions);
    for _ in 0..10 {
        nds.run_frame();
    }
    let arm7_delta = nds.arm7.instructions - before.1;
    let arm9_delta = nds.arm9.instructions - before.0;
    assert_eq!(arm7_delta, 0, "l'ARM7 ne tourne plus");
    // L'ARM9 se réveille à chaque VBlank, exécute le gestionnaire puis se
    // rendort : quelques dizaines d'instructions par image, pas des millions.
    assert!(arm9_delta < 10 * 200, "ARM9 : {arm9_delta} instructions en 10 images");
    let _ = CpuId::Arm9;
}

#[test]
fn bad_roms_are_rejected() {
    let mut nds = Nds::new();
    let err = nds.direct_boot(vec![0; 0x100]).unwrap_err();
    assert!(err.to_string().contains("trop petite"), "{err}");

    let mut rom = ROM.to_vec();
    rom[0x2C..0x30].copy_from_slice(&0x0100_0000u32.to_le_bytes()); // ARM9 de 16 Mo
    let err = nds.direct_boot(rom).unwrap_err();
    assert!(err.to_string().contains("hors de la ROM"), "{err}");

    let mut rom = ROM.to_vec();
    rom[0x28..0x2C].copy_from_slice(&0x0600_0000u32.to_le_bytes()); // ARM9 en VRAM
    let err = nds.direct_boot(rom).unwrap_err();
    assert!(err.to_string().contains("ARM9"), "{err}");
}
