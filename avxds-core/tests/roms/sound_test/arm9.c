/* ROM de test du son, partie ARM9 : rien à faire. */
void main9(void) {
    for (;;) __asm__ volatile("swi 0x06");
}
