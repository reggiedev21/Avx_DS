//! Contrôleur d'interruptions (un par CPU) : IME, IE, IF.

/// Sources d'interruption (bits de IE/IF).
pub mod bits {
    pub const VBLANK: u32 = 1 << 0;
    pub const HBLANK: u32 = 1 << 1;
    pub const VCOUNT: u32 = 1 << 2;
    pub const TIMER0: u32 = 1 << 3;
    pub const TIMER1: u32 = 1 << 4;
    pub const TIMER2: u32 = 1 << 5;
    pub const TIMER3: u32 = 1 << 6;
    pub const DMA0: u32 = 1 << 8;
    pub const DMA1: u32 = 1 << 9;
    pub const DMA2: u32 = 1 << 10;
    pub const DMA3: u32 = 1 << 11;
    pub const KEYPAD: u32 = 1 << 12;
    pub const GBA_SLOT: u32 = 1 << 13;
    pub const IPC_SYNC: u32 = 1 << 16;
    pub const IPC_SEND_EMPTY: u32 = 1 << 17;
    pub const IPC_RECV_NOT_EMPTY: u32 = 1 << 18;
    pub const CARD_TRANSFER: u32 = 1 << 19;
    pub const CARD_IREQ: u32 = 1 << 20;
    /// ARM9 uniquement.
    pub const GEOMETRY_FIFO: u32 = 1 << 21;
    /// ARM7 uniquement.
    pub const SCREENS_UNFOLDING: u32 = 1 << 22;
    pub const SPI: u32 = 1 << 23;
    pub const WIFI: u32 = 1 << 24;
}

#[derive(Clone, Debug, Default)]
pub struct IrqController {
    pub ime: bool,
    pub ie: u32,
    pub if_: u32,
}

impl IrqController {
    /// Lève une ou plusieurs sources (bits de IF).
    pub fn request(&mut self, bits: u32) {
        self.if_ |= bits;
    }

    /// Ligne IRQ vers le CPU : IME et au moins une source active et autorisée.
    pub fn line(&self) -> bool {
        self.ime && self.ie & self.if_ != 0
    }

    /// Condition de sortie de Halt : `IE & IF`, que IME soit à 1 ou non.
    pub fn wake(&self) -> bool {
        self.ie & self.if_ != 0
    }
}
