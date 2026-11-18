//! Sommets et polygones produits par le moteur de géométrie, mémoire des
//! polygones (double tampon) et découpage contre le volume de vue.
//!
//! GBATEK « Polygon Definitions by Vertices » : six plans (gauche, droite,
//! haut, bas, proche, lointain) ; un polygone entièrement dehors n'est pas
//! stocké. Le volume est pris ici en coordonnées homogènes : −w ≤ x, y, z ≤ w
//! (hypothèse : la page ne donne pas les inégalités).

use super::math::Vec4;

/// Capacités (GBATEK « DS 3D Status », RAM_COUNT : 0..2048 et 0..6144).
pub const MAX_POLYGONS: usize = 2048;
pub const MAX_VERTICES: usize = 6144;

/// Sommet transformé.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vertex {
    /// Coordonnées de découpage x, y, z, w (12 bits de fraction).
    pub clip: Vec4,
    /// Couleur, 6 bits par composante (R, G, B).
    pub color: [u8; 3],
    /// Coordonnées de texture brutes (S, T) ; transformations : étape 3D-4.
    pub tex: [i16; 2],
    /// Position à l'écran, en pixels, origine en haut à gauche (calculée au
    /// stockage, après découpage).
    pub screen: [i32; 2],
}

/// Polygone stocké (3 à 10 sommets après découpage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polygon {
    pub vertices: Vec<Vertex>,
    /// POLYGON_ATTR pris en compte au BEGIN_VTXS.
    pub attr: u32,
    pub teximage: u32,
    pub pltt_base: u32,
    /// Face avant vue (sens anti-horaire à l'écran).
    pub front: bool,
}

/// Une mémoire de polygones et de sommets (il y en a deux : l'une se
/// remplit pendant que l'autre est dessinée).
#[derive(Clone, Debug, Default)]
pub struct PolyRam {
    pub polygons: Vec<Polygon>,
    /// Entrées de la mémoire des sommets (les bandes non découpées
    /// partagent leurs sommets).
    pub vertex_count: usize,
    /// Un polygone n'a pas trouvé de place (DISP3DCNT bit 13).
    pub overflow: bool,
    /// Bits de SWAP_BUFFERS en vigueur pendant la construction (bit 0 : tri
    /// manuel des translucides, bit 1 : profondeur par W).
    pub swap_flags: u32,
}

impl PolyRam {
    pub fn clear(&mut self) {
        self.polygons.clear();
        self.vertex_count = 0;
        self.overflow = false;
    }

    /// Range un polygone qui ajoute `new_vertices` entrées de sommets.
    /// Renvoie `false` (et note le débordement) s'il n'y a plus de place.
    pub fn store(&mut self, p: Polygon, new_vertices: usize) -> bool {
        if self.polygons.len() >= MAX_POLYGONS || self.vertex_count + new_vertices > MAX_VERTICES {
            self.overflow = true;
            return false;
        }
        self.vertex_count += new_vertices;
        self.polygons.push(p);
        true
    }
}

/// Issue du découpage d'un polygone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clip {
    /// Entièrement dans le volume : sommets inchangés.
    Inside,
    /// Coupé : nouvelle liste de sommets.
    Clipped(Vec<Vertex>),
    /// Entièrement dehors, ou coupé par le plan lointain alors que
    /// POLYGON_ATTR bit 12 le cache.
    Rejected,
}

/// Distance signée au plan `n` (positive ou nulle = du bon côté). Plans :
/// 0 x ≥ −w, 1 x ≤ w, 2 y ≥ −w, 3 y ≤ w, 4 z ≥ −w (proche), 5 z ≤ w
/// (lointain).
fn distance(v: &Vertex, plane: usize) -> i64 {
    let w = v.clip[3] as i64;
    let c = v.clip[plane / 2] as i64;
    if plane.is_multiple_of(2) {
        w + c
    } else {
        w - c
    }
}

const FAR: usize = 5;

/// Point de l'arête `a`–`b` sur le plan, d'après les distances `da` ≥ 0 >
/// `db` (ou l'inverse) : interpolation linéaire de toutes les données.
fn intersect(a: &Vertex, b: &Vertex, da: i64, db: i64) -> Vertex {
    let num = da;
    let den = da - db;
    let lerp = |x: i64, y: i64| x + (y - x) * num / den;
    let mut v = *a;
    for k in 0..4 {
        v.clip[k] = lerp(a.clip[k] as i64, b.clip[k] as i64) as i32;
    }
    for k in 0..3 {
        v.color[k] = lerp(a.color[k] as i64, b.color[k] as i64) as u8;
    }
    for k in 0..2 {
        v.tex[k] = lerp(a.tex[k] as i64, b.tex[k] as i64) as i16;
    }
    v
}

/// Découpe un polygone contre les six plans (Sutherland–Hodgman).
pub fn clip(verts: &[Vertex], render_far_intersecting: bool) -> Clip {
    let outside = |plane: usize| verts.iter().any(|v| distance(v, plane) < 0);
    if !(0..6).any(outside) {
        return Clip::Inside;
    }
    if outside(FAR) && !render_far_intersecting {
        return Clip::Rejected;
    }
    let mut poly = verts.to_vec();
    for plane in 0..6 {
        if poly.is_empty() {
            break;
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = &poly[i];
            let b = &poly[(i + 1) % poly.len()];
            let (da, db) = (distance(a, plane), distance(b, plane));
            if da >= 0 {
                out.push(*a);
            }
            if (da >= 0) != (db >= 0) {
                out.push(intersect(a, b, da, db));
            }
        }
        poly = out;
    }
    if poly.len() < 3 {
        Clip::Rejected
    } else {
        Clip::Clipped(poly)
    }
}

/// Orientation du polygone : déterminant de (x, y, w) sur trois sommets,
/// en coordonnées homogènes (valable avant division par w). Positif =
/// anti-horaire dans un repère où y monte (celui du viewport, origine en
/// bas à gauche). On prend le premier triangle de l'éventail qui n'est pas
/// dégénéré ; 0 = segment ou point (toujours affiché, GBATEK).
pub fn orientation(verts: &[Vertex]) -> i128 {
    let det = |a: &Vertex, b: &Vertex, c: &Vertex| -> i128 {
        let [ax, ay, _, aw] = a.clip.map(|v| v as i128);
        let [bx, by, _, bw] = b.clip.map(|v| v as i128);
        let [cx, cy, _, cw] = c.clip.map(|v| v as i128);
        ax * (by * cw - bw * cy) - ay * (bx * cw - bw * cx) + aw * (bx * cy - by * cx)
    };
    (1..verts.len().saturating_sub(1)).map(|i| det(&verts[0], &verts[i], &verts[i + 1])).find(|&d| d != 0).unwrap_or(0)
}

/// Projection à l'écran (GBATEK) :
/// x = (xx + ww) × largeur / (2 × ww) + x1, y = (yy + ww) × hauteur /
/// (2 × ww) + y1, y compté depuis le bas ; rendu ici depuis le haut
/// (191 − y). Largeur = x2 − x1 + 1 (hypothèse : le plein écran 0..255
/// donne 256).
pub fn project(v: &mut Vertex, viewport: [u8; 4]) {
    let [x1, y1, x2, y2] = viewport.map(|c| c as i64);
    let (w, h) = (x2 - x1 + 1, y2 - y1 + 1);
    let ww = (v.clip[3] as i64).max(1);
    let sx = (v.clip[0] as i64 + ww) * w / (2 * ww) + x1;
    let sy = (v.clip[1] as i64 + ww) * h / (2 * ww) + y1;
    v.screen = [sx as i32, 191 - sy as i32];
}
