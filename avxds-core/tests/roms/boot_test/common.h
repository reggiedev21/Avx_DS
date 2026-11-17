/* Accès aux registres de la DS, sans libnds. */
#include <stdint.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef int32_t s32;

#define REG16(a) (*(volatile u16 *)(a))
#define REG32(a) (*(volatile u32 *)(a))

#define DISPSTAT REG16(0x04000004)
#define VCOUNT REG16(0x04000006)
#define TM0CNT REG32(0x04000100)
#define IPCSYNC REG16(0x04000180)
#define IPCFIFOCNT REG16(0x04000184)
#define IPCFIFOSEND REG32(0x04000188)
#define IPCFIFORECV REG32(0x04100000)
#define IME REG16(0x04000208)
#define IE REG32(0x04000210)
#define IF REG32(0x04000214)

#define FIFO_RECV_EMPTY (1 << 8)
#define FIFO_CLEAR (1 << 3)
#define FIFO_ENABLE (1 << 15)
