/* Partie ARM9 de la ROM de test du direct boot (compilée en Thumb). */
#include "common.h"

#define BIOS_IRQ_HANDLER REG32(0x03003FFC) /* DTCM + 0x3FFC */
#define BIOS_IRQ_FLAGS REG32(0x03003FF8)   /* DTCM + 0x3FF8 */
#define RESULTS ((volatile u32 *)0x02300000)

static volatile u32 vblanks;

/* Appelé par le BIOS en mode IRQ (via LDR PC, donc en Thumb sur ARMv5). */
static void irq_handler(void) {
    u32 f = IF & IE;
    if (f & 1) vblanks++;
    IF = f;
    BIOS_IRQ_FLAGS |= f; /* pour IntrWait / VBlankIntrWait */
}

static inline void vblank_intr_wait(void) {
    __asm__ volatile("swi 0x05" ::: "r0", "r1", "r2", "r3", "r12", "memory");
}

static inline s32 bios_div(s32 n, s32 d, s32 *rem, u32 *absq) {
    register s32 r0 __asm__("r0") = n;
    register s32 r1 __asm__("r1") = d;
    register u32 r3 __asm__("r3");
    __asm__ volatile("swi 0x09" : "+r"(r0), "+r"(r1), "=r"(r3) :: "r2", "r12");
    *rem = r1;
    *absq = r3;
    return r0;
}

static inline void bios_fast_set(const void *src, void *dst, u32 ctrl) {
    register const void *r0 __asm__("r0") = src;
    register void *r1 __asm__("r1") = dst;
    register u32 r2 __asm__("r2") = ctrl;
    __asm__ volatile("swi 0x0C" : "+r"(r0), "+r"(r1), "+r"(r2) :: "r3", "r12", "memory");
}

static u32 src_buf[16], dst_buf[16];

void main9(void) {
    RESULTS[0] = 0x900D0001;

    BIOS_IRQ_HANDLER = (u32)irq_handler;
    DISPSTAT = 1 << 3; /* IRQ VBlank */
    IE = 1;
    IME = 1;

    for (int i = 0; i < 3; i++) vblank_intr_wait();
    RESULTS[1] = vblanks;
    RESULTS[2] = VCOUNT;

    s32 rem;
    u32 absq;
    RESULTS[3] = (u32)bios_div(-1000, 7, &rem, &absq);
    RESULTS[4] = (u32)rem;
    RESULTS[5] = absq;

    REG16(0x04000280) = 0; /* diviseur matériel 32/32 */
    REG32(0x04000290) = 1000000;
    REG32(0x04000298) = (u32)-3;
    while (REG16(0x04000280) & 0x8000) {}
    RESULTS[6] = REG32(0x040002A0);

    IPCFIFOCNT = FIFO_ENABLE | FIFO_CLEAR;
    for (u32 k = 1; k <= 4; k++) IPCFIFOSEND = k * 100;
    for (u32 k = 0; k < 4; k++) {
        while (IPCFIFOCNT & FIFO_RECV_EMPTY) {}
        RESULTS[7 + k] = IPCFIFORECV;
    }
    while ((IPCSYNC & 0xF) != 0xA) {}
    RESULTS[11] = IPCSYNC & 0xF;

    for (u32 i = 0; i < 16; i++) src_buf[i] = i * 0x01010101u;
    bios_fast_set(src_buf, dst_buf, 16);
    u32 sum = 0;
    for (u32 i = 0; i < 16; i++) sum = sum * 31 + dst_buf[i];
    RESULTS[12] = sum;

    /* Timer 0 en F/64 : nombre de tics pendant une image. */
    TM0CNT = 0x00810000; /* F/64, activé, rechargement 0 */
    vblank_intr_wait();
    u32 t0 = TM0CNT & 0xFFFF;
    vblank_intr_wait();
    u32 t1 = TM0CNT & 0xFFFF;
    RESULTS[13] = (t1 - t0) & 0xFFFF;

    RESULTS[15] = 0x600DB007;
    for (;;) __asm__ volatile("swi 0x06"); /* Halt */
}
