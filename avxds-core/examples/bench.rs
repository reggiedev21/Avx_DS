//! Mesure grossière du débit de l'interpréteur : exécute en boucle un
//! programme de test et affiche des millions d'instructions par seconde.
//! `cargo run --release --example bench`
use avxds_core::{Arch, Armv4t, Armv5te, Bus, Cpu, Nds};
use std::time::Instant;

struct Flat(Vec<u8>, bool);
impl Bus for Flat {
    fn read8(&mut self, a: u32) -> u8 { self.0[a as usize & 0xFFFFF] }
    fn read16(&mut self, a: u32) -> u16 { let i = a as usize & 0xFFFFF; u16::from_le_bytes([self.0[i], self.0[i + 1]]) }
    fn read32(&mut self, a: u32) -> u32 { let i = a as usize & 0xFFFFF; u32::from_le_bytes(self.0[i..i + 4].try_into().unwrap()) }
    fn write8(&mut self, a: u32, v: u8) { self.0[a as usize & 0xFFFFF] = v }
    fn write16(&mut self, a: u32, v: u16) { let i = a as usize & 0xFFFFF; self.0[i..i + 2].copy_from_slice(&v.to_le_bytes()) }
    fn write32(&mut self, a: u32, v: u32) { let i = a as usize & 0xFFFFF; self.0[i..i + 4].copy_from_slice(&v.to_le_bytes()) }
    fn hle_swi(&mut self, n: u8, _: &mut [u32; 4]) -> bool { if n == 0xFF { self.1 = true; } n == 0xFF }
}

fn bench<A: Arch>(name: &str, prog: &[u8]) {
    let start = Instant::now();
    let mut total = 0u64;
    while start.elapsed().as_secs_f64() < 1.0 {
        let mut mem = vec![0; 1 << 20];
        mem[..prog.len()].copy_from_slice(prog);
        let mut bus = Flat(mem, false);
        let mut cpu = Cpu::<A>::new();
        cpu.reset(&mut bus);
        while !bus.1 { cpu.step(&mut bus); }
        total += cpu.instructions;
    }
    let secs = start.elapsed().as_secs_f64();
    println!("{name:<18} {:>7.1} M instructions/s", total as f64 / secs / 1e6);
}

/// Machine complète : l'ARM9 scrute DISPSTAT en boucle (comme un jeu qui
/// attend le VBlank), l'ARM7 tourne dans une boucle de calcul.
fn bench_machine() {
    let mut nds = Nds::new();
    let arm9: [u32; 4] = [0xE3A04301, 0xE1D400B4, 0xE3100001, 0xEAFFFFFC]; // mov r4,#0x4000000 ; ldrh ; tst ; b
    let arm7: [u32; 3] = [0xE2800001, 0xE0811000, 0xEAFFFFFC]; // add ; add ; b
    {
        let mut bus = nds.bus9();
        for (i, w) in arm9.iter().enumerate() { bus.write32(0x0200_0000 + 4 * i as u32, *w); }
    }
    {
        let mut bus = nds.bus7();
        for (i, w) in arm7.iter().enumerate() { bus.write32(0x0380_0000 + 4 * i as u32, *w); }
    }
    nds.arm9.jump(0x0200_0000, false);
    nds.arm7.jump(0x0380_0000, false);
    let start = Instant::now();
    let mut frames = 0;
    while start.elapsed().as_secs_f64() < 1.0 {
        nds.run_frame();
        frames += 1;
    }
    let secs = start.elapsed().as_secs_f64();
    println!("Machine complète  {:>7.1} images/s (une DS : 59,8)", frames as f64 / secs);
}

fn main() {
    bench_machine();
    bench::<Armv5te>("ARMv5TE ARM -O2", include_bytes!("../tests/programs/armv5te_arm_O2.bin"));
    bench::<Armv5te>("ARMv5TE Thumb -O2", include_bytes!("../tests/programs/armv5te_thumb_O2.bin"));
    bench::<Armv4t>("ARMv4T ARM -O0", include_bytes!("../tests/programs/armv4t_arm_O0.bin"));
}
