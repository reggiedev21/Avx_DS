@ Point d'entrée des programmes de test (ARM, compatible ARMv4T).
@ Prépare une pile, appelle run_tests(RESULTS) puis signale la fin
@ par SWI 0xFF (intercepté par le bus de test).

    .syntax unified
    .arm
    .section .text.start, "ax"
    .global _start
_start:
    ldr sp, =0x000F0000
    ldr r0, =0x00080000
    ldr r1, =run_tests
    mov lr, pc
    bx r1
    swi #0xFF0000
1:  b 1b
    .pool
