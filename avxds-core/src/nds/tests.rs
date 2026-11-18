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
    nds.bus9().read32(0x0400_0FF0); // adresse qui n'existe pas sur DS
    nds.bus7().write16(0x0400_0FF4, 0x200);
    let seen: Vec<_> = nds.hw.unhandled_io.iter().copied().collect();
    assert_eq!(seen, [(CpuId::Arm9, 0x0400_0FF0), (CpuId::Arm7, 0x0400_0FF4)]);
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

// ======================================================================
// En-tête, CRC16, HLE des SWI
// ======================================================================

#[test]
fn crc16_matches_reference_values() {
    assert_eq!(crc16(0xFFFF, *b"123456789"), 0x4B37, "CRC-16/MODBUS");
    assert_eq!(crc16(0, []), 0);
}

fn swi(nds: &mut Nds, num: u8, regs: [u32; 4]) -> (hle_swi::Swi, [u32; 4]) {
    let mut r = regs;
    let res = hle_swi::swi(&mut nds.bus9(), CpuId::Arm9, num, &mut r);
    (res, r)
}

fn put_bytes(nds: &mut Nds, addr: u32, data: &[u8]) {
    let mut bus = nds.bus9();
    for (i, b) in data.iter().enumerate() {
        bus.write8(addr + i as u32, *b);
    }
}

fn get_bytes(nds: &mut Nds, addr: u32, len: usize) -> Vec<u8> {
    let mut bus = nds.bus9();
    (0..len as u32).map(|i| bus.read8(addr + i)).collect()
}

#[test]
fn hle_lz77_and_run_length() {
    let mut nds = Nds::new();
    // "ABC" en clair, puis « 6 octets à 3 en arrière », puis "X".
    put_bytes(&mut nds, 0x0200_0000, &[0x10, 10, 0, 0, 0b0001_0000, b'A', b'B', b'C', 0x30, 0x02, b'X']);
    let (res, _) = swi(&mut nds, 0x11, [0x0200_0000, 0x0200_1000, 0, 0]);
    assert_eq!(res, hle_swi::Swi::Handled);
    assert_eq!(get_bytes(&mut nds, 0x0200_1000, 10), b"ABCABCABCX");

    // RLE : 5 × 'A' (compressé), 'B' (brut), "xyz" (brut).
    put_bytes(&mut nds, 0x0200_0100, &[0x30, 9, 0, 0, 0x82, b'A', 0x00, b'B', 0x02, b'x', b'y', b'z']);
    swi(&mut nds, 0x14, [0x0200_0100, 0x0200_2000, 0, 0]);
    assert_eq!(get_bytes(&mut nds, 0x0200_2000, 9), b"AAAAABxyz");
}

#[test]
fn hle_cpu_set_modes() {
    let mut nds = Nds::new();
    put_bytes(&mut nds, 0x0200_0000, &[1, 2, 3, 4, 5, 6, 7, 8]);
    swi(&mut nds, 0x0B, [0x0200_0000, 0x0200_0100, 3, 0]); // 3 demi-mots
    assert_eq!(get_bytes(&mut nds, 0x0200_0100, 8), [1, 2, 3, 4, 5, 6, 0, 0]);
    swi(&mut nds, 0x0B, [0x0200_0000, 0x0200_0200, 2 | 1 << 24 | 1 << 26, 0]); // remplissage 32 bits
    assert_eq!(get_bytes(&mut nds, 0x0200_0200, 8), [1, 2, 3, 4, 1, 2, 3, 4]);
    swi(&mut nds, 0x0C, [0x0200_0000, 0x0200_0300, 1 | 1 << 24, 0]); // FastSet : arrondi à 8 mots
    assert_eq!(get_bytes(&mut nds, 0x0200_0300 + 28, 4), [1, 2, 3, 4]);
    assert_eq!(get_bytes(&mut nds, 0x0200_0300 + 32, 4), [0, 0, 0, 0]);
}

#[test]
fn hle_math_crc_and_unhandled() {
    let mut nds = Nds::new();
    assert_eq!(swi(&mut nds, 0x0D, [1_000_001, 0, 0, 0]).1[0], 1000, "Sqrt");
    assert_eq!(swi(&mut nds, 0x0D, [u32::MAX, 0, 0, 0]).1[0], 0xFFFF);
    let (_, r) = swi(&mut nds, 0x09, [(-7i32) as u32, 2, 0, 0]);
    assert_eq!((r[0] as i32, r[1] as i32, r[3]), (-3, -1, 3), "Div");
    assert_eq!(swi(&mut nds, 0x09, [1, 0, 0, 0]).0, hle_swi::Swi::Unhandled, "division par zéro signalée");
    put_bytes(&mut nds, 0x0200_0000, b"123456789");
    assert_eq!(swi(&mut nds, 0x0E, [0xFFFF, 0x0200_0000, 9, 0]).1[0], 0x4B37, "GetCRC16");
    assert_eq!(swi(&mut nds, 0x05, [0; 4]).0, hle_swi::Swi::Bios, "VBlankIntrWait passe par le BIOS");
    assert_eq!(swi(&mut nds, 0x12, [0; 4]).0, hle_swi::Swi::Unhandled, "LZ77 par callback : à faire");
}

#[test]
fn unhandled_swi_is_recorded_and_skipped() {
    let mut nds = Nds::new();
    nds.hw.hle = true;
    load_arm(&mut nds, 0x0200_0000, &[0xEF13_0000, 0xE3A00007]); // swi 0x13 ; mov r0, #7
    nds.arm9.jump(0x0200_0000, false);
    nds.step();
    nds.step();
    assert_eq!(nds.arm9.reg(0), 7, "le jeu continue");
    assert!(nds.hw.unhandled_swi.contains(&(CpuId::Arm9, 0x13)));
}

#[test]
fn header_fields() {
    let mut rom = vec![0u8; 0x400];
    rom[..4].copy_from_slice(b"DEMO");
    rom[0x0C..0x10].copy_from_slice(b"ABCD");
    let fields: [(usize, u32); 8] = [
        (0x20, 0x200), (0x24, 0x0200_0800), (0x28, 0x0200_0000), (0x2C, 0x100),
        (0x30, 0x300), (0x34, 0x0238_0000), (0x38, 0x0238_0000), (0x3C, 0x100),
    ];
    for (o, v) in fields {
        rom[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    let h = Header::parse(&rom).unwrap();
    assert_eq!((h.title.as_str(), h.game_code_str().as_str()), ("DEMO", "ABCD"));
    assert_eq!((h.arm9_entry, h.arm7_ram_address), (0x0200_0800, 0x0238_0000));
    rom[0x38..0x3C].copy_from_slice(&0x0600_0000u32.to_le_bytes());
    assert!(matches!(Header::parse(&rom), Err(CartError::BadLoadAddress("ARM7", _, _))));
}

// ======================================================================
// GPU : moteurs 2D
// ======================================================================

const RED: u16 = 0x001F;
const GREEN: u16 = 0x03E0;
const BLUE: u16 = 0x7C00;

/// Machine sans CPU actif, écrans allumés, moteur A en haut.
fn gpu_nds() -> Nds {
    let mut nds = idle_nds();
    nds.bus9().write16(0x0400_0304, 0x8203);
    nds
}

fn pixel(nds: &Nds, screen: usize, x: usize, y: usize) -> u32 {
    nds.hw.gpu.screen(screen)[y * gpu::WIDTH + x]
}

fn rgb(c: u16) -> u32 {
    gpu::rgb555_to_rgb888(c)
}

#[test]
fn rgb555_conversion() {
    assert_eq!(rgb(RED), 0xFF0000);
    assert_eq!(rgb(GREEN), 0x00FF00);
    assert_eq!(rgb(0x7FFF), 0xFFFFFF);
    assert_eq!(rgb(0x0010), 0x840000, "16 → 0x84 (bits recopiés)");
}

#[test]
fn vram_display_mode() {
    let mut nds = gpu_nds();
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0240, 0x80); // A en LCDC
        bus.write32(0x0400_0000, 0x0002_0000); // mode 2 : affichage VRAM, banque A
        bus.write16(0x0680_0000 + (5 * 256 + 10) * 2, RED);
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 5), 0xFF0000);
    assert_eq!(pixel(&nds, 0, 11, 5), 0);
    assert_eq!(pixel(&nds, 1, 0, 0), 0xFFFFFF, "moteur B éteint : blanc");
}

#[test]
fn screens_swap_with_powcnt1() {
    let mut nds = gpu_nds();
    nds.bus9().write8(0x0400_0240, 0x80);
    nds.bus9().write32(0x0400_0000, 0x0002_0000);
    nds.bus9().write16(0x0680_0000, RED);
    nds.bus9().write16(0x0400_0304, 0x0203); // bit 15 = 0 : moteur A en bas
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), 0xFF0000);
    assert_eq!(pixel(&nds, 0, 0, 0), 0xFFFFFF);
}

/// Moteur B, BG0 en tuiles 4 bits : tuile 1 = pixels 1, 2, 0, 0… sur sa
/// première ligne. Carte en 0x1000, tuiles en 0x0000 (banque C en fond B).
fn text_setup(nds: &mut Nds) {
    let mut bus = nds.bus9();
    bus.write8(0x0400_0242, 0x84); // C → fond du moteur B
    bus.write32(0x0400_1000, 0x0001_0100); // mode 0, BG0
    bus.write16(0x0400_1008, 2 << 8); // BG0CNT : écran en 2 × 2 Ko
    bus.write16(0x0620_0020, 0x0021); // tuile 1, ligne 0 : idx 1, 2, 0, 0
    bus.write16(0x0620_1000, 0x0001); // carte (0,0) : tuile 1
    bus.write16(0x0620_1002, 0x0401); // carte (1,0) : tuile 1, miroir horizontal
    bus.write16(0x0500_0400, BLUE); // couleur de fond
    bus.write16(0x0500_0402, RED);
    bus.write16(0x0500_0404, GREEN);
}

#[test]
fn text_background_4bpp_with_flip_and_scroll() {
    let mut nds = gpu_nds();
    text_setup(&mut nds);
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(RED));
    assert_eq!(pixel(&nds, 1, 1, 0), rgb(GREEN));
    assert_eq!(pixel(&nds, 1, 2, 0), rgb(BLUE), "index 0 transparent");
    assert_eq!(pixel(&nds, 1, 15, 0), rgb(RED), "tuile retournée");
    assert_eq!(pixel(&nds, 1, 14, 0), rgb(GREEN));
    assert_eq!(pixel(&nds, 1, 0, 1), rgb(BLUE), "ligne 1 de la tuile vide");

    nds.bus9().write16(0x0400_1010, 1); // BG0HOFS = 1
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(GREEN), "défilement horizontal");
}

#[test]
fn background_priorities() {
    let mut nds = gpu_nds();
    text_setup(&mut nds);
    {
        let mut bus = nds.bus9();
        // BG1 : mêmes tuiles, carte en 0x1800, palette 1 (index 1 = vert).
        bus.write16(0x0400_100A, 3 << 8);
        bus.write16(0x0620_1800, 0x1001);
        bus.write16(0x0500_0422, GREEN);
        bus.write32(0x0400_1000, 0x0001_0300); // BG0 + BG1
        bus.write16(0x0400_1008, (2 << 8) | 1); // BG0 priorité 1
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(GREEN), "BG1 (priorité 0) devant");
    nds.bus9().write16(0x0400_1008, 2 << 8); // BG0 priorité 0 aussi
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(RED), "à égalité, le plus petit numéro gagne");
}

#[test]
fn extended_direct_bitmap_with_affine_zoom() {
    let mut nds = gpu_nds();
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0240, 0x81); // A → fond du moteur A
        bus.write32(0x0400_0000, 0x0001_0405); // mode 5, BG2
        bus.write16(0x0400_000C, 0x4084); // BG2 : bitmap direct 256×256
        bus.write32(0x0400_0020, 0x0000_0080); // PA = 0,5 (zoom ×2), PB = 0
        bus.write32(0x0400_0024, 0x0100_0000); // PC = 0, PD = 1
        bus.write16(0x0600_0000, RED | 0x8000);
        bus.write16(0x0600_0002, GREEN | 0x8000);
        bus.write16(0x0600_0004, BLUE); // bit 15 à 0 : transparent
        bus.write16(0x0600_0200, GREEN | 0x8000); // ligne 1
        bus.write16(0x0500_0000, 0x7FFF); // couleur de fond : blanc
    }
    nds.run_frame();
    let line0: Vec<u32> = (0..6).map(|x| pixel(&nds, 0, x, 0)).collect();
    assert_eq!(line0, [rgb(RED), rgb(RED), rgb(GREEN), rgb(GREEN), 0xFFFFFF, 0xFFFFFF]);
    assert_eq!(pixel(&nds, 0, 0, 1), rgb(GREEN), "PD fait avancer d'une ligne");
}

#[test]
fn bitmap_256_colors_and_affine_reference_point() {
    let mut nds = gpu_nds();
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0240, 0x81);
        bus.write32(0x0400_0000, 0x0001_0805); // mode 5, BG3
        bus.write16(0x0400_000E, 0x4080); // BG3 : bitmap 256 couleurs 256×256
        bus.write32(0x0400_0030, 0x0000_0100); // PA = 1
        bus.write32(0x0400_0034, 0x0100_0000); // PD = 1
        bus.write32(0x0400_0038, 3 << 8); // BG3X = 3 : décalage de 3 pixels
        bus.write16(0x0600_0002, 0x0700); // pixel (3,0) = index 7
        bus.write16(0x0500_000E, BLUE);
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(BLUE));
}

#[test]
fn master_brightness_and_forced_blank() {
    let mut nds = gpu_nds();
    text_setup(&mut nds);
    nds.bus9().write16(0x0400_106C, (2 << 14) | 16); // assombrissement total
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), 0);
    nds.bus9().write16(0x0400_106C, (1 << 14) | 8); // éclaircissement de moitié
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(0x001F | (15 << 5) | (15 << 10)));
    nds.bus9().write16(0x0400_106C, 0);
    nds.bus9().write32(0x0400_1000, 0x0001_0180); // « forced blank »
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), 0xFFFFFF);
}

#[test]
fn unsupported_features_are_noted() {
    let mut nds = gpu_nds();
    nds.bus9().write32(0x0400_0000, 0x0001_0308); // BG0 en 3D + BG1
    nds.bus9().write16(0x0400_000A, 0x0040); // BG1 avec mosaïque
    nds.run_frame();
    let notes = &nds.hw.gpu.unsupported;
    assert!(notes.contains("mosaïque"), "{notes:?}");
    assert!(!notes.contains("3D (BG0)"), "le calque 3D est rendu depuis 3D-3 : {notes:?}");
}

// ======================================================================
// DMA
// ======================================================================

fn dma(nds: &mut Nds, cpu: CpuId, n: u32, sad: u32, dad: u32, cnt: u32) {
    let base = 0x0400_00B0 + 12 * n;
    match cpu {
        CpuId::Arm9 => {
            let mut bus = nds.bus9();
            bus.write32(base, sad);
            bus.write32(base + 4, dad);
            bus.write32(base + 8, cnt);
        }
        CpuId::Arm7 => {
            let mut bus = nds.bus7();
            bus.write32(base, sad);
            bus.write32(base + 4, dad);
            bus.write32(base + 8, cnt);
        }
    }
}

const DMA_ON: u32 = 1 << 31;
const DMA_32: u32 = 1 << 26;

#[test]
fn dma_immediate_copy_and_fill() {
    let mut nds = Nds::new();
    for i in 0..8u32 {
        nds.bus9().write32(0x0200_0000 + 4 * i, i + 1);
    }
    dma(&mut nds, CpuId::Arm9, 3, 0x0200_0000, 0x0200_1000, DMA_ON | DMA_32 | 8);
    assert_eq!(nds.bus9().read32(0x0200_101C), 8);
    assert_eq!(nds.bus9().read32(0x0400_00D4 + 4) & DMA_ON, 0, "désactivé après un transfert immédiat");

    // Remplissage 16 bits (source fixe), destination décroissante.
    dma(&mut nds, CpuId::Arm9, 0, 0x0200_0000, 0x0200_2006, DMA_ON | (2 << 23) | (1 << 21) | 4);
    let got: Vec<u16> = (0..4).map(|i| nds.bus9().read16(0x0200_2000 + 2 * i)).collect();
    assert_eq!(got, [1, 1, 1, 1]);
    assert_eq!(nds.bus9().read16(0x0200_1FFE), 0, "pas un mot de trop");
}

#[test]
fn dma_through_cpu_view_and_irq() {
    let mut nds = Nds::new();
    nds.bus7().write32(0x0380_0000, 0xCAFE);
    // L'ARM7 copie de sa WRAM vers la Main RAM, avec IRQ de fin.
    dma(&mut nds, CpuId::Arm7, 1, 0x0380_0000, 0x0200_0000, DMA_ON | DMA_32 | (1 << 30) | 1);
    assert_eq!(nds.bus9().read32(0x0200_0000), 0xCAFE);
    assert_eq!(nds.hw.irq7.if_, irq_bits::DMA1);
    assert_eq!(nds.hw.dma[1].count(1), 1);
    nds.hw.dma[1].ch[0].cnt = 0;
    assert_eq!(nds.hw.dma[1].count(0), 0x4000, "0 = maximum (ARM7 canaux 0–2)");
}

#[test]
fn dma_vblank_fires_once() {
    let mut nds = idle_nds();
    nds.bus9().write32(0x0200_0000, 0x1234);
    dma(&mut nds, CpuId::Arm9, 1, 0x0200_0000, 0x0200_0100, DMA_ON | DMA_32 | (1 << 27) | 1);
    assert_eq!(nds.bus9().read32(0x0200_0100), 0, "attend le VBlank");
    nds.run_frame();
    assert_eq!(nds.bus9().read32(0x0200_0100), 0x1234);
    assert_eq!(nds.hw.dma[0].ch[1].cnt & DMA_ON, 0);
}

#[test]
fn hblank_dma_changes_backdrop_every_line() {
    let mut nds = gpu_nds();
    {
        let mut bus = nds.bus9();
        bus.write32(0x0400_1000, 0x0001_0000); // moteur B : graphique, aucun fond
        bus.write16(0x0500_0400, 0x7FFF);
        for y in 0..192u32 {
            bus.write16(0x0200_0000 + 2 * y, y as u16);
        }
    }
    nds.run_frame(); // on démarre au VBlank
    // Une couleur par HBlank, destination fixe, répétition.
    dma(&mut nds, CpuId::Arm9, 0, 0x0200_0000, 0x0500_0400, DMA_ON | (1 << 25) | (2 << 27) | (2 << 21) | 1);
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), 0xFFFFFF, "ligne 0 : couleur d'avant");
    for y in [1usize, 2, 100, 191] {
        assert_eq!(pixel(&nds, 1, 0, y), rgb((y - 1) as u16), "ligne {y}");
    }
    assert_ne!(nds.hw.dma[0].ch[0].cnt & DMA_ON, 0, "toujours actif (répétition)");
}

// ======================================================================
// GPU : sprites, fenêtres, mélange
// ======================================================================

/// Moteur A en haut : sprites activés (mapping 1D), banque B en VRAM de
/// sprites, banque A en VRAM de fonds. Tous les sprites masqués.
fn obj_nds(dispcnt: u32) -> Nds {
    let mut nds = gpu_nds();
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0240, 0x81); // A → fonds du moteur A
        bus.write8(0x0400_0241, 0x82); // B → sprites du moteur A
        bus.write32(0x0400_0000, 0x0001_0000 | dispcnt);
        for i in 0..128 {
            bus.write16(0x0700_0000 + 8 * i, 0x0200); // masqué
        }
        bus.write16(0x0500_0000, BLUE); // fond
        bus.write16(0x0500_0202, RED); // palette des sprites
        bus.write16(0x0500_0204, GREEN);
    }
    nds
}

fn sprite(nds: &mut Nds, i: u32, a0: u16, a1: u16, a2: u16) {
    let mut bus = nds.bus9();
    bus.write16(0x0700_0000 + 8 * i, a0);
    bus.write16(0x0700_0002 + 8 * i, a1);
    bus.write16(0x0700_0004 + 8 * i, a2);
}

/// Tuile 4 bits n° `tile` (mapping 1D, pas de 32) : ligne `row` = 1, 2, 0…
fn obj_tile_row(nds: &mut Nds, tile: u32, row: u32) {
    nds.bus9().write16(0x0640_0000 + tile * 32 + row * 4, 0x0021);
}

/// BG0 du moteur A rempli d'une couleur unie (tuile 1, index 1 partout).
fn solid_bg0(nds: &mut Nds, color: u16, prio: u16) {
    let mut bus = nds.bus9();
    for i in 0..8 {
        bus.write32(0x0600_0020 + 4 * i, 0x1111_1111);
    }
    for i in 0..1024 {
        bus.write16(0x0600_1000 + 2 * i, 0x0001);
    }
    bus.write16(0x0400_0008, (2 << 8) | prio);
    bus.write16(0x0500_0002, color);
    let d = bus.read32(0x0400_0000);
    bus.write32(0x0400_0000, d | 0x100);
}

#[test]
fn sprite_4bpp_basic_flip_and_negative_x() {
    let mut nds = obj_nds(0x1010);
    obj_tile_row(&mut nds, 0, 0);
    sprite(&mut nds, 0, 20, 10, 0); // 8×8 en (10, 20)
    sprite(&mut nds, 1, 20, 40 | 0x1000, 0); // miroir horizontal en (40, 20)
    sprite(&mut nds, 2, 30, 0x1FF, 0); // X = -1
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(RED));
    assert_eq!(pixel(&nds, 0, 11, 20), rgb(GREEN));
    assert_eq!(pixel(&nds, 0, 12, 20), rgb(BLUE), "index 0 transparent");
    assert_eq!(pixel(&nds, 0, 10, 19), rgb(BLUE));
    assert_eq!(pixel(&nds, 0, 47, 20), rgb(RED), "miroir");
    assert_eq!(pixel(&nds, 0, 46, 20), rgb(GREEN));
    assert_eq!(pixel(&nds, 0, 0, 30), rgb(GREEN), "X négatif : 1re colonne coupée");
}

#[test]
fn sprite_wraps_vertically() {
    let mut nds = obj_nds(0x1010);
    obj_tile_row(&mut nds, 0, 7);
    sprite(&mut nds, 0, 250, 10, 0); // lignes 250–255 puis 0–1
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 1), rgb(RED), "ligne 7 du sprite en Y = 1");
}

#[test]
fn sprite_256_colors_2d_mapping() {
    let mut nds = obj_nds(0x1000); // mapping 2D
    nds.bus9().write16(0x0640_0000 + 2 * 32 + 64, 0x0005); // tuile 2, 2e tuile en largeur
    nds.bus9().write16(0x0500_020A, GREEN);
    sprite(&mut nds, 0, 0x2000 | 0x4000 | 50, 60, 2); // 16×8, 256 couleurs
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 68, 50), rgb(GREEN));
    assert_eq!(pixel(&nds, 0, 60, 50), rgb(BLUE));
}

#[test]
fn sprite_priorities() {
    let mut nds = obj_nds(0x1010);
    obj_tile_row(&mut nds, 0, 0);
    solid_bg0(&mut nds, BLUE | GREEN, 1);
    sprite(&mut nds, 0, 20, 10, 1 << 10); // priorité 1 = celle du fond
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(RED), "à priorité égale, le sprite est devant");
    sprite(&mut nds, 0, 20, 10, 2 << 10);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(BLUE | GREEN), "priorité 2 : derrière le fond");

    // Deux sprites superposés : le plus petit numéro OAM gagne à priorité égale.
    nds.bus9().write16(0x0640_0020, 0x0012); // tuile 1 : vert puis rouge
    sprite(&mut nds, 0, 20, 10, 0);
    sprite(&mut nds, 1, 20, 10, 1);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(RED));
    sprite(&mut nds, 0, 20, 10, 1 << 10);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(GREEN), "sauf si l'autre a une priorité plus forte");
}

#[test]
fn affine_sprite_double_size_zoom() {
    let mut nds = obj_nds(0x1010);
    obj_tile_row(&mut nds, 0, 0);
    {
        let mut bus = nds.bus9();
        // Paramètres affines n° 0 : zoom ×2 (PA = PD = 0,5).
        bus.write16(0x0700_0006, 0x0080);
        bus.write16(0x0700_000E, 0);
        bus.write16(0x0700_0016, 0);
        bus.write16(0x0700_001E, 0x0080);
    }
    sprite(&mut nds, 0, 0x0300 | 40, 100, 0); // affine, double taille : boîte 16×16
    nds.run_frame();
    let row: Vec<u32> = (100..106).map(|x| pixel(&nds, 0, x, 40)).collect();
    assert_eq!(row, [rgb(RED), rgb(RED), rgb(GREEN), rgb(GREEN), rgb(BLUE), rgb(BLUE)]);
    assert_eq!(pixel(&nds, 0, 100, 41), rgb(RED), "chaque pixel source couvre 2 lignes");
    assert_eq!(pixel(&nds, 0, 100, 42), rgb(BLUE));
}

#[test]
fn bitmap_sprite() {
    let mut nds = obj_nds(0x1050); // OBJ, mapping bitmap 1D
    nds.bus9().write16(0x0640_0000, RED | 0x8000);
    nds.bus9().write16(0x0640_0002, GREEN); // bit 15 à 0 : transparent
    sprite(&mut nds, 0, 0x0C00 | 70, 80, 0xF000); // mode bitmap, alpha 15
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 80, 70), rgb(RED));
    assert_eq!(pixel(&nds, 0, 81, 70), rgb(BLUE));
    assert!(nds.hw.gpu.unsupported.is_empty(), "{:?}", nds.hw.gpu.unsupported);
}

#[test]
fn window_0_hides_background_inside() {
    let mut nds = obj_nds(0x2000); // WIN0, pas de sprites
    solid_bg0(&mut nds, RED, 0);
    {
        let mut bus = nds.bus9();
        bus.write16(0x0400_0040, (100 << 8) | 150); // X : 100 ≤ x < 150
        bus.write16(0x0400_0044, (50 << 8) | 60); // Y : 50 ≤ y < 60
        bus.write16(0x0400_0048, 0x0000); // dedans : rien
        bus.write16(0x0400_004A, 0x0001); // dehors : BG0
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 120, 55), rgb(BLUE), "dans la fenêtre : couleur de fond");
    assert_eq!(pixel(&nds, 0, 99, 55), rgb(RED));
    assert_eq!(pixel(&nds, 0, 150, 55), rgb(RED), "X2 exclu");
    assert_eq!(pixel(&nds, 0, 120, 60), rgb(RED), "Y2 exclu");
}

#[test]
fn object_window() {
    let mut nds = obj_nds(0x9010); // fenêtre OBJ + sprites (bits 15 et 12)
    solid_bg0(&mut nds, RED, 0);
    obj_tile_row(&mut nds, 0, 0);
    nds.bus9().write16(0x0400_004A, 0x0001); // dehors : BG0 ; fenêtre OBJ : rien
    sprite(&mut nds, 0, 0x0800 | 20, 10, 0); // mode « fenêtre »
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(BLUE), "sous la fenêtre OBJ : fond masqué");
    assert_eq!(pixel(&nds, 0, 12, 20), rgb(RED), "pixel transparent du sprite : dehors");
}

#[test]
fn alpha_blending_and_brightness() {
    let mut nds = obj_nds(0x1010);
    solid_bg0(&mut nds, BLUE, 0);
    obj_tile_row(&mut nds, 0, 0);
    // Sprite semi-transparent rouge sur BG0 bleu, EVA = EVB = 8.
    sprite(&mut nds, 0, 0x0400 | 20, 10, 0);
    nds.bus9().write16(0x0400_0050, 1 << 8); // 2e cible : BG0
    nds.bus9().write16(0x0400_0052, (8 << 8) | 8);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(15 | (15 << 10)), "moitié rouge, moitié bleu");

    // Mélange de fonds : BG0 (1re cible) sur la couleur de fond (2e cible).
    sprite(&mut nds, 0, 0x0200, 0, 0);
    nds.bus9().write16(0x0500_0000, GREEN);
    nds.bus9().write16(0x0400_0050, (1 << 6) | 1 | (1 << 13));
    nds.bus9().write16(0x0400_0052, (16 << 8) | 16);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(BLUE | GREEN), "saturé à 31");

    // Assombrissement de BG0 de moitié…
    nds.bus9().write16(0x0400_0050, (3 << 6) | 1);
    nds.bus9().write16(0x0400_0054, 8);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(16 << 10));
    // … sauf dans une fenêtre qui n'autorise pas les effets.
    let dispcnt = nds.bus9().read32(0x0400_0000);
    nds.bus9().write32(0x0400_0000, dispcnt | 0x2000);
    nds.bus9().write16(0x0400_0040, 0x00FF);
    nds.bus9().write16(0x0400_0044, 0x00C0);
    nds.bus9().write16(0x0400_0048, 0x0001); // dedans : BG0 sans effet
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(BLUE));
}

#[test]
fn window_with_x1_greater_than_x2_reaches_the_edge() {
    let mut nds = obj_nds(0x2000);
    solid_bg0(&mut nds, RED, 0);
    {
        let mut bus = nds.bus9();
        bus.write16(0x0400_0040, (200 << 8) | 10); // X1 = 200 > X2 = 10
        bus.write16(0x0400_0044, 0x00C0); // toutes les lignes
        bus.write16(0x0400_004A, 0x0001); // dehors : BG0
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 250, 5), rgb(BLUE), "de X1 jusqu'au bord : dedans");
    assert_eq!(pixel(&nds, 0, 5, 5), rgb(RED), "pas de retour au début de ligne");
}

#[test]
fn semi_transparent_sprite_without_second_target_stays_opaque() {
    let mut nds = obj_nds(0x1010);
    solid_bg0(&mut nds, BLUE, 0);
    obj_tile_row(&mut nds, 0, 0);
    sprite(&mut nds, 0, 0x0400 | 20, 10, 0); // semi-transparent
    nds.bus9().write16(0x0400_0050, 1 << 9); // 2e cible : BG1 seulement (absent ici)
    nds.bus9().write16(0x0400_0052, (8 << 8) | 8);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(RED), "rien à mélanger : sprite opaque");
}

// ======================================================================
// Cartouche (slot 1)
// ======================================================================

fn cart_nds() -> Nds {
    let mut nds = Nds::new();
    nds.hw.rom = (0..0x1_0000u32).map(|i| (i * 7 + (i >> 8)) as u8).collect();
    nds.hw.slot1.chip_id = 0x0000_00C2;
    nds
}

/// Envoie une commande de cartouche et lit les données en scrutant ROMCTRL.
fn cart_read(nds: &mut Nds, cmd: [u8; 8], block: u32) -> Vec<u8> {
    let mut bus = nds.bus9();
    bus.write32(0x0400_01A8, u32::from_le_bytes(cmd[..4].try_into().unwrap()));
    bus.write32(0x0400_01AC, u32::from_le_bytes(cmd[4..].try_into().unwrap()));
    bus.write32(0x0400_01A4, 0x8000_0000 | (1 << 29) | (block << 24));
    let mut out = Vec::new();
    while bus.read32(0x0400_01A4) & (1 << 23) != 0 {
        out.extend_from_slice(&bus.read32(0x0410_0010).to_le_bytes());
    }
    assert_eq!(bus.read32(0x0400_01A4) & (1 << 31), 0, "transfert terminé");
    out
}

fn b7(addr: u32) -> [u8; 8] {
    let a = addr.to_be_bytes();
    [0xB7, a[0], a[1], a[2], a[3], 0, 0, 0]
}

#[test]
fn cartridge_b7_read() {
    let mut nds = cart_nds();
    let data = cart_read(&mut nds, b7(0x9000), 1);
    assert_eq!(data.len(), 0x200);
    assert_eq!(&data[..], &nds.hw.rom[0x9000..0x9200]);
}

#[test]
fn cartridge_b7_redirect_wrap_and_mirror() {
    let mut nds = cart_nds();
    let data = cart_read(&mut nds, b7(0x0123), 1);
    assert_eq!(data[0], nds.hw.rom[0x8123], "adresses < 0x8000 → 0x8000 + (addr & 0x1FF)");
    let data = cart_read(&mut nds, b7(0x8F80), 1);
    assert_eq!(data[0x7F], nds.hw.rom[0x8FFF]);
    assert_eq!(data[0x80], nds.hw.rom[0x8000], "retour au début du bloc de 4 Ko");
    let data = cart_read(&mut nds, b7(0x1_9000), 7);
    assert_eq!(&data[..], &nds.hw.rom[0x9000..0x9004], "miroir au-delà de la taille de la puce");
}

#[test]
fn cartridge_chip_id_and_header() {
    let mut nds = cart_nds();
    let id = cart_read(&mut nds, [0xB8, 0, 0, 0, 0, 0, 0, 0], 7);
    assert_eq!(id, [0xC2, 0, 0, 0]);
    let hdr = cart_read(&mut nds, [0; 8], 1);
    assert_eq!(&hdr[..16], &nds.hw.rom[..16]);
    let dummy = cart_read(&mut nds, [0x9F, 0, 0, 0, 0, 0, 0, 0], 7);
    assert_eq!(dummy, [0xFF; 4]);
}

#[test]
fn cartridge_irq_goes_to_slot_owner() {
    let mut nds = cart_nds();
    nds.bus9().write16(0x0400_01A0, 0xC000); // slot activé + IRQ de fin
    cart_read(&mut nds, b7(0x8000), 7);
    assert_eq!(nds.hw.irq9.if_, irq_bits::CARD_TRANSFER);
    nds.hw.irq9.if_ = 0;
    nds.bus9().write16(0x0400_0204, 1 << 11); // slot 1 à l'ARM7
    cart_read(&mut nds, b7(0x8000), 7);
    assert_eq!((nds.hw.irq9.if_, nds.hw.irq7.if_), (0, irq_bits::CARD_TRANSFER));
    assert_eq!(nds.bus7().read16(0x0400_0204) & (1 << 11), 1 << 11, "l'ARM7 relit EXMEMCNT");
}

#[test]
fn cartridge_dma_before_and_after_start() {
    for dma_first in [true, false] {
        let mut nds = cart_nds();
        let start = |nds: &mut Nds| {
            let mut bus = nds.bus9();
            bus.write32(0x0400_01A8, u32::from_le_bytes([0xB7, 0, 0, 0xA0]));
            bus.write32(0x0400_01AC, 0);
            bus.write32(0x0400_01A4, 0x8000_0000 | (1 << 29) | (1 << 24));
        };
        let dma = |nds: &mut Nds| {
            // DMA3 : source fixe 0x04100010, 1 mot, répétition, déclencheur 5.
            dma(nds, CpuId::Arm9, 3, 0x0410_0010, 0x0200_0000,
                DMA_ON | DMA_32 | (1 << 25) | (5 << 27) | (2 << 23) | 1);
        };
        if dma_first {
            dma(&mut nds);
            start(&mut nds);
        } else {
            start(&mut nds);
            dma(&mut nds);
        }
        let got: Vec<u8> = (0..0x200u32).map(|i| nds.bus9().read8(0x0200_0000 + i)).collect();
        assert_eq!(&got[..], &nds.hw.rom[0xA000..0xA200], "DMA activée {}", if dma_first { "avant" } else { "après" });
        assert_eq!(nds.bus9().read32(0x0400_01A4) & (1 << 31), 0);
    }
}

#[test]
fn save_command_byte_answers_ff() {
    let mut nds = cart_nds();
    nds.bus9().write16(0x0400_01A0, 0xA040);
    nds.bus9().write8(0x0400_01A2, 0x05); // RDSR d'une EEPROM
    assert_eq!(nds.bus9().read8(0x0400_01A2), 0xFF);
}

// ======================================================================
// SPI : firmware, alimentation, écran tactile
// ======================================================================

/// Un octet sur le SPI. `last` = dernier octet de la commande.
fn spi(nds: &mut Nds, device: u16, byte: u8, last: bool) -> u8 {
    let mut bus = nds.bus7();
    let hold = if last { 0 } else { 1 << 11 };
    bus.write16(0x0400_01C0, 0x8000 | (device << 8) | hold);
    bus.write16(0x0400_01C2, byte as u16);
    assert_eq!(bus.read16(0x0400_01C0) & 0x80, 0, "jamais occupé");
    bus.read16(0x0400_01C2) as u8
}

fn spi_cmd(nds: &mut Nds, device: u16, send: &[u8], receive: usize) -> Vec<u8> {
    let total = send.len() + receive;
    let mut out = Vec::new();
    for i in 0..total {
        let b = send.get(i).copied().unwrap_or(0);
        let r = spi(nds, device, b, i == total - 1);
        if i >= send.len() {
            out.push(r);
        }
    }
    out
}

#[test]
fn firmware_id_header_and_user_settings() {
    let mut nds = Nds::new();
    assert_eq!(spi_cmd(&mut nds, 1, &[0x9F], 3), [0x20, 0x40, 0x12]);
    let ptr = spi_cmd(&mut nds, 1, &[0x03, 0x00, 0x00, 0x20], 2);
    assert_eq!(u16::from_le_bytes([ptr[0], ptr[1]]) as u32 * 8, 0x3FE00);
    let us = spi_cmd(&mut nds, 1, &[0x03, 0x03, 0xFE, 0x00], 0x74);
    assert_eq!(us[0], 5, "version");
    let nick: Vec<u16> = (0..5).map(|i| u16::from_le_bytes([us[6 + 2 * i], us[7 + 2 * i]])).collect();
    assert_eq!(String::from_utf16(&nick).unwrap(), "AvxDS");
    assert_eq!(u16::from_le_bytes([us[0x72], us[0x73]]), crc16(0xFFFF, us[..0x70].iter().copied()), "CRC valide");
}

#[test]
fn firmware_write_needs_write_enable() {
    let mut nds = Nds::new();
    spi_cmd(&mut nds, 1, &[0x0A, 0x00, 0x10, 0x00, 1, 2, 3], 0); // sans WREN
    assert_eq!(spi_cmd(&mut nds, 1, &[0x03, 0x00, 0x10, 0x00], 3), [0xFF; 3]);
    spi_cmd(&mut nds, 1, &[0x06], 0);
    assert_eq!(spi_cmd(&mut nds, 1, &[0x05], 1), [0x02], "WEL");
    spi_cmd(&mut nds, 1, &[0x0A, 0x00, 0x10, 0x00, 1, 2, 3], 0);
    assert_eq!(spi_cmd(&mut nds, 1, &[0x03, 0x00, 0x10, 0x00], 3), [1, 2, 3]);
    assert_eq!(spi_cmd(&mut nds, 1, &[0x05], 1), [0x00], "WEL retombe après l'écriture");
}

#[test]
fn power_manager_registers() {
    let mut nds = Nds::new();
    assert_eq!(spi_cmd(&mut nds, 0, &[0x80], 1), [0x0F]);
    spi_cmd(&mut nds, 0, &[0x00, 0x0C], 0);
    assert_eq!(spi_cmd(&mut nds, 0, &[0x80], 1), [0x0C]);
    assert!(!nds.hw.spi.power.shutdown_requested);
    spi_cmd(&mut nds, 0, &[0x00, 0x40], 0);
    assert!(nds.hw.spi.power.shutdown_requested, "extinction demandée");
}

/// Lit un canal du TSC en 12 bits, comme libnds : commande, puis 2 octets.
fn tsc(nds: &mut Nds, channel: u8) -> u16 {
    let r = spi_cmd(nds, 2, &[0x80 | (channel << 4)], 2);
    ((r[0] as u16) << 5) | (r[1] as u16 >> 3)
}

/// Formule de GBATEK pour revenir aux pixels.
fn adc_to_pixel(adc: u16, adc1: i32, scr1: i32, adc2: i32, scr2: i32) -> i32 {
    (adc as i32 - adc1) * (scr2 - scr1) / (adc2 - adc1) + (scr1 - 1)
}

#[test]
fn touch_screen_pen_up_and_down() {
    let mut nds = Nds::new();
    assert_eq!((tsc(&mut nds, 5), tsc(&mut nds, 1)), (0x000, 0xFFF), "stylet levé");
    assert_ne!(nds.bus7().read16(0x0400_0136) & (1 << 6), 0);

    for (x, y) in [(0u8, 0u8), (128, 96), (255, 191), (37, 150)] {
        nds.set_touch(Some((x, y)));
        assert_eq!(nds.bus7().read16(0x0400_0136) & (1 << 6), 0, "EXTKEYIN : stylet posé");
        let (ax, ay) = (tsc(&mut nds, 5), tsc(&mut nds, 1));
        let px = adc_to_pixel(ax, 0x200, 0x20, 0xE00, 0xE0);
        let py = adc_to_pixel(ay, 0x300, 0x20, 0xD00, 0xA0);
        assert!((px - x as i32).abs() <= 1 && (py - y as i32).abs() <= 1, "({x},{y}) → ({px},{py})");
    }
    nds.set_touch(None);
    assert_eq!(tsc(&mut nds, 1), 0xFFF);
}

#[test]
fn touch_screen_pipelined_commands() {
    let mut nds = Nds::new();
    nds.set_touch(Some((128, 96)));
    let x = tsc(&mut nds, 5);
    // X, puis l'octet haut, puis la commande Y en même temps que l'octet bas.
    let r = [spi(&mut nds, 2, 0xD0, false), spi(&mut nds, 2, 0, false), spi(&mut nds, 2, 0x90, false)];
    assert_eq!(((r[1] as u16) << 5) | (r[2] as u16 >> 3), x, "l'octet bas sort pendant la commande suivante");
    let hi = spi(&mut nds, 2, 0, false);
    let lo = spi(&mut nds, 2, 0, true);
    assert_eq!(((hi as u16) << 5) | (lo as u16 >> 3), tsc(&mut nds, 1));
}

// ======================================================================
// Horloge
// ======================================================================

/// Commande RTC pilotée bit à bit par l'ARM7. `msb_first` pour la commande
/// (comme libnds) ; données toujours LSB d'abord.
fn rtc(nds: &mut Nds, cmd: u8, msb_first: bool, write: &[u8], read: usize) -> Vec<u8> {
    let mut bus = nds.bus7();
    let base = 0x60; // horloge et sélection en sortie
    let mut put = |v: u32| bus.write8(0x0400_0138, v as u8);
    put(base | 0x02); // CS bas, /SCK haut
    put(base | 0x06); // CS haut
    let send = |put: &mut dyn FnMut(u32), byte: u8, msb: bool| {
        for bit in 0..8 {
            let b = if msb { (byte >> (7 - bit)) & 1 } else { (byte >> bit) & 1 } as u32;
            put(base | 0x14 | b); // /SCK bas, donnée
            put(base | 0x16 | b); // /SCK haut : échantillonnée
        }
    };
    send(&mut put, cmd, msb_first);
    for &b in write {
        send(&mut put, b, false);
    }
    let mut out = Vec::new();
    for _ in 0..read {
        let mut byte = 0u8;
        for bit in 0..8 {
            bus.write8(0x0400_0138, base as u8 | 0x04);
            bus.write8(0x0400_0138, base as u8 | 0x06);
            byte |= (bus.read8(0x0400_0138) & 1) << bit;
        }
        out.push(byte);
    }
    bus.write8(0x0400_0138, base as u8 | 0x02); // CS bas : fin
    out
}

#[test]
fn rtc_date_time_both_bit_orders() {
    let mut nds = Nds::new();
    nds.hw.rtc.fixed_time = Some(1_794_939_900); // mardi 17 novembre 2026, 18:25:00 UTC
    let expected = [0x26, 0x11, 0x17, 2, 0x18 | 0x40, 0x25, 0x00];
    assert_eq!(rtc(&mut nds, 0x65, true, &[], 7), expected, "commande MSB d'abord (libnds)");
    // 0x65 envoyé MSB d'abord = 0xA6 envoyé LSB d'abord : même train de
    // bits. Le cas réellement différent : 0x65 décalé LSB d'abord.
    assert_eq!(rtc(&mut nds, 0xA6, false, &[], 7), expected, "même train de bits");
    assert_eq!(rtc(&mut nds, 0x65, false, &[], 7), expected, "code 0110 reçu en dernier");
    assert_eq!(rtc(&mut nds, 0x67, true, &[], 3), expected[4..], "heure seule");
    assert_eq!(rtc(&mut nds, 0x61, true, &[], 1), [0x02], "status1 : mode 24 h");
}

#[test]
fn rtc_write_sets_an_offset() {
    let mut nds = Nds::new();
    nds.hw.rtc.fixed_time = Some(1_794_939_900);
    // Écrire 2027-01-02 03:04:05 (date et heure).
    rtc(&mut nds, 0x64, true, &[0x27, 0x01, 0x02, 6, 0x03, 0x04, 0x05], 0);
    assert_eq!(rtc(&mut nds, 0x65, true, &[], 7), [0x27, 0x01, 0x02, 6, 0x03, 0x04, 0x05]);
    nds.hw.rtc.fixed_time = Some(1_794_939_900 + 61);
    assert_eq!(rtc(&mut nds, 0x67, true, &[], 3), [0x03, 0x05, 0x06], "l'horloge continue d'avancer");
}

#[test]
fn direct_boot_keeps_firmware_and_uses_its_settings() {
    let mut nds = Nds::new();
    let mut fw = crate::nds::synthetic_firmware();
    // Copie 1 plus récente (compteur 2) avec la couleur favorite 3.
    let base = 0x3FF00;
    fw[base + 2] = 3;
    fw[base + 0x70] = 2;
    let crc = crc16(0xFFFF, fw[base..base + 0x70].iter().copied());
    fw[base + 0x72..base + 0x74].copy_from_slice(&crc.to_le_bytes());
    nds.load_firmware(&fw);
    nds.hw.rtc.fixed_time = Some(123);
    nds.set_touch(Some((1, 2)));
    nds.direct_boot(include_bytes!("../../tests/roms/boot_test.nds").to_vec()).unwrap();
    assert_eq!(nds.bus9().read8(0x027F_FC82), 3, "réglages pris dans la copie la plus récente");
    assert_eq!(nds.hw.spi.firmware.data[base + 2], 3, "firmware conservé");
    assert_eq!(nds.hw.rtc.fixed_time, Some(123));
    assert_eq!(nds.hw.spi.touch.pen, Some((1, 2)));
    // CRC faux : on retombe sur l'autre copie (couleur 11).
    let mut bad = fw.clone();
    bad[base + 0x72] ^= 1;
    nds.load_firmware(&bad);
    nds.direct_boot(include_bytes!("../../tests/roms/boot_test.nds").to_vec()).unwrap();
    assert_eq!(nds.bus9().read8(0x027F_FC82), 11);
}

// ======================================================================
// Puce de sauvegarde et détection
// ======================================================================

use crate::nds::spimem::Family;

/// Une transaction sur le SPI de la cartouche, comme libnds : slot activé
/// en mode SPI avec « hold », puis AUXSPICNT réécrit pour relâcher la puce.
fn aux(nds: &mut Nds, send: &[u8], receive: usize) -> Vec<u8> {
    let mut bus = nds.bus9();
    bus.write16(0x0400_01A0, 0xA040);
    let mut out = Vec::new();
    for i in 0..send.len() + receive {
        bus.write8(0x0400_01A2, send.get(i).copied().unwrap_or(0));
        let r = bus.read8(0x0400_01A2);
        if i >= send.len() {
            out.push(r);
        }
    }
    bus.write16(0x0400_01A0, 0x0040);
    out
}

fn aux_write(nds: &mut Nds, header: &[u8], data: &[u8]) {
    aux(nds, &[0x06], 0);
    let mut tx = header.to_vec();
    tx.extend_from_slice(data);
    aux(nds, &tx, 0);
}

fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed)).collect()
}

fn locked(nds: &Nds) -> Option<(Family, usize)> {
    nds.hw.slot1.save.locked()
}

#[test]
fn save_eeprom_8k_detected_on_first_write() {
    let mut nds = Nds::new();
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x40], 8), [0xFF; 8], "vierge : 0xFF sans avoir choisi");
    assert_eq!(locked(&nds), None, "les lectures ne verrouillent rien");
    let data = pattern(32, 1);
    aux_write(&mut nds, &[0x02, 0x00, 0x40], &data);
    assert_eq!(locked(&nds), Some((Family::Eeprom16, 8 << 10)));
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x40], 32), data);
    assert_eq!(aux(&mut nds, &[0x05], 1), [0x00], "WEL retombé après l'écriture");
    assert!(nds.save_dirty());
    assert_eq!(nds.save_data().unwrap().len(), 8 << 10);
}

#[test]
fn save_eeprom_64k_grows_with_addresses() {
    let mut nds = Nds::new();
    let data = pattern(128, 2);
    aux_write(&mut nds, &[0x02, 0x80, 0x00], &data);
    assert_eq!(locked(&nds), Some((Family::Eeprom16, 64 << 10)));
    assert_eq!(aux(&mut nds, &[0x03, 0x80, 0x00], 128), data);
}

#[test]
fn save_eeprom_512_high_half_via_opcode() {
    let mut nds = Nds::new();
    let data = pattern(16, 3);
    aux_write(&mut nds, &[0x0A, 0x10], &data); // WRHI : adresse 0x110
    assert_eq!(locked(&nds), Some((Family::Eeprom512, 512)));
    assert_eq!(aux(&mut nds, &[0x0B, 0x10], 16), data, "RDHI");
    assert_eq!(aux(&mut nds, &[0x03, 0x10], 1), [0xFF], "RDLO : l'autre moitié");
    assert_eq!(aux(&mut nds, &[0x05], 1), [0xF0], "bits 4–7 à 1 sur la 0,5 Ko");
    assert_eq!(nds.save_data().unwrap()[0x110..0x120], data[..]);
}

#[test]
fn save_flash_rdid_erase_and_program() {
    let mut nds = Nds::new();
    assert_eq!(aux(&mut nds, &[0x9F], 3), [0x20, 0x40, 0x12], "RDID : FLASH en tête");
    aux(&mut nds, &[0x06], 0);
    aux(&mut nds, &[0xD8, 0x01, 0x00, 0x00], 0); // effacement de secteur
    assert_eq!(locked(&nds), Some((Family::Flash, 256 << 10)), "l'effacement n'existe qu'en FLASH");
    let data = pattern(256, 4);
    aux_write(&mut nds, &[0x0A, 0x01, 0x00, 0x00], &data);
    assert_eq!(aux(&mut nds, &[0x03, 0x01, 0x00, 0x00], 256), data);
    assert_eq!(aux(&mut nds, &[0x0B, 0x01, 0x00, 0x10, 0x00], 4), data[0x10..0x14], "FAST READ");
    aux_write(&mut nds, &[0x02, 0x01, 0x00, 0x00], &[0x0F]);
    assert_eq!(aux(&mut nds, &[0x03, 0x01, 0x00, 0x00], 1), [data[0] & 0x0F], "PP : ET logique");
}

#[test]
fn save_flash_page_write_from_blank() {
    let mut nds = Nds::new();
    aux_write(&mut nds, &[0x0A, 0x00, 0x02, 0x00], &pattern(256, 5));
    assert_eq!(locked(&nds).map(|l| l.0), Some(Family::Flash), "260 octets : 3 + 256, pas 2 + 257");
}

#[test]
fn save_probe_as_8k_then_use_512() {
    // Cas signalé par GBATEK (Over the Hedge) : sondage en 16 bits, puis
    // utilisation d'une EEPROM de 0,5 Ko.
    let mut nds = Nds::new();
    aux(&mut nds, &[0x03, 0x00, 0x00], 8);
    aux_write(&mut nds, &[0x02, 0x20], &pattern(16, 0x11));
    assert_eq!(locked(&nds), Some((Family::Eeprom512, 512)));
    assert_eq!(aux(&mut nds, &[0x03, 0x20], 16), pattern(16, 0x11));
}

#[test]
fn save_read_dummy_run_reveals_address_width() {
    let mut nds = Nds::new();
    // 3 octets d'adresse non nuls puis des octets factices : pas une EEPROM 16 bits.
    // 15 octets lus : la puissance de 2 favoriserait la lecture 2 + 16.
    aux(&mut nds, &[0x03, 0x01, 0x02, 0x03], 15);
    assert_eq!(locked(&nds), None);
    assert_eq!(nds.hw.slot1.save.leading(), Family::Flash);
}

#[test]
fn save_odd_sized_struct_write_stays_eeprom() {
    // 17 octets en 0x0010, 1er octet nul : lu « 3 + 16, aligné », ce serait
    // une FLASH. Mais un 02 sans effacement préalable désigne une EEPROM.
    let mut nds = Nds::new();
    let mut data = pattern(17, 9);
    data[0] = 0;
    aux_write(&mut nds, &[0x02, 0x00, 0x10], &data);
    assert_eq!(locked(&nds).map(|l| l.0), Some(Family::Eeprom16));
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x10], 17), data);
}

#[test]
fn save_page_crossing_rules_out_a_family() {
    // Écriture 0A de 16 octets : en FLASH (3 + 13 à 0x10F8F8), elle
    // franchirait une page de 256 ; reste l'EEPROM de 0,5 Ko (0x110 + 16).
    let mut nds = Nds::new();
    let mut data = pattern(16, 10);
    data[0] = 0xF8;
    data[1] = 0xF8;
    aux_write(&mut nds, &[0x0A, 0x10], &data);
    assert_eq!(locked(&nds).map(|l| l.0), Some(Family::Eeprom512));
}

#[test]
fn save_exact_size_is_trusted_except_512k() {
    // 8 Ko dont seuls 0x100–0x1FF sont utilisés : une EEPROM 16 bits, pas
    // une 0,5 Ko (l'étendue seule le suggérerait).
    let mut file = vec![0xFF; 8 << 10];
    file[0x100..0x200].fill(0x42);
    let mut nds = Nds::new();
    nds.load_save(&file);
    assert_eq!(locked(&nds), Some((Family::Eeprom16, 8 << 10)));
    // La même chose à 512 Ko reste ambiguë.
    file.resize(512 << 10, 0xFF);
    nds.load_save(&file);
    assert_eq!(locked(&nds), None);
}

#[test]
fn save_eeprom_game_probing_rdid_gets_ff() {
    // Trois lectures « 2 + 16 » : l'EEPROM 16 bits mène nettement. Un
    // RDID ensuite reçoit FF FF FF, comme d'une vraie EEPROM.
    let mut nds = Nds::new();
    for _ in 0..3 {
        aux(&mut nds, &[0x03, 0x00, 0x10], 16);
    }
    assert_eq!(nds.hw.slot1.save.leading(), Family::Eeprom16);
    assert_eq!(aux(&mut nds, &[0x9F], 3), [0xFF; 3]);
}

#[test]
fn save_write_without_wren_is_ignored() {
    let mut nds = Nds::new();
    aux(&mut nds, &[0x02, 0x00, 0x00, 1, 2, 3, 4], 0);
    assert_eq!(locked(&nds), None);
    assert_eq!(nds.save_data(), None, "rien à sauvegarder");
}

#[test]
fn save_padded_import_keeps_eeprom_and_reads_right() {
    // Comme les .sav complétés à 512 Ko : données sur moins de 8 Ko.
    let mut file = vec![0xFF; 512 << 10];
    let data = pattern(0x1DC8, 6);
    file[..data.len()].copy_from_slice(&data);
    let mut nds = Nds::new();
    nds.load_save(&file);
    assert_eq!(locked(&nds), None, "ambigu : on ne verrouille pas");
    assert_eq!(nds.hw.slot1.save.leading(), Family::Eeprom16, "a priori : plus petite famille");
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x10], 16), data[0x10..0x20]);
    aux_write(&mut nds, &[0x02, 0x00, 0x00], &[9; 32]);
    assert_eq!(locked(&nds), Some((Family::Eeprom16, 8 << 10)));
    let out = nds.save_data().unwrap();
    assert_eq!(out.len(), 8 << 10, "réexporté à la vraie taille");
    assert_eq!(out[32..data.len()], data[32..]);
}

#[test]
fn save_large_import_locks_flash_at_once() {
    let mut file = vec![0xFF; 512 << 10];
    file[0x7F000..0x7F010].copy_from_slice(&pattern(16, 7));
    let mut nds = Nds::new();
    nds.load_save(&file);
    assert_eq!(locked(&nds), Some((Family::Flash, 512 << 10)), "données au-delà de 128 Ko");
    assert_eq!(aux(&mut nds, &[0x03, 0x07, 0xF0, 0x00], 16), pattern(16, 7));
    assert_eq!(aux(&mut nds, &[0x9F], 3), [0x20, 0x40, 0x13], "M45PE40");
}

#[test]
fn save_write_protection() {
    let mut nds = Nds::new();
    aux_write(&mut nds, &[0x02, 0x00, 0x00], &[1; 32]);
    aux(&mut nds, &[0x06], 0);
    aux(&mut nds, &[0x01, 0x0C], 0); // WRSR : tout protégé
    assert_eq!(aux(&mut nds, &[0x05], 1), [0x0C]);
    aux_write(&mut nds, &[0x02, 0x00, 0x00], &[2; 32]);
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x00], 1), [1], "écriture refusée");
}

#[test]
fn save_hold_bit_ends_transaction_per_byte() {
    // Sans « hold », la puce est relâchée après chaque octet.
    let mut nds = Nds::new();
    aux_write(&mut nds, &[0x02, 0x00, 0x00], &[5; 32]);
    {
        let mut bus = nds.bus9();
        bus.write16(0x0400_01A0, 0xA000);
        bus.write8(0x0400_01A2, 0x06); // WREN, transaction finie
        bus.write8(0x0400_01A2, 0x05); // nouvelle transaction : RDSR
    }
    let mut bus = nds.bus9();
    bus.write16(0x0400_01A0, 0xA040);
    bus.write8(0x0400_01A2, 0x05);
    bus.write8(0x0400_01A2, 0x00);
    assert_eq!(bus.read8(0x0400_01A2), 0x02, "WEL posé par le WREN isolé");
}

#[test]
fn save_survives_direct_boot_and_forced_type() {
    let mut nds = Nds::new();
    aux_write(&mut nds, &[0x02, 0x00, 0x00], &[7; 32]);
    nds.direct_boot(include_bytes!("../../tests/roms/boot_test.nds").to_vec()).unwrap();
    assert_eq!(locked(&nds), Some((Family::Eeprom16, 8 << 10)), "la puce est sur la cartouche");
    assert_eq!(aux(&mut nds, &[0x03, 0x00, 0x00], 2), [7, 7]);

    let mut nds = Nds::new();
    nds.force_save_type(Family::Flash, 1 << 20, &[]);
    assert_eq!(aux(&mut nds, &[0x9F], 3), [0x20, 0x40, 0x14]);
}

/// Les .sav réels de Yassine (non versionnés) : `AVXDS_SAVES=dossier`.
#[test]
fn real_save_files_if_present() {
    let Ok(dir) = std::env::var("AVXDS_SAVES") else { return };
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let file = std::fs::read(&path).unwrap();
        let extent = file.iter().rposition(|&b| b != 0xFF).map_or(0, |p| p + 1);
        let mut nds = Nds::new();
        nds.load_save(&file);
        let lead = nds.hw.slot1.save.leading();
        eprintln!("{} : {} o, données jusqu'à 0x{extent:X}, {:?}, en tête {lead:?}",
            path.display(), file.len(), locked(&nds), );
        // Relecture de 64 octets au début des données, sous l'hypothèse en tête.
        let hl = lead.header_len(0x03);
        let start = file.iter().position(|&b| b != 0xFF).unwrap_or(0) & !0xFF;
        let addr = (start as u32).to_be_bytes();
        let mut cmd = vec![0x03];
        cmd.extend_from_slice(&addr[4 - hl..]);
        assert_eq!(aux(&mut nds, &cmd, 64), file[start..start + 64], "{}", path.display());
        if locked(&nds).is_some() {
            assert_eq!(nds.save_data().unwrap(), file, "aller-retour exact");
        }
    }
}

#[test]
fn chip_id_size_byte_and_protocol_bit() {
    let rom = include_bytes!("../../tests/roms/boot_test.nds");
    let mut header = crate::nds::Header::parse(rom).unwrap();
    let id = |h: &crate::nds::Header, cap: u8| {
        let mut h = h.clone();
        h.device_capacity = cap;
        crate::nds::cart::chip_id(&h, 0)
    };
    // Capacité de l'en-tête : 128 Ko << n.
    assert_eq!(id(&header, 6), 0x0000_07C2, "8 Mo");
    assert_eq!(id(&header, 9), 0x0000_3FC2, "64 Mo : ancien protocole");
    assert_eq!(id(&header, 10), 0x8000_7FC2, "128 Mo");
    assert_eq!(id(&header, 11), 0x8000_FFC2, "256 Mo (exemple GBATEK : C2h,FFh)");
    assert_eq!(id(&header, 12), 0x8000_FEC2, "512 Mo (exemple GBATEK : C2h,FEh)");
    header.device_capacity = 0;
    assert_eq!(crate::nds::cart::chip_id(&header, 300 << 20), 0x8000_FEC2, "fichier plus grand que l'en-tête");
}

#[test]
fn ipc_trace_keeps_head_and_tail() {
    let mut t = crate::nds::IoTrace::default();
    for i in 0..600u32 {
        t.record(i as u64, CpuId::Arm9, 0x0200_0000, 0x0400_0188, true, i);
    }
    assert_eq!(t.ipc.len(), 256);
    assert_eq!(t.ipc_tail.len(), 256);
    assert_eq!(t.ipc_dropped, 600 - 512);
    assert_eq!(t.ipc[0].5, 0, "début conservé");
    assert_eq!(t.ipc_tail.back().unwrap().5, 599, "fin conservée");
    // Une boucle « écrire, lire » ne prend que deux lignes.
    let mut t = crate::nds::IoTrace::default();
    for _ in 0..1000 {
        t.record(0, CpuId::Arm7, 0x10, 0x0400_0180, true, 0x700);
        t.record(0, CpuId::Arm7, 0x14, 0x0400_0180, false, 0x700);
    }
    assert_eq!(t.ipc.len(), 2);
    assert_eq!((t.ipc[0].6, t.ipc[1].6), (1000, 1000));
}

// Cartouches à infrarouge (Pokémon N2) : préfixe 00h devant chaque commande
// de la sauvegarde, comme le fait le code ARM7 du jeu.
fn aux_ir(nds: &mut Nds, send: &[u8], receive: usize) -> Vec<u8> {
    let mut tx = vec![0x00];
    tx.extend_from_slice(send);
    aux(nds, &tx, receive)
}

#[test]
fn save_through_infrared_prefix() {
    let mut nds = Nds::new();
    assert_eq!(aux_ir(&mut nds, &[0x05], 1), [0x00], "RDSR via le préfixe : prête, pas FF");
    assert!(nds.hw.slot1.infrared);
    aux_ir(&mut nds, &[0x06], 0);
    assert_eq!(aux_ir(&mut nds, &[0x05], 1), [0x02], "WEL vu à travers le préfixe");
    let data = pattern(256, 12);
    let mut tx = vec![0x0A, 0x01, 0x00, 0x00];
    tx.extend_from_slice(&data);
    aux_ir(&mut nds, &tx, 0);
    assert_eq!(locked(&nds).map(|l| l.0), Some(Family::Flash));
    assert_eq!(aux_ir(&mut nds, &[0x03, 0x01, 0x00, 0x00], 256), data);
    // Une commande sans préfixe va à la puce infrarouge, pas à la sauvegarde.
    aux(&mut nds, &[0x06], 0);
    aux(&mut nds, &[0x0A, 0x01, 0x00, 0x00, 0x55], 0);
    assert_eq!(aux_ir(&mut nds, &[0x03, 0x01, 0x00, 0x00], 1), [data[0]], "sauvegarde intacte");
}

#[test]
fn infrared_prefix_split_across_hold_writes() {
    // Séquence exacte du jeu : 00 (hold, horloge lente), AUXSPICNT
    // réécrit avec hold, 05, puis AUXSPICNT sans hold et 00.
    let mut nds = Nds::new();
    let mut bus = nds.bus7();
    bus.write16(0x0400_01A0, 0xA040);
    bus.write16(0x0400_01A0, 0xA042);
    bus.write16(0x0400_01A2, 0x00);
    bus.write16(0x0400_01A0, 0xA040);
    bus.write16(0x0400_01A2, 0x05);
    bus.write16(0x0400_01A0, 0xA000);
    bus.write16(0x0400_01A2, 0x00);
    assert_eq!(bus.read16(0x0400_01A2), 0x00, "statut : prête");
}

#[test]
fn gpu3d_idle_engine_reports_an_empty_fifo() {
    let mut nds = Nds::new();
    let st = nds.bus9().read32(0x0400_0600);
    assert_ne!(st & (1 << 25), 0, "moins qu'à moitié pleine");
    assert_ne!(st & (1 << 26), 0, "vide");
    assert_eq!(st & (1 << 27), 0, "pas occupé");
    assert_eq!((st >> 16) & 0x1FF, 0, "0 entrée");
    // Commandes acceptées sans être notées comme I/O manquantes.
    nds.bus9().write32(0x0400_0400, 0x0000_0015);
    nds.bus9().write32(0x0400_0540, 0);
    assert!(nds.hw.unhandled_io.is_empty(), "{:x?}", nds.hw.unhandled_io);
    assert!(nds.hw.gpu.unsupported.iter().any(|n| n.starts_with("3D")));
    // L'ARM7 garde ses registres de son à ces adresses : l'écriture de
    // l'ARM9 n'y est pas arrivée, celle de l'ARM7 si.
    assert_eq!(nds.bus7().read32(0x0400_0400), 0);
    nds.bus7().write32(0x0400_0400, 0x0000_0015);
    assert_eq!(nds.bus7().read32(0x0400_0400), 0x15);
    assert!(nds.hw.unhandled_io.is_empty(), "{:x?}", nds.hw.unhandled_io);
}

#[test]
fn gpu3d_fifo_irq_is_level_triggered() {
    let mut nds = Nds::new();
    nds.bus9().write32(0x0400_0600, 2 << 30); // IRQ quand la file est vide
    assert_eq!(nds.bus9().read32(0x0400_0600) >> 30, 2);
    assert_ne!(nds.hw.irq9.if_ & irq_bits::GEOMETRY_FIFO, 0);
    nds.hw.irq9.if_ = 0; // acquittement
    nds.run_frame();
    assert_ne!(nds.hw.irq9.if_ & irq_bits::GEOMETRY_FIFO, 0, "reposée tant que la file est vide");
    nds.bus9().write32(0x0400_0600, 0);
    nds.hw.irq9.if_ = 0;
    nds.run_frame();
    assert_eq!(nds.hw.irq9.if_ & irq_bits::GEOMETRY_FIFO, 0, "plus d'IRQ en mode 0");
}

/// Entrées produites par une suite de mots écrits dans GXFIFO.
fn unpack(words: &[u32]) -> Vec<(u8, u32)> {
    let mut u = gpu3d::fifo::Unpacker::default();
    let mut out = Vec::new();
    for &w in words {
        u.feed(w, &mut |e| out.push((e.cmd, e.param)));
    }
    out
}

#[test]
fn gxfifo_packed_commands_take_their_parameters_in_order() {
    // MTX_MODE (1 param), MTX_PUSH (0), MTX_TRANS (3) ; puis un mot neuf.
    let e = unpack(&[0x001C_1110, 2, 0xA, 0xB, 0xC, 0x0000_0015]);
    assert_eq!(e, vec![(0x10, 2), (0x11, 0), (0x1C, 0xA), (0x1C, 0xB), (0x1C, 0xC), (0x15, 0)]);
    // Codes 00h et invalides ignorés, sans consommer de paramètre ; seuls
    // les codes invalides sont comptés comme tels.
    let e = unpack(&[0x10FF_0015, 3, 0x0000_0041]);
    assert_eq!(e, vec![(0x15, 0), (0x10, 3), (0x41, 0)]);
    let mut u = gpu3d::fifo::Unpacker::default();
    u.feed(0x10FF_0015, &mut |_| {});
    assert_eq!(u.invalid, 1, "FFh invalide, 00h = pas de commande");
    // Exemple GBATEK : Packed(00151515h) = trois MTX_IDENTITY.
    assert_eq!(unpack(&[0x0015_1515]), vec![(0x15, 0); 3]);
    // Format non compact : un code, puis ses 16 paramètres.
    let mut words = vec![0x16];
    words.extend(1..=16);
    let e = unpack(&words);
    assert_eq!(e.len(), 16);
    assert!(e.iter().zip(1..).all(|(&(c, p), k)| c == 0x16 && p == k));
}

#[test]
fn gxfifo_fills_the_pipe_first_and_refills_it_two_by_two() {
    use gpu3d::fifo::{CommandFifo, Entry};
    let mut f = CommandFifo::default();
    for k in 0..6 {
        assert!(f.push(Entry { cmd: 0x20, param: k }));
    }
    assert_eq!((f.pipe_len(), f.fifo_len()), (4, 2), "4 au PIPE, le reste à la FIFO");
    assert_eq!(f.pop().map(|e| e.param), Some(0));
    assert_eq!((f.pipe_len(), f.fifo_len()), (3, 2), "PIPE à 3 : pas encore de transfert");
    // PIPE non plein mais FIFO non vide : l'entrée va en FIFO (l'ordre tient).
    assert!(f.push(Entry { cmd: 0x20, param: 6 }));
    assert_eq!((f.pipe_len(), f.fifo_len()), (3, 3));
    assert_eq!(f.pop().map(|e| e.param), Some(1));
    assert_eq!((f.pipe_len(), f.fifo_len()), (4, 1), "PIPE sous 3 : 2 entrées montent");
    for k in 2..7 {
        assert_eq!(f.pop().map(|e| e.param), Some(k), "ordre conservé");
    }
    assert!(f.pop().is_none());
    // 4 + 256 entrées au plus.
    for k in 0..260 {
        assert!(f.push(Entry { cmd: 0x20, param: k }));
    }
    assert!(f.is_full());
    assert!(!f.push(Entry { cmd: 0x20, param: 0 }));
}

#[test]
fn gpu3d_commands_keep_the_engine_busy_for_their_duration() {
    let mut g = gpu3d::Gpu3d::default();
    g.write(0x0400_0454, 0, !0, 1000); // MTX_IDENTITY : 19 cycles de bus
    assert_ne!(g.read(0x0400_0600, 1000) & (1 << 27), 0, "occupé");
    assert_ne!(g.read(0x0400_0600, 1000 + 37) & (1 << 27), 0, "encore occupé");
    assert_eq!(g.read(0x0400_0600, 1000 + 38) & (1 << 27), 0, "libre après 19 × 2 cycles ARM9");
    // MTX_MULT_4x3 : 31 cycles, 61 en mode 2 ; la commande attend ses 12
    // paramètres avant de s'exécuter.
    g.write(0x0400_0440, 2, !0, 2000); // MTX_MODE 2
    for k in 0..11 {
        g.write(0x0400_0464, k, !0, 2000);
    }
    assert_ne!(g.read(0x0400_0600, 5000) & (1 << 27), 0, "attend son 12e paramètre");
    g.write(0x0400_0464, 11, !0, 5000);
    assert_ne!(g.read(0x0400_0600, 5000 + 121) & (1 << 27), 0);
    assert_eq!(g.read(0x0400_0600, 5000 + 122) & (1 << 27), 0, "61 × 2 cycles");
    assert_eq!(g.stats.commands[0x19], 1);
    // Trois commandes reçues au même instant : une seule s'exécute tout de
    // suite, les autres attendent leur tour sur l'horloge du moteur.
    for _ in 0..3 {
        g.write(0x0400_0454, 0, !0, 10_000);
    }
    assert_eq!(g.read(0x0400_0600, 10_000) & (1 << 27), 1 << 27);
    assert_eq!(g.stats.commands[0x15], 2, "la 1re de cette série seulement (+1 plus haut)");
    g.read(0x0400_0600, 10_000 + 3 * 38);
    assert_eq!(g.stats.commands[0x15], 4);
    // Port d'une commande inexistante (1Dh) : ignoré.
    g.write(0x0400_0474, 0, !0, 20_000);
    assert_eq!(g.read(0x0400_0600, 20_000) & (1 << 27), 0);
}

#[test]
fn gpu3d_swap_buffers_halts_the_engine_until_vblank() {
    let mut nds = Nds::new();
    nds.run_frame(); // au début du VBlank : une image entière devant nous
    nds.bus9().write32(0x0400_0540, 0); // SWAP_BUFFERS
    for k in 0..300 {
        nds.bus9().write32(0x0400_0480, k); // COLOR
    }
    let st = nds.bus9().read32(0x0400_0600);
    assert_eq!((st >> 16) & 0x1FF, 256, "FIFO pleine (bit 24)");
    assert_eq!(st & (3 << 25), 0, "ni à moitié vide, ni vide");
    assert_ne!(st & (1 << 27), 0, "occupé : attend le VBlank");
    assert_eq!(nds.hw.gpu3d.fifo.reserve_len(), 300 - 256 - 4, "le surplus attend hors file");
    nds.run_frame(); // VBlank suivant : l'échange a lieu, le moteur repart
    assert_eq!(nds.hw.gpu3d.last_frame.swaps, 1);
    for _ in 0..4 {
        nds.step(); // quelques instructions : le moteur rattrape l'horloge
    }
    nds.run_until(nds.now + 2 * LINE_CYCLES);
    let st = nds.bus9().read32(0x0400_0600);
    assert_ne!(st & (1 << 26), 0, "file vidée");
    assert_eq!(st & (1 << 27), 0);
    assert_eq!(nds.hw.gpu3d.stats.commands[0x20], 300, "toutes les couleurs exécutées");
}

#[test]
fn gpu3d_geometry_dma_sends_bursts_while_the_fifo_is_half_empty() {
    let mut nds = Nds::new();
    nds.run_frame();
    // En mémoire : SWAP_BUFFERS, puis 299 COLOR, en format non compact.
    let base = 0x0200_0000;
    let mut words = vec![0x50, 0];
    for k in 0..299 {
        words.extend([0x20, k]);
    }
    for (i, w) in words.iter().enumerate() {
        nds.bus9().write32(base + 4 * i as u32, *w);
    }
    // DMA0 : mode 7, mots de 32 bits, destination fixe, IRQ à la fin.
    nds.bus9().write32(0x0400_00B0, base);
    nds.bus9().write32(0x0400_00B4, 0x0400_0400);
    nds.bus9().write32(0x0400_00B8, 0xC000_0000 | 7 << 27 | 1 << 26 | 2 << 21 | words.len() as u32);
    // Le moteur s'arrête sur SWAP_BUFFERS : la DMA remplit la FIFO par
    // rafales de 112 mots tant qu'elle est à moins de moitié pleine.
    let ch = nds.hw.dma[0].ch[0];
    assert!(ch.enabled(), "transfert pas fini");
    assert_eq!(ch.left, 600 - 3 * 112, "trois rafales");
    let st = nds.bus9().read32(0x0400_0600);
    assert!((st >> 16) & 0x1FF >= 128, "plus à moitié vide : la DMA attend");
    assert_eq!(nds.hw.irq9.if_ & irq_bits::DMA0, 0);
    nds.run_frame(); // VBlank : échange, le moteur vide la file, la DMA reprend
    nds.run_until(nds.now + 4 * LINE_CYCLES);
    assert!(!nds.hw.dma[0].ch[0].enabled(), "transfert terminé");
    assert_ne!(nds.hw.irq9.if_ & irq_bits::DMA0, 0, "IRQ de fin");
    assert_eq!(nds.hw.gpu3d.stats.commands[0x20], 299);
}

#[test]
fn gpu3d_full_fifo_runs_the_engine_ahead_instead_of_dropping() {
    // Sans SwapBuffers, une file pleine fait avancer le moteur (le CPU
    // attendrait sur la console) : rien ne part en réserve.
    let mut g = gpu3d::Gpu3d::default();
    for k in 0..400 {
        g.write(0x0400_0480, k, !0, 50); // COLOR, toutes au même instant
    }
    assert_eq!(g.fifo.reserve_len(), 0);
    assert_eq!(g.stats.stalls, 0);
    // Le PIPE se recharge par 2 : la FIFO oscille entre 254 et 256.
    assert!(g.fifo.fifo_len() >= 254, "la FIFO reste pleine ({})", g.fifo.fifo_len());
    assert!(g.stats.commands[0x20] >= 400 - 260);
}

#[test]
fn gpu3d_half_full_threshold_is_128_entries() {
    // SwapBuffers arrête le moteur ; puis k COLOR : 4 au PIPE, k − 4 en FIFO.
    let fill = |k: u32| {
        let mut g = gpu3d::Gpu3d::default();
        g.write(0x0400_0540, 0, !0, 0);
        for c in 0..k {
            g.write(0x0400_0480, c, !0, 0);
        }
        g
    };
    let mut g = fill(4 + 127);
    let st = g.read(0x0400_0600, 0);
    assert_eq!((st >> 16) & 0x1FF, 127);
    assert_ne!(st & (1 << 25), 0, "127 : moins de moitié");
    assert!(g.wants_dma());
    g.write(0x0400_0600, 1 << 30, !0, 0);
    assert!(g.irq_pending(), "IRQ mode 1 : moins de moitié pleine");
    g.write(0x0400_0600, 2 << 30, !0, 0);
    assert!(!g.irq_pending(), "IRQ mode 2 : la FIFO n'est pas vide");
    let mut g = fill(4 + 128);
    let st = g.read(0x0400_0600, 0);
    assert_eq!((st >> 16) & 0x1FF, 128);
    assert_eq!(st & (1 << 25), 0, "128 : la moitié, plus « moins de moitié »");
    assert!(!g.wants_dma());
    g.write(0x0400_0600, 1 << 30, !0, 0);
    assert!(!g.irq_pending());
}

#[test]
fn gpu3d_geometry_registers_seen_from_the_bus() {
    let mut nds = Nds::new();
    nds.run_frame();
    let mut bus = nds.bus9();
    bus.write32(0x0400_0440, 1); // MTX_MODE position
    bus.write32(0x0400_0470, 0x1000); // MTX_TRANS x + 1.0
    bus.write32(0x0400_0470, 0);
    bus.write32(0x0400_0470, 0);
    bus.write32(0x0400_0444, 0); // MTX_PUSH
    bus.write32(0x0400_04A4, 0xC0 | 31 << 16); // POLYGON_ATTR
    bus.write32(0x0400_0500, 0); // BEGIN_VTXS triangles
    for (x, y) in [(-0x1800i32, 0), (-0x800, 0), (-0x1800, 0x800)] {
        bus.write32(0x0400_048C, (x as u32 & 0xFFFF) | (y as u32) << 16);
        bus.write32(0x0400_048C, 0);
    }
    bus.write32(0x0400_05C4, 0); // POS_TEST (0, 0, 0)
    bus.write32(0x0400_05C4, 0);
    // Le temps n'avance pas entre deux accès du test : on laisse passer
    // quelques lignes pour que le moteur exécute la file.
    nds.run_until(nds.now + 4 * LINE_CYCLES);
    let mut bus = nds.bus9();
    let st = bus.read32(0x0400_0600);
    assert_eq!(st & (1 << 27), 0, "file exécutée");
    assert_eq!(st >> 8 & 31, 1, "niveau de pile 1");
    assert_eq!(bus.read32(0x0400_0604), 1 | 3 << 16, "RAM_COUNT : 1 polygone, 3 sommets");
    assert_eq!(bus.read32(0x0400_0670), 0x1000, "CLIPMTX m[12] : translation");
    assert_eq!(bus.read32(0x0400_0620), 0x1000, "POS_RESULT x");
    assert_eq!(bus.read32(0x0400_062C), 0x1000, "POS_RESULT w");
    bus.write32(0x0400_0540, 0); // SWAP_BUFFERS
    nds.run_frame();
    assert_eq!(nds.hw.gpu3d.geo.rendering.polygons.len(), 1, "livré au rendu au VBlank");
    assert_eq!(nds.bus9().read32(0x0400_0604), 0, "RAM_COUNT remis à zéro");
}

#[test]
fn gpu3d_layer_is_engine_a_bg0() {
    use crate::nds::gpu3d::render::{Pixel, WIDTH as W3};
    let mut nds = Nds::new();
    nds.bus9().write16(0x0400_0304, 0x820F); // écrans, 2D A/B, 3D ; A en haut
    nds.bus9().write32(0x0400_0000, 0x0001_0109); // mode graphique, BG0 = 3D
    nds.bus9().write16(0x0500_0000, 0x7C00); // fond (backdrop) bleu
    nds.run_frame(); // le rendu 3D du VBlank efface le calque
    // Un pixel rouge opaque, un pixel vert d'alpha 0.
    let red = Pixel { rgb: [63, 0, 0], alpha: 31 };
    nds.hw.gpu3d.renderer.color.fill(Pixel { rgb: [0, 63, 0], alpha: 0 });
    nds.hw.gpu3d.renderer.color[10 * W3 + 20] = red;
    nds.run_until(nds.now + 192 * LINE_CYCLES); // les lignes visibles, sans le VBlank
    let top = nds.hw.gpu.screen(0);
    assert_eq!(top[10 * 256 + 20], 0xFF0000, "pixel 3D en rouge (6 bits ramenés à 5)");
    assert_eq!(top[10 * 256 + 21], 0x0000FF, "alpha 0 : transparent, le fond passe");
    // BG0HOFS : la 3D glisse vers la gauche ; 256..511 est transparent.
    nds.run_frame();
    nds.hw.gpu3d.renderer.color.fill(Pixel { rgb: [0; 3], alpha: 0 });
    nds.hw.gpu3d.renderer.color[10 * W3 + 20] = red;
    nds.bus9().write16(0x0400_0010, 16);
    nds.run_until(nds.now + 192 * LINE_CYCLES);
    let top = nds.hw.gpu.screen(0);
    assert_eq!(top[10 * 256 + 4], 0xFF0000, "décalé de 16");
    assert_eq!(top[10 * 256 + 20], 0x0000FF);
    nds.run_frame();
    nds.hw.gpu3d.renderer.color.fill(Pixel { rgb: [63; 3], alpha: 31 });
    nds.bus9().write16(0x0400_0010, 300);
    nds.run_until(nds.now + 192 * LINE_CYCLES);
    let top = nds.hw.gpu.screen(0);
    assert_eq!(top[10 * 256], 0x0000FF, "x = 300 : zone transparente");
    assert_eq!(top[10 * 256 + 255], 0xFFFFFF, "x = 555 → 43 : de nouveau la 3D");
}

#[test]
fn gpu3d_commands_to_pixels_end_to_end() {
    use crate::nds::gpu3d::render::WIDTH as W3;
    let mut nds = Nds::new();
    nds.run_frame();
    let tri = |nds: &mut Nds, swap: u32| {
        let mut bus = nds.bus9();
        bus.write32(0x0400_04A4, 0xC0 | 31 << 16); // POLYGON_ATTR opaque
        bus.write32(0x0400_0480, 31); // COLOR rouge
        bus.write32(0x0400_0500, 0); // BEGIN_VTXS triangles
        for (x, y) in [(-0x1000i32, -0x1000i32), (0, -0x1000), (-0x1000, 0)] {
            bus.write32(0x0400_048C, (x as u32 & 0xFFFF) | (y as u32) << 16);
            bus.write32(0x0400_048C, 0);
        }
        bus.write32(0x0400_0540, swap); // SWAP_BUFFERS
    };
    nds.bus9().write32(0x0400_0350, 31 << 10); // CLEAR_COLOR bleu, alpha 0
    nds.bus9().write32(0x0400_0354, 0x7FFF);
    tri(&mut nds, 2); // bits de cette liste-ci : ceux du SWAP précédent (0)
    nds.run_until(nds.now + 4 * LINE_CYCLES);
    nds.run_frame(); // VBlank : échange, puis rendu
    let r = &nds.hw.gpu3d.renderer;
    let at = |x: usize, y: usize| y * W3 + x;
    assert_eq!(r.color[at(10, 180)].rgb, [63, 0, 0], "triangle en bas à gauche");
    assert_eq!(r.color[at(200, 20)].alpha, 0, "plan arrière, alpha 0");
    assert_eq!(r.color[at(200, 20)].rgb, [0, 0, 63]);
    assert_eq!(r.depth[at(10, 180)], 0x80_0000, "tampon Z : z/w = 0 au milieu");
    // Liste suivante : construite après SWAP_BUFFERS(2), donc tampon W.
    tri(&mut nds, 0);
    nds.run_until(nds.now + 4 * LINE_CYCLES);
    nds.run_frame();
    assert_eq!(nds.hw.gpu3d.renderer.depth[at(10, 180)], 0x1000, "tampon W : w = 1.0");
}

// ======================================================================
// Capture d'écran (DISPCAPCNT)
// ======================================================================

/// Moteur A en mode graphique, fond uni (backdrop) `color`, banque D en LCDC.
fn capture_nds(color: u16) -> Nds {
    let mut nds = gpu_nds();
    nds.bus9().write32(0x0400_0000, 0x0001_0000); // mode graphique, aucun fond
    nds.bus9().write16(0x0500_0000, color);
    nds.bus9().write8(0x0400_0243, 0x80); // VRAMCNT_D : LCDC
    nds.run_frame();
    nds
}

fn bank_d(nds: &Nds, offset: usize) -> u16 {
    let b = &nds.hw.vram.banks[3];
    u16::from_le_bytes([b[offset], b[offset + 1]])
}

const CAP_ON: u32 = 1 << 31;
const CAP_D_FULL: u32 = 3 << 16 | 3 << 20; // banque D, 256×192

#[test]
fn capture_copies_the_graphics_screen_into_vram() {
    let mut nds = capture_nds(0x03E0);
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL);
    assert_eq!(bank_d(&nds, 0), 0, "rien avant la ligne 0 suivante");
    nds.run_frame(); // lignes 0..191 capturées, puis bit 31 retombe à 192
    assert_eq!(bank_d(&nds, 0), 0x83E0, "vert, alpha à 1");
    assert_eq!(bank_d(&nds, (191 * 256 + 255) * 2), 0x83E0, "dernier pixel");
    assert_eq!(nds.bus9().read32(0x0400_0064) & CAP_ON, 0, "bit 31 effacé à la ligne 192");
    // Image suivante : plus de capture.
    nds.bus9().write16(0x0500_0000, 0x001F);
    nds.run_frame();
    assert_eq!(bank_d(&nds, 0), 0x83E0);
}

#[test]
fn capture_blends_with_the_previous_vram_image() {
    let mut nds = capture_nds(0x001F); // rouge 31
    for o in (0..0x1_8000).step_by(2) {
        nds.hw.vram.banks[3][o..o + 2].copy_from_slice(&(0x8000u16 | 0x7C00).to_le_bytes()); // bleu 31
    }
    // A + B, EVA = 12, EVB = 4, lecture dans D (DISPCNT bits 18–19).
    nds.bus9().write32(0x0400_0000, 0x0001_0000 | 3 << 18);
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL | 2 << 29 | 12 | 4 << 8);
    nds.run_frame();
    // Rouge 31 × 12 / 16 = 23 ; bleu 31 × 4 / 16 = 7.
    assert_eq!(bank_d(&nds, 0), 0x8000 | 7 << 10 | 23);
    // Source B sans alpha : seule A compte.
    let mut nds = capture_nds(0x001F);
    nds.bus9().write32(0x0400_0000, 0x0001_0000 | 3 << 18);
    nds.hw.vram.banks[3][..2].copy_from_slice(&0x7C00u16.to_le_bytes());
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL | 2 << 29 | 12 | 4 << 8);
    nds.run_frame();
    assert_eq!(bank_d(&nds, 0), 0x8000 | 23);
    // Source B seule.
    let mut nds = capture_nds(0x001F);
    nds.bus9().write32(0x0400_0000, 0x0001_0000 | 3 << 18);
    nds.hw.vram.banks[3][2..4].copy_from_slice(&0x83E0u16.to_le_bytes());
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL | 1 << 29);
    nds.run_frame();
    assert_eq!(bank_d(&nds, 2), 0x83E0);
}

#[test]
fn capture_size_offset_and_3d_source() {
    use crate::nds::gpu3d::render::Pixel;
    // 128×128 au décalage 08000h : lignes de 128 pixels.
    let mut nds = capture_nds(0x001F);
    nds.bus9().write32(0x0400_0064, CAP_ON | 3 << 16 | 1 << 18);
    nds.run_frame();
    assert_eq!(bank_d(&nds, 0x8000 + (127 * 128 + 127) * 2), 0x801F);
    assert_eq!(bank_d(&nds, 0x8000 + 128 * 128 * 2), 0, "rien après 128 lignes");
    // Banque A (bits 16–17 = 0) : D reste intacte.
    let mut nds = capture_nds(0x001F);
    nds.bus9().write8(0x0400_0240, 0x80); // VRAMCNT_A : LCDC
    nds.bus9().write32(0x0400_0064, CAP_ON | 3 << 20);
    nds.run_frame();
    assert_eq!(u16::from_le_bytes([nds.hw.vram.banks[0][0], nds.hw.vram.banks[0][1]]), 0x801F);
    assert_eq!(bank_d(&nds, 0), 0);
    // Source A = écran 3D : couleur 6 bits ramenée à 5, alpha 0 respecté.
    let mut nds = capture_nds(0);
    nds.hw.gpu3d.renderer.color.fill(Pixel { rgb: [0, 0, 0], alpha: 0 });
    nds.hw.gpu3d.renderer.color[3] = Pixel { rgb: [62, 0, 2], alpha: 31 };
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL | 1 << 24);
    nds.run_until(nds.now + 192 * LINE_CYCLES);
    assert_eq!(bank_d(&nds, 6), 0x8000 | 1 << 10 | 31);
    assert_eq!(bank_d(&nds, 0), 0, "pixel 3D transparent : alpha 0");
}

#[test]
fn captured_bank_is_shown_in_vram_display_mode() {
    // Le schéma de l'écran titre de Pokémon N2 : capture de l'écran
    // graphique dans D, affichage de D (mode 2).
    let mut nds = capture_nds(0x001F);
    nds.bus9().write32(0x0400_0000, 0x0002_0000 | 3 << 18); // mode VRAM, banque D
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL);
    nds.run_frame(); // capture
    nds.bus9().write32(0x0400_0064, CAP_ON | CAP_D_FULL);
    nds.run_frame(); // affiche l'image capturée (et capture de nouveau)
    assert_eq!(pixel(&nds, 0, 100, 100), rgb(0x001F), "l'écran graphique, via la banque D");
}

#[test]
fn gpu3d_textured_triangle_end_to_end() {
    use crate::nds::gpu3d::render::WIDTH as W3;
    let mut nds = Nds::new();
    // Texture 8×8 en 16 couleurs, unie d'index 1, dans la banque A (via le
    // mode LCDC), puis A → slot de texture 0 ; palette dans la banque E.
    let mut bus = nds.bus9();
    bus.write8(0x0400_0240, 0x80); // A : LCDC
    for o in (0..32).step_by(4) {
        bus.write32(0x0680_0000 + o, 0x1111_1111);
    }
    bus.write8(0x0400_0244, 0x80); // E : LCDC
    bus.write16(0x0688_0000 + 2, 31 << 5); // palette[1] = vert
    bus.write8(0x0400_0240, 0x80 | 3); // A : texture, slot 0
    bus.write8(0x0400_0244, 0x80 | 3); // E : palettes
    bus.write16(0x0400_0060, 1); // DISP3DCNT : textures
    bus.write32(0x0400_0350, 0); // plan arrière
    bus.write32(0x0400_0354, 0x7FFF);
    nds.run_frame();
    let mut bus = nds.bus9();
    bus.write32(0x0400_04A4, 0xC0 | 31 << 16); // POLYGON_ATTR opaque, modulation
    bus.write32(0x0400_04A8, 3 << 26 | 1 << 16 | 1 << 17); // 16 couleurs, 8×8, répétée
    bus.write32(0x0400_04AC, 0); // PLTT_BASE 0
    bus.write32(0x0400_0480, 0x7FFF); // COLOR blanc
    bus.write32(0x0400_0500, 0); // BEGIN_VTXS
    for (x, y) in [(-0x1000i32, -0x1000i32), (0, -0x1000), (-0x1000, 0)] {
        bus.write32(0x0400_0488, 0); // TEXCOORD
        bus.write32(0x0400_048C, (x as u32 & 0xFFFF) | (y as u32) << 16);
        bus.write32(0x0400_048C, 0);
    }
    bus.write32(0x0400_0540, 0); // SWAP_BUFFERS
    nds.run_until(nds.now + 4 * LINE_CYCLES);
    nds.run_frame();
    let p = nds.hw.gpu3d.renderer.color[180 * W3 + 10];
    assert_eq!(p.rgb, [0, 63, 0], "texel vert (31 → 63) modulé par du blanc");
    assert_eq!(p.alpha, 31);
}

#[test]
fn gpu3d_layer_blends_with_its_own_alpha() {
    use crate::nds::gpu3d::render::{Pixel, WIDTH as W3};
    // Le réglage de l'introduction de Pokémon N2 : BLDCNT = 0941 (alpha,
    // BG0 en 1re cible, BG0 et BG3 en 2e), BLDALPHA = 0. Le calque 3D se
    // mélange avec son propre alpha, pas avec EVA = EVB = 0.
    let mut nds = Nds::new();
    nds.bus9().write16(0x0400_0304, 0x820F);
    nds.bus9().write32(0x0400_0000, 0x0001_0109); // BG0 = 3D
    nds.bus9().write16(0x0500_0000, 0x7C00); // fond bleu 31
    nds.bus9().write16(0x0400_0050, 0x2141); // BLDCNT : BG0 → (BG0, fond)
    nds.bus9().write16(0x0400_0052, 0); // BLDALPHA : EVA = EVB = 0
    nds.run_frame();
    nds.hw.gpu3d.renderer.color.fill(Pixel { rgb: [0; 3], alpha: 0 });
    nds.hw.gpu3d.renderer.color[10 * W3 + 20] = Pixel { rgb: [62, 0, 0], alpha: 31 };
    nds.hw.gpu3d.renderer.color[10 * W3 + 21] = Pixel { rgb: [62, 0, 0], alpha: 16 };
    nds.run_until(nds.now + 192 * LINE_CYCLES);
    let top = nds.hw.gpu.screen(0);
    // Opaque : EVA = 15, EVB = 1 → rouge 31 × 15 / 16 = 29, bleu 31 / 16 = 1.
    assert_eq!(top[10 * 256 + 20], rgb(29 | 1 << 10));
    // Alpha 16 : EVA = 8, EVB = 8 → rouge 15, bleu 15.
    assert_eq!(top[10 * 256 + 21], rgb(15 | 15 << 10));
    // Sans 2e cible sous le pixel : la couleur 3D telle quelle.
    nds.bus9().write16(0x0400_0050, 0x0141);
    nds.run_frame();
    nds.hw.gpu3d.renderer.color[10 * W3 + 20] = Pixel { rgb: [62, 0, 0], alpha: 31 };
    nds.run_until(nds.now + 192 * LINE_CYCLES);
    assert_eq!(nds.hw.gpu.screen(0)[10 * 256 + 20], rgb(31));
}

#[test]
fn gpu3d_skips_identical_renders() {
    use crate::nds::gpu3d::render::WIDTH as W3;
    let mut nds = Nds::new();
    nds.run_frame();
    let tri = |nds: &mut Nds| {
        let mut bus = nds.bus9();
        bus.write32(0x0400_04A4, 0xC0 | 31 << 16);
        bus.write32(0x0400_0480, 31);
        bus.write32(0x0400_0500, 0);
        for (x, y) in [(-0x1000i32, -0x1000i32), (0, -0x1000), (-0x1000, 0)] {
            bus.write32(0x0400_048C, (x as u32 & 0xFFFF) | (y as u32) << 16);
            bus.write32(0x0400_048C, 0);
        }
        bus.write32(0x0400_0540, 0);
    };
    nds.bus9().write32(0x0400_0354, 0x7FFF);
    tri(&mut nds);
    nds.run_frame(); // échange + rendu
    let skipped = nds.hw.gpu3d.renders_skipped;
    nds.run_frame(); // rien de neuf : rendu évité, image conservée
    assert_eq!(nds.hw.gpu3d.renders_skipped, skipped + 1);
    assert_eq!(nds.hw.gpu3d.renderer.color[180 * W3 + 10].rgb, [63, 0, 0]);
    // Un registre de rendu change : on redessine.
    nds.bus9().write32(0x0400_0350, 31 << 10 | 31 << 16); // plan arrière bleu opaque
    nds.run_frame();
    assert_eq!(nds.hw.gpu3d.renders_skipped, skipped + 1);
    assert_eq!(nds.hw.gpu3d.renderer.color[20 * W3 + 200].rgb, [0, 0, 63]);
    // La VRAM des textures change : on redessine aussi.
    nds.bus9().write8(0x0400_0240, 0x80);
    nds.bus9().write16(0x0680_0000, 1); // (les écritures 8 bits en VRAM sont ignorées)
    nds.bus9().write8(0x0400_0240, 0x83);
    nds.run_frame();
    assert_eq!(nds.hw.gpu3d.renders_skipped, skipped + 1);
}

#[test]
fn sound_produces_32728_stereo_samples_per_second() {
    let mut nds = Nds::new();
    for _ in 0..60 {
        nds.run_frame();
    }
    let n = nds.hw.sound.out.len() / 2;
    // Un échantillon toutes les 1024 périodes du bus (2048 cycles ARM9).
    let expected = nds.now() as f64 / 2048.0;
    assert!(expected > 32_000.0, "≈ une seconde émulée");
    assert!((n as f64 - expected).abs() <= 2.0, "{n} au lieu de ≈{expected:.0}");
    assert!(nds.hw.sound.out.iter().all(|&s| s == 0), "silence sans programme");
}

#[test]
fn swi_sound_bias_sets_the_level_and_keeps_the_rest() {
    let mut nds = Nds::new();
    let call = |nds: &mut Nds, level: u32| {
        let mut r = [level, 0x80, 0, 0];
        hle_swi::swi(&mut nds.bus7(), CpuId::Arm7, 0x08, &mut r)
    };
    assert_eq!(call(&mut nds, 1), hle_swi::Swi::Handled);
    assert_eq!(nds.bus7().read16(0x0400_0504), 0x200, "r0 ≠ 0 : 200h");
    call(&mut nds, 0);
    assert_eq!(nds.bus7().read16(0x0400_0504), 0, "r0 = 0 : 000h");
    call(&mut nds, 0x1234);
    assert_eq!(nds.hw.sound.bias, 0x200);
}

#[test]
fn direct_boot_leaves_the_sound_bias_at_its_usual_level() {
    let mut nds = Nds::new();
    nds.direct_boot(include_bytes!("../../tests/roms/boot_test.nds").to_vec()).unwrap();
    assert_eq!(nds.bus7().read16(0x0400_0504), 0x200);
}

// --- Palettes étendues (GBATEK « DS Video Extended Palettes ») ---

const YELLOW: u16 = 0x03FF;
const CYAN: u16 = 0x7FE0;

/// Écrit `color` à l'entrée `index` (palette × 256 + couleur) du slot `slot`
/// d'une banque, en la passant d'abord en LCDC, puis la place avec `cnt`.
/// `lcdc` : adresse LCDC de la banque.
fn ext_palette_bank(nds: &mut Nds, bank: u32, lcdc: u32, slot: u32, index: u32, color: u16, cnt: u8) {
    // VRAMCNT_A–G en 4000240h–4000246h, H et I en 4000248h–4000249h
    // (4000247h est WRAMCNT).
    let reg = if bank < 7 { 0x0400_0240 + bank } else { 0x0400_0241 + bank };
    let mut bus = nds.bus9();
    bus.write8(reg, 0x80);
    bus.write16(lcdc + slot * 0x2000 + index * 2, color);
    bus.write8(reg, cnt);
}

/// Moteur A, BG0 en texte 256 couleurs : tuile 1 = index 5 partout, carte
/// (0,0) = tuile 1 avec le numéro de palette `pnum`. Palette standard :
/// index 5 = rouge. `bgcnt_extra` est ajouté à BG0CNT.
fn ext_text_setup(nds: &mut Nds, pnum: u16, bgcnt_extra: u16, dispcnt_extra: u32) {
    let mut bus = nds.bus9();
    bus.write8(0x0400_0240, 0x81); // A → fonds du moteur A
    bus.write32(0x0400_0000, 0x0001_0100 | dispcnt_extra); // mode 0, BG0
    bus.write16(0x0400_0008, 0x80 | 2 << 8 | bgcnt_extra); // 256 couleurs, carte en 0x1000
    for i in 0..16 {
        bus.write32(0x0600_0040 + 4 * i, 0x0505_0505); // tuile 1 (64 octets)
    }
    bus.write16(0x0600_1000, 1 | pnum << 12);
    bus.write16(0x0500_0000, BLUE);
    bus.write16(0x0500_000A, RED);
}

#[test]
fn bg_ext_palette_picks_the_palette_from_the_map_entry() {
    let mut nds = gpu_nds();
    // E en palette étendue de fonds A (MST 4) : slot 0, palette 11, index 5.
    ext_palette_bank(&mut nds, 4, 0x0688_0000, 0, 11 * 256 + 5, GREEN, 0x84);
    ext_text_setup(&mut nds, 11, 0, 1 << 30);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(GREEN), "palette 11 du slot 0");
    assert_eq!(pixel(&nds, 0, 8, 0), rgb(BLUE), "tuile 0 : index 0 transparent");
    // Bit 30 à 0 : palette standard, numéro de palette ignoré.
    nds.bus9().write32(0x0400_0000, 0x0001_0100);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(RED));
    assert!(!nds.hw.gpu.unsupported.contains("palettes étendues de fonds"));
}

#[test]
fn bg_ext_palette_slots_per_background_and_bank() {
    // E couvre les slots 0–3 avec ses 32 Ko du bas : BG1 lit le slot 1.
    let mut nds = gpu_nds();
    ext_palette_bank(&mut nds, 4, 0x0688_0000, 1, 5, GREEN, 0x84);
    ext_palette_bank(&mut nds, 4, 0x0688_0000, 0, 5, YELLOW, 0x84);
    {
        ext_text_setup(&mut nds, 0, 0, 1 << 30);
        let mut bus = nds.bus9();
        bus.write32(0x0400_0000, 0x4001_0200); // BG1 seul
        bus.write16(0x0400_000A, 0x80 | 2 << 8); // BG1CNT = celui de BG0
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(GREEN), "BG1 → slot 1");
    // BG0 avec BGxCNT bit 13 → slot 2, fourni par F en OFS = 1 (slots 2–3).
    let mut nds = gpu_nds();
    ext_palette_bank(&mut nds, 5, 0x0689_0000, 0, 5, CYAN, 0x8C);
    ext_text_setup(&mut nds, 0, 1 << 13, 1 << 30);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(CYAN), "BG0 → slot 2 = premier slot de F (OFS 1)");
    // Même chose avec G en OFS = 0 : il couvre les slots 0–1, pas le 2.
    let mut nds = gpu_nds();
    ext_palette_bank(&mut nds, 6, 0x0689_4000, 0, 5, CYAN, 0x84);
    ext_text_setup(&mut nds, 0, 1 << 13, 1 << 30);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), 0, "slot 2 vide : couleur 0 (noir)");
    nds.bus9().write16(0x0400_0008, 0x80 | 2 << 8); // bit 13 à 0 → slot 0
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(CYAN), "G (OFS 0) → slot 0");
}

#[test]
fn bg_ext_palette_of_engine_b_lives_in_bank_h() {
    let mut nds = gpu_nds();
    // H en palette étendue de fonds B (MST 2) : slot 0, palette 2.
    ext_palette_bank(&mut nds, 7, 0x0689_8000, 0, 2 * 256 + 1, GREEN, 0x82);
    text_setup(&mut nds); // moteur B, BG0, tuile 1 = index 1, 2…
    {
        let mut bus = nds.bus9();
        bus.write32(0x0400_1000, 0x4001_0100);
        bus.write16(0x0400_1008, 0x80 | 2 << 8); // 256 couleurs
        bus.write16(0x0620_0040, 0x0101); // tuile 1, 256 couleurs : index 1
        bus.write16(0x0620_1000, 0x2001); // palette 2
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 0, 0), rgb(GREEN));
    // E n'est pas au moteur B : ses données ne s'y voient pas.
    let mut nds2 = gpu_nds();
    ext_palette_bank(&mut nds2, 4, 0x0688_0000, 0, 2 * 256 + 1, GREEN, 0x84);
    text_setup(&mut nds2);
    {
        let mut bus = nds2.bus9();
        bus.write32(0x0400_1000, 0x4001_0100);
        bus.write16(0x0400_1008, 0x80 | 2 << 8);
        bus.write16(0x0620_0040, 0x0101);
        bus.write16(0x0620_1000, 0x2001);
    }
    nds2.run_frame();
    assert_eq!(pixel(&nds2, 1, 0, 0), 0);
    // Et H ne sert pas au moteur A.
    let mut nds3 = gpu_nds();
    ext_palette_bank(&mut nds3, 7, 0x0689_8000, 0, 5, GREEN, 0x82);
    ext_text_setup(&mut nds3, 0, 0, 1 << 30);
    nds3.run_frame();
    assert_eq!(pixel(&nds3, 0, 0, 0), 0, "H : moteur B seulement");
}

#[test]
fn bg_ext_palette_only_for_16_bit_map_entries() {
    // Mode 5 : BG2 en rot/scal étendu à entrées 16 bits (BGxCNT bit 7 = 0),
    // BG3 en bitmap 256 couleurs. Seul BG2 prend la palette étendue.
    let mut nds = gpu_nds();
    ext_palette_bank(&mut nds, 4, 0x0688_0000, 2, 4 * 256 + 5, GREEN, 0x84);
    ext_palette_bank(&mut nds, 4, 0x0688_0000, 3, 5, YELLOW, 0x84);
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0240, 0x81);
        bus.write32(0x0400_0000, 0x4001_0405); // mode 5, BG2, palettes étendues
        bus.write16(0x0400_000C, 1 << 2 | 2 << 8); // BG2 : tuiles en 0x4000, carte en 0x1000
        bus.write32(0x0400_0020, 0x0000_0100); // PA = 1
        bus.write32(0x0400_0024, 0x0100_0000); // PD = 1
        for i in 0..16 {
            bus.write32(0x0600_4040 + 4 * i, 0x0505_0505); // tuile 1
        }
        bus.write16(0x0600_1000, 0x4001); // tuile 1, palette 4
        bus.write16(0x0500_000A, RED);
        bus.write16(0x0500_0000, BLUE);
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(GREEN), "rot/scal à entrées 16 bits : étendue");
    // BG3 en bitmap 256 couleurs : palette standard.
    {
        let mut bus = nds.bus9();
        bus.write32(0x0400_0000, 0x4001_0805);
        bus.write16(0x0400_000E, 0x0080 | 4 << 8); // bitmap 256 couleurs en 0x10000
        bus.write32(0x0400_0030, 0x0000_0100);
        bus.write32(0x0400_0034, 0x0100_0000);
        bus.write16(0x0601_0000, 0x0005);
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(RED), "bitmap : standard");
    // Mode 2 : BG2 en rot/scal classique (entrées 8 bits) : standard.
    {
        let mut bus = nds.bus9();
        bus.write32(0x0400_0000, 0x4001_0402);
        bus.write16(0x0400_000C, 1 << 2 | 2 << 8);
        bus.write16(0x0600_1000, 0x0001); // carte 8 bits : tuile 1
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 0, 0), rgb(RED), "rot/scal 8 bits : standard");
}

#[test]
fn bg_ext_palette_leaves_16_color_tiles_alone() {
    let mut nds_a = gpu_nds();
    ext_palette_bank(&mut nds_a, 4, 0x0688_0000, 0, 1, GREEN, 0x84);
    {
        let mut bus = nds_a.bus9();
        bus.write8(0x0400_0240, 0x81);
        bus.write32(0x0400_0000, 0x4001_0100);
        bus.write16(0x0400_0008, 2 << 8); // 16 couleurs
        bus.write16(0x0600_0020, 0x0001); // tuile 1 : index 1
        bus.write16(0x0600_1000, 0x0001);
        bus.write16(0x0500_0002, RED);
    }
    nds_a.run_frame();
    assert_eq!(pixel(&nds_a, 0, 0, 0), rgb(RED), "16 couleurs : standard");
}

#[test]
fn obj_ext_palette_for_256_color_sprites() {
    // F en palette étendue de sprites A (MST 5) : palette 6, index 1.
    let mut nds = obj_nds(0x1010 | 1 << 31);
    ext_palette_bank(&mut nds, 5, 0x0689_0000, 0, 6 * 256 + 1, GREEN, 0x85);
    {
        let mut bus = nds.bus9();
        bus.write16(0x0640_0000, 0x0101); // tuile 0, 256 couleurs : index 1, 1
        bus.write16(0x0500_0202, RED);
    }
    sprite(&mut nds, 0, 20 | 0x2000, 10, 6 << 12); // 256 couleurs, palette 6
    // Sprite 16 couleurs, tuile 2, palette 0 : standard.
    nds.bus9().write16(0x0640_0040, 0x0001); // tuile 2 : index 1
    sprite(&mut nds, 1, 40, 10, 2);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(GREEN), "sprite 256 couleurs : palette étendue 6");
    assert_eq!(pixel(&nds, 0, 10, 40), rgb(RED), "sprite 16 couleurs : standard");
    // Bit 31 à 0 : palette standard.
    nds.bus9().write32(0x0400_0000, 0x0001_1010);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), rgb(RED));
    // Seuls les 8 Ko du bas de F servent : le « slot 1 » de F est ignoré.
    let mut nds = obj_nds(0x1010 | 1 << 31);
    ext_palette_bank(&mut nds, 5, 0x0689_0000, 1, 6 * 256 + 1, GREEN, 0x85);
    nds.bus9().write16(0x0640_0000, 0x0101);
    sprite(&mut nds, 0, 20 | 0x2000, 10, 6 << 12);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), 0);
}

#[test]
fn obj_ext_palette_of_engine_b_lives_in_bank_i() {
    let mut nds = gpu_nds();
    // I en palette étendue de sprites B (MST 3) ; D en sprites du moteur B.
    ext_palette_bank(&mut nds, 8, 0x068A_0000, 0, 256 + 1, GREEN, 0x83);
    {
        let mut bus = nds.bus9();
        bus.write8(0x0400_0243, 0x84); // D → sprites du moteur B
        bus.write32(0x0400_1000, 0x8001_1010); // sprites, 1D, palettes étendues
        for i in 0..128 {
            bus.write16(0x0700_0400 + 8 * i, 0x0200);
        }
        bus.write16(0x0660_0000, 0x0101);
        bus.write16(0x0700_0400, 20 | 0x2000);
        bus.write16(0x0700_0402, 10);
        bus.write16(0x0700_0404, 1 << 12);
    }
    nds.run_frame();
    assert_eq!(pixel(&nds, 1, 10, 20), rgb(GREEN));
    // I ne sert pas au moteur A.
    let mut nds = obj_nds(0x1010 | 1 << 31);
    ext_palette_bank(&mut nds, 8, 0x068A_0000, 0, 256 + 1, GREEN, 0x83);
    nds.bus9().write16(0x0640_0000, 0x0101);
    sprite(&mut nds, 0, 20 | 0x2000, 10, 1 << 12);
    nds.run_frame();
    assert_eq!(pixel(&nds, 0, 10, 20), 0, "I : moteur B seulement");
}
