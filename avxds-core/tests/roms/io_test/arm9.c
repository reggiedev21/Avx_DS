/* ROM de test des entrées-sorties, partie ARM9 : lit la cartouche par
 * scrutation puis par DMA, et l'identifiant de puce. */
#include "../boot_test/common.h"

#define AUXSPICNT REG16(0x040001A0)
#define ROMCTRL REG32(0x040001A4)
#define CARDCMD0 REG32(0x040001A8)
#define CARDCMD1 REG32(0x040001AC)
#define CARDDATA REG32(0x04100010)
#define DMA3SAD REG32(0x040000D4)
#define DMA3DAD REG32(0x040000D8)
#define DMA3CNT REG32(0x040000DC)

#define RESULTS ((volatile u32 *)0x02300000)
#define BUF_POLL ((volatile u32 *)0x02310000)
#define BUF_DMA ((volatile u32 *)0x02310200)

static void command(u8 c, u32 arg) {
    /* Octets : CMD0 = c, arg (MSB d'abord), CMD1 = 0. */
    CARDCMD0 = c | ((arg >> 24) & 0xFF) << 8 | ((arg >> 16) & 0xFF) << 16 | ((arg >> 8) & 0xFF) << 24;
    CARDCMD1 = arg & 0xFF;
}

#define START (1u << 31)
#define RESB (1u << 29)
#define READY (1u << 23)
#define BLOCK(n) ((u32)(n) << 24)

void main9(void) {
    RESULTS[0] = 0x900D0009;
    AUXSPICNT = 0x8000; /* slot activé, mode ROM */

    /* 1. B7 à 0x8000, 0x200 octets, scrutation de ROMCTRL bit 23. */
    command(0xB7, 0x8000);
    ROMCTRL = START | RESB | BLOCK(1);
    u32 n = 0;
    while (ROMCTRL & READY) BUF_POLL[n++] = CARDDATA;
    RESULTS[1] = n;
    RESULTS[2] = ROMCTRL & START;

    /* 2. B7 à 0x9000 par DMA3 (mode cartouche, source fixe, répétition). */
    DMA3SAD = 0x04100010;
    DMA3DAD = (u32)BUF_DMA;
    DMA3CNT = 1u << 31 | 1u << 26 | 5u << 27 | 1u << 25 | 2u << 23 | 1;
    command(0xB7, 0x9000);
    ROMCTRL = START | RESB | BLOCK(1);
    while (ROMCTRL & START) {}
    DMA3CNT = 0;
    RESULTS[3] = 1;

    /* 3. Identifiant de puce (B8, 4 octets). */
    command(0xB8, 0);
    ROMCTRL = START | RESB | BLOCK(7);
    while (!(ROMCTRL & READY)) {}
    RESULTS[4] = CARDDATA;

    /* Attendre l'ARM7. */
    while ((IPCSYNC & 0xF) != 0xD) {}
    RESULTS[15] = 0x600DB007;
    for (;;) __asm__ volatile("swi 0x06");
}
