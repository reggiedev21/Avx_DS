//! Calcul à virgule fixe du moteur de géométrie.
//!
//! GBATEK (« Matrix Types », « Maths Basics », « Polygon Definitions ») :
//! - éléments signés de 32 bits, 12 bits de fraction (1.0 = 0x1000) ;
//! - vecteurs-lignes multipliés à droite : (x, y, z, 1) × M ; m[0..3] est
//!   donc la première ligne, et la translation est dans la dernière ;
//! - le produit garde toute la fraction (24 bits) pendant les additions, puis
//!   décale de 12 à droite à la fin. Arrondi ou troncature : non précisé ;
//!   AvxDS tronque (décalage arithmétique).

/// Un vecteur de 4 composantes à 12 bits de fraction.
pub type Vec4 = [i32; 4];

pub const ONE: i32 = 1 << 12;

/// Matrice 4×4, rangée ligne par ligne (`m[ligne][colonne]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mat4(pub [[i32; 4]; 4]);

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([[ONE, 0, 0, 0], [0, ONE, 0, 0], [0, 0, ONE, 0], [0, 0, 0, ONE]]);

    /// MTX_LOAD_4x4 / MTX_MULT_4x4 : m[0..15], ligne par ligne.
    pub fn from_4x4(p: &[u32]) -> Mat4 {
        let mut m = [[0; 4]; 4];
        for (i, &v) in p.iter().take(16).enumerate() {
            m[i / 4][i % 4] = v as i32;
        }
        Mat4(m)
    }

    /// MTX_LOAD_4x3 / MTX_MULT_4x3 : m[0..11] dans les trois premières
    /// colonnes de quatre lignes ; quatrième colonne = 0, 0, 0, 1.0.
    pub fn from_4x3(p: &[u32]) -> Mat4 {
        let mut m = [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, ONE]];
        for (i, &v) in p.iter().take(12).enumerate() {
            m[i / 3][i % 3] = v as i32;
        }
        Mat4(m)
    }

    /// MTX_MULT_3x3 : m[0..8] en haut à gauche ; le reste de l'identité.
    pub fn from_3x3(p: &[u32]) -> Mat4 {
        let mut m = Mat4::IDENTITY.0;
        for (i, &v) in p.iter().take(9).enumerate() {
            m[i / 3][i % 3] = v as i32;
        }
        Mat4(m)
    }

    /// MTX_SCALE : diagonale m[0], m[1], m[2].
    pub fn scale(p: &[u32]) -> Mat4 {
        let mut m = Mat4::IDENTITY.0;
        for (i, &v) in p.iter().take(3).enumerate() {
            m[i][i] = v as i32;
        }
        Mat4(m)
    }

    /// MTX_TRANS : translation x, y, z dans la dernière ligne.
    pub fn translation(p: &[u32]) -> Mat4 {
        let mut m = Mat4::IDENTITY.0;
        for (i, &v) in p.iter().take(3).enumerate() {
            m[3][i] = v as i32;
        }
        Mat4(m)
    }

    /// Produit `self × rhs`, avec le cumul en 64 bits puis `>> 12`.
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let mut out = [[0; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let sum: i64 = (0..4).map(|k| self.0[r][k] as i64 * rhs.0[k][c] as i64).sum();
                *cell = (sum >> 12) as i32;
            }
        }
        Mat4(out)
    }

    /// Vecteur-ligne `v × self`.
    pub fn transform(&self, v: Vec4) -> Vec4 {
        let mut out = [0; 4];
        for (c, cell) in out.iter_mut().enumerate() {
            let sum: i64 = (0..4).map(|k| v[k] as i64 * self.0[k][c] as i64).sum();
            *cell = (sum >> 12) as i32;
        }
        out
    }

    /// Vecteur de direction (x, y, z, 0) × partie 3×3.
    pub fn transform_dir(&self, v: [i32; 3]) -> [i32; 3] {
        let t = self.transform([v[0], v[1], v[2], 0]);
        [t[0], t[1], t[2]]
    }

    /// Élément i (0..15), ligne par ligne : CLIPMTX_RESULT.
    pub fn at(&self, i: usize) -> i32 {
        self.0[i / 4][i % 4]
    }
}

/// Produit scalaire de deux vecteurs 3D à 12 bits de fraction.
pub fn dot3(a: [i32; 3], b: [i32; 3]) -> i32 {
    let sum: i64 = (0..3).map(|k| a[k] as i64 * b[k] as i64).sum();
    (sum >> 12) as i32
}

/// Entier signé de `bits` bits, étendu.
pub fn sext(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

/// Trois composantes de 10 bits (bits 0–9, 10–19, 20–29), étendues.
pub fn unpack10(v: u32) -> [i32; 3] {
    [sext(v, 10), sext(v >> 10, 10), sext(v >> 20, 10)]
}
