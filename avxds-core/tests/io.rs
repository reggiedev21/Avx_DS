//! ROM `tests/roms/io_test` : la cartouche (scrutation, DMA, identifiant)
//! côté ARM9 ; l'écran tactile, le firmware, l'alimentation et l'horloge
//! côté ARM7, pilotés par du vrai code ARM comme le ferait libnds.

use avxds_core::{Bus, Nds};

const ROM: &[u8] = include_bytes!("roms/io_test.nds");
const R9: u32 = 0x0230_0000;
const R7: u32 = 0x0230_0040;

fn r(nds: &mut Nds, base: u32, n: u32) -> u32 {
    nds.bus9().read32(base + 4 * n)
}

fn run(touch: Option<(u8, u8)>) -> Nds {
    let mut nds = Nds::new();
    nds.hw.rtc.fixed_time = Some(1_794_939_900); // 2026-11-17 18:25:00
    nds.set_touch(touch);
    let header = nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    assert_eq!(header.game_code_str(), "AVXI");
    for _ in 0..10 {
        if r(&mut nds, R9, 15) == 0x600D_B007 {
            assert!(nds.hw.unhandled_io.is_empty(), "I/O non gérées : {:x?}", nds.hw.unhandled_io);
            assert!(nds.hw.unhandled_swi.is_empty());
            return nds;
        }
        nds.run_frame();
    }
    panic!("ne termine pas\nARM9 {:?}\nARM7 {:?}", nds.arm9, nds.arm7);
}

fn bytes(nds: &mut Nds, addr: u32, len: u32) -> Vec<u8> {
    (0..len).map(|i| nds.bus9().read8(addr + i)).collect()
}

#[test]
fn cartridge_from_arm_code() {
    let mut nds = run(None);
    assert_eq!(r(&mut nds, R9, 0), 0x900D_0009);
    assert_eq!(r(&mut nds, R9, 1), 0x80, "0x200 octets = 0x80 mots");
    assert_eq!(r(&mut nds, R9, 2), 0, "transfert terminé");
    assert_eq!(bytes(&mut nds, 0x0231_0000, 0x200), &ROM[0x8000..0x8200], "lecture par scrutation");
    assert_eq!(bytes(&mut nds, 0x0231_0200, 0x200), &ROM[0x9000..0x9200], "lecture par DMA");
    let chip_id = r(&mut nds, R9, 4);
    assert_eq!(chip_id & 0xFF, 0xC2, "fabricant");
    assert_eq!(chip_id, nds.bus9().read32(0x027F_F800), "même identifiant qu'au direct boot");
}

#[test]
fn spi_and_rtc_from_arm_code() {
    let mut nds = run(Some((100, 50)));
    assert_eq!(r(&mut nds, R7, 0), 0x900D_0007);
    assert_eq!(r(&mut nds, R7, 1) & (1 << 6), 0, "EXTKEYIN : stylet posé");
    // Calibrage lu comme un jeu : dans les réglages copiés en RAM.
    let mut u16_at = |a: u32| nds.bus9().read16(0x027F_FC80 + a) as i32;
    let (ax1, ay1, ax2, ay2) = (u16_at(0x58), u16_at(0x5A), u16_at(0x5E), u16_at(0x60));
    let s = |nds: &mut Nds, a: u32| nds.bus9().read8(0x027F_FC80 + a) as i32;
    let (sx1, sy1, sx2, sy2) = (s(&mut nds, 0x5C), s(&mut nds, 0x5D), s(&mut nds, 0x62), s(&mut nds, 0x63));
    let x = (r(&mut nds, R7, 2) as i32 - ax1) * (sx2 - sx1) / (ax2 - ax1) + sx1 - 1;
    let y = (r(&mut nds, R7, 3) as i32 - ay1) * (sy2 - sy1) / (ay2 - ay1) + sy1 - 1;
    assert!((x - 100).abs() <= 1 && (y - 50).abs() <= 1, "stylet ({x}, {y})");

    assert_eq!(r(&mut nds, R7, 4), 0x12_4020, "JEDEC 20 40 12");
    assert_eq!(r(&mut nds, R7, 5), 0x3FE00, "réglages utilisateur");
    let nick = bytes(&mut nds, R7 + 24, 10);
    let nick: Vec<u16> = nick.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&nick).unwrap(), "AvxDS");
    assert_eq!(r(&mut nds, R7, 9), 0x0F, "alimentation : registre 0");
    assert_eq!(bytes(&mut nds, R7 + 40, 7), [0x26, 0x11, 0x17, 2, 0x58, 0x25, 0x00], "date et heure");
}

#[test]
fn pen_up_from_arm_code() {
    let mut nds = run(None);
    assert_ne!(r(&mut nds, R7, 1) & (1 << 6), 0);
    assert_eq!((r(&mut nds, R7, 2), r(&mut nds, R7, 3)), (0, 0xFFF));
}
