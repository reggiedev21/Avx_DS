/* ROM de test du son, partie ARM7 : un triangle PCM16 en boucle sur le
 * canal 0 (tout à gauche) et un rectangle PSG 50 % sur le canal 8 (tout à
 * droite), tous deux de période 32 échantillons de sortie. Puis l'ARM7
 * dort : le son continue tout seul. */
#define REG32(a) (*(volatile unsigned int *)(a))
#define REG16(a) (*(volatile unsigned short *)(a))

static short wave[32];

void main7(void) {
    for (int i = 0; i < 32; i++) {
        int t = i < 16 ? i : 32 - i; /* 0..16..1 */
        wave[i] = (short)((t - 8) * 0x800);
    }
    REG16(0x04000500) = 0x8000 | 127; /* mixeur actif, volume général 127 */
    REG16(0x04000504) = 0x200;        /* biais */
    /* Canal 0 : PCM16, boucle, volume 127, pan 0 ; un pas par sortie. */
    REG32(0x04000404) = (unsigned int)wave;
    REG32(0x04000408) = 0x10000 - 512;   /* TMR, PNT = 0 */
    REG32(0x0400040C) = sizeof wave / 4; /* LEN en mots */
    REG32(0x04000400) = 0x80000000u | 1u << 29 | 1u << 27 | 127;
    /* Canal 8 : PSG duty 3 (50 %), pan 127 ; 8 pas de 4 sorties. */
    REG32(0x04000488) = 0x10000 - 2048;
    REG32(0x04000480) = 0x80000000u | 3u << 29 | 3u << 24 | 127u << 16 | 127;
    for (;;) __asm__ volatile("swi 0x060000");
}
