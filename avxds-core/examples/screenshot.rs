//! Démarre une ROM, la fait tourner N images et enregistre les deux écrans
//! (haut au-dessus, bas en dessous) dans un PNG de 256×384.
//!
//! `cargo run --release --example screenshot -- jeu.nds [images] [sortie.png] [touches]`
//!
//! `touches` simule des appuis, par exemple `A@60,START@120` : la touche est
//! appuyée pendant 5 images à partir de l'image indiquée. Noms : A, B,
//! SELECT, START, RIGHT, LEFT, UP, DOWN, R, L.

use avxds_core::nds::gpu::{HEIGHT, WIDTH};
use avxds_core::{png, Nds};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage : screenshot <rom.nds> [images] [sortie.png]");
        std::process::exit(2);
    };
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let out = args.next().unwrap_or_else(|| "capture.png".into());
    let presses: Vec<(u16, u32)> = args
        .next()
        .map(|spec| {
            spec.split(',')
                .filter_map(|p| {
                    let (key, at) = p.split_once('@')?;
                    let names = ["A", "B", "SELECT", "START", "RIGHT", "LEFT", "UP", "DOWN", "R", "L"];
                    let bit = names.iter().position(|n| n.eq_ignore_ascii_case(key))?;
                    Some((1u16 << bit, at.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut nds = Nds::new();
    if let Err(e) = nds.direct_boot(std::fs::read(&path).unwrap_or_else(|e| panic!("{path} : {e}"))) {
        eprintln!("{path} : {e}");
        std::process::exit(1);
    }
    for f in 0..frames {
        let mut keys = 0x03FF;
        for &(bit, at) in &presses {
            if (at..at + 5).contains(&f) {
                keys &= !bit;
            }
        }
        nds.hw.keyinput = keys;
        nds.run_frame();
    }
    // run_frame s'arrête au début du VBlank : les 192 lignes sont dessinées.
    let mut pixels = Vec::with_capacity(WIDTH * HEIGHT * 2);
    pixels.extend_from_slice(nds.hw.gpu.screen(0));
    pixels.extend_from_slice(nds.hw.gpu.screen(1));
    std::fs::write(&out, png::encode_rgb(WIDTH, HEIGHT * 2, &pixels)).expect("écriture du PNG");
    println!("{out} : {frames} images");
    if !nds.hw.gpu.unsupported.is_empty() || !nds.hw.unsupported.is_empty() {
        println!("pas encore rendu : {:?} {:?}", nds.hw.gpu.unsupported, nds.hw.unsupported);
    }
}
