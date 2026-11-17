/* Partie ARM7 de la ROM de test : renvoie 2x+1 pour chaque mot reçu. */
#include "common.h"

void main7(void) {
    IPCFIFOCNT = FIFO_ENABLE;
    for (int k = 0; k < 4; k++) {
        while (IPCFIFOCNT & FIFO_RECV_EMPTY) {}
        u32 v = IPCFIFORECV;
        IPCFIFOSEND = v * 2 + 1;
    }
    IPCSYNC = 0xA << 8;
    for (;;) __asm__ volatile("swi 0x060000"); /* Halt (IE = 0 : pour toujours) */
}
