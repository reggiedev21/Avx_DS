/* ROM de test de la file 3D, partie ARM7 : rien à faire. */
void main7(void) {
    for (;;) __asm__ volatile("swi 0x060000");
}
