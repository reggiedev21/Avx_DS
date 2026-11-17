/* Partie ARM7 de la démo : rien à faire, Halt définitif. */
#include "../boot_test/common.h"

void main7(void) {
    for (;;) __asm__ volatile("swi 0x060000");
}
