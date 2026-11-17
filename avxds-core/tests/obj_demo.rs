//! ROM de démonstration des sprites, fenêtres et effets
//! (`tests/roms/obj_test/`, reconstruite par `tools/build_test_roms.sh`).

use avxds_core::nds::gpu::{rgb555_to_rgb888, WIDTH};
use avxds_core::{Bus, Nds};

const ROM: &[u8] = include_bytes!("roms/obj_test.nds");

fn rgb(r: u16, g: u16, b: u16) -> u32 {
    rgb555_to_rgb888(r | g << 5 | b << 10)
}

fn boot() -> Nds {
    let mut nds = Nds::new();
    nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    for _ in 0..3 {
        nds.run_frame();
    }
    assert_eq!(nds.bus9().read32(0x0230_003C), 0x600D_B007, "démo prête");
    nds
}

fn px(nds: &Nds, screen: usize, x: usize, y: usize) -> u32 {
    nds.hw.gpu.screen(screen)[y * WIDTH + x]
}

#[test]
fn ring_sprite_window_and_blending() {
    let nds = boot();
    // Anneau 16×16 en (120, 88) : bord jaune puis rouge en descendant.
    assert_eq!(px(&nds, 0, 127, 89), rgb(31, 24, 0));
    assert_eq!(px(&nds, 0, 127, 90), rgb(31, 6, 2));
    assert_eq!(px(&nds, 0, 127, 95), rgb(6, 10, 20), "centre transparent : damier");
    // Fenêtre 0 : le damier disparaît, la couleur de fond apparaît.
    assert_eq!(px(&nds, 0, 40, 150), rgb(4, 4, 6));
    assert_eq!(px(&nds, 0, 40, 190), rgb(6, 10, 20), "sous la fenêtre (Y2 exclu)");
    // Carré blanc semi-transparent sur le damier : 10/16 × 31 + 6/16 × fond.
    assert_eq!(px(&nds, 0, 30, 30), rgb(21, 23, 26));
    assert!(nds.hw.gpu.unsupported.is_empty(), "{:?}", nds.hw.gpu.unsupported);
}

#[test]
fn dpad_moves_the_ring() {
    let mut nds = boot();
    nds.hw.keyinput = 0x03FF & !(1 << 4); // → maintenu
    for _ in 0..10 {
        nds.run_frame();
    }
    nds.hw.keyinput = 0x03FF;
    nds.run_frame();
    let x = nds.bus9().read32(0x0230_0004);
    assert_eq!(x, 130, "1 pixel par image");
    assert_eq!(px(&nds, 0, 137, 89), rgb(31, 24, 0), "anneau redessiné à la nouvelle place");
}

#[test]
fn bottom_screen_fade_follows_the_program() {
    let mut nds = boot();
    for _ in 0..70 {
        nds.run_frame();
        // run_frame s'arrête au VBlank : l'image vient des registres posés
        // pour l'image n° RESULTS[0].
        let f = nds.bus9().read32(0x0230_0000);
        let phase = f & 63;
        let (darken, evy) = if phase < 32 {
            (true, if phase < 16 { phase } else { 31 - phase })
        } else {
            (false, if phase - 32 < 16 { phase - 32 } else { 63 - phase })
        };
        let evy = evy.min(16) as u16;
        let expected = if darken {
            rgb(31 - 31 * evy / 16, 0, 0)
        } else {
            rgb(31, 31 * evy / 16, 31 * evy / 16)
        };
        assert_eq!(px(&nds, 1, 0, 100), expected, "image {f} (phase {phase})");
    }
}
