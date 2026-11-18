//! AvxDS : fenêtre de l'émulateur.
//!
//! `avxds [--firmware firmware.bin] [--save-type type] jeu.nds`
//!
//! Touches : flèches = croix, X = A, Z = B, S = X, A = Y, Q = L, W = R,
//! Entrée = Start, Retour arrière = Select, Tab (maintenu) = accéléré,
//! F12 = capture PNG, F11 = diagnostic 3D (écrans, calque 3D, rapport),
//! M = couper ou rétablir le son, Échap = quitter. Clic gauche sur l'écran
//! du bas = stylet.
//!
//! Son : sortie audio par défaut du système. La DS produit ≈ 32 728 Hz,
//! rééchantillonnés à la fréquence de la carte son, avec une réserve de
//! 60 ms (`avxds_core::audio`). L'image donne la cadence ; le son suit.
//! En accéléré (Tab), le son est coupé.
//!
//! `--firmware` : image de 256 Ko du firmware de sa propre console (les
//! réglages utilisateur et le calibrage de l'écran tactile en viennent).
//!
//! Sauvegarde : `jeu.sav` à côté de la ROM, écrit une seconde après la
//! dernière écriture du jeu et en quittant. Avant d'écraser un fichier
//! existant, une copie `jeu.sav.bak` est faite (une fois). Le type de puce
//! est détecté ; `--save-type` l'impose : eeprom512, eeprom8k, eeprom64k,
//! fram32k, eeprom128k, flash256k, flash512k, flash1m, flash8m.

use avxds_core::nds::gpu::{HEIGHT, WIDTH};
use avxds_core::nds::spimem::Family;
use avxds_core::nds::FIRMWARE_SIZE;
use std::path::{Path, PathBuf};
use avxds_core::audio::AudioQueue;
use avxds_core::nds::sound::SAMPLE_RATE;
use avxds_core::{png, Nds};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use std::sync::{Arc, Mutex};
use pixels::{Pixels, SurfaceTexture};
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{Event, VirtualKeyCode};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::WindowBuilder;
use winit_input_helper::WinitInputHelper;

const SCALE: f64 = 2.0;
/// Une image de DS : 560 190 cycles à 33,513982 MHz ≈ 16,715 ms (59,83 Hz).
const FRAME: Duration = Duration::from_nanos(16_715_000);

/// Touches de KEYINPUT (bits 0–9) et de EXTKEYIN (X = bit 0, Y = bit 1).
const KEYS: [(VirtualKeyCode, u16); 10] = [
    (VirtualKeyCode::X, 0),         // A
    (VirtualKeyCode::Z, 1),         // B
    (VirtualKeyCode::Back, 2),      // Select
    (VirtualKeyCode::Return, 3),    // Start
    (VirtualKeyCode::Right, 4),
    (VirtualKeyCode::Left, 5),
    (VirtualKeyCode::Up, 6),
    (VirtualKeyCode::Down, 7),
    (VirtualKeyCode::W, 8),         // R
    (VirtualKeyCode::Q, 9),         // L
];
fn save_type(name: &str) -> Option<(Family, usize)> {
    Some(match name {
        "eeprom512" => (Family::Eeprom512, 512),
        "eeprom8k" => (Family::Eeprom16, 8 << 10),
        "fram32k" => (Family::Eeprom16, 32 << 10),
        "eeprom64k" => (Family::Eeprom16, 64 << 10),
        "eeprom128k" => (Family::Eeprom24, 128 << 10),
        "flash256k" => (Family::Flash, 256 << 10),
        "flash512k" => (Family::Flash, 512 << 10),
        "flash1m" => (Family::Flash, 1 << 20),
        "flash8m" => (Family::Flash, 8 << 20),
        _ => return None,
    })
}

/// Écrit la sauvegarde ; copie d'abord l'ancien fichier en `.bak` (une fois).
fn write_save(nds: &Nds, path: &Path, backed_up: &mut bool) {
    let Some(data) = nds.save_data() else { return };
    if !*backed_up {
        *backed_up = true;
        if let Ok(old) = std::fs::read(path) {
            let bak = PathBuf::from(format!("{}.bak", path.display()));
            if old != data && !bak.exists() {
                if let Err(e) = std::fs::write(&bak, &old) {
                    eprintln!("{} : {e} ; sauvegarde non écrite", bak.display());
                    return;
                }
                println!("ancienne sauvegarde copiée dans {}", bak.display());
            }
        }
    }
    match std::fs::write(path, &data) {
        Ok(()) => println!("{} enregistrée ({} octets)", path.display(), data.len()),
        Err(e) => eprintln!("{} : {e}", path.display()),
    }
}

/// Ouvre la sortie audio par défaut. `None` si elle est indisponible :
/// le jeu tourne alors sans son.
fn open_audio() -> Option<(cpal::Stream, Arc<Mutex<AudioQueue>>)> {
    let device = cpal::default_host().default_output_device().or_else(|| {
        eprintln!("son : aucune sortie audio");
        None
    })?;
    let supported = device
        .default_output_config()
        .map_err(|e| eprintln!("son : {e}"))
        .ok()?;
    let (rate, channels) = (supported.sample_rate(), supported.channels() as usize);
    let queue = Arc::new(Mutex::new(AudioQueue::new(SAMPLE_RATE, rate as f64)));
    let config = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, channels, &queue),
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, channels, &queue),
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, channels, &queue),
        other => {
            eprintln!("son : format {other} non géré");
            return None;
        }
    }?;
    stream.play().map_err(|e| eprintln!("son : {e}")).ok()?;
    println!("son : {device}, {rate} Hz, {channels} canaux");
    Some((stream, queue))
}

/// Flux de sortie au format `T` de la carte son : la file remplit un
/// tampon `f32`, converti ensuite.
fn build_stream<T: SizedSample + Sample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    queue: &Arc<Mutex<AudioQueue>>,
) -> Option<cpal::Stream> {
    let queue = Arc::clone(queue);
    let mut buf: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                buf.resize(data.len(), 0.0);
                match queue.lock() {
                    Ok(mut q) => q.fill(&mut buf, channels),
                    Err(_) => buf.fill(0.0),
                }
                for (d, &s) in data.iter_mut().zip(&buf) {
                    *d = T::from_sample(s);
                }
            },
            |e| eprintln!("son : {e}"),
            None,
        )
        .map_err(|e| eprintln!("son : {e}"))
        .ok()
}

const EXT_KEYS: [(VirtualKeyCode, u16); 2] = [(VirtualKeyCode::S, 0), (VirtualKeyCode::A, 1)];

fn main() {
    let usage = || -> ! {
        eprintln!("usage : avxds [--firmware firmware.bin] <jeu.nds>");
        std::process::exit(2);
    };
    let (mut path, mut firmware, mut forced) = (None, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--firmware" => firmware = Some(args.next().unwrap_or_else(|| usage())),
            "--save-type" => {
                let t = args.next().unwrap_or_else(|| usage());
                forced = Some(save_type(&t).unwrap_or_else(|| {
                    eprintln!("type de sauvegarde inconnu : {t}");
                    usage()
                }));
            }
            _ if path.is_none() => path = Some(a),
            _ => usage(),
        }
    }
    let Some(path) = path else { usage() };
    let rom = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path} : {e}");
        std::process::exit(1);
    });
    let mut nds = Nds::new();
    if let Some(fw) = firmware {
        match std::fs::read(&fw) {
            Ok(data) if data.len() == FIRMWARE_SIZE => nds.load_firmware(&data),
            Ok(data) => {
                eprintln!("{fw} : {} octets, {FIRMWARE_SIZE} attendus", data.len());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{fw} : {e}");
                std::process::exit(1);
            }
        }
    }
    let save_path = Path::new(&path).with_extension("sav");
    let existing = std::fs::read(&save_path).unwrap_or_default();
    match forced {
        Some((family, size)) => nds.force_save_type(family, size, &existing),
        None => nds.load_save(&existing),
    }
    if !existing.is_empty() {
        println!("{} chargée ({} octets)", save_path.display(), existing.len());
    }
    let header = nds.direct_boot(rom).unwrap_or_else(|e| {
        eprintln!("{path} : {e}");
        std::process::exit(1);
    });
    let title = format!("AvxDS - {}", if header.title.trim().is_empty() { &path } else { &header.title });

    let event_loop = EventLoop::new();
    let mut input = WinitInputHelper::new();
    let window = {
        let size = LogicalSize::new(WIDTH as f64 * SCALE, (HEIGHT * 2) as f64 * SCALE);
        WindowBuilder::new()
            .with_title(&title)
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(WIDTH as f64, (HEIGHT * 2) as f64))
            .build(&event_loop)
            .unwrap()
    };
    let mut pixels = {
        let size = window.inner_size();
        let surface = SurfaceTexture::new(size.width, size.height, &window);
        Pixels::new(WIDTH as u32, (HEIGHT * 2) as u32, surface).unwrap()
    };

    let mut next_frame = Instant::now();
    let mut fps_timer = Instant::now();
    let mut fps_frames = 0u32;
    let mut captures = 0u32;
    let mut backed_up = false;
    let mut save_pending: Option<Instant> = None;
    let mut save_kind = nds.hw.slot1.save.locked();
    // Le flux doit vivre aussi longtemps que la boucle : il part avec elle.
    let audio = open_audio();
    let mut muted = false;

    event_loop.run(move |event, _, control_flow| {
        if let Event::RedrawRequested(_) = event {
            let frame = pixels.frame_mut();
            let screens = nds.hw.gpu.screen(0).iter().chain(nds.hw.gpu.screen(1));
            for (px, &c) in frame.chunks_exact_mut(4).zip(screens) {
                px.copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF]);
            }
            if let Err(e) = pixels.render() {
                eprintln!("rendu : {e}");
                *control_flow = ControlFlow::Exit;
                return;
            }
        }

        if input.update(&event) {
            if input.key_pressed(VirtualKeyCode::Escape) || input.quit() {
                if save_pending.is_some() {
                    write_save(&nds, &save_path, &mut backed_up);
                }
                *control_flow = ControlFlow::Exit;
                return;
            }
            if let Some(size) = input.window_resized() {
                if pixels.resize_surface(size.width, size.height).is_err() {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
            }
            if input.key_pressed(VirtualKeyCode::M) {
                muted = !muted;
                println!("son {}", if muted { "coupé" } else { "rétabli" });
            }
            if input.key_pressed(VirtualKeyCode::F12) {
                let mut img = nds.hw.gpu.screen(0).to_vec();
                img.extend_from_slice(nds.hw.gpu.screen(1));
                captures += 1;
                let name = format!("capture_{captures}.png");
                match std::fs::write(&name, png::encode_rgb(WIDTH, HEIGHT * 2, &img)) {
                    Ok(()) => println!("{name} enregistrée"),
                    Err(e) => eprintln!("{name} : {e}"),
                }
            }

            if input.key_pressed(VirtualKeyCode::F11) {
                // Diagnostic 3D : écrans, calque 3D seul, rapport texte.
                let mut img = nds.hw.gpu.screen(0).to_vec();
                img.extend_from_slice(nds.hw.gpu.screen(1));
                captures += 1;
                let n = captures;
                let files = [
                    (format!("diag_{n}_ecrans.png"), png::encode_rgb(WIDTH, HEIGHT * 2, &img)),
                    (
                        format!("diag_{n}_calque3d.png"),
                        png::encode_rgb(WIDTH, HEIGHT, &avxds_core::nds::gpu3d::layer_rgb(&nds.hw.gpu3d)),
                    ),
                    (format!("diag_{n}_rapport3d.txt"), nds.report_3d().into_bytes()),
                ];
                for (name, data) in files {
                    match std::fs::write(&name, data) {
                        Ok(()) => println!("{name} enregistré"),
                        Err(e) => eprintln!("{name} : {e}"),
                    }
                }
            }

            // Touches : 0 = appuyée.
            let mut keys = 0x03FF;
            for (k, bit) in KEYS {
                if input.key_held(k) {
                    keys &= !(1 << bit);
                }
            }
            let mut ext = 0x007F;
            for (k, bit) in EXT_KEYS {
                if input.key_held(k) {
                    ext &= !(1 << bit);
                }
            }
            nds.hw.keyinput = keys;
            nds.hw.extkeyin = ext;

            // Stylet : bouton gauche tenu au-dessus de l'écran du bas
            // (lignes 192–383 de l'image affichée).
            let pen = input
                .mouse()
                .filter(|_| input.mouse_held(0))
                .and_then(|pos| pixels.window_pos_to_pixel(pos).ok())
                .filter(|&(_, y)| y >= HEIGHT)
                .map(|(x, y)| (x as u8, (y - HEIGHT) as u8));
            nds.set_touch(pen);

            // Cadence de la DS ; Tab maintenu = pas d'attente.
            let turbo = input.key_held(VirtualKeyCode::Tab);
            let now = Instant::now();
            if turbo || now >= next_frame {
                let mut ran = 0;
                while (turbo && ran < 4) || (!turbo && Instant::now() >= next_frame && ran < 4) {
                    nds.run_frame();
                    next_frame += FRAME;
                    ran += 1;
                    fps_frames += 1;
                }
                if turbo || Instant::now() > next_frame + FRAME * 4 {
                    next_frame = Instant::now() + FRAME; // retard trop grand : on repart
                }
                // Son produit par ces images : vers la carte son, sauf en
                // accéléré ou coupé (on le jette, sinon il s'accumule).
                if let Some((_, queue)) = &audio {
                    if !turbo && !muted {
                        if let Ok(mut q) = queue.lock() {
                            q.push(&nds.hw.sound.out);
                        }
                    }
                }
                nds.hw.sound.out.clear();
                window.request_redraw();
            }
            // Sauvegarde : une seconde après la dernière écriture du jeu.
            if nds.save_dirty() {
                nds.clear_save_dirty();
                save_pending = Some(Instant::now());
            }
            if save_pending.is_some_and(|t| t.elapsed() >= Duration::from_secs(1)) {
                write_save(&nds, &save_path, &mut backed_up);
                save_pending = None;
            }
            let kind = nds.hw.slot1.save.locked();
            if kind != save_kind {
                if let Some((family, size)) = kind {
                    println!("puce de sauvegarde : {family:?}, {} Ko", size >> 10);
                }
                save_kind = kind;
            }

            if fps_timer.elapsed() >= Duration::from_secs(1) {
                let underruns = audio.as_ref().and_then(|(_, q)| q.lock().ok().map(|q| q.underruns)).unwrap_or(0);
                let sound = match (&audio, muted) {
                    (None, _) => " - sans son".to_string(),
                    (_, true) => " - son coupé".to_string(),
                    _ if underruns > 0 => format!(" - son : {underruns} coupures"),
                    _ => String::new(),
                };
                window.set_title(&format!("{title} - {fps_frames} images/s{sound}"));
                fps_frames = 0;
                fps_timer = Instant::now();
            }
            *control_flow = if turbo { ControlFlow::Poll } else { ControlFlow::WaitUntil(next_frame) };
        }
    });
}
