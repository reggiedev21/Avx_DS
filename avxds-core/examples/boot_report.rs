//! Démarre une ROM en direct boot, la fait tourner, et affiche ce qu'elle a
//! utilisé et qui n'est pas encore émulé.
//!
//! `cargo run --release --example boot_report -- chemin/vers/jeu.nds [images]`

use avxds_core::nds::CpuId;
use avxds_core::Nds;
use std::collections::BTreeMap;

/// Nom des registres I/O les plus courants (GBATEK), pour un rapport lisible.
fn io_name(cpu: CpuId, addr: u32) -> &'static str {
    if cpu == CpuId::Arm7 && (0x0400_0400..=0x0400_051C).contains(&addr) {
        return "son";
    }
    match addr & !3 {
        0x0400_0000 => "DISPCNT (moteur A)",
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
        0x0400_01A0..=0x0400_01BC => "cartouche (AUXSPI, ROMCTRL, commande)",
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

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage : boot_report <rom.nds> [images]");
        std::process::exit(2);
    };
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{path} : {e}"));

    let mut nds = Nds::new();
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

    let start = std::time::Instant::now();
    for _ in 0..frames {
        nds.run_frame();
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
}
