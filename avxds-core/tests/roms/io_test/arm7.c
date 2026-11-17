/* ROM de test des entrées-sorties, partie ARM7 : écran tactile, firmware,
 * contrôleur d'alimentation et horloge, pilotés comme le ferait libnds. */
#include "../boot_test/common.h"

#define EXTKEYIN REG16(0x04000136)
#define RTC REG16(0x04000138)
#define SPICNT REG16(0x040001C0)
#define SPIDATA REG16(0x040001C2)

#define RESULTS ((volatile u32 *)0x02300040)

#define SPI_ENABLE (1 << 15)
#define SPI_HOLD (1 << 11)
#define SPI_BUSY (1 << 7)
#define DEV_POWER (0 << 8)
#define DEV_FIRMWARE (1 << 8)
#define DEV_TOUCH (2 << 8)

static u8 spi(u16 dev, u8 v, int last) {
    SPICNT = SPI_ENABLE | dev | (last ? 0 : SPI_HOLD);
    SPIDATA = v;
    while (SPICNT & SPI_BUSY) {}
    return SPIDATA & 0xFF;
}

static u16 touch(u8 channel) {
    spi(DEV_TOUCH, 0x80 | channel << 4, 0);
    u16 hi = spi(DEV_TOUCH, 0, 0);
    u16 lo = spi(DEV_TOUCH, 0, 1);
    return (hi << 5) | (lo >> 3);
}

static void fw_read(u32 addr, volatile u8 *out, u32 len) {
    spi(DEV_FIRMWARE, 0x03, 0);
    spi(DEV_FIRMWARE, addr >> 16, 0);
    spi(DEV_FIRMWARE, addr >> 8, 0);
    spi(DEV_FIRMWARE, addr, 0);
    for (u32 i = 0; i < len; i++) out[i] = spi(DEV_FIRMWARE, 0, i == len - 1);
}

/* Horloge : bit 0 données, 1 /SCK, 2 CS, 4 sens des données ; 0x60 =
 * /SCK et CS en sortie. Commande MSB d'abord, données LSB d'abord. */
static void rtc_byte_out(u8 b, int msb) {
    for (int i = 0; i < 8; i++) {
        u16 bit = msb ? (b >> (7 - i)) & 1 : (b >> i) & 1;
        RTC = 0x74 | bit;
        RTC = 0x76 | bit;
    }
}

static u8 rtc_byte_in(void) {
    u8 b = 0;
    for (int i = 0; i < 8; i++) {
        RTC = 0x64;
        RTC = 0x66;
        b |= (RTC & 1) << i;
    }
    return b;
}

static void rtc_read(u8 cmd, volatile u8 *out, int len) {
    RTC = 0x62;
    RTC = 0x66;
    rtc_byte_out(cmd, 1);
    for (int i = 0; i < len; i++) out[i] = rtc_byte_in();
    RTC = 0x62;
}

void main7(void) {
    RESULTS[0] = 0x900D0007;
    RESULTS[1] = EXTKEYIN;
    RESULTS[2] = touch(5);
    RESULTS[3] = touch(1);

    /* Firmware : identifiant JEDEC et surnom des réglages utilisateur. */
    u32 id = spi(DEV_FIRMWARE, 0x9F, 0);
    id = spi(DEV_FIRMWARE, 0, 0);
    id |= spi(DEV_FIRMWARE, 0, 0) << 8;
    id |= spi(DEV_FIRMWARE, 0, 1) << 16;
    RESULTS[4] = id;
    u8 ptr[2];
    fw_read(0x20, ptr, 2);
    u32 us = (ptr[0] | ptr[1] << 8) * 8;
    RESULTS[5] = us;
    fw_read(us + 6, (volatile u8 *)&RESULTS[6], 10); /* "AvxDS" en UTF-16 */

    /* Contrôleur d'alimentation : lecture du registre 0. */
    spi(DEV_POWER, 0x80, 0);
    RESULTS[9] = spi(DEV_POWER, 0, 1);

    /* Horloge : date et heure (7 octets). */
    rtc_read(0x65, (volatile u8 *)&RESULTS[10], 7);

    IPCSYNC = 0xD << 8;
    for (;;) __asm__ volatile("swi 0x060000");
}
