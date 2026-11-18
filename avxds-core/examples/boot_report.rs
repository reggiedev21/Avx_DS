//! Démarre une ROM en direct boot, la fait tourner, et affiche ce qu'elle a
//! utilisé et qui n'est pas encore émulé.
//!
//! `cargo run --release --example boot_report -- chemin/vers/jeu.nds [images] [--until-pc ADRESSE]`
//!
//! `--until-pc` : l'ARM9 avance instruction par instruction jusqu'à exécuter
//! cette adresse (hexadécimale), au plus `images` images. Le rapport donne
//! alors l'image atteinte, la pile, les derniers sauts de l'ARM9 et les
//! derniers accès I/O des deux CPU, dans l'ordre.
//!
//! `--dump ADRESSE:TAILLE` (répétable) : contenu de la mémoire à la fin, vu
//! par l'ARM9. `--watch ADRESSE:TAILLE` : toutes les écritures dans cette
//! zone (CPU, instruction, valeur, image). Les adresses et tailles sont en
//! hexadécimal.
//!
//! `--firmware fichier.bin` : firmware de 256 Ko de sa console ;
//! `--save fichier.sav` : sauvegarde à charger.
//!
//! `--keys A@1800,START@2400` : appuis simulés (5 images chacun) ; noms
//! A, B, SELECT, START, RIGHT, LEFT, UP, DOWN, R, L.
//! `--png fichier.png` : capture des deux écrans à la fin.
//!
//! `--dump-ram PRÉFIXE` : à la fin, écrit la mémoire en fichiers bruts,
//! pour l'analyse hors ligne : PRÉFIXE-main.bin (Main RAM, 4 Mo, en
//! 0x02000000), PRÉFIXE-wram7.bin (WRAM de l'ARM7, 64 Ko, en 0x03800000),
//! PRÉFIXE-swram.bin (WRAM partagée, 32 Ko) et PRÉFIXE-regs.txt (registres
//! des deux CPU).
//!
//! Après ces images, une image de plus est observée : adresses où chaque CPU
//! passe son temps, instructions autour, registres I/O accédés depuis quelles
//! instructions, état des IRQ et de l'IPC. C'est ce qui dit *où* et *sur
//! quoi* un jeu attend.

use avxds_core::nds::{CpuId, IoTrace, Watch, FRAME_CYCLES};
use avxds_core::{Bus, Nds};
use std::collections::BTreeMap;

/// Nom des registres I/O les plus courants (GBATEK), pour un rapport lisible.
fn io_name(cpu: CpuId, addr: u32) -> &'static str {
    if cpu == CpuId::Arm7 && (0x0400_0400..=0x0400_051C).contains(&addr) {
        return "son";
    }
    match addr & !3 {
        0x0400_0000 => "DISPCNT (moteur A)",
        0x0400_0004 => "DISPSTAT / VCOUNT",
        0x0400_0100..=0x0400_010C => "timers",
        0x0400_0180 => "IPCSYNC",
        0x0400_0184 => "IPCFIFOCNT",
        0x0400_0188 => "IPCFIFOSEND",
        0x0410_0000 => "IPCFIFORECV",
        0x0400_0208 => "IME",
        0x0400_0210 => "IE",
        0x0400_0214 => "IF",
        0x0400_0240..=0x0400_0248 => "VRAMCNT / WRAMCNT",
        0x0400_0280..=0x0400_02BC => "division / racine",
        0x0400_0300 => "POSTFLG / HALTCNT",
        0x0400_0304 => "POWCNT",
        0x0400_0008 | 0x0400_000C => "BGxCNT (moteur A)",
        0x0400_0010..=0x0400_001C => "BGxHOFS/VOFS (moteur A)",
        0x0400_0020..=0x0400_003C => "BG2/BG3 affine (moteur A)",
        0x0400_0040..=0x0400_004C => "fenêtres (moteur A)",
        0x0400_0050..=0x0400_0054 => "BLDCNT/BLDALPHA/BLDY (moteur A)",
        0x0400_0060 => "DISP3DCNT",
        0x0400_0064 => "DISPCAPCNT",
        0x0400_006C => "MASTER_BRIGHT (moteur A)",
        0x0400_00B0..=0x0400_00DC => "DMA",
        0x0400_00E0..=0x0400_00EC => "DMA fill",
        0x0400_0130 => "KEYINPUT/KEYCNT",
        0x0400_0134 => "RCNT / EXTKEYIN",
        0x0400_0138 => "RTC",
        0x0400_01A0 => "AUXSPICNT / AUXSPIDATA",
        0x0400_01A4..=0x0400_01BC => "cartouche (ROMCTRL, commande)",
        0x0400_01C0 => "SPI (firmware, tactile, alim.)",
        0x0400_0204 => "EXMEMCNT",
        0x0400_0320..=0x0400_06A0 => "moteur 3D",
        0x0400_1000 => "DISPCNT (moteur B)",
        0x0400_1004..=0x0400_106C => "registres du moteur B",
        0x0410_0010 => "données cartouche",
        0x0400_4000..=0x0400_4FFF => "DSi (SCFG…)",
        _ => "?",
    }
}

/// Activité du moteur 3D pendant la dernière image complète.
fn print_3d(nds: &Nds) {
    print!("{}", nds.report_3d());
    use avxds_core::nds::gpu3d::commands;
    let g = &nds.hw.gpu3d;
    let f = &g.last_frame;
    println!(
        "3D, dernière image : {} commandes, {} mots GXFIFO, {} écritures de ports, {} SwapBuffers",
        f.total_commands(),
        f.gxfifo_words,
        f.port_writes,
        f.swaps
    );
    let list: Vec<String> = (0..0x80u8)
        .filter(|&c| f.commands[c as usize] > 0)
        .map(|c| format!("{} {}", commands::name(c), f.commands[c as usize]))
        .collect();
    if !list.is_empty() {
        println!("  {}", list.join(", "));
    }
    println!(
        "  file : {} en FIFO, {} au PIPE, {} en réserve ; blocages {}, commandes interrompues {}, codes invalides {}, 4e emplacement sans paramètre {}",
        g.fifo.fifo_len(),
        g.fifo.pipe_len(),
        g.fifo.reserve_len(),
        f.stalls,
        f.broken,
        g.unpack.invalid,
        g.unpack.top_slot_without_params
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage : boot_report <rom.nds> [images]");
        std::process::exit(2);
    };
    let mut frames: u32 = 120;
    let mut until_pc: Option<u32> = None;
    let mut dumps: Vec<(u32, u32)> = Vec::new();
    let mut watch: Option<(u32, u32)> = None;
    let mut firmware: Option<String> = None;
    let mut save: Option<String> = None;
    let mut dump_ram: Option<String> = None;
    let mut presses: Vec<(u16, u32)> = Vec::new();
    let mut png_out: Option<String> = None;
    let mut wire_out: Option<String> = None;
    let mut gx_log_out: Option<String> = None;
    let mut png3d_out: Option<String> = None;
    let mut wav_out: Option<String> = None;
    let mut wav: Vec<i16> = Vec::new();
    let hex = |v: &str| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok();
    let range = |v: Option<String>| -> (u32, u32) {
        let v = v.unwrap_or_default();
        let (a, l) = v.split_once(':').unwrap_or((&v, "40"));
        match (hex(a), hex(l)) {
            (Some(a), Some(l)) => (a, l),
            _ => {
                eprintln!("ADRESSE:TAILLE hexadécimales attendues, reçu {v:?}");
                std::process::exit(2);
            }
        }
    };
    while let Some(a) = args.next() {
        if a == "--keys" {
            let spec = args.next().unwrap_or_default();
            for p in spec.split(',') {
                let names = ["A", "B", "SELECT", "START", "RIGHT", "LEFT", "UP", "DOWN", "R", "L"];
                let parsed = p.split_once('@').and_then(|(k, at)| {
                    let bit = names.iter().position(|n| n.eq_ignore_ascii_case(k))?;
                    Some((1u16 << bit, at.parse().ok()?))
                });
                match parsed {
                    Some(x) => presses.push(x),
                    None => {
                        eprintln!("--keys : « {p} » non compris (ex. A@1800)");
                        std::process::exit(2);
                    }
                }
            }
        } else if a == "--wav" {
            wav_out = args.next();
        } else if a == "--png3d" {
            png3d_out = args.next();
        } else if a == "--gx-log" {
            gx_log_out = args.next();
        } else if a == "--wire" {
            wire_out = args.next();
        } else if a == "--png" {
            png_out = args.next();
        } else if a == "--dump-ram" {
            dump_ram = args.next();
        } else if a == "--firmware" {
            firmware = args.next();
        } else if a == "--save" {
            save = args.next();
        } else if a == "--dump" {
            dumps.push(range(args.next()));
        } else if a == "--watch" {
            watch = Some(range(args.next()));
        } else if a == "--until-pc" {
            let v = args.next().unwrap_or_default();
            until_pc = Some(u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or_else(|_| {
                eprintln!("--until-pc : adresse hexadécimale attendue");
                std::process::exit(2);
            }));
        } else if let Ok(n) = a.parse() {
            frames = n;
        }
    }
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{path} : {e}"));

    let mut nds = Nds::new();
    if let Some(f) = &firmware {
        let data = std::fs::read(f).unwrap_or_else(|e| panic!("{f} : {e}"));
        nds.load_firmware(&data);
        println!("firmware : {f} ({} octets)", data.len());
    }
    if let Some(f) = &save {
        let data = std::fs::read(f).unwrap_or_else(|e| panic!("{f} : {e}"));
        nds.load_save(&data);
        println!("sauvegarde : {f} ({} octets)", data.len());
    }
    let h = match nds.direct_boot(rom) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{path} : {e}");
            std::process::exit(1);
        }
    };
    println!("== {path}");
    println!("titre {:?}, code {:?}", h.title, h.game_code_str());
    println!("ARM9 : {:#X} octets en {:#010X}, entrée {:#010X}", h.arm9_size, h.arm9_ram_address, h.arm9_entry);
    println!("ARM7 : {:#X} octets en {:#010X}, entrée {:#010X}", h.arm7_size, h.arm7_ram_address, h.arm7_entry);

    if let Some((a, l)) = watch {
        nds.hw.watch = Some(Watch::new(a, l));
        nds.hw.io_trace = Some(IoTrace::default()); // pour l'adresse des instructions
    }
    if let Some(target) = until_pc {
        run_until_pc(&mut nds, target, frames, &presses);
        extras(&mut nds, &dumps);
        if let Some(p) = &png_out {
            write_png(&nds, p);
        }
        if let Some(p) = &dump_ram {
            write_ram(&nds, p);
        }
        return;
    }

    let start = std::time::Instant::now();
    println!("\nimage   haut  bas   DISPCNT A  DISPCNT B  BRIGHT A BRIGHT B  POWCNT1  cmd 3D  swap  poly");
    for f in 1..=frames {
        if gx_log_out.is_some() && f + 3 == frames {
            nds.hw.gpu3d.log = Some(Vec::new()); // les 4 dernières images
        }
        nds.hw.keyinput = keys_at(&presses, f - 1);
        nds.run_frame();
        if wav_out.is_some() {
            wav.append(&mut nds.hw.sound.out);
        } else {
            nds.hw.sound.out.clear();
        }
        if f % 60 == 0 || f == frames {
            // Part de pixels non noirs : dit quand et sur quel écran l'image disparaît.
            let lit = |n: usize| {
                let s = nds.hw.gpu.screen(n);
                100.0 * s.iter().filter(|&&c| c & 0xFFFFFF != 0).count() as f64 / s.len() as f64
            };
            let (top, bottom) = (lit(0), lit(1));
            let g = &nds.hw.gpu3d.last_frame;
            let (cmds, swaps) = (g.total_commands(), g.swaps);
            let polys = nds.hw.gpu3d.geo.rendering.polygons.len();
            let mut bus = nds.bus9();
            println!(
                "{f:5} {top:5.1}% {bottom:5.1}%  {:08X}   {:08X}   {:04X}     {:04X}     {:04X}  {cmds:6}  {swaps:4}  {polys:4}",
                bus.read32(0x0400_0000),
                bus.read32(0x0400_1000),
                bus.read16(0x0400_006C),
                bus.read16(0x0400_106C),
                bus.read16(0x0400_0304)
            );
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!("\n{frames} images en {secs:.2} s ({:.0} images/s)", frames as f64 / secs);
    for (name, cpu) in [("ARM9", &nds.arm9 as &dyn std::fmt::Debug), ("ARM7", &nds.arm7)] {
        println!("--- {name}\n{cpu:?}");
    }
    println!("ARM9 : {} instructions, {}", nds.arm9.instructions, if nds.arm9.halted { "en pause" } else { "actif" });
    println!("ARM7 : {} instructions, {}", nds.arm7.instructions, if nds.arm7.halted { "en pause" } else { "actif" });

    let mut by_name: BTreeMap<(CpuId, &str), Vec<u32>> = BTreeMap::new();
    for &(cpu, addr) in &nds.hw.unhandled_io {
        by_name.entry((cpu, io_name(cpu, addr))).or_default().push(addr);
    }
    println!("\nI/O non émulées ({}) :", nds.hw.unhandled_io.len());
    for ((cpu, name), addrs) in &by_name {
        let list: Vec<String> = addrs.iter().map(|a| format!("{a:08X}")).collect();
        println!("  {cpu:?} {name:<38} {}", list.join(" "));
    }
    println!("SWI non émulés : {:x?}", nds.hw.unhandled_swi);
    println!("Affichage, fonctions demandées non émulées : {:?}", nds.hw.gpu.unsupported);
    // État brut de l'affichage, pour décoder un écran faux : registres de
    // chaque moteur (0x00 = DISPCNT, 0x08 = BG0CNT|BG1CNT, 0x0C = BG2CNT|BG3CNT,
    // 0x10–0x1C = défilements, 0x20–0x3C = affines…) et banques de VRAM.
    for (e, name) in ["A (0x04000000)", "B (0x04001000)"].iter().enumerate() {
        let eng = &nds.hw.gpu.engines[e];
        println!("Moteur {name} :");
        for (k, chunk) in eng.regs.chunks(4).enumerate() {
            let words: Vec<String> = chunk.iter().map(|w| format!("{w:08X}")).collect();
            println!("  +{:02X}: {}", k * 16, words.join(" "));
        }
    }
    let cnt: Vec<String> = nds.hw.vram.cnt.iter().map(|c| format!("{c:02X}")).collect();
    println!("VRAMCNT A–I : {}", cnt.join(" "));
    print_3d(&nds);
    println!("Autres fonctions demandées non émulées : {:?}", nds.hw.unsupported);
    let save = &nds.hw.slot1.save;
    match save.locked() {
        Some((family, size)) => println!("Sauvegarde : {family:?}, {} Ko", size >> 10),
        None => println!("Sauvegarde : pas encore déterminée (en tête : {:?})", save.leading()),
    }
    for line in &save.trace {
        println!("  {line}");
    }
    if let Some(p) = &png_out {
        write_png(&nds, p);
    }
    if let Some(p) = &wire_out {
        write_wire(&nds, p);
    }
    if let Some(p) = &gx_log_out {
        write_gx_log(&mut nds, p);
    }
    if let Some(p) = &png3d_out {
        write_png3d(&nds, p);
    }
    if let Some(p) = &wav_out {
        write_wav(p, &wav);
    }
    report_sound(&nds);
    diagnose(&mut nds);
    extras(&mut nds, &dumps);
    if let Some(p) = &dump_ram {
        write_ram(&nds, p);
    }
}

/// Les deux écrans (haut au-dessus) en PNG.
fn write_png(nds: &Nds, path: &str) {
    use avxds_core::nds::gpu::{HEIGHT, WIDTH};
    let mut px = nds.hw.gpu.screen(0).to_vec();
    px.extend_from_slice(nds.hw.gpu.screen(1));
    match std::fs::write(path, avxds_core::png::encode_rgb(WIDTH, HEIGHT * 2, &px)) {
        Ok(()) => println!("{path} écrit"),
        Err(e) => eprintln!("{path} : {e}"),
    }
}

/// État du contrôleur son en fin de course : registres généraux et
/// canaux actifs.
fn report_sound(nds: &Nds) {
    let snd = &nds.hw.sound;
    println!(
        "Son : SOUNDCNT {:04X}  SOUNDBIAS {:03X}  SNDCAPCNT {:04X}",
        snd.read(0x0400_0500),
        snd.read(0x0400_0504),
        snd.read(0x0400_0508)
    );
    const FORMATS: [&str; 4] = ["PCM8", "PCM16", "ADPCM", "PSG"];
    for n in 0..16u32 {
        let cnt = snd.read(0x0400_0400 + 16 * n);
        if cnt >> 31 == 0 {
            continue;
        }
        let tmr = snd.ch[n as usize].tmr;
        let hz = 16_756_991.0 / (0x1_0000 - tmr as u32) as f64;
        println!(
            "  canal {n:2} : {:5} vol {:3} ÷{:<2} pan {:3} répétition {} TMR {tmr:04X} ({hz:.0} Hz)",
            FORMATS[(cnt >> 29 & 3) as usize],
            cnt & 0x7F,
            [1, 2, 4, 16][(cnt >> 8 & 3) as usize],
            cnt >> 16 & 0x7F,
            cnt >> 27 & 3,
        );
    }
}

/// Son produit pendant toute la course, en WAV 16 bits stéréo.
fn write_wav(path: &str, samples: &[i16]) {
    let rate = avxds_core::nds::sound::SAMPLE_RATE.round() as u32;
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stéréo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    let (lo, hi) = (samples.iter().min().copied().unwrap_or(0), samples.iter().max().copied().unwrap_or(0));
    match std::fs::write(path, out) {
        Ok(()) => println!(
            "{path} écrit ({:.1} s à {rate} Hz, de {lo} à {hi})",
            samples.len() as f64 / 2.0 / rate as f64
        ),
        Err(e) => eprintln!("{path} : {e}"),
    }
}

/// Calque 3D seul, avant la composition 2D (alpha 0 en magenta sombre).
fn write_png3d(nds: &Nds, path: &str) {
    let px = avxds_core::nds::gpu3d::layer_rgb(&nds.hw.gpu3d);
    match std::fs::write(path, avxds_core::png::encode_rgb(256, 192, &px)) {
        Ok(()) => println!("{path} écrit (calque 3D seul ; magenta sombre = transparent)"),
        Err(e) => eprintln!("{path} : {e}"),
    }
}

/// Journal des commandes 3D des dernières images, une ligne par événement :
/// `W` mot GXFIFO brut, `P` port, `X` commande exécutée, `VBLANK`.
fn write_gx_log(nds: &mut Nds, path: &str) {
    use avxds_core::nds::gpu3d::{commands, GxEvent};
    use std::fmt::Write as _;
    let log = nds.hw.gpu3d.log.take().unwrap_or_default();
    let mut out = String::new();
    for e in &log {
        let _ = match *e {
            GxEvent::Word(w) => writeln!(out, "W {w:08X}"),
            GxEvent::Port(c, v) => writeln!(out, "P {:<14} {v:08X}", commands::name(c)),
            GxEvent::Exec(c, p) => writeln!(out, "X {:<14} {p:08X}", commands::name(c)),
            GxEvent::VBlank { swapped } => writeln!(out, "VBLANK{}", if swapped { " (échange)" } else { "" }),
        };
    }
    match std::fs::write(path, out) {
        Ok(()) => println!("{path} écrit ({} événements)", log.len()),
        Err(e) => eprintln!("{path} : {e}"),
    }
}

/// Dernière liste de polygones livrée au rendu, en fil de fer (blanc ;
/// faces arrière en gris ; sommets aux positions calculées par le viewport).
fn write_wire(nds: &Nds, path: &str) {
    let (w, h) = (256i32, 192i32);
    let mut px = vec![0u32; (w * h) as usize];
    let mut plot = |x: i32, y: i32, c: u32| {
        if (0..w).contains(&x) && (0..h).contains(&y) {
            px[(y * w + x) as usize] = c;
        }
    };
    let list = &nds.hw.gpu3d.geo.rendering;
    for p in &list.polygons {
        let c = if p.front { 0xFFFFFF } else { 0x808080 };
        let n = p.vertices.len();
        for i in 0..n {
            let [x0, y0] = p.vertices[i].screen;
            let [x1, y1] = p.vertices[(i + 1) % n].screen;
            // Bresenham.
            let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
            let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
            let (mut x, mut y, mut err) = (x0, y0, dx + dy);
            loop {
                plot(x, y, c);
                if x == x1 && y == y1 {
                    break;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y += sy;
                }
            }
        }
    }
    match std::fs::write(path, avxds_core::png::encode_rgb(w as usize, h as usize, &px)) {
        Ok(()) => println!("{path} écrit ({} polygones, {} sommets)", list.polygons.len(), list.vertex_count),
        Err(e) => eprintln!("{path} : {e}"),
    }
}

/// Mémoire et registres en fichiers bruts (analyse hors ligne).
fn write_ram(nds: &Nds, prefix: &str) {
    let regs = format!("{:?}\n{:?}\nimage {:.2}\n", nds.arm9, nds.arm7, nds.now() as f64 / FRAME_CYCLES as f64);
    let files: [(&str, &[u8]); 4] = [
        ("main.bin", &nds.hw.main_ram),
        ("wram7.bin", &nds.hw.arm7_wram),
        ("swram.bin", &nds.hw.shared_wram),
        ("regs.txt", regs.as_bytes()),
    ];
    for (name, data) in files {
        let path = format!("{prefix}-{name}");
        match std::fs::write(&path, data) {
            Ok(()) => println!("{path} écrit ({} octets)", data.len()),
            Err(e) => eprintln!("{path} : {e}"),
        }
    }
}

/// Contenu de la mémoire, écritures surveillées et historique IPC.
fn extras(nds: &mut Nds, dumps: &[(u32, u32)]) {
    for &(a, l) in dumps {
        println!("--- Mémoire {a:08X} ({l:#X} octets), en mots de 32 bits");
        let start = a & !3;
        let mut k = 0;
        while k < l {
            let line: Vec<String> = (0..4).map(|i| format!("{:08X}", nds.bus9().read32(start + k + 4 * i))).collect();
            println!("  {:08X}: {}", start + k, line.join(" "));
            k += 16;
        }
    }
    if let Some(w) = nds.hw.watch.take() {
        println!("--- Écritures dans {:08X}..{:08X} : {} au total ({} dernières)", w.start, w.end, w.total, w.hits.len());
        for (t, cpu, pc, addr, size, v) in &w.hits {
            println!(
                "  image {:7.2} {cpu:?} pc={pc:08X} {addr:08X} ← {v:0width$X} ({size} o)",
                *t as f64 / FRAME_CYCLES as f64,
                width = *size as usize * 2
            );
        }
    }
    if let Some(trace) = &nds.hw.io_trace {
        if !trace.ipc.is_empty() {
            println!("--- Échanges IPC, SPI de la cartouche et EXMEMCNT ({} premiers)", trace.ipc.len());
            let tail_header = format!(
                "--- … {} accès omis … puis les {} derniers",
                trace.ipc_dropped,
                trace.ipc_tail.len()
            );
            let all = trace.ipc.iter().map(Some).chain(std::iter::once(None)).chain(trace.ipc_tail.iter().map(Some));
            for e in all {
                let Some((t, cpu, pc, addr, write, v, n)) = e else {
                    if !trace.ipc_tail.is_empty() {
                        println!("{tail_header}");
                    }
                    continue;
                };
                println!(
                    "  image {:7.2} {cpu:?} pc={pc:08X} {} {:<22} {v:08X}{}",
                    *t as f64 / FRAME_CYCLES as f64,
                    if *write { "écrit" } else { "lit  " },
                    io_name(*cpu, *addr),
                    if *n > 1 { format!(" ×{n}") } else { String::new() }
                );
            }
        }
    }
}

/// Une image observée : où chaque CPU passe son temps, et sur quels
/// registres il boucle.
fn diagnose(nds: &mut Nds) {
    const SAMPLES: u64 = 2000;
    nds.hw.io_trace = Some(IoTrace::default());
    let mut hot: [BTreeMap<(u32, bool), u32>; 2] = Default::default();
    let mut halted = [0u32; 2];
    for _ in 0..SAMPLES {
        let t = nds.now() + FRAME_CYCLES / SAMPLES;
        nds.run_until(t);
        let cpus = [
            (nds.arm9.halted, nds.arm9.pc(), nds.arm9.thumb()),
            (nds.arm7.halted, nds.arm7.pc(), nds.arm7.thumb()),
        ];
        for (i, (h, pc, thumb)) in cpus.into_iter().enumerate() {
            if h {
                halted[i] += 1;
            } else {
                *hot[i].entry((pc, thumb)).or_default() += 1;
            }
        }
    }
    let trace = nds.hw.io_trace.take().unwrap_or_default();

    println!("\n== Diagnostic sur une image ({SAMPLES} échantillons)");
    for (i, name) in ["ARM9", "ARM7"].into_iter().enumerate() {
        println!("--- {name} : en pause {:.0} % du temps", 100.0 * halted[i] as f64 / SAMPLES as f64);
        let mut list: Vec<_> = hot[i].iter().collect();
        list.sort_by_key(|e| std::cmp::Reverse(*e.1));
        for ((pc, thumb), n) in list.iter().take(8) {
            println!("  {pc:08X} {} {:5.1} %", if *thumb { "Thumb" } else { "ARM  " }, 100.0 * **n as f64 / SAMPLES as f64);
        }
        if let Some(((pc, thumb), _)) = list.first() {
            // Le code autour de l'adresse la plus fréquente.
            let (step, count) = if *thumb { (2, 24) } else { (4, 12) };
            let from = pc.wrapping_sub(step * (count / 2)) & !(step - 1);
            println!("  code autour de {pc:08X} :");
            for k in 0..count {
                let a = from + k * step;
                let w = if i == 0 {
                    if *thumb { nds.bus9().read16(a) as u32 } else { nds.bus9().read32(a) }
                } else if *thumb {
                    nds.bus7().read16(a) as u32
                } else {
                    nds.bus7().read32(a)
                };
                let mark = if a == *pc { "  <==" } else { "" };
                if *thumb {
                    println!("    {a:08X}: {w:04X}{mark}");
                } else {
                    println!("    {a:08X}: {w:08X}{mark}");
                }
            }
        }
    }

    println!("--- Accès I/O pendant cette image (les plus fréquents)");
    let mut acc: Vec<_> = trace.counts.iter().collect();
    acc.sort_by_key(|e| std::cmp::Reverse(e.1 .0));
    for ((cpu, pc, addr, write), (n, last)) in acc.iter().take(30) {
        println!(
            "  {cpu:?} pc={pc:08X} {} {addr:08X} {:<36} ×{n:<7} dernière valeur {last:08X}",
            if *write { "écrit" } else { "lit  " },
            io_name(*cpu, *addr)
        );
    }

    println!("--- État");
    let hw = &nds.hw;
    println!("  ARM9 IME={} IE={:08X} IF={:08X}", hw.irq9.ime as u8, hw.irq9.ie, hw.irq9.if_);
    println!("  ARM7 IME={} IE={:08X} IF={:08X}", hw.irq7.ime as u8, hw.irq7.ie, hw.irq7.if_);
    println!("  IPCSYNC ARM9={:04X} ARM7={:04X}", hw.ipcsync9, hw.ipcsync7);
    let f9 = nds.bus9().read16(0x0400_0184);
    let f7 = nds.bus7().read16(0x0400_0184);
    println!("  IPCFIFOCNT ARM9={f9:04X} ARM7={f7:04X}");
    println!("  VCOUNT={} DISPSTAT ARM9={:04X}", nds.bus9().read16(0x0400_0006), nds.bus9().read16(0x0400_0004));
}


/// L'ARM9 avance pas à pas jusqu'à exécuter `target`.
/// KEYINPUT à l'image `f` (0 = appuyée).
fn keys_at(presses: &[(u16, u32)], f: u32) -> u16 {
    let mut keys = 0x03FF;
    for &(bit, at) in presses {
        if (at..at + 5).contains(&f) {
            keys &= !bit;
        }
    }
    keys
}

fn run_until_pc(nds: &mut Nds, target: u32, frames: u32, presses: &[(u16, u32)]) {
    const BRANCHES: usize = 64;
    let limit = nds.now() + frames as u64 * FRAME_CYCLES;
    let t0 = nds.now();
    nds.hw.io_trace = Some(IoTrace::default());
    let mut branches: std::collections::VecDeque<((u32, u32, bool), u64)> = Default::default();
    let mut prev = (nds.arm9.pc(), nds.arm9.thumb());
    let mut steps = 0u64;
    let start = std::time::Instant::now();
    let found = loop {
        if nds.arm9.pc() == target && !nds.arm9.halted {
            break true;
        }
        if nds.now() >= limit {
            break false;
        }
        if steps.is_multiple_of(4096) {
            nds.hw.keyinput = keys_at(presses, ((nds.now() - t0) / FRAME_CYCLES) as u32);
        }
        nds.step();
        steps += 1;
        let cur = (nds.arm9.pc(), nds.arm9.thumb());
        let size = if prev.1 { 2 } else { 4 };
        if cur.0 != prev.0 && cur.0 != prev.0.wrapping_add(size) {
            // Saut (branchement, retour, IRQ…). Une boucle répète les mêmes
            // sauts : s'il figure parmi les 16 derniers, on le compte.
            let jump = (prev.0, cur.0, cur.1);
            if let Some(e) = branches.iter_mut().rev().take(16).find(|e| e.0 == jump) {
                e.1 += 1;
            } else {
                if branches.len() == BRANCHES {
                    branches.pop_front();
                }
                branches.push_back((jump, 1));
            }
        }
        prev = cur;
    };
    let frame = (nds.now() - t0) as f64 / FRAME_CYCLES as f64;
    println!(
        "\n{} {target:08X} après {steps} pas, à l'image {frame:.2} ({:.1} s)",
        if found { "ARM9 arrivé en" } else { "ARM9 jamais passé par" },
        start.elapsed().as_secs_f64()
    );
    println!("--- ARM9\n{:?}", nds.arm9);
    println!("--- ARM7 ({})\n{:?}", if nds.arm7.halted { "en pause" } else { "actif" }, nds.arm7);

    println!("--- Pile de l'ARM9 (* = ressemble à une adresse de retour)");
    let sp = nds.arm9.reg(13);
    for k in 0..48u32 {
        let a = sp.wrapping_add(4 * k);
        let w = nds.bus9().read32(a);
        let code = (0x0200_0000..0x0240_0000).contains(&w) || (0x0100_0000..0x0200_0000).contains(&w);
        println!("  {a:08X}: {w:08X}{}", if code { " *" } else { "" });
    }

    println!("--- Derniers sauts de l'ARM9 (du plus ancien au plus récent)");
    for ((from, to, thumb), n) in &branches {
        let times = if *n > 1 { format!(" ×{n}") } else { String::new() };
        println!("  {from:08X} -> {to:08X} {}{times}", if *thumb { "Thumb" } else { "ARM" });
    }

    let trace = nds.hw.io_trace.clone().unwrap_or_default();
    println!("--- Derniers accès I/O (du plus ancien au plus récent)");
    for (cpu, pc, addr, write, v) in &trace.recent {
        println!(
            "  {cpu:?} pc={pc:08X} {} {addr:08X} {:<36} {v:08X}",
            if *write { "écrit" } else { "lit  " },
            io_name(*cpu, *addr)
        );
    }
    let hw = &nds.hw;
    println!("--- État");
    println!("  ARM9 IME={} IE={:08X} IF={:08X}", hw.irq9.ime as u8, hw.irq9.ie, hw.irq9.if_);
    println!("  ARM7 IME={} IE={:08X} IF={:08X}", hw.irq7.ime as u8, hw.irq7.ie, hw.irq7.if_);
    println!("  IPCSYNC ARM9={:04X} ARM7={:04X}", hw.ipcsync9, hw.ipcsync7);
    let save = &nds.hw.slot1.save;
    println!("  sauvegarde : {:?} (en tête {:?})", save.locked(), save.leading());
    for line in &save.trace {
        println!("    {line}");
    }
    let mut unhandled: Vec<_> = nds.hw.unhandled_io.iter().collect();
    unhandled.sort();
    println!("  I/O non émulées jusqu'ici : {unhandled:x?}");
}
