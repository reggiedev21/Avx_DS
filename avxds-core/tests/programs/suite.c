/*
 * Suite de tests d'intégration du CPU.
 *
 * Le même code est compilé :
 *   - pour ARM (ARMv4T / ARMv5TE, ARM / Thumb, -O0 / -O2) et exécuté dans
 *     l'émulateur ;
 *   - nativement sur la machine hôte (-DHOST) pour produire les valeurs
 *     attendues.
 * Tout est en entiers de taille fixe : les résultats ne dépendent pas de
 * la plateforme. Pas de division (`/`, `%`) : sur ARM elle appellerait
 * la libgcc, qu'on n'a pas.
 */
#include <stdint.h>

typedef uint8_t u8;
typedef int8_t s8;
typedef uint16_t u16;
typedef int16_t s16;
typedef uint32_t u32;
typedef int32_t s32;
typedef uint64_t u64;
typedef int64_t s64;

#if defined(__arm__) && defined(__thumb__)
#define OTHER_MODE __attribute__((target("arm"), noinline))
#elif defined(__arm__)
#define OTHER_MODE __attribute__((target("thumb"), noinline))
#else
#define OTHER_MODE __attribute__((noinline))
#endif

#define NOINLINE __attribute__((noinline))

/* Les compilateurs transforment certaines boucles en appels memcpy/memset. */
void *memcpy(void *d, const void *s, unsigned long n) {
    u8 *dd = d;
    const u8 *ss = s;
    while (n--) *dd++ = *ss++;
    return d;
}
void *memset(void *d, int c, unsigned long n) {
    u8 *dd = d;
    while (n--) *dd++ = (u8)c;
    return d;
}

#if defined(__arm__) && !defined(HOST)
/* Multiplication 64 bits de l'ABI ARM, appelée par le code Thumb.
 * Uniquement des produits 16×16 → 32 bits pour ne pas se rappeler soi-même. */
u64 __aeabi_lmul(u64 a, u64 b) {
    u32 al = (u32)a, ah = (u32)(a >> 32), bl = (u32)b, bh = (u32)(b >> 32);
    u32 a0 = al & 0xFFFF, a1 = al >> 16, b0 = bl & 0xFFFF, b1 = bl >> 16;
    u32 p00 = a0 * b0, p01 = a0 * b1, p10 = a1 * b0, p11 = a1 * b1;
    u32 mid = (p00 >> 16) + (p01 & 0xFFFF) + (p10 & 0xFFFF);
    u32 lo = (p00 & 0xFFFF) | (mid << 16);
    u32 hi = p11 + (p01 >> 16) + (p10 >> 16) + (mid >> 16) + al * bh + ah * bl;
    return ((u64)hi << 32) | lo;
}
#endif

/* Valeur lue à l'exécution : empêche clang de tout calculer à la compilation. */
volatile u32 opaque = 1;
#define IN(x) ((u32)(x) * opaque)

static u32 lcg_state;
static u32 lcg(void) {
    lcg_state = lcg_state * 1664525u + 1013904223u;
    return lcg_state;
}

static NOINLINE u32 fib_iter(u32 n) {
    u32 a = 0, b = 1;
    while (n--) {
        u32 t = a + b;
        a = b;
        b = t;
    }
    return a;
}

static NOINLINE u32 fib_rec(u32 n) {
    return n < 2 ? n : fib_rec(n - 1) + fib_rec(n - 2);
}

static NOINLINE u32 test_sort(void) {
    s32 a[48];
    lcg_state = IN(12345);
    for (int i = 0; i < 48; i++) a[i] = (s32)lcg() >> 7;
    for (int i = 0; i < 48; i++)
        for (int j = 0; j + 1 < 48 - i; j++)
            if (a[j] > a[j + 1]) {
                s32 t = a[j];
                a[j] = a[j + 1];
                a[j + 1] = t;
            }
    u32 sum = 0;
    for (int i = 0; i < 48; i++) sum = sum * 31 + (u32)a[i];
    for (int i = 0; i + 1 < 48; i++)
        if (a[i] > a[i + 1]) return 0xBAD;
    return sum;
}

static NOINLINE u32 crc32(const u8 *p, u32 n) {
    u32 crc = 0xFFFFFFFFu;
    while (n--) {
        crc ^= *p++;
        for (int k = 0; k < 8; k++) crc = (crc >> 1) ^ (0xEDB88320u & -(crc & 1));
    }
    return ~crc;
}

static NOINLINE u32 test_crc(void) {
    u8 buf[300];
    for (int i = 0; i < 300; i++) buf[i] = (u8)(IN(i) * 7 + 3);
    return crc32(buf, 300) ^ crc32(buf + 1, 257);
}

/* Division par décalages et soustractions. */
static NOINLINE u32 udivmod(u32 n, u32 d, u32 *rem) {
    u32 q = 0, r = 0;
    for (int i = 31; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= 1u << i;
        }
    }
    *rem = r;
    return q;
}

static NOINLINE u32 test_div(void) {
    u32 acc = 0;
    lcg_state = IN(777);
    for (int i = 0; i < 200; i++) {
        u32 n = lcg(), d = (lcg() >> (i & 31)) | 1, r;
        u32 q = udivmod(n, d, &r);
        if (q * d + r != n || r >= d) return 0xBAD0000 + i;
        acc = acc * 33 + q + r;
    }
    return acc;
}

static NOINLINE u32 test_mul64(void) {
    u64 uacc = 0;
    u64 sacc = 0; /* accumulé en non signé : pas de débordement indéfini */
    lcg_state = IN(4242);
    for (int i = 0; i < 100; i++) {
        u32 a = lcg(), b = lcg();
        uacc += (u64)a * b;
        sacc += (u64)((s64)(s32)a * (s32)b);
        sacc -= (u64)((s64)(s16)a * (s16)(b >> 16));
    }
    return (u32)uacc ^ (u32)(uacc >> 32) ^ (u32)sacc * 3 ^ (u32)(sacc >> 32);
}

static NOINLINE u32 classify(u32 x) {
    switch (x) {
    case 0: return 11;
    case 1: return 23;
    case 2: return 37;
    case 3: return 41;
    case 4: return 59;
    case 5: return 67;
    case 6: return 73;
    case 7: return 89;
    case 8: return 97;
    case 9: return 101;
    case 10: return 113;
    case 11: return 127;
    default: return x * 3;
    }
}

static NOINLINE u32 test_switch(void) {
    u32 acc = 0;
    for (u32 i = 0; i < IN(20); i++) acc = acc * 7 + classify(i);
    return acc;
}

struct mixed {
    u8 a;
    s8 b;
    u16 c;
    s16 d;
    u32 e;
    s8 arr[5];
};

static NOINLINE u32 test_struct(void) {
    struct mixed m[6];
    for (int i = 0; i < 6; i++) {
        m[i].a = (u8)(200 + IN(i) * 11);
        m[i].b = (s8)(-100 - i * 5);
        m[i].c = (u16)(60000 + i * 999);
        m[i].d = (s16)(-30000 + i * 7);
        m[i].e = 0xDEAD0000u + i;
        for (int k = 0; k < 5; k++) m[i].arr[k] = (s8)(k * 60 - 120);
    }
    u32 acc = 0;
    for (int i = 0; i < 6; i++) {
        acc += (u32)(m[i].a + m[i].b + m[i].c + m[i].d) + m[i].e;
        for (int k = 0; k < 5; k++) acc ^= (u32)(m[i].arr[k] * (1 << k));
    }
    return acc;
}

static NOINLINE u32 my_clz(u32 x) {
#if defined(HOST) || (defined(__ARM_ARCH) && __ARM_ARCH >= 5 && !defined(__thumb__))
    return (u32)__builtin_clz(x);
#else
    u32 n = 0;
    while (!(x & 0x80000000u)) {
        x <<= 1;
        n++;
    }
    return n;
#endif
}

static NOINLINE u32 rotl(u32 x, u32 r) {
    return (x << r) | (x >> (32 - r));
}

static NOINLINE u32 test_bits(void) {
    u32 acc = 0;
    lcg_state = IN(99);
    for (u32 r = 1; r < 32; r++) {
        u32 x = lcg() | 1;
        u32 pop = 0;
        for (u32 y = x; y; y &= y - 1) pop++;
        acc += rotl(x, r) ^ (my_clz(x >> (r & 15)) << 8) ^ pop;
        acc ^= (u32)((s32)x >> r) + (x >> r);
    }
    return acc;
}

typedef u32 (*op_fn)(u32, u32);
static NOINLINE u32 op_add(u32 a, u32 b) { return a + b; }
static NOINLINE u32 op_sub(u32 a, u32 b) { return a - b; }
static NOINLINE u32 op_xor(u32 a, u32 b) { return a ^ b; }
static NOINLINE u32 op_mul(u32 a, u32 b) { return a * b; }

static NOINLINE u32 test_fnptr(void) {
    static const op_fn ops[4] = {op_add, op_sub, op_xor, op_mul};
    u32 acc = IN(1);
    for (u32 i = 0; i < 40; i++) acc = ops[i & 3](acc, i * 2654435761u);
    return acc;
}

static NOINLINE u32 test_memops(void) {
    u8 a[97], b[97];
    memset(a, (int)IN(0x5A), sizeof a);
    for (int i = 0; i < 97; i++) b[i] = (u8)i;
    memcpy(a + 3, b + 1, 77);
    u32 acc = 0;
    for (int i = 0; i < 97; i++) acc = acc * 131 + a[i];
    /* Accès 16 et 32 bits alignés depuis un tampon d'octets. */
    u16 h[8];
    u32 w[4];
    memcpy(h, a + 10, sizeof h);
    memcpy(w, a + 20, sizeof w);
    for (int i = 0; i < 8; i++) acc += h[i];
    for (int i = 0; i < 4; i++) acc ^= w[i];
    return acc;
}

/* Fonctions compilées dans l'autre jeu d'instructions (interworking). */
OTHER_MODE u32 other_mode_mix(u32 a, u32 b, u32 c, u32 d, u32 e) {
    return (a * 3 + b) ^ (c << 2) ^ (d - e);
}
OTHER_MODE u32 other_mode_callback(op_fn f, u32 x) {
    return f(x, 17) + 1;
}

static NOINLINE u32 test_interwork(void) {
    u32 acc = 0;
    for (u32 i = 0; i < IN(10); i++) acc += other_mode_mix(i, acc, i * 5, 1000, i);
    acc += other_mode_callback(op_mul, acc);
    return acc;
}

static NOINLINE u32 test_u64_shifts(void) {
    u64 x = 0x0123456789ABCDEFull * IN(1);
    u64 acc = 0;
    acc ^= x << 4;
    acc ^= x >> 12;
    acc ^= x << 33;
    acc ^= x >> 40;
    acc += (u64)(s64)(s32)0x80000001;
    s64 y = -1234567890123ll * (s32)IN(1);
    acc ^= (u64)(y >> 20);
    return (u32)acc ^ (u32)(acc >> 32) ^ (x > acc) ^ ((y < 0) << 4);
}

static NOINLINE s32 clamp16(s32 v) {
    return v > 32767 ? 32767 : v < -32768 ? -32768 : v;
}

static NOINLINE u32 test_clamp(void) {
    u32 acc = 0;
    lcg_state = IN(31337);
    for (int i = 0; i < 64; i++) {
        s32 a = (s32)lcg() >> 14, b = (s32)lcg() >> 14;
        acc = acc * 17 + (u32)clamp16(a + b) + (u32)clamp16(a - b);
    }
    return acc;
}

static const char *const words[] = {"nitro", "arm946", "arm7tdmi", "avxds", "thumb", ""};

static NOINLINE u32 my_strlen(const char *s) {
    u32 n = 0;
    while (s[n]) n++;
    return n;
}
static NOINLINE s32 my_strcmp(const char *a, const char *b) {
    while (*a && *a == *b) a++, b++;
    return (u8)*a - (u8)*b;
}

static NOINLINE u32 test_strings(void) {
    u32 acc = 0;
    for (int i = 0; i < 6; i++) {
        acc = acc * 13 + my_strlen(words[IN(i)]);
        for (int j = 0; j < 6; j++) acc += (u32)my_strcmp(words[i], words[j]) * (u32)(i + 2 * j);
    }
    return acc;
}

static NOINLINE u32 ackermann(u32 m, u32 n) {
    if (m == 0) return n + 1;
    if (n == 0) return ackermann(m - 1, 1);
    return ackermann(m - 1, ackermann(m, n - 1));
}

#define N_TESTS 18

void run_tests(u32 *out) {
    out[0] = fib_iter(IN(47));
    out[1] = fib_rec(IN(17));
    out[2] = test_sort();
    out[3] = test_crc();
    out[4] = test_div();
    out[5] = test_mul64();
    out[6] = test_switch();
    out[7] = test_struct();
    out[8] = test_bits();
    out[9] = test_fnptr();
    out[10] = test_memops();
    out[11] = test_interwork();
    out[12] = test_u64_shifts();
    out[13] = test_clamp();
    out[14] = test_strings();
    out[15] = ackermann(IN(2), IN(3));
    out[16] = (u32)(s32)(s8)out[3] + (u32)(s16)out[4];
    out[17] = 0x600DC0DE;
}

#ifdef HOST
#include <stdio.h>
int main(void) {
    u32 r[N_TESTS];
    run_tests(r);
    printf("// Généré par tools/build_programs.sh : ne pas éditer.\n");
    printf("pub const EXPECTED: [u32; %d] = [\n", N_TESTS);
    for (int i = 0; i < N_TESTS; i++) printf("    0x%08X,\n", r[i]);
    printf("];\n");
    return 0;
}
#endif
