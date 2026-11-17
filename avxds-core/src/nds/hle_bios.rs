//! Mini-BIOS d'AvxDS, écrit pour ce projet (aucun octet du BIOS Nintendo).
//!
//! Il ne contient que ce que le HLE en Rust ne peut pas faire :
//! * les vecteurs d'exception ;
//! * le relais d'IRQ vers le gestionnaire du jeu (DTCM+0x3FFC sur l'ARM9,
//!   0x0380FFFC sur l'ARM7), comme le vrai BIOS ;
//! * IntrWait, VBlankIntrWait et Halt, qui doivent *attendre* : ils mettent
//!   le CPU en pause et laissent passer les IRQ.
//!
//! Les autres SWI (Div, CpuSet, LZ77…) sont traités en Rust par
//! `Bus::hle_swi` et n'arrivent jamais ici.
//!
//! Comme le BIOS du GBA et de la DS, le gestionnaire SWI repasse en mode
//! System avec le bit I de l'appelant : les IRQ restent possibles pendant
//! l'attente.
//!
//! Blocs régénérables : `python3 tools/asm.py src/nds/hle_bios.rs`.

/// BIOS ARM9 (placé en 0xFFFF0000).
pub const BIOS9: &[u32] = // asm: arm
&[
    0xEAFFFFFE, // reset: b reset
    0xEAFFFFFE, // undef: b undef
    0xEA00000D, // b swi_handler
    0xEAFFFFFE, // pabt: b pabt
    0xEAFFFFFE, // dabt: b dabt
    0xEAFFFFFE, // resv: b resv
    0xEA000000, // b irq_handler
    0xEAFFFFFE, // fiq: b fiq
    0xE92D500F, // irq_handler: stmfd sp!, {r0-r3, r12, lr}
    0xEE190F11, // mrc p15, 0, r0, c9, c1, 0
    0xE1A00620, // mov r0, r0, lsr #12
    0xE1A00600, // mov r0, r0, lsl #12
    0xE2800901, // add r0, r0, #0x4000
    0xE28FE000, // add lr, pc, #0
    0xE510F004, // ldr pc, [r0, #-4]
    0xE8BD500F, // ldmfd sp!, {r0-r3, r12, lr}
    0xE25EF004, // subs pc, lr, #4
    0xE92D5800, // swi_handler: stmfd sp!, {r11, r12, lr}
    0xE14FB000, // mrs r11, spsr
    0xE92D0800, // stmfd sp!, {r11}
    0xE31B0020, // tst r11, #0x20
    0x115EC0B2, // ldrhne r12, [lr, #-2]
    0x051EC004, // ldreq r12, [lr, #-4]
    0x01A0C82C, // moveq r12, r12, lsr #16
    0xE20CC0FF, // and r12, r12, #0xFF
    0xE20BB080, // and r11, r11, #0x80
    0xE38BB01F, // orr r11, r11, #0x1F
    0xE121F00B, // msr cpsr_c, r11
    0xE92D400C, // stmfd sp!, {r2, r3, lr}
    0xE35C0004, // cmp r12, #4
    0x0A00000F, // beq intr_wait
    0xE35C0005, // cmp r12, #5
    0x0A00000B, // beq vblank_intr_wait
    0xE35C0006, // cmp r12, #6
    0x0A000006, // beq halt
    0xE8BD400C, // swi_return: ldmfd sp!, {r2, r3, lr}
    0xE3A0C0D3, // mov r12, #0xD3
    0xE121F00C, // msr cpsr_c, r12
    0xE8BD0800, // ldmfd sp!, {r11}
    0xE16FF00B, // msr spsr_fsxc, r11
    0xE8BD5800, // ldmfd sp!, {r11, r12, lr}
    0xE1B0F00E, // movs pc, lr
    0xE3A02000, // halt: mov r2, #0
    0xEE072F90, // mcr p15, 0, r2, c7, c0, 4
    0xEAFFFFF5, // b swi_return
    0xE3A00001, // vblank_intr_wait: mov r0, #1
    0xE3A01001, // mov r1, #1
    0xE3A0C301, // intr_wait: mov r12, #0x04000000
    0xEE193F11, // mrc p15, 0, r3, c9, c1, 0
    0xE1A03623, // mov r3, r3, lsr #12
    0xE1A03603, // mov r3, r3, lsl #12
    0xE2833901, // add r3, r3, #0x4000
    0xE2433008, // sub r3, r3, #8
    0xE3500000, // cmp r0, #0
    0x0A000004, // beq iw_check
    0xE3A02000, // mov r2, #0
    0xE5CC2208, // strb r2, [r12, #0x208]
    0xE5932000, // ldr r2, [r3]
    0xE1C22001, // bic r2, r2, r1
    0xE5832000, // str r2, [r3]
    0xE3A02000, // iw_check: mov r2, #0
    0xE5CC2208, // strb r2, [r12, #0x208]
    0xE5932000, // ldr r2, [r3]
    0xE0120001, // ands r0, r2, r1
    0x11C22001, // bicne r2, r2, r1
    0x15832000, // strne r2, [r3]
    0xE3A02001, // mov r2, #1
    0xE5CC2208, // strb r2, [r12, #0x208]
    0x1AFFFFDD, // bne swi_return
    0xE3A02000, // mov r2, #0
    0xEE072F90, // mcr p15, 0, r2, c7, c0, 4
    0xEAFFFFF3, // b iw_check
];

/// BIOS ARM7 (placé en 0x00000000). Même structure ; la pause passe par
/// HALTCNT et les drapeaux d'IRQ du BIOS sont en 0x0380FFF8.
pub const BIOS7: &[u32] = // asm: arm-v4
&[
    0xEAFFFFFE, // reset: b reset
    0xEAFFFFFE, // undef: b undef
    0xEA00000A, // b swi_handler
    0xEAFFFFFE, // pabt: b pabt
    0xEAFFFFFE, // dabt: b dabt
    0xEAFFFFFE, // resv: b resv
    0xEA000000, // b irq_handler
    0xEAFFFFFE, // fiq: b fiq
    0xE92D500F, // irq_handler: stmfd sp!, {r0-r3, r12, lr}
    0xE3A00301, // mov r0, #0x04000000
    0xE28FE000, // add lr, pc, #0
    0xE510F004, // ldr pc, [r0, #-4]
    0xE8BD500F, // ldmfd sp!, {r0-r3, r12, lr}
    0xE25EF004, // subs pc, lr, #4
    0xE92D5800, // swi_handler: stmfd sp!, {r11, r12, lr}
    0xE14FB000, // mrs r11, spsr
    0xE92D0800, // stmfd sp!, {r11}
    0xE31B0020, // tst r11, #0x20
    0x115EC0B2, // ldrhne r12, [lr, #-2]
    0x051EC004, // ldreq r12, [lr, #-4]
    0x01A0C82C, // moveq r12, r12, lsr #16
    0xE20CC0FF, // and r12, r12, #0xFF
    0xE20BB080, // and r11, r11, #0x80
    0xE38BB01F, // orr r11, r11, #0x1F
    0xE121F00B, // msr cpsr_c, r11
    0xE92D400C, // stmfd sp!, {r2, r3, lr}
    0xE35C0004, // cmp r12, #4
    0x0A000012, // beq intr_wait
    0xE35C0005, // cmp r12, #5
    0x0A00000E, // beq vblank_intr_wait
    0xE35C0006, // cmp r12, #6
    0x0A000008, // beq halt
    0xE35C0007, // cmp r12, #7
    0x0A000006, // beq halt
    0xE8BD400C, // swi_return: ldmfd sp!, {r2, r3, lr}
    0xE3A0C0D3, // mov r12, #0xD3
    0xE121F00C, // msr cpsr_c, r12
    0xE8BD0800, // ldmfd sp!, {r11}
    0xE16FF00B, // msr spsr_fsxc, r11
    0xE8BD5800, // ldmfd sp!, {r11, r12, lr}
    0xE1B0F00E, // movs pc, lr
    0xE3A03301, // halt: mov r3, #0x04000000
    0xE3A02080, // mov r2, #0x80
    0xE5C32301, // strb r2, [r3, #0x301]
    0xEAFFFFF4, // b swi_return
    0xE3A00001, // vblank_intr_wait: mov r0, #1
    0xE3A01001, // mov r1, #1
    0xE3A0C301, // intr_wait: mov r12, #0x04000000
    0xE3A0350E, // mov r3, #0x03800000
    0xE3833CFF, // orr r3, r3, #0xFF00
    0xE38330F8, // orr r3, r3, #0xF8
    0xE3500000, // cmp r0, #0
    0x0A000004, // beq iw_check
    0xE3A02000, // mov r2, #0
    0xE5CC2208, // strb r2, [r12, #0x208]
    0xE5932000, // ldr r2, [r3]
    0xE1C22001, // bic r2, r2, r1
    0xE5832000, // str r2, [r3]
    0xE3A02000, // iw_check: mov r2, #0
    0xE5CC2208, // strb r2, [r12, #0x208]
    0xE5932000, // ldr r2, [r3]
    0xE0120001, // ands r0, r2, r1
    0x11C22001, // bicne r2, r2, r1
    0x15832000, // strne r2, [r3]
    0xE3A02001, // mov r2, #1
    0xE5CC2208, // strb r2, [r12, #0x208]
    0x1AFFFFDE, // bne swi_return
    0xE3A02080, // mov r2, #0x80
    0xE5CC2301, // strb r2, [r12, #0x301]
    0xEAFFFFF3, // b iw_check
];

/// Octets prêts à charger dans la zone BIOS.
pub fn bytes(code: &[u32]) -> Vec<u8> {
    code.iter().flat_map(|w| w.to_le_bytes()).collect()
}
