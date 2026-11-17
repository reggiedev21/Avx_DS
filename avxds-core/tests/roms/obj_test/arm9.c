/*
 * ROM de démonstration des sprites, fenêtres et effets (ARM9, Thumb).
 *
 * Écran du haut (moteur A) : damier en BG0 ; sprite 0 (16×16, 16 couleurs)
 * déplacé à la croix ; sprite 1 (32×32, 256 couleurs, affine double taille)
 * qui tourne, A = pause ; sprite 2 semi-transparent ; fenêtre 0 qui masque
 * le damier dans le coin bas-gauche.
 * Écran du bas (moteur B) : 8 bandes de couleur et un fondu en boucle
 * (assombrissement / éclaircissement par BLDCNT).
 *
 * État exposé pour les tests en 0x02300000 : [0] = image, [1] = X du
 * sprite 0, [2] = Y du sprite 0, [3] = angle, [15] = 0x600DB007 une fois prêt.
 */
#include "../boot_test/common.h"
#include "sin_table.h"

#define REG_DISPCNT_A REG32(0x04000000)
#define REG_DISPCNT_B REG32(0x04001000)
#define KEYINPUT REG16(0x04000130)
#define POWCNT1 REG16(0x04000304)
#define VRAMCNT(n) (*(volatile u8 *)(0x04000240 + (n)))
#define RESULTS ((volatile u32 *)0x02300000)
#define RGB(r, g, b) ((u16)((r) | ((g) << 5) | ((b) << 10)))

#define BG_A ((volatile u16 *)0x06000000)
#define OBJ_A ((volatile u16 *)0x06400000)
#define BG_B ((volatile u16 *)0x06200000)
#define PAL_BG_A ((volatile u16 *)0x05000000)
#define PAL_OBJ_A ((volatile u16 *)0x05000200)
#define PAL_BG_B ((volatile u16 *)0x05000400)
#define OAM_A ((volatile u16 *)0x07000000)

#define BIOS_IRQ_HANDLER REG32(0x03003FFC)
#define BIOS_IRQ_FLAGS REG32(0x03003FF8)

static void irq_handler(void) {
    u32 f = IF & IE;
    IF = f;
    BIOS_IRQ_FLAGS |= f;
}

static inline void vblank_intr_wait(void) {
    __asm__ volatile("swi 0x05" ::: "r0", "r1", "r2", "r3", "r12", "memory");
}

/* Écrit les 3 attributs d'un sprite. */
static void set_sprite(int i, u16 a0, u16 a1, u16 a2) {
    OAM_A[i * 4 + 0] = a0;
    OAM_A[i * 4 + 1] = a1;
    OAM_A[i * 4 + 2] = a2;
}

static void set_affine(int p, s32 pa, s32 pb, s32 pc, s32 pd) {
    OAM_A[p * 16 + 3] = (u16)pa;
    OAM_A[p * 16 + 7] = (u16)pb;
    OAM_A[p * 16 + 11] = (u16)pc;
    OAM_A[p * 16 + 15] = (u16)pd;
}

/* Pixel d'une tuile 4 bits (mapping 1D) : écriture 16 bits obligatoire. */
static void tile4_set(volatile u16 *base, int tile, int x, int y, int idx) {
    volatile u16 *p = base + tile * 16 + y * 2 + x / 4;
    int s = (x & 3) * 4;
    *p = (u16)((*p & ~(0xF << s)) | (idx << s));
}

static void setup_top(void) {
    VRAMCNT(0) = 0x81; /* A → fonds du moteur A */
    VRAMCNT(1) = 0x82; /* B → sprites du moteur A */

    /* BG0 : damier de deux tuiles pleines (index 1 et 2). */
    for (int i = 0; i < 16; i++) {
        BG_A[16 + i] = 0x1111;
        BG_A[32 + i] = 0x2222;
    }
    for (int y = 0; y < 32; y++)
        for (int x = 0; x < 32; x++)
            BG_A[0x800 + y * 32 + x] = (u16)(1 + ((x ^ y) & 1)); /* carte en 0x1000 */
    PAL_BG_A[0] = RGB(4, 4, 6);    /* fond (visible dans la fenêtre) */
    PAL_BG_A[1] = RGB(6, 10, 20);
    PAL_BG_A[2] = RGB(10, 14, 26);
    REG16(0x04000008) = (2 << 8) | 1; /* BG0CNT : carte 2 × 2 Ko, priorité 1 */

    /* Sprite 0 : anneau 16×16 (tuiles 0–3), couleurs 1 et 2. */
    for (int y = 0; y < 16; y++)
        for (int x = 0; x < 16; x++) {
            int dx = 2 * x - 15, dy = 2 * y - 15, d = dx * dx + dy * dy;
            int idx = d < 100 ? 0 : d < 169 ? 2 : d < 225 ? 1 : 0;
            int tile = (y / 8) * 2 + x / 8;
            tile4_set(OBJ_A, tile, x & 7, y & 7, idx);
        }
    PAL_OBJ_A[1] = RGB(31, 24, 0);
    PAL_OBJ_A[2] = RGB(31, 6, 2);

    /* Sprite 1 : flèche 32×32 en 256 couleurs (tuiles 8 bits à partir du
     * n° 4 en unités de 32 octets, soit l'octet 128). */
    volatile u16 *t8 = OBJ_A + 4 * 16;
    for (int y = 0; y < 32; y++)
        for (int x = 0; x < 32; x += 2) {
            u16 pair = 0;
            for (int k = 0; k < 2; k++) {
                int px = x + k;
                int idx = 0;
                if (y >= 13 && y < 19 && px >= 4 && px < 22) idx = 3; /* tige */
                if (px >= 22 && px < 30 && (y - 16 < px - 21) && (16 - y < px - 21) &&
                    (y - 16 < 30 - px) && (16 - y < 30 - px)) idx = 4;  /* pointe */
                pair |= (u16)(idx << (8 * k));
            }
            int tile = (y / 8) * 4 + x / 8;
            t8[tile * 32 + (y & 7) * 4 + (x & 7) / 2] = pair;
        }
    PAL_OBJ_A[3] = RGB(4, 28, 8);
    PAL_OBJ_A[4] = RGB(28, 31, 8);

    /* Sprite 2 : carré plein 32×32 (tuile 4 bits n° 40, index 5). */
    for (int i = 0; i < 16 * 16; i++) OBJ_A[40 * 16 + i] = 0x5555;
    PAL_OBJ_A[5] = RGB(31, 31, 31);

    for (int i = 0; i < 128; i++) set_sprite(i, 0x0200, 0, 0); /* tout masquer */

    /* Fenêtre 0 en bas à gauche : sprites seulement (le damier disparaît). */
    REG16(0x04000040) = (8 << 8) | 88;   /* X : 8 ≤ x < 88 */
    REG16(0x04000044) = (120 << 8) | 184; /* Y : 120 ≤ y < 184 */
    REG16(0x04000048) = 0x0010;          /* dedans : OBJ */
    REG16(0x0400004A) = 0x003F;          /* dehors : tout + effets */

    /* Mélange : sprite semi-transparent sur BG0, 10/16 + 6/16. */
    REG16(0x04000050) = 1 << 8;
    REG16(0x04000052) = (6 << 8) | 10;

    /* Mode 0, BG0, sprites en mapping 1D (pas de 32 octets), fenêtre 0. */
    REG_DISPCNT_A = 0x00010000 | (1 << 8) | (1 << 12) | (1 << 4) | (1 << 13);
}

static void setup_bottom(void) {
    VRAMCNT(2) = 0x84; /* C → fonds du moteur B */
    for (int t = 1; t <= 8; t++)
        for (int i = 0; i < 16; i++) BG_B[t * 16 + i] = (u16)(0x1111 * 1); /* index 1 */
    for (int y = 0; y < 32; y++)
        for (int x = 0; x < 32; x++)
            BG_B[0x800 + y * 32 + x] = (u16)((x / 4 + 1) | ((x / 4) << 12)); /* palette = bande */
    static const u16 colors[8] = {
        RGB(31, 0, 0), RGB(31, 16, 0), RGB(31, 31, 0), RGB(0, 31, 0),
        RGB(0, 31, 31), RGB(0, 0, 31), RGB(16, 0, 31), RGB(31, 31, 31),
    };
    for (int b = 0; b < 8; b++) PAL_BG_B[b * 16 + 1] = colors[b];
    PAL_BG_B[0] = 0;
    REG16(0x04001008) = 2 << 8;
    REG_DISPCNT_B = 0x00010000 | (1 << 8);
}

void main9(void) {
    POWCNT1 = 0x820F; /* moteur A en haut */
    BIOS_IRQ_HANDLER = (u32)irq_handler;
    DISPSTAT = 1 << 3;
    IE = 1;
    IME = 1;
    setup_top();
    setup_bottom();

    int px = 120, py = 88, angle = 0, paused = 0;
    u16 prev_keys = 0x3FF;
    u32 frame = 0;
    RESULTS[15] = 0x600DB007;

    for (;;) {
        /* Sprite 0 : anneau, priorité 0. */
        set_sprite(0, (u16)(py & 0xFF), (u16)(px & 0x1FF) | (1 << 14), 0);
        /* Sprite 1 : flèche affine, double taille (boîte 64×64), priorité 0. */
        s32 c = SIN_TABLE[(angle + 64) & 255], s = SIN_TABLE[angle & 255];
        set_affine(0, c, -s, s, c);
        set_sprite(1, (u16)(64 | 0x0300 | 0x2000), (u16)(160 | (2 << 14)), 4);
        /* Sprite 2 : carré semi-transparent (mode 1), priorité 0. */
        set_sprite(2, (u16)(20 | 0x0400), (u16)(24 | (2 << 14)), 40);

        /* Écran du bas : fondu sur 64 images (assombrir puis éclaircir). */
        u32 phase = frame & 63;
        u32 evy = phase < 32 ? (phase < 16 ? phase : 31 - phase) : 0;
        u32 effect = phase < 32 ? 3 : 2;
        if (phase >= 32) evy = (phase - 32) < 16 ? (phase - 32) : 63 - phase;
        REG16(0x04001050) = (u16)((effect << 6) | 1);
        REG16(0x04001054) = (u16)evy;

        RESULTS[0] = frame;
        RESULTS[1] = (u32)px;
        RESULTS[2] = (u32)py;
        RESULTS[3] = (u32)angle;

        vblank_intr_wait();
        frame++;

        u16 keys = KEYINPUT;
        if (!(keys & (1 << 4))) px++;
        if (!(keys & (1 << 5))) px--;
        if (!(keys & (1 << 6))) py--;
        if (!(keys & (1 << 7))) py++;
        if (!(keys & 1) && (prev_keys & 1)) paused ^= 1; /* A : pause */
        prev_keys = keys;
        if (!paused) angle = (angle + 2) & 255;
    }
}
