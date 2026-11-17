//! Tests unitaires du CPU, instruction par instruction.
//!
//! Les blocs `// asm: …` sont générés par `tools/asm.py` à partir du
//! commentaire de chaque ligne : modifier le commentaire puis relancer
//! `python3 tools/asm.py src/cpu/tests.rs`.

use super::*;
use crate::bus::{Bus, Cp15Op};

const MEM_SIZE: usize = 1 << 20;

/// Bus de test : 1 Mo de RAM à plat, répétée sur tout l'espace d'adressage.
struct TestBus {
    mem: Vec<u8>,
    vector_base: u32,
    cp15_writes: Vec<(Cp15Op, u32)>,
    cp15_value: u32,
    hle: bool,
    swis: Vec<u8>,
}

impl TestBus {
    fn new() -> Self {
        TestBus { mem: vec![0; MEM_SIZE], vector_base: 0, cp15_writes: vec![], cp15_value: 0, hle: false, swis: vec![] }
    }
    fn idx(addr: u32) -> usize {
        addr as usize & (MEM_SIZE - 1)
    }
    fn arm(&mut self, addr: u32, code: &[u32]) {
        for (i, w) in code.iter().enumerate() {
            self.write32(addr + 4 * i as u32, *w);
        }
    }
    fn thumb(&mut self, addr: u32, code: &[u16]) {
        for (i, h) in code.iter().enumerate() {
            self.write16(addr + 2 * i as u32, *h);
        }
    }
    fn word(&mut self, addr: u32) -> u32 {
        self.read32(addr)
    }
}

impl Bus for TestBus {
    fn read8(&mut self, a: u32) -> u8 {
        self.mem[Self::idx(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        assert_eq!(a & 1, 0, "read16 non aligné");
        let i = Self::idx(a);
        u16::from_le_bytes([self.mem[i], self.mem[i + 1]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        assert_eq!(a & 3, 0, "read32 non aligné");
        let i = Self::idx(a);
        u32::from_le_bytes(self.mem[i..i + 4].try_into().unwrap())
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.mem[Self::idx(a)] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        assert_eq!(a & 1, 0, "write16 non aligné");
        let i = Self::idx(a);
        self.mem[i..i + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn write32(&mut self, a: u32, v: u32) {
        assert_eq!(a & 3, 0, "write32 non aligné");
        let i = Self::idx(a);
        self.mem[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn exception_base(&mut self) -> u32 {
        self.vector_base
    }
    fn cp15_read(&mut self, _op: Cp15Op) -> u32 {
        self.cp15_value
    }
    fn cp15_write(&mut self, op: Cp15Op, val: u32) {
        self.cp15_writes.push((op, val));
    }
    fn hle_swi(&mut self, num: u8, regs: &mut [u32; 4]) -> bool {
        self.swis.push(num);
        if self.hle {
            regs[0] = regs[0].wrapping_add(1);
        }
        self.hle
    }
}

/// CPU en mode System, interruptions autorisées, PC = 0.
fn cpu<A: Arch>() -> Cpu<A> {
    let mut c = Cpu::<A>::new();
    c.set_cpsr(Mode::System as u32);
    c
}

fn steps<A: Arch>(c: &mut Cpu<A>, bus: &mut TestBus, n: usize) {
    for _ in 0..n {
        c.step(bus);
    }
}

fn run_to<A: Arch>(c: &mut Cpu<A>, bus: &mut TestBus, pc: u32) {
    let n = c.run_until(bus, 10_000, |c| c.pc() == pc);
    assert!(n < 10_000, "PC {pc:#X} jamais atteint, bloqué en {:#X}", c.pc());
}

fn flags<A: Arch>(c: &Cpu<A>) -> &'static str {
    const NAMES: [&str; 16] = ["----", "---V", "--C-", "--CV", "-Z--", "-Z-V", "-ZC-", "-ZCV",
        "N---", "N--V", "N-C-", "N-CV", "NZ--", "NZ-V", "NZC-", "NZCV"];
    NAMES[(c.cpsr() >> 28) as usize]
}

// ======================================================================
// Pipeline : la valeur de R15
// ======================================================================

#[test]
fn pc_reads_as_plus_8_in_arm() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE1A0000F, // mov r0, pc
        0xE28F1004, // add r1, pc, #4
        0xE51F2004, // ldr r2, [pc, #-4]
        0xCAFEBABE, // .word 0xCAFEBABE
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 3);
    assert_eq!(c.reg(0), 8);
    assert_eq!(c.reg(1), 16);
    assert_eq!(c.reg(2), 0xCAFE_BABE, "literal pool : l'ancien bug du PC+4");
    assert_eq!(c.pc(), 12);
}

#[test]
fn pc_reads_as_plus_12_with_register_shift_and_str() {
    let mut bus = TestBus::new();
    bus.arm(0, &[
        0xE3A01000, // mov r1, #0
        0xE08F0111, // add r0, pc, r1, lsl r1   (PC lu à +12)
        0xE3A02C01, // mov r2, #0x100
        0xE582F000, // str pc, [r2]             (stocke PC+12)
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 4);
    assert_eq!(c.reg(0), 4 + 12);
    assert_eq!(bus.word(0x100), 12 + 12);
}

#[test]
fn pc_reads_as_plus_4_in_thumb() {
    let mut bus = TestBus::new();
    bus.thumb(0, // asm: thumb
    &[
        0x46C0, // mov r8, r8
        0x4800, // ldr r0, [pc, #0]
        0x5678, 0x1234, // .word 0x12345678
    ]);
    bus.thumb(0x40, // asm: thumb
    &[
        0x4679, // mov r1, pc
        0xA202, // .short 0xA202 @ add r2, pc, #8
    ]);
    let mut c = cpu::<Armv5te>();
    c.jump(0, true);
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.reg(0), 0x1234_5678, "LDR PC-relatif : (PC+4) & !3");
    c.jump(0x40, true);
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.reg(1), 0x44);
    assert_eq!(c.reg(2), 0x44 + 8, "ADD Rd, PC : (PC+4) & !3");
}

// ======================================================================
// Drapeaux et barrel shifter
// ======================================================================

#[test]
fn add_sub_flags() {
    let cases: &[(u32, &str, u32, &str)] = &[
        // (instruction, r0 initial → résultat r1, drapeaux)
        (0xE2501001, "subs r1, r0, #1 ; r0=1", 0, "-ZC-"),
        (0xE2501001, "subs r1, r0, #1 ; r0=0", 0xFFFF_FFFF, "N---"),
        (0xE2901001, "adds r1, r0, #1 ; r0=0x7FFFFFFF", 0x8000_0000, "N--V"),
        (0xE2901001, "adds r1, r0, #1 ; r0=0xFFFFFFFF", 0, "-ZC-"),
        (0xE2701000, "rsbs r1, r0, #0 ; r0=0x80000000", 0x8000_0000, "N--V"),
    ];
    let inputs = [1u32, 0, 0x7FFF_FFFF, 0xFFFF_FFFF, 0x8000_0000];
    for (&(insn, what, expect, f), &r0) in cases.iter().zip(&inputs) {
        let mut bus = TestBus::new();
        bus.arm(0, &[insn]);
        let mut c = cpu::<Armv5te>();
        c.set_reg(0, r0);
        c.step(&mut bus);
        assert_eq!(c.reg(1), expect, "{what}");
        assert_eq!(flags(&c), f, "{what}");
    }
}

#[test]
fn carry_chain_adc_sbc_rsc() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE3E00000, // mvn r0, #0
        0xE2901001, // adds r1, r0, #1
        0xE2A12005, // adc r2, r1, #5
        0xE2C23001, // sbc r3, r2, #1
        0xE2514001, // subs r4, r1, #1
        0xE2C25001, // sbc r5, r2, #1
        0xE2E2600A, // rsc r6, r2, #10
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 7);
    assert_eq!(c.reg(2), 6, "ADC avec C=1");
    assert_eq!(c.reg(3), 5, "SBC avec C=1 : pas d'emprunt");
    assert_eq!(c.reg(5), 4, "SBC avec C=0 : emprunt");
    assert_eq!(c.reg(6), 3, "RSC avec C=0");
}

#[test]
fn immediate_shift_special_cases() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE3A01102, // mov r1, #0x80000000
        0xE1B00021, // movs r0, r1, lsr #32
        0xE1B02041, // movs r2, r1, asr #32
        0xE3A03003, // mov r3, #3
        0xE328F202, // msr cpsr_f, #0x20000000
        0xE1B04063, // movs r4, r3, rrx
        0xE3A05080, // mov r5, #0x80
        0xE1A060C5, // mov r6, r5, asr #1
        0xE3A07008, // mov r7, #8
        0xE1A081E7, // mov r8, r7, ror #3
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(0), flags(&c)), (0, "-ZC-"), "LSR #32");
    steps(&mut c, &mut bus, 1);
    assert_eq!((c.reg(2), flags(&c)), (0xFFFF_FFFF, "N-C-"), "ASR #32");
    steps(&mut c, &mut bus, 3);
    assert_eq!((c.reg(4), flags(&c)), (0x8000_0001, "N-C-"), "RRX");
    steps(&mut c, &mut bus, 4);
    // bits 7-4 = 1100 et 1110 : décodés à tort comme LDRH/STRH avant.
    assert_eq!(c.reg(6), 0x40, "ASR #1 (ancien mauvais décodage)");
    assert_eq!(c.reg(8), 1, "ROR #3 (ancien mauvais décodage)");
}

#[test]
fn register_shift_special_cases() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE3A01001, // mov r1, #1
        0xE3A02020, // mov r2, #32
        0xE1B00211, // movs r0, r1, lsl r2
        0xE3A02021, // mov r2, #33
        0xE1B03211, // movs r3, r1, lsl r2
        0xE3A04102, // mov r4, #0x80000000
        0xE3A02020, // mov r2, #32
        0xE1B05274, // movs r5, r4, ror r2
        0xE3A02000, // mov r2, #0
        0xE1B06234, // movs r6, r4, lsr r2
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 3);
    assert_eq!((c.reg(0), flags(&c)), (0, "-ZC-"), "LSL par 32");
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(3), flags(&c)), (0, "-Z--"), "LSL par 33");
    steps(&mut c, &mut bus, 3);
    assert_eq!((c.reg(5), flags(&c)), (0x8000_0000, "N-C-"), "ROR par 32");
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(6), flags(&c)), (0x8000_0000, "N-C-"), "LSR par 0 : C inchangé");
}

#[test]
fn condition_codes_match_spec() {
    for nzcv in 0u32..16 {
        let (n, z, c, v) = (nzcv & 8 != 0, nzcv & 4 != 0, nzcv & 2 != 0, nzcv & 1 != 0);
        let expected = [z, !z, c, !c, n, !n, v, !v, c && !z, !c || z, n == v, n != v,
            !z && n == v, z || n != v, true];
        for (cond, &e) in expected.iter().enumerate() {
            assert_eq!(alu::condition(cond as u32, nzcv << 28), e, "cond {cond:X} NZCV {nzcv:04b}");
        }
    }
}

#[test]
fn logical_ops_keep_v_and_set_shifter_carry() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE328F201, // msr cpsr_f, #0x10000000
        0xE3A01003, // mov r1, #3
        0xE01100A1, // ands r0, r1, r1, lsr #1
        0xE3110102, // tst r1, #0x80000000
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 3);
    assert_eq!((c.reg(0), flags(&c)), (1, "--CV"));
    steps(&mut c, &mut bus, 1);
    assert_eq!(flags(&c), "-ZCV", "immédiat avec rotation : C = bit 31");
}

// ======================================================================
// Branchements et changement d'état
// ======================================================================

#[test]
fn branch_and_link() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xEA000000, // b skip
        0xE3A00001, // mov r0, #1
        0xEB000000, // skip: bl func
        0xEA000001, // b end
        0xE3A01002, // func: mov r1, #2
        0xE12FFF1E, // bx lr
        0xE3A02003, // end: mov r2, #3
    ]);
    let mut c = cpu::<Armv5te>();
    run_to(&mut c, &mut bus, 28);
    assert_eq!((c.reg(0), c.reg(1), c.reg(2), c.reg(14)), (0, 2, 3, 12));
}

#[test]
fn backward_branch_loop() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE3A00005, // mov r0, #5
        0xE3A01000, // mov r1, #0
        0xE2811002, // loop: add r1, r1, #2
        0xE2500001, // subs r0, r0, #1
        0x1AFFFFFC, // bne loop
    ]);
    let mut c = cpu::<Armv4t>();
    run_to(&mut c, &mut bus, 20);
    assert_eq!(c.reg(1), 10);
}

#[test]
fn bx_arm_to_thumb_and_back() {
    let mut bus = TestBus::new();
    bus.arm(0, &[
        0xE28F0001, // add r0, pc, #1     → r0 = 9 (Thumb en 8)
        0xE12FFF10, // bx r0
    ]);
    bus.thumb(8, // asm: thumb
    &[
        0x212A, // movs r1, #42
        0x4778, // bx pc
    ]);
    bus.arm(12, // asm: arm
    &[
        0xE3A02007, // mov r2, #7
    ]);
    for v5 in [false, true] {
        let mut c4 = cpu::<Armv4t>();
        let mut c5 = cpu::<Armv5te>();
        if v5 { run_to(&mut c5, &mut bus, 16) } else { run_to(&mut c4, &mut bus, 16) }
        let (r1, r2, t) = if v5 { (c5.reg(1), c5.reg(2), c5.thumb()) } else { (c4.reg(1), c4.reg(2), c4.thumb()) };
        assert_eq!((r1, r2, t), (42, 7, false));
    }
}

#[test]
fn blx_immediate_is_v5_only() {
    let mut bus = TestBus::new();
    bus.arm(0, &[
        0xFB000000, // blx #+10 (H=1)   → Thumb en 10, LR = 4
        0xE3A01006, // mov r1, #6
    ]);
    bus.thumb(10, // asm: thumb
    &[
        0x2005, // movs r0, #5
        0x4770, // bx lr
    ]);
    let mut c = cpu::<Armv5te>();
    run_to(&mut c, &mut bus, 8);
    assert_eq!((c.reg(0), c.reg(1), c.thumb()), (5, 6, false));

    let mut c = cpu::<Armv4t>();
    run_to(&mut c, &mut bus, 8);
    assert_eq!((c.reg(0), c.reg(1)), (0, 6), "condition NV : jamais exécutée sur ARMv4");
}

#[test]
fn thumb_blx_suffix_to_arm() {
    let mut bus = TestBus::new();
    bus.thumb(0, &[
        0xF000, // bl/blx préfixe : LR = PC+4
        0xE802, // blx suffixe : (LR + 4) & !3 = 8
        0x2009, // movs r0, #9
    ]);
    bus.arm(8, &[0xE12FFF1E]); // bx lr
    let mut c = cpu::<Armv5te>();
    c.jump(0, true);
    run_to(&mut c, &mut bus, 6);
    assert_eq!((c.reg(0), c.thumb(), c.reg(14)), (9, true, 5));
}

#[test]
fn pop_pc_interworks_only_on_v5() {
    let mut bus = TestBus::new();
    bus.thumb(0, // asm: thumb
    &[
        0x2020, // movs r0, #0x20
        0xB401, // push {r0}
        0xBD00, // pop {pc}
    ]);
    let mut c5 = cpu::<Armv5te>();
    c5.set_reg(13, 0x8000);
    c5.jump(0, true);
    steps(&mut c5, &mut bus, 3);
    assert_eq!((c5.pc(), c5.thumb()), (0x20, false), "ARMv5 : bit 0 = 0 → ARM");

    let mut c4 = cpu::<Armv4t>();
    c4.set_reg(13, 0x8000);
    c4.jump(0, true);
    steps(&mut c4, &mut bus, 3);
    assert_eq!((c4.pc(), c4.thumb()), (0x20, true), "ARMv4 : reste en Thumb");
    assert_eq!(c4.reg(13), 0x8000);
}

#[test]
fn ldr_pc_interworks_only_on_v5() {
    let mut bus = TestBus::new();
    bus.arm(0, &[0xE5900000 | (15 << 12)]); // ldr pc, [r0]
    bus.write32(0x100, 0x201);
    let mut c5 = cpu::<Armv5te>();
    c5.set_reg(0, 0x100);
    c5.step(&mut bus);
    assert_eq!((c5.pc(), c5.thumb()), (0x200, true));
    let mut c4 = cpu::<Armv4t>();
    c4.set_reg(0, 0x100);
    c4.step(&mut bus);
    assert_eq!((c4.pc(), c4.thumb()), (0x200, false));
}

// ======================================================================
// Transferts mémoire
// ======================================================================

#[test]
fn block_transfer_addressing_modes() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE8A0001E, // stmia r0!, {r1-r4}
        0xE9200006, // stmdb r0!, {r1, r2}
        0xE9900060, // ldmib r0, {r5, r6}
        0xE8100180, // ldmda r0, {r7, r8}
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_reg(0, 0x1000);
    for i in 1..=4 {
        c.set_reg(i, i as u32);
    }
    steps(&mut c, &mut bus, 1);
    assert_eq!((c.reg(0), bus.word(0x1000), bus.word(0x100C)), (0x1010, 1, 4));
    steps(&mut c, &mut bus, 1);
    assert_eq!((c.reg(0), bus.word(0x1008), bus.word(0x100C)), (0x1008, 1, 2));
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(5), c.reg(6)), (2, 0));
    assert_eq!((c.reg(7), c.reg(8)), (2, 1));
}

#[test]
fn ldm_with_base_in_list() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE8B00003, // ldmia r0!, {r0, r1}
        0xE8B3000C, // ldmia r3!, {r2, r3}
    ]);
    bus.write32(0x1000, 0xAA);
    bus.write32(0x1004, 0xBB);
    let run = |v5: bool, bus: &mut TestBus| {
        let (r0, r3) = if v5 {
            let mut c = cpu::<Armv5te>();
            c.set_reg(0, 0x1000);
            c.set_reg(3, 0x1000);
            steps(&mut c, bus, 2);
            (c.reg(0), c.reg(3))
        } else {
            let mut c = cpu::<Armv4t>();
            c.set_reg(0, 0x1000);
            c.set_reg(3, 0x1000);
            steps(&mut c, bus, 2);
            (c.reg(0), c.reg(3))
        };
        (r0, r3)
    };
    assert_eq!(run(false, &mut bus), (0xAA, 0xBB), "ARMv4 : pas de writeback si Rn ∈ liste");
    assert_eq!(run(true, &mut bus), (0x1008, 0xBB), "ARMv5 : writeback si Rn n'est pas le dernier");
}

#[test]
fn stm_with_base_in_list() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE8A10003, // stmia r1!, {r0, r1}
    ]);
    let mut c4 = cpu::<Armv4t>();
    c4.set_reg(1, 0x1000);
    c4.step(&mut bus);
    assert_eq!(bus.word(0x1004), 0x1008, "ARMv4 : nouvelle base si Rn n'est pas le premier");
    let mut c5 = cpu::<Armv5te>();
    c5.set_reg(1, 0x1000);
    c5.step(&mut bus);
    assert_eq!(bus.word(0x1004), 0x1000, "ARMv5 : toujours l'ancienne base");
}

#[test]
fn function_call_with_push_pop() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE3A04055, // mov r4, #0x55
        0xEB000000, // bl func
        0xEAFFFFFE, // end: b end
        0xE92D4010, // func: push {r4, lr}
        0xE3A04001, // mov r4, #1
        0xE2840029, // add r0, r4, #41
        0xE8BD8010, // pop {r4, pc}
    ]);
    let mut c = cpu::<Armv4t>();
    c.set_reg(13, 0x8000);
    run_to(&mut c, &mut bus, 8);
    assert_eq!((c.reg(0), c.reg(4), c.reg(13)), (42, 0x55, 0x8000));
}

#[test]
fn unaligned_loads() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE5901001, // ldr r1, [r0, #1]
        0xE1D020B1, // ldrh r2, [r0, #1]
        0xE1D030F1, // ldrsh r3, [r0, #1]
        0xE1D040D1, // ldrsb r4, [r0, #1]
        0xE1C051B1, // strh r5, [r0, #0x11]
    ]);
    bus.write32(0x1000, 0x1122_8844);
    let mut c5 = cpu::<Armv5te>();
    c5.set_reg(0, 0x1000);
    c5.set_reg(5, 0xBEEF);
    steps(&mut c5, &mut bus, 5);
    assert_eq!(c5.reg(1), 0x4411_2288, "LDR non aligné : rotation");
    assert_eq!(c5.reg(2), 0x8844, "ARMv5 LDRH impair : lecture alignée");
    assert_eq!(c5.reg(3), 0xFFFF_8844);
    assert_eq!(c5.reg(4), 0xFFFF_FF88);
    assert_eq!(bus.word(0x1010), 0xBEEF);

    let mut c4 = cpu::<Armv4t>();
    c4.set_reg(0, 0x1000);
    steps(&mut c4, &mut bus, 3);
    assert_eq!(c4.reg(2), 0x4400_0088, "ARMv4 LDRH impair : rotation de 8");
    assert_eq!(c4.reg(3), 0xFFFF_FF88, "ARMv4 LDRSH impair = LDRSB");
}

#[test]
fn ldrd_strd_and_writeback() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE1C020F8, // strd r2, r3, [r0, #8]
        0xE1E040D8, // ldrd r4, r5, [r0, #8]!
        0xE4106004, // ldr r6, [r0], #-4
        0xE7917108, // ldr r7, [r1, r8, lsl #2]
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_reg(0, 0x1000);
    c.set_reg(1, 0x1000);
    c.set_reg(2, 0x1111_1111);
    c.set_reg(3, 0x2222_2222);
    c.set_reg(8, 3);
    steps(&mut c, &mut bus, 4);
    assert_eq!((bus.word(0x1008), bus.word(0x100C)), (0x1111_1111, 0x2222_2222));
    assert_eq!((c.reg(4), c.reg(5)), (0x1111_1111, 0x2222_2222));
    assert_eq!(c.reg(6), 0x1111_1111, "post-indexé : adresse = base");
    assert_eq!(c.reg(0), 0x1004);
    assert_eq!(c.reg(7), 0x2222_2222);
}

#[test]
fn swap() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE1002091, // swp r2, r1, [r0]
        0xE1403091, // swpb r3, r1, [r0]
    ]);
    bus.write32(0x1000, 5);
    let mut c = cpu::<Armv4t>();
    c.set_reg(0, 0x1000);
    c.set_reg(1, 0x1234_5609);
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.reg(2), 5);
    assert_eq!(c.reg(3), 0x09);
    assert_eq!(bus.word(0x1000), 0x1234_5609);
}

// ======================================================================
// Multiplications
// ======================================================================

#[test]
fn multiplies() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE0020190, // mul r2, r0, r1
        0xE0234190, // mla r3, r0, r1, r4
        0xE0865897, // umull r5, r6, r7, r8
        0xE0CA9890, // smull r9, r10, r0, r8
        0xE0EA9191, // smlal r9, r10, r1, r1
        0xE01B0C90, // muls r11, r0, r12
    ]);
    let mut c = cpu::<Armv4t>();
    c.set_reg(0, (-3i32) as u32);
    c.set_reg(1, 7);
    c.set_reg(4, 100);
    c.set_reg(7, 0xFFFF_FFFF);
    c.set_reg(8, 2);
    c.set_reg(12, 0);
    steps(&mut c, &mut bus, 6);
    assert_eq!(c.reg(2) as i32, -21);
    assert_eq!(c.reg(3), 79);
    assert_eq!((c.reg(5), c.reg(6)), (0xFFFF_FFFE, 1), "UMULL");
    assert_eq!((c.reg(9), c.reg(10)), (43, 0), "SMULL -6 puis SMLAL +49");
    assert_eq!(flags(&c), "-Z--");
}

// ======================================================================
// Modes, PSR et exceptions
// ======================================================================

#[test]
fn mode_switch_banks_sp_and_lr() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE10F0000, // mrs r0, cpsr
        0xE3C0101F, // bic r1, r0, #0x1F
        0xE3811012, // orr r1, r1, #0x12
        0xE121F001, // msr cpsr_c, r1
        0xE3A0DA02, // mov sp, #0x2000
        0xE381101F, // orr r1, r1, #0x1F
        0xE121F001, // msr cpsr_c, r1
        0xE3A0DA03, // mov sp, #0x3000
        0xE121F000, // msr cpsr_c, r0
    ]);
    let mut c = Cpu::<Armv5te>::new(); // Superviseur
    c.set_reg(13, 0x111);
    steps(&mut c, &mut bus, 9);
    assert_eq!(c.mode(), Mode::Supervisor);
    assert_eq!(c.reg(13), 0x111);
    assert_eq!(c.banked_sp(Mode::Irq), 0x2000);
    assert_eq!(c.banked_sp(Mode::System), 0x3000);
    assert_eq!(c.banked_sp(Mode::User), 0x3000, "User et System partagent R13");
}

#[test]
fn fiq_banks_r8_to_r12() {
    let mut c = cpu::<Armv4t>();
    c.set_reg(8, 1);
    c.set_reg(12, 5);
    c.set_cpsr(Mode::Fiq as u32);
    assert_eq!(c.reg(8), 0);
    c.set_reg(8, 99);
    c.set_cpsr(Mode::Irq as u32);
    assert_eq!((c.reg(8), c.reg(12)), (1, 5));
    c.set_cpsr(Mode::Fiq as u32);
    assert_eq!(c.reg(8), 99);
}

#[test]
fn user_mode_cannot_change_mode() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE328F20F, // msr cpsr_f, #0xF0000000
        0xE321F01F, // msr cpsr_c, #0x1F
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_cpsr(Mode::User as u32);
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.mode(), Mode::User);
    assert_eq!(flags(&c), "NZCV");
}

#[test]
fn irq_entry_and_return_arm() {
    let mut bus = TestBus::new();
    bus.arm(0x18, // asm: arm
    &[
        0xE3A05007, // mov r5, #7
        0xE25EF004, // subs pc, lr, #4
    ]);
    bus.arm(0x100, // asm: arm
    &[
        0xE3A00001, // mov r0, #1
        0xE3A01002, // mov r1, #2
    ]);
    let mut c = cpu::<Armv5te>();
    c.jump(0x100, false);
    c.step(&mut bus);
    c.irq_line = true;
    c.step(&mut bus);
    assert_eq!((c.pc(), c.mode(), c.reg(14)), (0x18, Mode::Irq, 0x108));
    assert_eq!(c.spsr(), Some(Mode::System as u32));
    assert_ne!(c.cpsr() & FLAG_I, 0);
    c.irq_line = false;
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.pc(), c.mode()), (0x104, Mode::System));
    assert_eq!(c.cpsr() & FLAG_I, 0);
    c.step(&mut bus);
    assert_eq!((c.reg(0), c.reg(1), c.reg(5)), (1, 2, 7));
}

#[test]
fn irq_entry_and_return_thumb() {
    let mut bus = TestBus::new();
    bus.arm(0x18, &[0xE25EF004]); // subs pc, lr, #4
    bus.thumb(0x200, // asm: thumb
    &[
        0x2001, // movs r0, #1
        0x2102, // movs r1, #2
    ]);
    let mut c = cpu::<Armv4t>();
    c.jump(0x200, true);
    c.step(&mut bus);
    c.irq_line = true;
    c.step(&mut bus);
    assert_eq!((c.reg(14), c.thumb()), (0x206, false));
    c.irq_line = false;
    c.step(&mut bus);
    assert_eq!((c.pc(), c.thumb()), (0x202, true));
    c.step(&mut bus);
    assert_eq!(c.reg(1), 2);
}

#[test]
fn irq_masked_by_i_flag() {
    let mut bus = TestBus::new();
    let mut c = cpu::<Armv5te>();
    c.set_cpsr(Mode::System as u32 | FLAG_I);
    c.irq_line = true;
    c.step(&mut bus);
    assert_eq!(c.pc(), 4);
}

#[test]
fn swi_exception_and_hle() {
    let mut bus = TestBus::new();
    bus.vector_base = 0xFFFF_0000;
    bus.arm(0, // asm: arm
    &[
        0xEF060000, // swi #0x060000
    ]);
    let mut c = cpu::<Armv5te>();
    c.step(&mut bus);
    assert_eq!((c.pc(), c.mode(), c.reg(14)), (0xFFFF_0008, Mode::Supervisor, 4));
    assert_eq!(bus.swis, [6]);

    bus.hle = true;
    let mut c = cpu::<Armv5te>();
    c.set_reg(0, 41);
    c.step(&mut bus);
    assert_eq!((c.pc(), c.mode(), c.reg(0)), (4, Mode::System, 42));

    bus.thumb(0x40, // asm: thumb
    &[
        0xDF0B, // svc #0x0B
    ]);
    let mut c = cpu::<Armv4t>();
    c.jump(0x40, true);
    c.step(&mut bus);
    assert_eq!(bus.swis.last(), Some(&0x0B));
    assert_eq!(c.pc(), 0x42);
}

#[test]
fn undefined_instruction_traps() {
    let mut bus = TestBus::new();
    bus.arm(0, &[0xE7F000F0]); // udf
    let mut c = cpu::<Armv5te>();
    c.step(&mut bus);
    assert_eq!((c.pc(), c.mode(), c.reg(14)), (4, Mode::Undefined, 4));
}

#[test]
fn cp15_access_and_halt() {
    let mut bus = TestBus::new();
    bus.cp15_value = 0x2078;
    bus.arm(0, // asm: arm
    &[
        0xEE111F10, // mrc p15, 0, r1, c1, c0, 0
        0xEE070F90, // mcr p15, 0, r0, c7, c0, 4
    ]);
    let mut c = cpu::<Armv5te>();
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.reg(1), 0x2078);
    assert!(c.halted);
    assert_eq!(bus.cp15_writes[0].0, Cp15Op { opc1: 0, crn: 7, crm: 0, opc2: 4 });
    c.step(&mut bus);
    assert_eq!(c.pc(), 8, "en pause, le CPU n'avance pas");

    let mut c = cpu::<Armv4t>();
    c.step(&mut bus);
    assert_eq!(c.mode(), Mode::Undefined, "pas de CP15 sur l'ARM7");
}

// ======================================================================
// Extensions ARMv5TE
// ======================================================================

#[test]
fn clz_and_saturating_arithmetic() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE16F2F10, // clz r2, r0
        0xE16F3F11, // clz r3, r1
        0xE1064055, // qadd r4, r5, r6
        0xE1267058, // qsub r7, r8, r6
        0xE14A9056, // qdadd r9, r6, r10
        0xE16AB056, // qdsub r11, r6, r10
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_reg(0, 0x0001_0000);
    c.set_reg(1, 0);
    c.set_reg(5, 0x7FFF_FFFF);
    c.set_reg(6, 1);
    c.set_reg(8, 0x8000_0000);
    c.set_reg(10, 0x4000_0000);
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(2), c.reg(3)), (15, 32));
    assert_eq!(c.cpsr() & FLAG_Q, 0);
    steps(&mut c, &mut bus, 1);
    assert_eq!(c.reg(4), 0x7FFF_FFFF);
    assert_ne!(c.cpsr() & FLAG_Q, 0, "saturation → Q");
    steps(&mut c, &mut bus, 3);
    assert_eq!(c.reg(7), 0x8000_0000);
    assert_eq!(c.reg(9), 0x7FFF_FFFF, "1 + sat(2 × 0x40000000)");
    assert_eq!(c.reg(11), 0x8000_0002, "1 - sat(2 × 0x40000000)");

    let mut c = cpu::<Armv4t>();
    c.step(&mut bus);
    assert_eq!(c.mode(), Mode::Undefined, "CLZ indéfini sur ARMv4");
}

#[test]
fn dsp_multiplies() {
    let mut bus = TestBus::new();
    bus.arm(0, // asm: arm
    &[
        0xE1600281, // smulbb r0, r1, r2
        0xE16302E1, // smultt r3, r1, r2
        0xE16402C1, // smulbt r4, r1, r2
        0xE1056281, // smlabb r5, r1, r2, r6
        0xE12702A1, // smulwb r7, r1, r2
        0xE1498281, // smlalbb r8, r9, r1, r2
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_reg(1, 0x0005_FFFE); // haut 5, bas -2
    c.set_reg(2, 0xFFFD_0003); // haut -3, bas 3
    c.set_reg(6, 100);
    steps(&mut c, &mut bus, 6);
    assert_eq!(c.reg(0) as i32, -6);
    assert_eq!(c.reg(3) as i32, -15);
    assert_eq!(c.reg(4) as i32, 6);
    assert_eq!(c.reg(5), 94);
    assert_eq!(c.reg(7), ((0x0005_FFFEi64 * 3) >> 16) as u32);
    assert_eq!((c.reg(8), c.reg(9)), (0xFFFF_FFFA, 0xFFFF_FFFF));
}

// ======================================================================
// Thumb
// ======================================================================

#[test]
fn thumb_alu() {
    let mut bus = TestBus::new();
    bus.thumb(0, // asm: thumb
    &[
        0x200A, // movs r0, #10
        0x2103, // movs r1, #3
        0x1A42, // subs r2, r0, r1
        0x010B, // lsls r3, r1, #4
        0x434B, // muls r3, r1, r3
        0x424C, // negs r4, r1
        0x1064, // asrs r4, r4, #1
        0x43C5, // mvns r5, r0
        0xAE02, // add r6, sp, #8
        0x4680, // mov r8, r0
        0x44C0, // add r8, r8
        0x4647, // mov r7, r8
        0x2121, // movs r1, #33
        0x4088, // lsls r0, r1
        0x32FA, // adds r2, #250
        0x438A, // bics r2, r1
    ]);
    let mut c = cpu::<Armv4t>();
    c.set_reg(13, 0x8000);
    c.jump(0, true);
    steps(&mut c, &mut bus, 12);
    assert_eq!(c.reg(2), 7);
    assert_eq!(c.reg(3), 144);
    assert_eq!(c.reg(4), 0xFFFF_FFFE);
    assert_eq!(c.reg(5), !10);
    assert_eq!(c.reg(6), 0x8008);
    assert_eq!(c.reg(7), 20);
    steps(&mut c, &mut bus, 2);
    assert_eq!((c.reg(0), flags(&c)), (0, "-Z--"), "LSL par registre de 33");
    steps(&mut c, &mut bus, 2);
    assert_eq!(c.reg(2), 257 & !33);
}

#[test]
fn thumb_function_call() {
    let mut bus = TestBus::new();
    bus.thumb(0, // asm: thumb
    &[
        0x2005, // movs r0, #5
        0xF000, 0xF802, // bl func
        0x2201, // movs r2, #1
        0xE7FE, // end: b end
        0xB510, // func: push {r4, lr}
        0x2403, // movs r4, #3
        0x1900, // adds r0, r0, r4
        0xBD10, // pop {r4, pc}
    ]);
    let mut c = cpu::<Armv4t>();
    c.set_reg(4, 0x55);
    c.set_reg(13, 0x8000);
    c.jump(0, true);
    run_to(&mut c, &mut bus, 8);
    assert_eq!((c.reg(0), c.reg(2), c.reg(4), c.reg(13)), (8, 1, 0x55, 0x8000));
}

#[test]
fn thumb_conditional_loop_and_memory() {
    let mut bus = TestBus::new();
    bus.thumb(0, // asm: thumb
    &[
        0x2004, // movs r0, #4
        0x2100, // movs r1, #0
        0x5450, // loop: strb r0, [r2, r1]
        0x3101, // adds r1, #1
        0x3801, // subs r0, #1
        0xD1FB, // bne loop
        0x6813, // ldr r3, [r2]
        0x8854, // ldrh r4, [r2, #2]
        0x2502, // movs r5, #2
        0x5F56, // ldrsh r6, [r2, r5]
        0xC203, // stmia r2!, {r0, r1}
        0xCA20, // ldmia r2!, {r5}
        0xE7FE, // end: b end
    ]);
    let mut c = cpu::<Armv5te>();
    c.set_reg(2, 0x1000);
    bus.write32(0x1008, 0xDEAD);
    c.jump(0, true);
    run_to(&mut c, &mut bus, 24);
    assert_eq!(c.reg(3), 0x0102_0304);
    assert_eq!(c.reg(4), 0x0102);
    assert_eq!(c.reg(6), 0x0102);
    assert_eq!((bus.word(0x1000), bus.word(0x1004)), (0, 4));
    assert_eq!((c.reg(5), c.reg(2)), (0xDEAD, 0x100C));
}

#[test]
fn lut_sanity() {
    use super::arm::{lut_index, ArmOp, ARM_LUT};
    use super::thumb::{ThumbOp, THUMB_LUT};
    let a = |op: u32| ARM_LUT[lut_index(op)];
    assert_eq!(a(0xE0810182), ArmOp::DataProc); // add r0, r1, r2, lsl #3
    assert_eq!(a(0xE1A000C1), ArmOp::DataProc); // mov r0, r1, asr #1
    assert_eq!(a(0xE1C010B0), ArmOp::Halfword); // strh r1, [r0]
    assert_eq!(a(0xE0020091), ArmOp::Multiply); // mul r2, r1, r0
    assert_eq!(a(0xE12FFF1E), ArmOp::Bx);
    assert_eq!(a(0xE10F0000), ArmOp::Mrs);
    assert_eq!(a(0xE8BD8010), ArmOp::BlockTransfer);
    assert_eq!(a(0xEE070F90), ArmOp::CoprocRegister);
    let t = |op: u16| THUMB_LUT[(op >> 6) as usize];
    assert_eq!(t(0x4770), ThumbOp::HiReg); // bx lr
    assert_eq!(t(0xB510), ThumbOp::PushPop);
    assert_eq!(t(0xBD10), ThumbOp::PushPop);
    assert_eq!(t(0xDF01), ThumbOp::Swi);
    assert_eq!(t(0xF000), ThumbOp::BlPrefix);
}
