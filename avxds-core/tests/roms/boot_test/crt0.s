@ Démarrage commun ARM9 / ARM7 (ARM, compatible ARMv4T).
@ La pile est déjà prête (direct boot). On efface le BSS, on passe en
@ mode System avec les IRQ autorisées, puis on appelle MAIN.
    .syntax unified
    .arm
    .section .text.start, "ax"
    .global _start
_start:
    ldr r0, =__bss_start
    ldr r1, =__bss_end
    mov r2, #0
1:  cmp r0, r1
    strlo r2, [r0], #4
    blo 1b
    msr cpsr_c, #0x1F
    ldr r0, =MAIN
    mov lr, pc
    bx r0
2:  b 2b
    .pool
