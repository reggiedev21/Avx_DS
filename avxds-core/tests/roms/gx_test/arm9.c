/* ROM de test de la file de commandes 3D (étape 3D-1), partie ARM9.
 *
 * Envoie chaque commande de géométrie valide par les quatre chemins
 * (ports, GXFIFO non compact, GXFIFO compact, DMA en mode « FIFO
 * géométrie ») et relève GXSTAT autour d'un SwapBuffers. Le nombre de
 * paramètres de chaque commande est recopié ici depuis GBATEK (« DS 3D
 * I/O Map »), indépendamment de la table du moteur : une erreur dans
 * l'une ou l'autre fausse le compte des commandes exécutées. */
#include "../boot_test/common.h"

#define POWCNT1 REG16(0x04000304)
#define GXFIFO REG32(0x04000400)
#define GXSTAT REG32(0x04000600)
#define PORT(cmd) REG32(0x04000400 + 4 * (cmd))
#define DMA0SAD REG32(0x040000B0)
#define DMA0DAD REG32(0x040000B4)
#define DMA0CNT REG32(0x040000B8)

#define RESULTS ((volatile u32 *)0x02300000)
#define DMA_BUF ((volatile u32 *)0x02310000)

#define SWAP_BUFFERS 0x50
#define COLOR 0x20

/* Code et nombre de paramètres, d'après GBATEK ; SWAP_BUFFERS est à part. */
static const u8 CMDS[][2] = {
    {0x10, 1}, {0x11, 0}, {0x12, 1}, {0x13, 1}, {0x14, 1}, {0x15, 0}, {0x16, 16},
    {0x17, 12}, {0x18, 16}, {0x19, 12}, {0x1A, 9}, {0x1B, 3}, {0x1C, 3}, {0x20, 1},
    {0x21, 1}, {0x22, 1}, {0x23, 2}, {0x24, 1}, {0x25, 1}, {0x26, 1}, {0x27, 1},
    {0x28, 1}, {0x29, 1}, {0x2A, 1}, {0x2B, 1}, {0x30, 1}, {0x31, 1}, {0x32, 1},
    {0x33, 1}, {0x34, 32}, {0x40, 1}, {0x41, 0}, {0x60, 1}, {0x70, 3}, {0x71, 2},
    {0x72, 1},
};
#define NCMDS (sizeof CMDS / sizeof CMDS[0])

/* Paramètre factice : MTX_MODE reste en mode 0, le reste est sans effet
 * sur la file. */
static u32 param(u8 cmd, u32 k) {
    return cmd == 0x10 ? 0 : 0x100 + k;
}

static void wait_vblank_start(void) {
    while (VCOUNT == 192) {}
    while (VCOUNT != 192) {}
}

void main9(void) {
    POWCNT1 = 0x020F;
    for (int i = 0; i < 16; i++) RESULTS[i] = 0;

    /* 1. Ports : N écritures pour N paramètres, une seule sans paramètre. */
    for (u32 i = 0; i < NCMDS; i++) {
        u8 c = CMDS[i][0], n = CMDS[i][1];
        if (n == 0) PORT(c) = 0;
        for (u32 k = 0; k < n; k++) PORT(c) = param(c, k);
    }
    /* 2. GXFIFO non compact : le code seul, puis ses paramètres. */
    for (u32 i = 0; i < NCMDS; i++) {
        u8 c = CMDS[i][0], n = CMDS[i][1];
        GXFIFO = c;
        for (u32 k = 0; k < n; k++) GXFIFO = param(c, k);
    }
    /* 3. GXFIFO compact, par paires : deux codes, puis leurs paramètres. */
    for (u32 i = 0; i < NCMDS; i += 2) {
        u8 c0 = CMDS[i][0], n0 = CMDS[i][1];
        u8 c1 = i + 1 < NCMDS ? CMDS[i + 1][0] : 0;
        u8 n1 = i + 1 < NCMDS ? CMDS[i + 1][1] : 0;
        GXFIFO = c0 | (u32)c1 << 8;
        for (u32 k = 0; k < n0; k++) GXFIFO = param(c0, k);
        for (u32 k = 0; k < n1; k++) GXFIFO = param(c1, k);
    }
    /* Tout est exécuté avant le SwapBuffers. */
    while (GXSTAT & (1 << 27)) {}
    RESULTS[0] = GXSTAT;

    /* 4. SwapBuffers : le moteur reste occupé jusqu'au VBlank. */
    while (VCOUNT >= 192) {}
    PORT(SWAP_BUFFERS) = 0;
    RESULTS[1] = GXSTAT;            /* occupé */
    RESULTS[2] = VCOUNT;            /* avant la ligne 192 */
    while (GXSTAT & (1 << 27)) {}
    RESULTS[3] = VCOUNT;            /* libre à partir de la ligne 192 */

    /* 5. DMA « FIFO géométrie » : SwapBuffers, puis 200 COLOR non compacts.
     * Le moteur s'arrête sur SwapBuffers : la DMA ne peut envoyer que
     * quelques rafales avant que la FIFO dépasse la moitié. */
    while (VCOUNT >= 192) {}
    DMA_BUF[0] = SWAP_BUFFERS;
    DMA_BUF[1] = 0;
    for (u32 k = 0; k < 200; k++) {
        DMA_BUF[2 + 2 * k] = COLOR;
        DMA_BUF[3 + 2 * k] = k;
    }
    DMA0SAD = (u32)DMA_BUF;
    DMA0DAD = 0x04000400;
    DMA0CNT = 0x80000000u | 7u << 27 | 1u << 26 | 2u << 21 | 402;
    RESULTS[4] = GXSTAT;            /* FIFO au-delà de la moitié */
    RESULTS[5] = DMA0CNT >> 31;     /* transfert encore en cours */
    while (DMA0CNT & 0x80000000u) {}
    while (GXSTAT & (1 << 27)) {}
    RESULTS[6] = GXSTAT;
    wait_vblank_start();
    RESULTS[15] = 0x600DB007;
    for (;;) {}
}
