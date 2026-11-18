//! Table des commandes de géométrie : nombre de paramètres et durée.
//!
//! Source : GBATEK « DS 3D I/O Map » (voir `docs/3D_SPEC.md`, section 1).
//! Codes valides d'après « DS 3D Geometry Commands » : 10h–1Ch, 20h–2Bh,
//! 30h–33h, 40h–41h, 50h, 60h, 70h–72h. La table de l'I/O Map ajoute
//! SHININESS (34h, 32 paramètres) : AvxDS le tient pour valide (point
//! ouvert, noté dans la spécification).

/// Ce que le moteur doit savoir d'une commande.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CmdInfo {
    /// Nombre de paramètres (0 : une seule entrée dans la file).
    pub params: u8,
    /// Durée d'exécution, en cycles de bus (33,51 MHz, hypothèse : la page
    /// ne précise pas l'horloge).
    pub cycles: u16,
}

const fn c(params: u8, cycles: u16) -> Option<CmdInfo> {
    Some(CmdInfo { params, cycles })
}

/// Paramètres et durée d'une commande, ou `None` si le code est invalide
/// (ignoré par le matériel, sans consommer de paramètres).
pub const fn info(cmd: u8) -> Option<CmdInfo> {
    match cmd {
        0x10 => c(1, 1),    // MTX_MODE
        0x11 => c(0, 17),   // MTX_PUSH
        0x12 => c(1, 36),   // MTX_POP
        0x13 => c(1, 17),   // MTX_STORE
        0x14 => c(1, 36),   // MTX_RESTORE
        0x15 => c(0, 19),   // MTX_IDENTITY
        0x16 => c(16, 34),  // MTX_LOAD_4x4
        0x17 => c(12, 30),  // MTX_LOAD_4x3
        0x18 => c(16, 35),  // MTX_MULT_4x4 (+30 en mode 2)
        0x19 => c(12, 31),  // MTX_MULT_4x3 (+30 en mode 2)
        0x1A => c(9, 28),   // MTX_MULT_3x3 (+30 en mode 2)
        0x1B => c(3, 22),   // MTX_SCALE
        0x1C => c(3, 22),   // MTX_TRANS (+30 en mode 2)
        0x20 => c(1, 1),    // COLOR
        0x21 => c(1, 9),    // NORMAL (9..12 selon les lumières : 9 retenu)
        0x22 => c(1, 1),    // TEXCOORD
        0x23 => c(2, 9),    // VTX_16
        0x24 => c(1, 8),    // VTX_10
        0x25 => c(1, 8),    // VTX_XY
        0x26 => c(1, 8),    // VTX_XZ
        0x27 => c(1, 8),    // VTX_YZ
        0x28 => c(1, 8),    // VTX_DIFF
        0x29 => c(1, 1),    // POLYGON_ATTR
        0x2A => c(1, 1),    // TEXIMAGE_PARAM
        0x2B => c(1, 1),    // PLTT_BASE
        0x30 => c(1, 4),    // DIF_AMB
        0x31 => c(1, 4),    // SPE_EMI
        0x32 => c(1, 6),    // LIGHT_VECTOR
        0x33 => c(1, 1),    // LIGHT_COLOR
        0x34 => c(32, 32),  // SHININESS
        0x40 => c(1, 1),    // BEGIN_VTXS
        0x41 => c(0, 1),    // END_VTXS
        0x50 => c(1, 392),  // SWAP_BUFFERS
        0x60 => c(1, 1),    // VIEWPORT
        0x70 => c(3, 103),  // BOX_TEST
        0x71 => c(2, 9),    // POS_TEST
        0x72 => c(1, 5),    // VEC_TEST
        _ => None,
    }
}

/// Cycles en plus en mode de matrice 2 (position et vecteur) : MTX_MULT_*
/// et MTX_TRANS (GBATEK, note de l'I/O Map).
pub const fn mode2_extra(cmd: u8) -> u16 {
    match cmd {
        0x18 | 0x19 | 0x1A | 0x1C => 30,
        _ => 0,
    }
}

/// Nom GBATEK de la commande, pour les rapports.
pub const fn name(cmd: u8) -> &'static str {
    match cmd {
        0x10 => "MTX_MODE",
        0x11 => "MTX_PUSH",
        0x12 => "MTX_POP",
        0x13 => "MTX_STORE",
        0x14 => "MTX_RESTORE",
        0x15 => "MTX_IDENTITY",
        0x16 => "MTX_LOAD_4x4",
        0x17 => "MTX_LOAD_4x3",
        0x18 => "MTX_MULT_4x4",
        0x19 => "MTX_MULT_4x3",
        0x1A => "MTX_MULT_3x3",
        0x1B => "MTX_SCALE",
        0x1C => "MTX_TRANS",
        0x20 => "COLOR",
        0x21 => "NORMAL",
        0x22 => "TEXCOORD",
        0x23 => "VTX_16",
        0x24 => "VTX_10",
        0x25 => "VTX_XY",
        0x26 => "VTX_XZ",
        0x27 => "VTX_YZ",
        0x28 => "VTX_DIFF",
        0x29 => "POLYGON_ATTR",
        0x2A => "TEXIMAGE_PARAM",
        0x2B => "PLTT_BASE",
        0x30 => "DIF_AMB",
        0x31 => "SPE_EMI",
        0x32 => "LIGHT_VECTOR",
        0x33 => "LIGHT_COLOR",
        0x34 => "SHININESS",
        0x40 => "BEGIN_VTXS",
        0x41 => "END_VTXS",
        0x50 => "SWAP_BUFFERS",
        0x60 => "VIEWPORT",
        0x70 => "BOX_TEST",
        0x71 => "POS_TEST",
        0x72 => "VEC_TEST",
        _ => "?",
    }
}
