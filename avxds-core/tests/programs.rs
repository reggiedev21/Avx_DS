//! Tests d'intégration : de vrais programmes C compilés par clang pour
//! ARMv4T et ARMv5TE, en ARM et en Thumb, en -O0 et -O2.
//!
//! Les binaires et `expected.rs` sont produits par `tools/build_programs.sh`
//! (les valeurs attendues viennent du même code compilé nativement).

use avxds_core::{Arch, Armv4t, Armv5te, Bus, Cpu};

mod expected {
    include!("programs/expected.rs");
}
use expected::EXPECTED;

const MEM_SIZE: usize = 1 << 20;
const RESULTS: usize = 0x80000;

/// 1 Mo de RAM à plat. SWI 0xFF signale la fin du programme.
struct FlatBus {
    mem: Vec<u8>,
    done: bool,
}

impl FlatBus {
    fn new(program: &[u8]) -> Self {
        let mut mem = vec![0; MEM_SIZE];
        mem[..program.len()].copy_from_slice(program);
        FlatBus { mem, done: false }
    }
    fn i(a: u32) -> usize {
        a as usize & (MEM_SIZE - 1)
    }
    fn result(&self, n: usize) -> u32 {
        let o = RESULTS + 4 * n;
        u32::from_le_bytes(self.mem[o..o + 4].try_into().unwrap())
    }
}

impl Bus for FlatBus {
    fn read8(&mut self, a: u32) -> u8 {
        self.mem[Self::i(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let i = Self::i(a);
        u16::from_le_bytes([self.mem[i], self.mem[i + 1]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let i = Self::i(a);
        u32::from_le_bytes(self.mem[i..i + 4].try_into().unwrap())
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.mem[Self::i(a)] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let i = Self::i(a);
        self.mem[i..i + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn write32(&mut self, a: u32, v: u32) {
        let i = Self::i(a);
        self.mem[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn hle_swi(&mut self, num: u8, _regs: &mut [u32; 4]) -> bool {
        if num == 0xFF {
            self.done = true;
            return true;
        }
        false
    }
}

const NAMES: [&str; 18] = [
    "fib_iter", "fib_rec", "sort", "crc32", "div", "mul64", "switch", "struct",
    "bits", "fnptr", "memops", "interwork", "u64_shifts", "clamp", "strings",
    "ackermann", "sign_ext", "fin",
];

fn run<A: Arch>(name: &str, program: &[u8]) {
    let mut bus = FlatBus::new(program);
    let mut cpu = Cpu::<A>::new();
    cpu.reset(&mut bus);
    let mut steps = 0u64;
    while !bus.done {
        cpu.step(&mut bus);
        steps += 1;
        assert!(steps < 50_000_000, "{name} : ne termine pas\n{cpu:?}");
    }
    let mut errors = Vec::new();
    for (n, &expected) in EXPECTED.iter().enumerate() {
        let got = bus.result(n);
        if got != expected {
            errors.push(format!("  {:<11} attendu {expected:#010X}, obtenu {got:#010X}", NAMES[n]));
        }
    }
    assert!(errors.is_empty(), "{name} ({steps} instructions) :\n{}", errors.join("\n"));
}

macro_rules! program_tests {
    ($($test:ident : $arch:ty, $file:literal;)*) => {$(
        #[test]
        fn $test() {
            run::<$arch>($file, include_bytes!(concat!("programs/", $file, ".bin")));
        }
    )*};
}

program_tests! {
    armv4t_arm_o0: Armv4t, "armv4t_arm_O0";
    armv4t_arm_o2: Armv4t, "armv4t_arm_O2";
    armv4t_thumb_o0: Armv4t, "armv4t_thumb_O0";
    armv4t_thumb_o2: Armv4t, "armv4t_thumb_O2";
    armv5te_arm_o0: Armv5te, "armv5te_arm_O0";
    armv5te_arm_o2: Armv5te, "armv5te_arm_O2";
    armv5te_thumb_o0: Armv5te, "armv5te_thumb_O0";
    armv5te_thumb_o2: Armv5te, "armv5te_thumb_O2";
}
