//! ROM `tests/roms/sound_test` : du code ARM7 programme un triangle PCM16
//! en boucle (canal 0, à gauche) et un rectangle PSG 50 % (canal 8, à
//! droite), puis dort. On vérifie la sortie stéréo de bout en bout.

use avxds_core::Nds;

const ROM: &[u8] = include_bytes!("roms/sound_test.nds");

/// Quelques images, puis les 512 derniers échantillons (gauche, droite).
fn run() -> (Vec<i16>, Vec<i16>) {
    let mut nds = Nds::new();
    nds.direct_boot(ROM.to_vec()).expect("ROM valide");
    for _ in 0..10 {
        nds.run_frame();
    }
    assert!(nds.hw.unhandled_io.is_empty(), "I/O non gérées : {:x?}", nds.hw.unhandled_io);
    let out = &nds.hw.sound.out;
    let tail = &out[out.len() - 1024..];
    (tail.iter().step_by(2).copied().collect(), tail.iter().skip(1).step_by(2).copied().collect())
}

/// Plus petite période (en échantillons) qui répète le signal.
fn period(v: &[i16]) -> usize {
    (1..=64).find(|&p| v.iter().zip(&v[p..]).all(|(a, b)| a == b)).expect("périodique")
}

#[test]
fn channels_play_their_waves_on_their_side() {
    let (l, r) = run();
    assert_eq!(period(&l), 32, "triangle de 32 pas");
    assert_eq!(period(&r), 32, "rectangle de 8 × 4 pas");
    // Triangle −4000h..+4000h, volumes 127 (= 128), tout à gauche : v en
    // 16.8 après le panoramique, × 128 >> 21 → v / 64, × 64 en sortie.
    assert_eq!((*l.iter().min().unwrap(), *l.iter().max().unwrap()), (-0x4000, 0x4000));
    // Rectangle ±7FFFh tout à droite : 511 ou −512 (arrondi par défaut), × 64.
    let mut levels: Vec<i16> = r.clone();
    levels.sort();
    levels.dedup();
    assert_eq!(levels, vec![-0x8000, 511 * 64]);
    assert_eq!(r.iter().filter(|&&x| x > 0).count(), r.len() / 2, "50 %");
}
