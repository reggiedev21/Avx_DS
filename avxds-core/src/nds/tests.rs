//! Tests de la machine : carte mémoire, CP15, WRAM, VRAM, IRQ, IPC, math.
//!
//! Les blocs `// asm: …` se régénèrent avec `python3 tools/asm.py src/nds/tests.rs`.

use super::*;
use crate::cpu::Mode;

fn cp15(crn: u8, crm: u8, opc2: u8) -> Cp15Op {
    Cp15Op { opc1: 0, crn, crm, opc2 }
}

fn load_arm(nds: &mut Nds, addr: u32, code: &[u32]) {
    let mut bus = nds.bus9();
    for (i, w) in code.iter().enumerate() {
        bus.write32(addr + 4 * i as u32, *w);
    }
}

// ======================================================================
// Carte mémoire
// ======================================================================

#[test]
fn main_ram_is_mirrored_and_shared() {
    let mut nds = Nds::new();
    nds.bus9().write32(0x0200_0010, 0xDEAD_BEEF);
    assert_eq!(nds.bus9().read32(0x0240_0010), 0xDEAD_BEEF, "miroir tous les 4 Mo");
    assert_eq!(nds.bus7().read32(0x02C0_0010), 0xDEAD_BEEF, "même RAM côté ARM7");
    assert_eq!(nds.bus9().read16(0x0200_0012), 0xDEAD);
    assert_eq!(nds.bus9().read8(0x0200_0011), 0xBE);
}

#[test]
fn arm9_ignores_byte_writes_to_palette_vram_oam() {
    let mut nds = Nds::new();
    nds.bus9().write16(0x0500_0000, 0x1234);
    nds.bus9().write8(0x0500_0000, 0xFF);
    nds.bus9().write8(0x0700_0000, 0xFF);
    assert_eq!(nds.bus9().read16(0x0500_0000), 0x1234);
    assert_eq!(nds.bus9().read16(0x0700_0000), 0);
    assert_eq!(nds.bus9().read16(0x0500_0800), 0x1234, "palette en miroir tous les 2 Ko");
}

#[test]
fn bios_regions_are_read_only() {
    let mut nds = Nds::new();
    nds.load_bios9(&[0x11, 0x22, 0x33, 0x44]);
    nds.load_bios7(&[0xAA, 0xBB]);
    nds.bus9().write32(0xFFFF_0000, 0);
    nds.bus7().write16(0, 0);
    assert_eq!(nds.bus9().read32(0xFFFF_0000), 0x4433_2211);
    assert_eq!(nds.bus7().read16(0), 0xBBAA);
    assert_eq!(nds.bus9().read32(0x0000_0000), 0, "ITCM désactivée au reset : rien en 0");
}

#[test]
fn arm7_wram_and_open_bus() {
    let mut nds = Nds::new();
    nds.bus7().write32(0x0380_0000, 7);
    assert_eq!(nds.bus7().read32(0x0381_0000), 7, "WRAM ARM7 en miroir tous les 64 Ko");
    assert_eq!(nds.bus9().read32(0x0380_0000), 0, "l'ARM9 ne la voit pas");
}

// ======================================================================
// CP15 et TCM
// ======================================================================

#[test]
fn cp15_reset_state_and_ids() {
    let mut nds = Nds::new();
    let mut bus = nds.bus9();
    assert_eq!(bus.cp15_read(cp15(0, 0, 0)), 0x4105_9461);
    assert_eq!(bus.cp15_read(cp15(1, 0, 0)), 0x2078);
    assert_eq!(bus.exception_base(), 0xFFFF_0000, "vecteurs hauts au reset");
    bus.cp15_write(cp15(1, 0, 0), 0x0000_0001);
    assert_eq!(bus.cp15_read(cp15(1, 0, 0)), 0x79, "bits 3-6 toujours à 1");
    assert_eq!(bus.exception_base(), 0);
}

#[test]
fn tcm_configured_like_libnds() {
    let mut nds = Nds::new();
    {
        let mut bus = nds.bus9();
        // DTCM en 0x0B000000, 16 Ko ; ITCM de 32 Mo virtuels ; activation.
        bus.cp15_write(cp15(9, 1, 0), 0x0B00_000A);
        bus.cp15_write(cp15(9, 1, 1), 0x0000_0020);
        bus.cp15_write(cp15(1, 0, 0), 0x0005_0000 | 0x2078);
        bus.write32(0x0B00_0000, 0x1111_1111);
        bus.write32(0x0B00_3FFC, 0x2222_2222);
        bus.write32(0x0100_0004, 0x3333_3333); // ITCM vue à travers un miroir
    }
    assert_eq!(&nds.hw.dtcm[0..4], &0x1111_1111u32.to_le_bytes());
    assert_eq!(&nds.hw.dtcm[0x3FFC..], &0x2222_2222u32.to_le_bytes());
    assert_eq!(nds.bus9().read32(0x0000_0004), 0x3333_3333, "ITCM en 0, miroir tous les 32 Ko");
    assert_eq!(nds.bus9().read32(0x0000_8004), 0x3333_3333);
    assert_eq!(nds.bus9().read32(0x0B00_4000), 0, "hors de la DTCM");
    assert_eq!(nds.bus7().read32(0x0B00_0000), 0, "l'ARM7 ne voit pas les TCM");
}

#[test]
fn dtcm_has_priority_over_main_ram() {
    let mut nds = Nds::new();
    nds.bus9().write32(0x027C_0000, 0xAAAA);
    {
        let mut bus = nds.bus9();
        bus.cp15_write(cp15(9, 1, 0), 0x027C_000A);
        bus.cp15_write(cp15(1, 0, 0), 0x0001_0000 | 0x2078);
        bus.write32(0x027C_0000, 0xBBBB);
    }
    assert_eq!(nds.bus9().read32(0x027C_0000), 0xBBBB);
    assert_eq!(nds.bus7().read32(0x027C_0000), 0xAAAA, "la Main RAM dessous est intacte");
}

#[test]
fn cpu_configures_tcm_through_mcr() {
    let mut nds = Nds::new();
    load_arm(&mut nds, 0x0200_0000, // asm: arm
    &[
        0xE59F001C, // ldr r0, =0x0B00000A
        0xEE090F11, // mcr p15, 0, r0, c9, c1, 0
        0xEE111F10, // mrc p15, 0, r1, c1, c0, 0
        0xE3811801, // orr r1, r1, #0x10000
        0xEE011F10, // mcr p15, 0, r1, c1, c0, 0
        0xE3A0240B, // mov r2, #0x0B000000
        0xE3A0302A, // mov r3, #42
        0xE5823000, // str r3, [r2]
        0xEAFFFFFE, // end: b end
        0x0B00000A, // .pool
    ]);
    nds.arm9.jump(0x0200_0000, false);
    for _ in 0..8 {
        nds.step();
    }
    assert_eq!(nds.hw.dtcm[0], 42);
}

// ======================================================================
// WRAM partagée
// ======================================================================

#[test]
fn wramcnt_modes() {
    let mut nds = Nds::new();
    // Mode 3 (défaut au démarrage) : tout pour l'ARM7.
    nds.bus7().write32(0x0300_0000, 0x77);
    assert_eq!(nds.bus9().read32(0x0300_0000), 0);
    assert_eq!(nds.bus7().read32(0x0300_8000), 0x77, "miroir de 32 Ko");

    // Mode 0 : tout pour l'ARM9, l'ARM7 voit sa propre WRAM à la place.
    nds.bus9().write8(0x0400_0247, 0);
    assert_eq!(nds.bus7().read8(0x0400_0241), 0, "WRAMSTAT");
    assert_eq!(nds.bus9().read32(0x0300_0000), 0x77);
    nds.bus7().write32(0x0380_0000, 0x55);
    assert_eq!(nds.bus7().read32(0x0300_0000), 0x55);

    // Mode 1 : ARM9 = 2e moitié, ARM7 = 1re moitié.
    nds.bus9().write8(0x0400_0247, 1);
    nds.bus9().write32(0x0300_0000, 0x99);
    assert_eq!(nds.hw.shared_wram[0x4000], 0x99);
    assert_eq!(nds.bus7().read32(0x0300_0000), 0x77);
}

// ======================================================================
// VRAM
// ======================================================================

#[test]
fn vram_lcdc_then_engine_a_background() {
    let mut nds = Nds::new();
    nds.bus9().write8(0x0400_0240, 0x80); // A en LCDC
    nds.bus9().write16(0x0680_0002, 0x7C1F);
    assert_eq!(nds.hw.vram.banks[0][2], 0x1F);
    assert_eq!(nds.bus9().read16(0x0600_0002), 0, "pas encore en fond A");

    nds.bus9().write8(0x0400_0240, 0x81); // A en fond du moteur A, offset 0
    assert_eq!(nds.bus9().read16(0x0600_0002), 0x7C1F);
    assert_eq!(nds.bus9().read16(0x0608_0002), 0x7C1F, "zone de 512 Ko en miroir");
    assert_eq!(nds.bus9().read16(0x0680_0002), 0, "plus en LCDC");
    assert_eq!(nds.bus9().read8(0x0400_0240), 0x81);
}

#[test]
fn vram_overlapping_banks_or_on_read() {
    let mut nds = Nds::new();
    nds.bus9().write32(0x0400_0240, 0x8181); // A et B en fond A, offset 0
    nds.bus9().write16(0x0600_0000, 0x0F0F); // écrit dans les deux
    assert_eq!(nds.hw.vram.banks[0][0], 0x0F);
    assert_eq!(nds.hw.vram.banks[1][0], 0x0F);
    nds.hw.vram.banks[1][0] = 0xF0;
    assert_eq!(nds.bus9().read8(0x0600_0000), 0xFF, "OU des deux banques");
}

#[test]
fn vram_c_d_as_arm7_wram() {
    let mut nds = Nds::new();
    nds.bus9().write8(0x0400_0242, 0x82); // C → ARM7, offset 0
    nds.bus9().write8(0x0400_0243, 0x8A); // D → ARM7, offset 1
    assert_eq!(nds.bus7().read8(0x0400_0240), 0b11, "VRAMSTAT");
    nds.bus7().write8(0x0602_0001, 0x5A); // ARM7 : écritures 8 bits permises
    assert_eq!(nds.hw.vram.banks[3][1], 0x5A);
    assert_eq!(nds.bus9().read16(0x0602_0000), 0, "l'ARM9 ne voit plus D");
}

#[test]
fn vram_small_banks_offsets() {
    let mut nds = Nds::new();
    nds.bus9().write8(0x0400_0246, 0x81 | (3 << 3)); // G en fond A, OFS 3 → 0x14000
    nds.bus9().write16(0x0601_4000, 0xBEEF);
    assert_eq!(nds.hw.vram.banks[6][0], 0xEF);
    nds.bus9().write8(0x0400_0249, 0x81); // I en fond B → 0x06208000
    nds.bus9().write16(0x0620_8000, 0x1234);
    assert_eq!(nds.hw.vram.banks[8][0], 0x34);
    nds.bus9().write8(0x0400_0244, 0x83); // E → palette de textures (GPU)
    assert_eq!(nds.hw.vram.mapping[4], Mapping::Gpu(GpuRole::TexturePalette(0)));
}

// ======================================================================
// Interruptions
// ======================================================================

#[test]
fn irq_registers() {
    let mut nds = Nds::new();
    let mut bus = nds.bus9();
    bus.write32(0x0400_0210, 0xFFFF_FFFF);
    assert_eq!(bus.read32(0x0400_0210), 0x003F_3F7F, "bits de IE inexistants ignorés");
    bus.hw.irq9.request(irq_bits::VBLANK | irq_bits::TIMER0);
    assert!(!bus.hw.irq9.line(), "IME = 0");
    bus.write16(0x0400_0208, 1);
    assert!(bus.hw.irq9.line());
    bus.write32(0x0400_0214, irq_bits::VBLANK); // acquittement : 1 = effacer
    assert_eq!(bus.read32(0x0400_0214), irq_bits::TIMER0);
    assert!(!nds.hw.irq7.line(), "contrôleurs séparés");
}

#[test]
fn irq_goes_through_bios_vector() {
    let mut nds = Nds::new();
    // Mini BIOS : vecteur IRQ en 0xFFFF0018 → handler qui note et revient.
    let mut bios = vec![0u8; 0x40];
    let code: [u32; 3] = [
        0xE3A05001, // 0x18 : mov r5, #1
        0xE25EF004, // 0x1C : subs pc, lr, #4
        0,
    ];
    for (i, w) in code.iter().enumerate() {
        bios[0x18 + 4 * i..0x1C + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    nds.load_bios9(&bios);
    load_arm(&mut nds, 0x0200_0000, &[0xE3A00001, 0xE3A01002, 0xEAFFFFFE]); // mov r0,#1 ; mov r1,#2 ; b .
    nds.arm9.set_cpsr(Mode::System as u32);
    nds.arm9.jump(0x0200_0000, false);
    nds.hw.irq9.ime = true;
    nds.hw.irq9.ie = irq_bits::VBLANK;
    nds.step();
    nds.hw.irq9.request(irq_bits::VBLANK);
    nds.step();
    assert_eq!((nds.arm9.pc(), nds.arm9.mode()), (0xFFFF_0018, Mode::Irq));
    nds.hw.irq9.if_ = 0; // le jeu acquitterait IF dans son handler
    nds.step();
    nds.step();
    assert_eq!((nds.arm9.pc(), nds.arm9.reg(5)), (0x0200_0004, 1));
}

#[test]
fn halt_wakes_on_ie_and_if_even_without_ime() {
    let mut nds = Nds::new();
    load_arm(&mut nds, 0x0200_0000, &[
        0xEE070F90, // mcr p15, 0, r0, c7, c0, 4  (wait for interrupt)
        0xE3A00007, // mov r0, #7
    ]);
    nds.arm9.set_cpsr(Mode::System as u32);
    nds.arm9.jump(0x0200_0000, false);
    nds.step();
    assert!(nds.arm9.halted);
    nds.step();
    assert!(nds.arm9.halted);
    nds.hw.irq9.ie = irq_bits::TIMER1;
    nds.hw.irq9.request(irq_bits::TIMER1);
    nds.step();
    assert!(!nds.arm9.halted);
    assert_eq!(nds.arm9.reg(0), 7, "IME = 0 : réveil sans IRQ");
}

#[test]
fn arm7_haltcnt() {
    let mut nds = Nds::new();
    // Programme ARM7 en WRAM : strb r0, [r1] avec r1 = 0x04000301.
    {
        let mut bus = nds.bus7();
        bus.write32(0x0380_0000, 0xE5C10000); // strb r0, [r1]
        bus.write32(0x0380_0004, 0xE3A02005); // mov r2, #5
    }
    nds.arm7.set_cpsr(Mode::System as u32);
    nds.arm7.set_reg(0, 0x80);
    nds.arm7.set_reg(1, 0x0400_0301);
    nds.arm7.jump(0x0380_0000, false);
    for _ in 0..4 {
        nds.step();
    }
    assert!(nds.arm7.halted);
    assert_eq!(nds.arm7.reg(2), 0);
}

// ======================================================================
// IPCSYNC
// ======================================================================

#[test]
fn ipcsync_exchange_and_irq() {
    let mut nds = Nds::new();
    nds.bus9().write16(0x0400_0180, 0x0500); // ARM9 sort 5
    assert_eq!(nds.bus7().read16(0x0400_0180) & 0xF, 5);
    nds.bus7().write16(0x0400_0180, 0x4300); // ARM7 sort 3 et accepte l'IRQ
    assert_eq!(nds.bus9().read16(0x0400_0180), 0x0503);
    nds.bus9().write16(0x0400_0180, 0x2500); // ARM9 demande une IRQ à l'ARM7
    assert_eq!(nds.hw.irq7.if_, irq_bits::IPC_SYNC);
    assert_eq!(nds.hw.irq9.if_, 0);
    assert_eq!(nds.bus9().read16(0x0400_0180) & 0x2000, 0, "bit 13 en écriture seule");
}

// ======================================================================
// Division et racine carrée
// ======================================================================

fn div(nds: &mut Nds, mode: u32, n: u64, d: u64) -> (i64, i64, bool) {
    let mut bus = nds.bus9();
    bus.write16(0x0400_0280, mode as u16);
    bus.write32(0x0400_0290, n as u32);
    bus.write32(0x0400_0294, (n >> 32) as u32);
    bus.write32(0x0400_0298, d as u32);
    bus.write32(0x0400_029C, (d >> 32) as u32);
    let q = bus.read32(0x0400_02A0) as u64 | (bus.read32(0x0400_02A4) as u64) << 32;
    let r = bus.read32(0x0400_02A8) as u64 | (bus.read32(0x0400_02AC) as u64) << 32;
    let dz = bus.read16(0x0400_0280) & 0x4000 != 0;
    (q as i64, r as i64, dz)
}

#[test]
fn hardware_divider() {
    let mut nds = Nds::new();
    assert_eq!(div(&mut nds, 0, (-7i64) as u64, 2), (-3, -1, false), "32/32 signé");
    assert_eq!(div(&mut nds, 1, 1 << 40, 3), ((1i64 << 40) / 3, (1i64 << 40) % 3, false), "64/32");
    assert_eq!(div(&mut nds, 2, 1 << 62, 1 << 33), (1 << 29, 0, false), "64/64");
    let (q, r, dz) = div(&mut nds, 0, 5, 0);
    assert_eq!((q as u64, r, dz), (0x0000_0000_FFFF_FFFF, 5, true), "÷0 en 32 bits : haut inversé");
    let (q, _, _) = div(&mut nds, 2, i64::MIN as u64, (-1i64) as u64);
    assert_eq!(q, i64::MIN, "débordement : pas de panique");
}

#[test]
fn hardware_square_root() {
    let mut nds = Nds::new();
    let mut bus = nds.bus9();
    bus.write32(0x0400_02B8, 1_000_000);
    assert_eq!(bus.read32(0x0400_02B4), 1000);
    bus.write16(0x0400_02B0, 1); // 64 bits
    bus.write32(0x0400_02BC, 0xFFFF_FFFF);
    bus.write32(0x0400_02B8, 0xFFFF_FFFF);
    assert_eq!(bus.read32(0x0400_02B4), 0xFFFF_FFFF);
}

#[test]
fn unhandled_io_is_recorded() {
    let mut nds = Nds::new();
    nds.bus9().read32(0x0400_00B0); // DMA0SAD : pas encore émulé
    nds.bus7().write16(0x0400_0504, 0x200);
    let seen: Vec<_> = nds.hw.unhandled_io.iter().copied().collect();
    assert_eq!(seen, [(CpuId::Arm9, 0x0400_00B0), (CpuId::Arm7, 0x0400_0504)]);
}

// ======================================================================
// Scheduler
// ======================================================================

#[test]
fn scheduler_order_and_cancel() {
    let mut s = Scheduler::default();
    assert_eq!(s.next_time(), u64::MAX);
    s.schedule(Event::LineEnd, 100);
    s.schedule(Event::Timer(CpuId::Arm7, 2), 50);
    s.schedule(Event::HBlank, 75);
    assert_eq!(s.next_time(), 50);
    assert_eq!(s.pop_due(49), None);
    assert_eq!(s.pop_due(80), Some((Event::Timer(CpuId::Arm7, 2), 50)));
    assert_eq!(s.pop_due(80), Some((Event::HBlank, 75)));
    assert_eq!(s.pop_due(80), None);
    s.cancel(Event::LineEnd);
    assert_eq!(s.next_time(), u64::MAX);
}

/// Machine dont les deux CPU dorment : le temps avance par sauts.
fn idle_nds() -> Nds {
    let mut nds = Nds::new();
    nds.arm9.halted = true;
    nds.arm7.halted = true;
    nds
}

#[test]
fn frame_cadence() {
    let mut nds = idle_nds();
    nds.run_frame();
    assert_eq!((nds.hw.lcd.vcount, nds.hw.lcd.frames), (192, 1));
    assert_eq!(nds.hw.lcd.line_start, 192 * LINE_CYCLES);
    for _ in 0..59 {
        nds.run_frame();
    }
    assert_eq!(nds.hw.lcd.frames, 60);
    assert_eq!(nds.hw.lcd.line_start, 192 * LINE_CYCLES + 59 * FRAME_CYCLES);
    assert_eq!(FRAME_CYCLES, 560_190 * 2, "1 120 380 cycles ARM9 par image");
    assert!(nds.arm9.instructions == 0 && nds.arm7.instructions == 0, "aucune instruction exécutée");
}

fn dispstat9(nds: &mut Nds) -> u16 {
    nds.bus9().read16(0x0400_0004)
}

#[test]
fn dispstat_flags_and_vcount() {
    let mut nds = idle_nds();
    nds.run_until(100);
    assert_eq!(dispstat9(&mut nds) & 7, 0);
    nds.run_until(lcd::HBLANK_START);
    assert_eq!(dispstat9(&mut nds) & 7, 0b010, "HBlank");
    nds.run_until(LINE_CYCLES);
    assert_eq!(nds.bus9().read16(0x0400_0006), 1, "VCOUNT");
    assert_eq!(dispstat9(&mut nds) & 2, 0, "HBlank effacé en début de ligne");
    nds.run_until(192 * LINE_CYCLES);
    assert_eq!(dispstat9(&mut nds) & 1, 1, "VBlank à la ligne 192");
    nds.run_until(261 * LINE_CYCLES);
    assert_eq!(dispstat9(&mut nds) & 1, 1, "encore VBlank en 261");
    nds.run_until(262 * LINE_CYCLES);
    assert_eq!(dispstat9(&mut nds) & 1, 0, "plus de VBlank en 262");
    nds.run_until(FRAME_CYCLES);
    assert_eq!(nds.bus7().read16(0x0400_0006), 0, "retour à la ligne 0");
}

#[test]
fn vblank_irq_per_cpu() {
    let mut nds = idle_nds();
    nds.bus9().write16(0x0400_0004, 1 << 3); // ARM9 : IRQ VBlank
    nds.run_frame();
    assert_eq!(nds.hw.irq9.if_ & irq_bits::VBLANK, irq_bits::VBLANK);
    assert_eq!(nds.hw.irq7.if_, 0, "l'ARM7 n'a rien autorisé");
    assert_eq!(dispstat9(&mut nds) & 0xFFF8, 1 << 3, "drapeaux en lecture seule");
}

#[test]
fn vcount_match_with_9_bit_lyc() {
    let mut nds = idle_nds();
    nds.bus7().write16(0x0400_0004, (100 << 8) | (1 << 5)); // ARM7 : ligne 100
    nds.bus9().write16(0x0400_0004, (4 << 8) | 0x80 | (1 << 5)); // ARM9 : ligne 260
    nds.run_until(100 * LINE_CYCLES);
    assert_eq!(nds.hw.irq7.if_, irq_bits::VCOUNT);
    assert_eq!(nds.bus7().read16(0x0400_0004) & 4, 4);
    assert_eq!(nds.hw.irq9.if_, 0);
    nds.run_until(101 * LINE_CYCLES);
    assert_eq!(nds.bus7().read16(0x0400_0004) & 4, 0, "drapeau effacé à la ligne suivante");
    nds.run_until(260 * LINE_CYCLES);
    assert_eq!(nds.hw.irq9.if_, irq_bits::VCOUNT, "LYC sur 9 bits");
}

#[test]
fn halted_cpu_wakes_on_vblank_without_busy_loop() {
    let mut nds = Nds::new();
    nds.hw.irq9.ie = irq_bits::VBLANK;
    nds.bus9().write16(0x0400_0004, 1 << 3);
    nds.arm9.halted = true;
    nds.arm7.halted = true;
    nds.run_until(192 * LINE_CYCLES + 10);
    assert!(!nds.arm9.halted, "réveillé par IE & IF");
    assert!(nds.arm9.instructions < 20, "pas d'attente active : {}", nds.arm9.instructions);
}

// ======================================================================
// Timers
// ======================================================================

fn timer_write(nds: &mut Nds, cpu: CpuId, n: u32, reload: u16, control: u16) {
    let addr = 0x0400_0100 + 4 * n;
    let v = reload as u32 | (control as u32) << 16;
    match cpu {
        CpuId::Arm9 => nds.bus9().write32(addr, v),
        CpuId::Arm7 => nds.bus7().write32(addr, v),
    }
}

#[test]
fn timer_counts_overflows_and_raises_irq() {
    let mut nds = idle_nds();
    nds.run_until(1000);
    timer_write(&mut nds, CpuId::Arm9, 0, 0xFFF0, 0xC0); // F/1, IRQ, activé
    let start = nds.now();
    nds.run_until(start + 10 * BUS_CYCLE);
    assert_eq!(nds.bus9().read16(0x0400_0100), 0xFFFA, "compteur paresseux");
    assert_eq!(nds.hw.irq9.if_, 0);
    nds.run_until(start + 16 * BUS_CYCLE);
    assert_eq!(nds.hw.irq9.if_, irq_bits::TIMER0);
    assert_eq!(nds.bus9().read16(0x0400_0100), 0xFFF0, "rechargé");
    nds.run_until(start + 32 * BUS_CYCLE);
    assert_eq!(nds.hw.timers[0].t[0].start, start + 32 * BUS_CYCLE, "2e débordement à l'heure");
    assert_eq!(nds.hw.irq7.if_, 0, "timers séparés");
}

#[test]
fn timer_prescaler_schedules_far_overflow() {
    let mut nds = idle_nds();
    timer_write(&mut nds, CpuId::Arm7, 3, 0, 0x83); // F/1024, sans IRQ
    let expected = 0x1_0000 * 1024 * BUS_CYCLE;
    assert_eq!(nds.hw.sched.time_of(Event::Timer(CpuId::Arm7, 3)), Some(expected));
    nds.run_until(1024 * BUS_CYCLE * 5);
    assert_eq!(nds.bus7().read16(0x0400_010C), 5);
}

#[test]
fn timer_cascade() {
    let mut nds = idle_nds();
    timer_write(&mut nds, CpuId::Arm9, 1, 0xFFFE, 0xC4); // cascade, IRQ
    timer_write(&mut nds, CpuId::Arm9, 0, 0xFFFF, 0x80); // déborde à chaque tic
    let start = nds.now();
    assert_eq!(nds.hw.sched.time_of(Event::Timer(CpuId::Arm9, 1)), None, "pas d'événement en cascade");
    nds.run_until(start + BUS_CYCLE);
    assert_eq!(nds.bus9().read16(0x0400_0104), 0xFFFF);
    assert_eq!(nds.hw.irq9.if_, 0);
    nds.run_until(start + 2 * BUS_CYCLE);
    assert_eq!(nds.hw.irq9.if_, irq_bits::TIMER1);
    assert_eq!(nds.bus9().read16(0x0400_0104), 0xFFFE);
}

#[test]
fn timer_disable_freezes_and_reenable_reloads() {
    let mut nds = idle_nds();
    timer_write(&mut nds, CpuId::Arm9, 2, 100, 0x80);
    nds.run_until(nds.now() + 7 * BUS_CYCLE);
    nds.bus9().write16(0x0400_010A, 0x00); // arrêt
    nds.run_until(nds.now() + 50 * BUS_CYCLE);
    assert_eq!(nds.bus9().read16(0x0400_0108), 107, "valeur figée");
    assert_eq!(nds.hw.sched.time_of(Event::Timer(CpuId::Arm9, 2)), None);
    nds.bus9().write16(0x0400_010A, 0x80); // relance : repart du rechargement
    assert_eq!(nds.bus9().read16(0x0400_0108), 100);
}

// ======================================================================
// FIFO IPC
// ======================================================================

#[test]
fn ipc_fifo_transfer_irqs_and_errors() {
    let mut nds = Nds::new();
    nds.bus9().write16(0x0400_0184, 0x8000 | 0x4); // ARM9 : FIFO + IRQ « envoi vide »
    assert_eq!(nds.hw.irq9.if_, irq_bits::IPC_SEND_EMPTY, "autoriser l'IRQ quand c'est déjà vide");
    nds.hw.irq9.if_ = 0;
    nds.bus7().write16(0x0400_0184, 0x8000 | 0x400); // ARM7 : FIFO + IRQ « réception »
    for v in [10, 20, 30] {
        nds.bus9().write32(0x0400_0188, v);
    }
    assert_eq!(nds.hw.irq7.if_, irq_bits::IPC_RECV_NOT_EMPTY);
    assert_eq!(nds.bus7().read16(0x0400_0184) & 0x0300, 0, "ni vide ni pleine");
    assert_eq!(nds.bus9().read16(0x0400_0184) & 0x0003, 0);
    let got: Vec<u32> = (0..3).map(|_| nds.bus7().read32(0x0410_0000)).collect();
    assert_eq!(got, [10, 20, 30]);
    assert_eq!(nds.hw.irq9.if_, irq_bits::IPC_SEND_EMPTY, "l'émetteur est prévenu");
    assert_eq!(nds.bus7().read32(0x0410_0000), 30, "vide : relit le dernier mot");
    assert_ne!(nds.bus7().read16(0x0400_0184) & 0x4000, 0, "et lève l'erreur");
    nds.bus7().write16(0x0400_0184, 0xC400);
    assert_eq!(nds.bus7().read16(0x0400_0184) & 0x4000, 0, "erreur acquittée");
    for v in 0..17 {
        nds.bus9().write32(0x0400_0188, v);
    }
    let cnt = nds.bus9().read16(0x0400_0184);
    assert_eq!(cnt & 0x4002, 0x4002, "pleine, et erreur au 17e mot");
    nds.bus9().write16(0x0400_0184, 0x8008 | 0x4); // vider la file d'envoi
    assert_eq!(nds.bus7().read16(0x0400_0184) & 0x100, 0x100, "l'ARM7 voit une file vide");
}

#[test]
fn ipc_fifo_disabled_ignores_writes() {
    let mut nds = Nds::new();
    nds.bus9().write32(0x0400_0188, 1);
    assert!(nds.hw.fifo.queue[0].is_empty());
}

#[test]
fn arm9_program_polls_three_vblanks() {
    let mut nds = Nds::new();
    load_arm(&mut nds, 0x0200_0000, // asm: arm
    &[
        0xE3A04301, // mov r4, #0x04000000
        0xE3A05000, // mov r5, #0
        0xE1D400B4, // wait_end: ldrh r0, [r4, #4]
        0xE3100001, // tst r0, #1
        0x1AFFFFFC, // bne wait_end
        0xE1D400B4, // wait_start: ldrh r0, [r4, #4]
        0xE3100001, // tst r0, #1
        0x0AFFFFFC, // beq wait_start
        0xE2855001, // add r5, r5, #1
        0xE3550003, // cmp r5, #3
        0x1AFFFFF6, // bne wait_end
        0xEAFFFFFE, // done: b done
    ]);
    nds.arm9.jump(0x0200_0000, false);
    nds.arm7.halted = true;
    for _ in 0..3 {
        nds.run_frame();
    }
    assert_eq!(nds.arm9.reg(5), 2, "le 3e VBlank vient juste de commencer");
    nds.run_until(nds.now() + 200);
    assert_eq!(nds.arm9.reg(5), 3);
    assert_eq!(nds.arm9.pc(), 0x0200_002C);
    let frame = nds.now() as f64 / FRAME_CYCLES as f64;
    assert!((2.68..2.75).contains(&frame), "≈ 2 images + 192/263 : {frame:.3}");
}
