//! ROM `tests/roms/save_test` : du code ARM pilote la puce de sauvegarde
//! comme libnds (EEPROM 8 Ko, EEPROM 0,5 Ko, FLASH). Premier démarrage :
//! écriture puis relecture, sur puce vierge, type détecté. Second démarrage
//! depuis le `.sav` exporté : les données sont retrouvées.

use avxds_core::nds::spimem::Family;
use avxds_core::{Bus, Nds};

const ROM: &[u8] = include_bytes!("roms/save_test.nds");
const RESULTS: u32 = 0x0230_0000;

fn run(mode: u32, save: Option<&[u8]>) -> Nds {
    let mut nds = Nds::new();
    if let Some(s) = save {
        nds.load_save(s);
    }
    nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    nds.bus9().write32(0x0220_0000, mode);
    for _ in 0..10 {
        if nds.bus9().read32(RESULTS + 60) == 0x600D_B007 {
            assert!(nds.hw.unhandled_io.is_empty(), "I/O non gérées : {:x?}", nds.hw.unhandled_io);
            return nds;
        }
        nds.run_frame();
    }
    panic!("ne termine pas (mode {mode:#x})\nARM9 {:?}", nds.arm9);
}

fn r(nds: &mut Nds, n: u32) -> u32 {
    nds.bus9().read32(RESULTS + 4 * n)
}

fn check(mode: u32, family: Family, size: usize, base: usize) {
    let mut nds = run(mode, None);
    assert_eq!(r(&mut nds, 0), 0x5A7E_0009);
    assert_eq!((2..6).map(|n| r(&mut nds, n)).collect::<Vec<_>>(), [u32::MAX; 4], "vierge au 1er démarrage");
    assert_eq!(r(&mut nds, 6), 0, "relecture identique ({family:?})");
    assert_eq!(r(&mut nds, 7) & 3, 0, "ni WIP ni WEL à la fin");
    assert_eq!(nds.hw.slot1.save.locked(), Some((family, size)), "trace : {:#?}", nds.hw.slot1.save.trace);
    let sav = nds.save_data().unwrap();
    assert_eq!(sav.len(), size);
    let expected: Vec<u8> = (0..256).map(|i| (i * 7 + mode as usize) as u8).collect();
    assert_eq!(sav[base..base + 256], expected[..]);

    // Second démarrage depuis le .sav : le type est redéduit, les données relues.
    let mut nds = run(mode | 0x100, Some(&sav));
    assert_eq!(r(&mut nds, 2) & 0xFF, expected[0] as u32, "le jeu retrouve sa sauvegarde");
    assert_eq!(r(&mut nds, 6), 0, "relecture complète ({family:?})");
    assert_eq!(nds.save_data().unwrap(), sav, "rien de modifié");
}

#[test]
fn eeprom_8k_from_arm_code() {
    check(0, Family::Eeprom16, 8 << 10, 0x100);
}

#[test]
fn eeprom_512_from_arm_code() {
    check(1, Family::Eeprom512, 512, 0x100);
}

#[test]
fn flash_from_arm_code() {
    check(2, Family::Flash, 256 << 10, 0x10000);
}
