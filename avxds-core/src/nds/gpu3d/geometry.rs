//! Moteur de géométrie (étape 3D-2) : matrices et piles, sommets,
//! assemblage des primitives, faces cachées, découpage, viewport, lumière,
//! tests. Écrit d'abord simple et lisible : c'est la référence des
//! versions rapides à venir.
//!
//! Sources : `docs/3D_SPEC.md`, sections 5 à 8 (GBATEK). Les choix que
//! GBATEK ne tranche pas sont marqués « hypothèse ».

use super::math::{dot3, sext, unpack10, Mat4, ONE};
use super::polygon::{self, Clip, PolyRam, Polygon, Vertex};

/// Couleur 5 bits → 6 bits : X × 2 + (X + 31) / 32 (GBATEK, COLOR) ;
/// (X + 31) / 32 s'écrit `div_ceil(32)`.
fn expand5(x: u32) -> u8 {
    let x = x & 31;
    (x * 2 + x.div_ceil(32)) as u8
}

fn rgb5(v: u32) -> [u8; 3] {
    [(v & 31) as u8, ((v >> 5) & 31) as u8, ((v >> 10) & 31) as u8]
}

fn expand_rgb(c: [u8; 3]) -> [u8; 3] {
    c.map(|x| expand5(x as u32))
}

#[derive(Clone, Debug)]
pub struct Geometry {
    /// MTX_MODE : 0 projection, 1 position, 2 position et vecteur, 3 texture.
    pub mode: u8,
    pub projection: Mat4,
    pub position: Mat4,
    pub vector: Mat4,
    pub texture: Mat4,
    /// ClipMatrix = PositionMatrix × ProjectionMatrix, recalculée à chaque
    /// changement de l'une ou l'autre.
    pub clip: Mat4,
    proj_stack: Mat4,
    proj_sp: u8,
    tex_stack: Mat4,
    tex_sp: u8,
    pos_stack: [Mat4; 32],
    vec_stack: [Mat4; 32],
    /// Pointeur commun des piles position et vecteur (6 bits).
    sp: u8,
    /// GXSTAT bit 15 : débordement de pile.
    pub stack_error: bool,

    /// Derniers x, y, z reçus (12 bits de fraction) : VTX_XY & co. gardent
    /// la coordonnée manquante, VTX_DIFF s'y ajoute.
    vtx: [i32; 3],
    color: [u8; 3],
    /// S, T reçus par TEXCOORD (1+11+4 bits), et ceux du prochain sommet,
    /// après la transformation choisie par TEXIMAGE_PARAM bits 30–31.
    raw_tex: [i16; 2],
    tex: [i16; 2],

    diffuse: [u8; 3],
    ambient: [u8; 3],
    specular: [u8; 3],
    emission: [u8; 3],
    shininess_table_on: bool,
    shininess: [u8; 128],
    light_dir: [[i32; 3]; 4],
    light_half: [[i32; 3]; 4],
    light_color: [[u8; 3]; 4],

    /// POLYGON_ATTR écrit, puis pris en compte au BEGIN_VTXS.
    attr_pending: u32,
    attr: u32,
    teximage: u32,
    pltt_base: u32,
    prim: u8,
    /// Sommets en attente de former un polygone.
    pending: Vec<Vertex>,
    /// Triangles déjà formés dans la bande en cours (pour l'alternance).
    strip_tris: usize,
    /// Le polygone précédent de la bande a été stocké sans découpage : le
    /// suivant partage ses sommets.
    strip_link: bool,
    viewport: [u8; 4],

    /// Mémoire en construction, et celle livrée au rendu au dernier échange.
    pub building: PolyRam,
    pub rendering: PolyRam,
    swap_flags: u32,

    pub pos_result: [i32; 4],
    pub vec_result: [i16; 3],
    pub box_result: bool,
    pub disp_1dot_depth: u16,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            mode: 0,
            projection: Mat4::IDENTITY,
            position: Mat4::IDENTITY,
            vector: Mat4::IDENTITY,
            texture: Mat4::IDENTITY,
            clip: Mat4::IDENTITY,
            proj_stack: Mat4::IDENTITY,
            proj_sp: 0,
            tex_stack: Mat4::IDENTITY,
            tex_sp: 0,
            pos_stack: [Mat4::IDENTITY; 32],
            vec_stack: [Mat4::IDENTITY; 32],
            sp: 0,
            stack_error: false,
            vtx: [0; 3],
            color: [63; 3],
            raw_tex: [0; 2],
            tex: [0; 2],
            diffuse: [0; 3],
            ambient: [0; 3],
            specular: [0; 3],
            emission: [0; 3],
            shininess_table_on: false,
            shininess: [0; 128],
            light_dir: [[0; 3]; 4],
            light_half: [[0; 3]; 4],
            light_color: [[0; 3]; 4],
            attr_pending: 0,
            attr: 0,
            teximage: 0,
            pltt_base: 0,
            prim: 0,
            pending: Vec::with_capacity(4),
            strip_tris: 0,
            strip_link: false,
            viewport: [0, 0, 255, 191],
            building: PolyRam::default(),
            rendering: PolyRam::default(),
            swap_flags: 0,
            pos_result: [0; 4],
            vec_result: [0; 3],
            box_result: false,
            disp_1dot_depth: 0,
        }
    }
}

impl Geometry {
    /// Exécute une commande complète (paramètres déjà réunis).
    pub fn execute(&mut self, cmd: u8, p: &[u32]) {
        let p0 = p.first().copied().unwrap_or(0);
        match cmd {
            0x10 => self.mode = (p0 & 3) as u8,
            0x11 => self.push(),
            0x12 => self.pop(p0),
            0x13 => self.store(p0),
            0x14 => self.restore(p0),
            0x15 => self.apply(false, |_| Mat4::IDENTITY),
            0x16 => self.apply(false, |_| Mat4::from_4x4(p)),
            0x17 => self.apply(false, |_| Mat4::from_4x3(p)),
            0x18 => self.apply(false, |c| Mat4::from_4x4(p).mul(c)),
            0x19 => self.apply(false, |c| Mat4::from_4x3(p).mul(c)),
            0x1A => self.apply(false, |c| Mat4::from_3x3(p).mul(c)),
            0x1B => self.apply(true, |c| Mat4::scale(p).mul(c)),
            0x1C => self.apply(false, |c| Mat4::translation(p).mul(c)),
            0x20 => self.color = expand_rgb(rgb5(p0)),
            0x21 => self.normal(p0),
            0x22 => {
                self.raw_tex = [p0 as i16, (p0 >> 16) as i16];
                self.tex = match self.tex_mode() {
                    1 => self.tex_from_texcoord(),
                    _ => self.raw_tex,
                };
            }
            0x23 => {
                let p1 = p.get(1).copied().unwrap_or(0);
                self.vertex([sext(p0, 16), sext(p0 >> 16, 16), sext(p1, 16)]);
            }
            0x24 => self.vertex(unpack10(p0).map(|c| c << 6)),
            0x25 => self.vertex([sext(p0, 16), sext(p0 >> 16, 16), self.vtx[2]]),
            0x26 => self.vertex([sext(p0, 16), self.vtx[1], sext(p0 >> 16, 16)]),
            0x27 => self.vertex([self.vtx[0], sext(p0, 16), sext(p0 >> 16, 16)]),
            0x28 => {
                // 9 bits de fraction divisés par 8 = 12 bits de fraction ; le
                // résultat reste dans les 16 bits d'une coordonnée.
                let d = unpack10(p0);
                let v = [0, 1, 2].map(|k| (self.vtx[k] + d[k]) as i16 as i32);
                self.vertex(v);
            }
            0x29 => self.attr_pending = p0,
            0x2A => self.teximage = p0,
            0x2B => self.pltt_base = p0,
            0x30 => {
                self.diffuse = rgb5(p0);
                self.ambient = rgb5(p0 >> 16);
                if p0 & (1 << 15) != 0 {
                    self.color = expand_rgb(self.diffuse);
                }
            }
            0x31 => {
                self.specular = rgb5(p0);
                self.emission = rgb5(p0 >> 16);
                self.shininess_table_on = p0 & (1 << 15) != 0;
            }
            0x32 => {
                let i = (p0 >> 30) as usize;
                let d = self.vector.transform_dir(unpack10(p0).map(|c| c << 3));
                self.light_dir[i] = d;
                // HalfVector = (LightVector + (0, 0, −1.0)) / 2.
                self.light_half[i] = [d[0] / 2, d[1] / 2, (d[2] - ONE) / 2];
            }
            0x33 => self.light_color[(p0 >> 30) as usize] = rgb5(p0),
            0x34 => {
                for (i, &w) in p.iter().take(32).enumerate() {
                    self.shininess[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
                }
            }
            0x40 => self.begin(p0),
            0x41 => {} // END_VTXS : sans effet (GBATEK)
            0x60 => self.viewport = p0.to_le_bytes(),
            0x70 => self.box_test(p),
            0x71 => {
                let p1 = p.get(1).copied().unwrap_or(0);
                self.vtx = [sext(p0, 16), sext(p0 >> 16, 16), sext(p1, 16)];
                self.pos_result = self.clip.transform([self.vtx[0], self.vtx[1], self.vtx[2], ONE]);
            }
            0x72 => {
                let r = self.vector.transform_dir(unpack10(p0).map(|c| c << 3));
                // 4 bits de signe, 12 de fraction : +1.0 se relit F000h.
                self.vec_result = r.map(|c| sext(c as u32, 13) as i16);
            }
            _ => {} // SWAP_BUFFERS : géré par le moteur (échange au VBlank)
        }
    }

    // ---- Matrices ----

    /// Applique `f` aux matrices du mode courant. En mode 2, la matrice de
    /// vecteur suit, sauf pour MTX_SCALE (`scale`).
    fn apply(&mut self, scale: bool, f: impl Fn(&Mat4) -> Mat4) {
        match self.mode {
            0 => self.projection = f(&self.projection),
            1 => self.position = f(&self.position),
            2 => {
                self.position = f(&self.position);
                if !scale {
                    self.vector = f(&self.vector);
                }
            }
            _ => self.texture = f(&self.texture),
        }
        self.update_clip();
    }

    fn update_clip(&mut self) {
        self.clip = self.position.mul(&self.projection);
    }

    fn push(&mut self) {
        match self.mode {
            0 => {
                self.stack_error |= self.proj_sp != 0;
                self.proj_stack = self.projection;
                self.proj_sp ^= 1;
            }
            3 => {
                self.stack_error |= self.tex_sp != 0;
                self.tex_stack = self.texture;
                self.tex_sp ^= 1;
            }
            _ => {
                // Entrées 0..30 ; 31 existe mais pose l'erreur ; 32..63 sont
                // des miroirs de 0..31 (GBATEK « Matrix Stack »).
                self.stack_error |= self.sp >= 31;
                let i = (self.sp & 31) as usize;
                self.pos_stack[i] = self.position;
                self.vec_stack[i] = self.vector;
                self.sp = (self.sp + 1) & 63;
            }
        }
    }

    fn pop(&mut self, p: u32) {
        match self.mode {
            0 => {
                self.proj_sp ^= 1;
                self.stack_error |= self.proj_sp != 0;
                self.projection = self.proj_stack;
            }
            3 => {
                self.tex_sp ^= 1;
                self.stack_error |= self.tex_sp != 0;
                self.texture = self.tex_stack;
            }
            _ => {
                let n = sext(p, 6);
                self.sp = (self.sp as i32 - n) as u8 & 63;
                self.stack_error |= self.sp >= 31;
                let i = (self.sp & 31) as usize;
                self.position = self.pos_stack[i];
                self.vector = self.vec_stack[i];
            }
        }
        self.update_clip();
    }

    fn store(&mut self, p: u32) {
        match self.mode {
            0 => self.proj_stack = self.projection,
            3 => self.tex_stack = self.texture,
            _ => {
                let i = (p & 31) as usize;
                self.stack_error |= i == 31;
                self.pos_stack[i] = self.position;
                self.vec_stack[i] = self.vector;
            }
        }
    }

    fn restore(&mut self, p: u32) {
        match self.mode {
            0 => self.projection = self.proj_stack,
            3 => self.texture = self.tex_stack,
            _ => {
                let i = (p & 31) as usize;
                self.stack_error |= i == 31;
                self.position = self.pos_stack[i];
                self.vector = self.vec_stack[i];
            }
        }
        self.update_clip();
    }

    /// GXSTAT bits 8–15 : niveaux de pile et erreur.
    pub fn stack_status(&self) -> u32 {
        ((self.sp & 31) as u32) << 8 | (self.proj_sp as u32) << 13 | (self.stack_error as u32) << 15
    }

    /// Écriture de 1 dans GXSTAT bit 15 : acquitte l'erreur et remet à zéro
    /// le pointeur de projection (et, « probablement », celui des textures).
    pub fn ack_stack_error(&mut self) {
        self.stack_error = false;
        self.proj_sp = 0;
        self.tex_sp = 0;
    }

    // ---- Lumière ----

    /// NORMAL : couleur du sommet d'après les lumières (formule GBATEK).
    /// Hypothèses (échelles non données) : couleur × couleur / 32 ; niveaux
    /// à 12 bits de fraction ; brillance ramenée à 8 bits, table indexée par
    /// ses 7 bits de poids fort.
    fn normal(&mut self, p: u32) {
        if self.tex_mode() == 2 {
            self.tex = self.tex_from_source(unpack10(p), 21);
        }
        let n = self.vector.transform_dir(unpack10(p).map(|c| c << 3));
        let mut c = self.emission.map(|x| x as i64);
        for i in 0..4 {
            if self.attr & (1 << i) == 0 {
                continue;
            }
            let diffuse = (-dot3(self.light_dir[i], n)).max(0) as i64;
            let s = (-dot3(self.light_half[i], n)).max(0) as i64;
            let mut shine = ((s * s) >> 16).min(255);
            if self.shininess_table_on {
                shine = self.shininess[(shine >> 1) as usize] as i64;
            }
            for (k, ck) in c.iter_mut().enumerate() {
                let lc = self.light_color[i][k] as i64;
                *ck += (self.specular[k] as i64 * lc * shine) >> 13;
                *ck += (self.diffuse[k] as i64 * lc * diffuse) >> 17;
                *ck += (self.ambient[k] as i64 * lc) >> 5;
            }
        }
        self.color = c.map(|x| expand5(x.clamp(0, 31) as u32));
    }

    // ---- Coordonnées de texture (GBATEK « Texture Coordinates ») ----

    fn tex_mode(&self) -> u32 {
        self.teximage >> 30
    }

    /// Mode 1 : (S, T, 1/16, 1/16) × colonnes 0–1 de la matrice de texture.
    /// S, T ont 4 bits de fraction, la matrice 12 : la somme en a 16, et
    /// 1/16 × m = m sur cette échelle. Résultat ramené à 4 bits de fraction
    /// et masqué à 16 bits.
    fn tex_from_texcoord(&self) -> [i16; 2] {
        let m = &self.texture.0;
        let [s, t] = self.raw_tex.map(|c| c as i64);
        [0, 1].map(|c| ((s * m[0][c] as i64 + t * m[1][c] as i64 + m[2][c] as i64 + m[3][c] as i64) >> 12) as i16)
    }

    /// Modes 2 et 3 : (x, y, z) × lignes 0–2, plus S, T du dernier
    /// TEXCOORD. `frac` : bits de fraction du produit (normale 9 + 12 = 21,
    /// sommet 12 + 12 = 24), ramené à 4. Décalages : hypothèse (GBATEK dit
    /// seulement « décalé vers 4 bits de fraction, masqué à 16 bits »).
    fn tex_from_source(&self, v: [i32; 3], frac: u32) -> [i16; 2] {
        let m = &self.texture.0;
        [0, 1].map(|c| {
            let sum: i64 = (0..3).map(|k| v[k] as i64 * m[k][c] as i64).sum();
            ((sum >> (frac - 4)) + self.raw_tex[c] as i64) as i16
        })
    }

    // ---- Sommets et polygones ----

    fn begin(&mut self, p: u32) {
        self.prim = (p & 3) as u8;
        self.attr = self.attr_pending;
        self.pending.clear();
        self.strip_tris = 0;
        self.strip_link = false;
    }

    fn vertex(&mut self, xyz: [i32; 3]) {
        self.vtx = xyz;
        if self.tex_mode() == 3 {
            self.tex = self.tex_from_source(xyz, 24);
        }
        let clip = self.clip.transform([xyz[0], xyz[1], xyz[2], ONE]);
        self.pending.push(Vertex { clip, color: self.color, tex: self.tex, screen: [0; 2] });
        let v = &self.pending;
        match (self.prim, v.len()) {
            (0, 3) | (1, 4) => {
                let poly = v.clone();
                self.pending.clear();
                self.polygon(&poly, false);
            }
            (2, 3) => {
                // Un triangle sur deux arrive dans le sens horaire : on le
                // remet dans le sens des autres.
                let poly = if self.strip_tris % 2 == 1 { vec![v[1], v[0], v[2]] } else { v.clone() };
                self.strip_tris += 1;
                self.pending.remove(0);
                self.polygon(&poly, true);
            }
            (3, 4) => {
                // Bande de quadrilatères en « haut-bas » : 0 1 3 2.
                let poly = vec![v[0], v[1], v[3], v[2]];
                self.pending.drain(0..2);
                self.polygon(&poly, true);
            }
            _ => {}
        }
    }

    /// Faces cachées, découpage, projection, stockage.
    fn polygon(&mut self, verts: &[Vertex], strip: bool) {
        let o = polygon::orientation(verts);
        let front = o > 0;
        let shown = o == 0 || if front { self.attr & (1 << 7) != 0 } else { self.attr & (1 << 6) != 0 };
        if !shown {
            self.strip_link = false;
            return;
        }
        let (mut out, unclipped) = match polygon::clip(verts, self.attr & (1 << 12) != 0) {
            Clip::Inside => (verts.to_vec(), true),
            Clip::Clipped(v) => (v, false),
            Clip::Rejected => {
                self.strip_link = false;
                return;
            }
        };
        for v in &mut out {
            polygon::project(v, self.viewport);
        }
        // Une bande non découpée partage ses sommets avec le polygone
        // précédent ; découpée, elle devient un polygone séparé.
        let new_vertices = match (strip && unclipped && self.strip_link, self.prim) {
            (true, 2) => 1,
            (true, _) => 2,
            (false, _) => out.len(),
        };
        let p = Polygon { vertices: out, attr: self.attr, teximage: self.teximage, pltt_base: self.pltt_base, front };
        let stored = self.building.store(p, new_vertices);
        self.strip_link = stored && strip && unclipped;
    }

    // ---- Tests ----

    /// BOX_TEST : une face au moins du pavé est-elle (en partie) dans le
    /// volume de vue ? Les faces seules comptent (GBATEK).
    fn box_test(&mut self, p: &[u32]) {
        let g = |i: usize| p.get(i).copied().unwrap_or(0);
        let (x, y) = (sext(g(0), 16), sext(g(0) >> 16, 16));
        let (z, w) = (sext(g(1), 16), sext(g(1) >> 16, 16));
        let (h, d) = (sext(g(2), 16), sext(g(2) >> 16, 16));
        let corner = |i: usize| {
            let c = [x + if i & 1 != 0 { w } else { 0 }, y + if i & 2 != 0 { h } else { 0 }, z + if i & 4 != 0 { d } else { 0 }];
            Vertex { clip: self.clip.transform([c[0], c[1], c[2], ONE]), ..Vertex::default() }
        };
        let faces: [[usize; 4]; 6] = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
        self.box_result = faces.iter().any(|f| polygon::clip(&f.map(corner), true) != Clip::Rejected);
    }

    // ---- Échange ----

    /// SWAP_BUFFERS exécuté : ses bits vaudront pour la liste suivante.
    pub fn request_swap(&mut self, flags: u32) {
        self.swap_flags = flags & 3;
    }

    /// VBlank après SWAP_BUFFERS : la liste construite part au rendu, une
    /// liste vide reprend la construction.
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.building, &mut self.rendering);
        self.building.clear();
        self.building.swap_flags = self.swap_flags;
    }

    /// RAM_COUNT : polygones (bits 0–11) et sommets (bits 16–28) en cours.
    pub fn ram_count(&self) -> u32 {
        self.building.polygons.len() as u32 | (self.building.vertex_count as u32) << 16
    }

    /// VECMTX_RESULT : m[0..8] de la matrice de vecteur, en 3×3.
    pub fn vecmtx(&self, i: usize) -> i32 {
        self.vector.0[i / 3][i % 3]
    }
}
