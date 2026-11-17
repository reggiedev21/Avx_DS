/* ROM de test de la sauvegarde : pilote la puce comme libnds. Le mode est
 * lu en 0x02200000 (posé par le test) : 0 = EEPROM 8 Ko, 1 = EEPROM 0,5 Ko,
 * 2 = FLASH ; bit 8 = relecture seule (deuxième démarrage). */
#include "../boot_test/common.h"

#define AUXSPICNT REG16(0x040001A0)
#define AUXSPIDATA (*(volatile u8 *)0x040001A2)
#define MODE (*(volatile u32 *)0x02200000)
#define RESULTS ((volatile u32 *)0x02300000)

static u8 xfer(u8 v) {
    AUXSPIDATA = v;
    while (AUXSPICNT & 0x80) {}
    return AUXSPIDATA;
}
static void begin(void) { AUXSPICNT = 0xA040; } /* slot, SPI, hold */
static void end(void) { AUXSPICNT = 0x0040; }    /* relâche la puce */

static void command(u8 c) { begin(); xfer(c); end(); }

static void wait_ready(void) {
    begin();
    xfer(0x05);
    while (xfer(0) & 1) {}
    end();
}

static void address(u32 addr, int bytes) {
    for (int i = bytes - 1; i >= 0; i--) xfer(addr >> (8 * i));
}

static u8 opcode(u8 base, u32 addr, int abytes) {
    /* EEPROM 0,5 Ko : 9e bit d'adresse dans la commande (bit 3). */
    return abytes == 1 ? base | ((addr >> 8) & 1) << 3 : base;
}

static void read(u32 addr, u8 *buf, int n, int abytes) {
    begin();
    xfer(opcode(0x03, addr, abytes));
    address(addr, abytes);
    for (int i = 0; i < n; i++) buf[i] = xfer(0);
    end();
}

static void write(u32 addr, const u8 *buf, int n, int abytes, int page, u8 op) {
    while (n > 0) {
        int chunk = page - (addr % page);
        if (chunk > n) chunk = n;
        command(0x06);
        begin();
        xfer(opcode(op, addr, abytes));
        address(addr, abytes);
        for (int i = 0; i < chunk; i++) xfer(buf[i]);
        end();
        wait_ready();
        addr += chunk;
        buf += chunk;
        n -= chunk;
    }
}

static u8 data[256], back[256];

void main9(void) {
    u32 mode = MODE & 0xFF, verify_only = MODE & 0x100;
    int abytes = mode == 0 ? 2 : mode == 1 ? 1 : 3;
    int page = mode == 0 ? 32 : mode == 1 ? 16 : 256;
    u32 base = mode == 2 ? 0x10000 : 0x100;
    RESULTS[0] = 0x5A7E0009;
    RESULTS[1] = MODE;

    /* Au démarrage, un jeu relit d'abord sa sauvegarde. */
    read(base, back, 16, abytes);
    for (int i = 0; i < 4; i++)
        RESULTS[2 + i] = back[4 * i] | back[4 * i + 1] << 8 | back[4 * i + 2] << 16 | (u32)back[4 * i + 3] << 24;

    for (int i = 0; i < 256; i++) data[i] = (u8)(i * 7 + mode);
    if (!verify_only) {
        if (mode == 2) {
            command(0x06);
            begin(); xfer(0xD8); address(base, 3); end(); /* effacement du secteur */
            wait_ready();
        }
        write(base, data, 256, abytes, page, mode == 2 ? 0x0A : 0x02);
    }
    read(base, back, 256, abytes);
    u32 bad = 0;
    for (int i = 0; i < 256; i++) bad += back[i] != data[i];
    RESULTS[6] = bad;
    begin(); xfer(0x05); RESULTS[7] = xfer(0); end();

    RESULTS[15] = 0x600DB007;
    for (;;) __asm__ volatile("swi 0x06");
}
