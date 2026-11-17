//! Mesure grossière du débit de l'interpréteur : exécute en boucle un
//! programme de test et affiche des millions d'instructions par seconde.
//! `cargo run --release --example bench`
use avxds_core::{Arch, Armv4t, Armv5te, Bus, Cpu};
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

fn main() {
    bench::<Armv5te>("ARMv5TE ARM -O2", include_bytes!("../tests/programs/armv5te_arm_O2.bin"));
    bench::<Armv5te>("ARMv5TE Thumb -O2", include_bytes!("../tests/programs/armv5te_thumb_O2.bin"));
    bench::<Armv4t>("ARMv4T ARM -O0", include_bytes!("../tests/programs/armv4t_arm_O0.bin"));
}
