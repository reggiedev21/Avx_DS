//! Moteur de rendu, rastériseur **scalaire**, écrit pour être lu. Il
//! servira d'oracle à la version AVX2 (3D-5).
//!
//! - 3D-3 : plan arrière (couleur, alpha, profondeur), polygones opaques et
//!   fil de fer, couleurs de sommet, tampon de profondeur Z ou W, tests
//!   « inférieur » et « égal ».
//! - 3D-4 : interpolation corrigée de la perspective, textures (7 formats),
//!   mélange texture/sommet (modulation, décalque, toon, highlight), test
//!   alpha, polygones translucides dessinés après les opaques.
//!
//! Pas encore : plan arrière en bitmap, polygones d'ombre, effets (3D-6).
//! GBATEK ne décrit pas la rastérisation : règle de remplissage, profondeur
//! et interpolation sont des hypothèses (`docs/3D_SPEC.md`, section 10).

use super::polygon::{PolyRam, Polygon, Vertex};
use super::texture::{TexMem, TexParams};

pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 192;

/// Profondeur maximale (24 bits).
const DEPTH_MAX: u32 = 0xFF_FFFF;


/// Pixel du calque 3D : couleur 6 bits par composante, alpha 5 bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pixel {
    pub rgb: [u8; 3],
    pub alpha: u8,
}

/// Réglages lus dans les registres au moment du rendu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderParams {
    /// CLEAR_COLOR (0x04000350).
    pub clear_color: u32,
    /// CLEAR_DEPTH (0x04000354), 15 bits.
    pub clear_depth: u16,
    /// SWAP_BUFFERS bit 1 : profondeur par W au lieu de Z.
    pub w_buffer: bool,
    /// SWAP_BUFFERS bit 0 : translucides dans l'ordre reçu (sinon triés).
    pub manual_sort: bool,
    /// DISP3DCNT (0x04000060).
    pub disp3dcnt: u32,
    /// ALPHA_TEST_REF (0x04000340), 5 bits.
    pub alpha_ref: u8,
    /// TOON_TABLE (0x04000380), 32 couleurs 15 bits.
    pub toon: [u16; 32],
}

#[derive(Clone, Debug)]
pub struct Renderer {
    pub color: Vec<Pixel>,
    pub depth: Vec<u32>,
    /// Polygones translucides dessinés au dernier rendu.
    pub translucent: usize,
}

impl Default for Renderer {
    fn default() -> Self {
        Renderer { color: vec![Pixel::default(); WIDTH * HEIGHT], depth: vec![DEPTH_MAX; WIDTH * HEIGHT], translucent: 0 }
    }
}

/// Profondeur de 15 bits étendue à 24 bits : X × 200h + ((X + 1) / 8000h) ×
/// 1FFh (GBATEK « Rear Plane »).
pub fn expand_depth(x: u16) -> u32 {
    let x = (x & 0x7FFF) as u32;
    x * 0x200 + ((x + 1) / 0x8000) * 0x1FF
}

/// Couleur 5 bits → 6 bits, comme COLOR : X × 2 + (X + 31) / 32.
fn expand5(x: u32) -> u8 {
    let x = x & 31;
    (x * 2 + x.div_ceil(32)) as u8
}

/// 5 bits → 6 bits pour le mélange : « IF X>0 THEN X=X*2+1 » (GBATEK
/// « Texture Blending »).
fn to6(x: u8) -> u32 {
    if x > 0 {
        x as u32 * 2 + 1
    } else {
        0
    }
}

fn rgb15_to6(c: u16) -> [u32; 3] {
    [c & 31, (c >> 5) & 31, (c >> 10) & 31].map(|x| to6(x as u8))
}

/// Profondeur Z d'un sommet sur 24 bits. Hypothèse : z/w de [−1, 1]
/// ramené à [0, FFFFFFh] (en mode W, la profondeur vient de 1/w).
fn z_depth(v: &Vertex) -> i64 {
    let (z, w) = (v.clip[2] as i64, (v.clip[3] as i64).max(1));
    (z * 0x80_0000 / w + 0x80_0000).clamp(0, DEPTH_MAX as i64)
}

/// Marge ajoutée avant de tronquer une valeur reconstruite par
/// (a/w) × w : le calcul flottant peut rendre 29,99999 pour 30.
const EPS: f32 = 1.0 / 1024.0;

/// Point d'un bord : position et profondeur entières ; couleur et
/// coordonnées de texture divisées par w, avec 1/w, en flottants. Ces
/// quotients varient linéairement à l'écran : interpolés puis
/// redivisés par 1/w, ils suivent la perspective.
#[derive(Clone, Copy, Debug)]
struct Attr {
    x: i64,
    z_depth: i64,
    inv_w: f32,
    rgb: [f32; 3],
    st: [f32; 2],
}

impl Attr {
    fn of(v: &Vertex) -> Attr {
        let inv_w = ONE_F / (v.clip[3].max(1) as f32);
        Attr {
            x: v.screen[0] as i64,
            z_depth: z_depth(v),
            inv_w,
            rgb: v.color.map(|c| c as f32 * inv_w),
            st: v.tex.map(|c| c as f32 * inv_w),
        }
    }

    /// Point de l'arête `a`–`b` à la fraction `num / den` (une fois par
    /// bord et par ligne : les divisions entières ne coûtent pas ici).
    fn lerp(a: &Attr, b: &Attr, num: i64, den: i64) -> Attr {
        if den == 0 {
            return *a;
        }
        let t = num as f32 / den as f32;
        let f = |p: f32, q: f32| p + (q - p) * t;
        Attr {
            x: a.x + (b.x - a.x) * num / den,
            z_depth: a.z_depth + (b.z_depth - a.z_depth) * num / den,
            inv_w: f(a.inv_w, b.inv_w),
            rgb: [0, 1, 2].map(|k| f(a.rgb[k], b.rgb[k])),
            st: [0, 1].map(|k| f(a.st[k], b.st[k])),
        }
    }
}

/// Partie entière par défaut, sans appel à `floorf` (sans SSE4.1, x86-64 n'a
/// pas d'instruction d'arrondi et `f32::floor` devient un appel de
/// bibliothèque, mesuré à 23 instructions par pixel).
fn floor_i32(v: f32) -> i32 {
    let i = v as i32;
    i - ((i as f32) > v) as i32
}

/// 1.0 en w (12 bits de fraction) : 1/w est calculé en unités de w réelles.
const ONE_F: f32 = 4096.0;

/// Ce qu'un pixel reçoit : couleur du sommet (6 bits), texel (S, T
/// entiers), profondeur.
#[derive(Clone, Copy, Debug)]
struct Frag {
    rgb: [u32; 3],
    st: [i32; 2],
    depth: u32,
}

/// Une ligne d'un polygone, de `l` à `r` : les pas sont calculés une fois,
/// chaque pixel ne coûte plus qu'une division (1/w) et des multiplications.
struct Span {
    l: Attr,
    x0: i64,
    /// 1 / (largeur), en flottant ; 0 pour une ligne d'un pixel.
    inv_width: f32,
    d_inv_w: f32,
    d_rgb: [f32; 3],
    d_st: [f32; 2],
    /// Pas de profondeur Z, 16 bits de fraction.
    d_z: i64,
}

impl Span {
    fn new(l: &Attr, r: &Attr) -> Span {
        let den = r.x - l.x;
        Span {
            l: *l,
            x0: l.x,
            inv_width: if den == 0 { 0.0 } else { 1.0 / den as f32 },
            d_inv_w: r.inv_w - l.inv_w,
            d_rgb: [0, 1, 2].map(|k| r.rgb[k] - l.rgb[k]),
            d_st: [0, 1].map(|k| r.st[k] - l.st[k]),
            d_z: if den == 0 { 0 } else { ((r.z_depth - l.z_depth) << 16) / den },
        }
    }

    fn at(&self, x: i64, w_buffer: bool) -> Frag {
        let i = x - self.x0;
        let t = i as f32 * self.inv_width;
        let recip = 1.0 / (self.l.inv_w + self.d_inv_w * t);
        let rgb = [0, 1, 2].map(|k| (((self.l.rgb[k] + self.d_rgb[k] * t) * recip + EPS) as i32).clamp(0, 63) as u32);
        // S, T ont 4 bits de fraction : le texel est leur partie entière.
        let st = [0, 1].map(|k| floor_i32(((self.l.st[k] + self.d_st[k] * t) * recip + EPS) * (1.0 / 16.0)));
        let depth = if w_buffer { (recip * ONE_F) as i64 } else { self.l.z_depth + ((self.d_z * i) >> 16) };
        Frag { rgb, st, depth: depth.clamp(0, DEPTH_MAX as i64) as u32 }
    }
}

/// Ce qui ne change pas d'un pixel à l'autre d'un polygone.
struct PolyCtx<'a> {
    attr: u32,
    /// Alpha du polygone, 5 bits (31 pour le fil de fer).
    alpha: u8,
    tex: Option<TexParams>,
    mem: &'a TexMem,
    p: &'a RenderParams,
}

impl PolyCtx<'_> {
    /// Couleur et alpha (6 bits) d'un pixel, d'après GBATEK « Texture
    /// Blending ».
    fn shade(&self, f: &Frag) -> ([u32; 3], u32) {
        let v = f.rgb;
        let av = to6(self.alpha);
        let mode = (self.attr >> 4) & 3;
        let modulate = |t: u32, x: u32| ((t + 1) * (x + 1) - 1) / 64;
        let texel = self.tex.map(|t| t.sample(self.mem, f.st[0], f.st[1]));
        match (mode, texel) {
            // Sans texture : la couleur du sommet (modulation et décalque).
            (0 | 1 | 3, None) => (v, av),
            (0 | 3, Some(t)) => {
                // Modulation (le mode ombre suit la modulation en attendant
                // 3D-6).
                let tc = t.rgb.map(to6);
                ([0, 1, 2].map(|k| modulate(tc[k], v[k])), modulate(to6(t.alpha), av))
            }
            (1, Some(t)) => {
                // Décalque : l'alpha de texture règle le dosage ; aux bornes
                // (0 et 31) la couleur du sommet ou celle du texel.
                let tc = t.rgb.map(to6);
                let at = to6(t.alpha);
                let c = [0, 1, 2].map(|k| match t.alpha {
                    0 => v[k],
                    31 => tc[k],
                    _ => (tc[k] * at + v[k] * (63 - at)) / 64,
                });
                (c, av)
            }
            (_, t) => {
                // Toon ou highlight (DISP3DCNT bit 1) : R du sommet indexe la
                // table (index = R / 2 sur 6 bits, hypothèse). Sans texture,
                // texel blanc opaque (hypothèse).
                let s = rgb15_to6(self.p.toon[(v[0] >> 1) as usize]);
                let (tc, at) = t.map_or(([63; 3], 63), |t| (t.rgb.map(to6), to6(t.alpha)));
                let highlight = self.p.disp3dcnt & 2 != 0;
                let c = [0, 1, 2].map(|k| {
                    let m = modulate(tc[k], s[k]);
                    if highlight { (m + s[k]).min(63) } else { m }
                });
                (c, modulate(at, av))
            }
        }
    }
}

impl Renderer {
    /// Dessine une image complète à partir de la liste livrée au rendu.
    pub fn render(&mut self, list: &PolyRam, mem: &TexMem, p: &RenderParams) {
        self.clear(p);
        let textures_on = p.disp3dcnt & 1 != 0;
        // Opaques d'abord, dans l'ordre reçu ; puis les translucides.
        let translucent = |poly: &Polygon| {
            let alpha = (poly.attr >> 16) & 31;
            let format = (poly.teximage >> 26) & 7;
            (1..=30).contains(&alpha) || (textures_on && (format == 1 || format == 6))
        };
        let (mut back, front): (Vec<&Polygon>, Vec<&Polygon>) = list.polygons.iter().partition(|p| translucent(p));
        for poly in front {
            self.draw(poly, mem, p);
        }
        // Tri automatique des translucides : hypothèse, par y du haut puis du
        // bas croissants (GBATEK ne donne pas la règle) ; tri stable.
        if !p.manual_sort {
            let key = |poly: &&Polygon| {
                let ys = poly.vertices.iter().map(|v| v.screen[1]);
                (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0))
            };
            back.sort_by_key(key);
        }
        self.translucent = back.len();
        for poly in back {
            self.draw(poly, mem, p);
        }
    }

    /// Plan arrière en couleur unie (le mode bitmap viendra plus tard).
    fn clear(&mut self, p: &RenderParams) {
        let c = p.clear_color;
        let px = Pixel { rgb: [expand5(c), expand5(c >> 5), expand5(c >> 10)], alpha: ((c >> 16) & 31) as u8 };
        self.color.fill(px);
        self.depth.fill(expand_depth(p.clear_depth));
    }

    /// Remplit un polygone convexe ligne par ligne. Règle « haut-gauche » :
    /// la dernière ligne et la dernière colonne sont exclues, si bien que
    /// deux polygones voisins ne se recouvrent pas. Un polygone sans hauteur
    /// ou sans largeur (segment, point) garde sa ligne ou son pixel.
    /// GBATEK ne donne pas la règle ; les sprites découpés de Pokémon N2
    /// montraient des lignes parasites avec des bords inclus.
    fn draw(&mut self, poly: &Polygon, mem: &TexMem, p: &RenderParams) {
        let alpha = ((poly.attr >> 16) & 31) as u8;
        let wire = alpha == 0;
        let tex = TexParams::new(poly.teximage, poly.pltt_base);
        let ctx = PolyCtx {
            attr: poly.attr,
            alpha: if wire { 31 } else { alpha },
            tex: (p.disp3dcnt & 1 != 0 && tex.format != 0).then_some(tex),
            mem,
            p,
        };
        let verts: Vec<(i64, Attr)> = poly.vertices.iter().map(|v| (v.screen[1] as i64, Attr::of(v))).collect();
        let ymin = verts.iter().map(|v| v.0).min().unwrap_or(0).max(0);
        let ytop = verts.iter().map(|v| v.0).min().unwrap_or(0);
        let ybottom = verts.iter().map(|v| v.0).max().unwrap_or(-1);
        let last_row = if ybottom > ytop { ybottom - 1 } else { ybottom };
        let ymax = last_row.min(HEIGHT as i64 - 1);
        let n = verts.len();
        for y in ymin..=ymax {
            // Les bords qui traversent cette ligne : le plus à gauche et le
            // plus à droite délimitent la portion à remplir.
            let mut left: Option<Attr> = None;
            let mut right: Option<Attr> = None;
            for i in 0..n {
                let (y0, a0) = verts[i];
                let (y1, a1) = verts[(i + 1) % n];
                if y < y0.min(y1) || y > y0.max(y1) {
                    continue;
                }
                let hits = if y0 == y1 { [a0, a1] } else { [Attr::lerp(&a0, &a1, y - y0, y1 - y0); 2] };
                for h in hits {
                    if left.is_none_or(|l| h.x < l.x) {
                        left = Some(h);
                    }
                    if right.is_none_or(|r| h.x > r.x) {
                        right = Some(h);
                    }
                }
            }
            let (Some(l), Some(r)) = (left, right) else { continue };
            let edge_row = y == ytop || y == last_row;
            let last_x = if r.x > l.x { r.x - 1 } else { r.x };
            let span = Span::new(&l, &r);
            for x in l.x.max(0)..=last_x.min(WIDTH as i64 - 1) {
                if wire && !edge_row && x != l.x && x != last_x {
                    continue;
                }
                let frag = span.at(x, p.w_buffer);
                self.plot(x as usize, y as usize, &frag, &ctx);
            }
        }
    }

    fn plot(&mut self, x: usize, y: usize, f: &Frag, ctx: &PolyCtx) {
        let (rgb, a6) = ctx.shade(f);
        let alpha = (a6 >> 1) as u8;
        // Pixels totalement transparents : rien n'est écrit (GBATEK).
        if alpha == 0 {
            return;
        }
        // Test alpha : seulement si l'alpha est strictement supérieur.
        if ctx.p.disp3dcnt & 4 != 0 && alpha <= ctx.p.alpha_ref {
            return;
        }
        let i = y * WIDTH + x;
        let d = f.depth;
        let old = self.depth[i];
        // « Égal » : tolérance de ±200h (GBATEK, POLYGON_ATTR bit 14).
        let pass = if ctx.attr & (1 << 14) != 0 { d.abs_diff(old) <= 0x200 } else { d < old };
        if !pass {
            return;
        }
        let fb = self.color[i];
        let blend = ctx.p.disp3dcnt & 8 != 0 && alpha < 31 && fb.alpha != 0;
        self.color[i] = if blend {
            let pa = alpha as u32;
            let c = [0, 1, 2].map(|k| ((rgb[k] * (pa + 1) + fb.rgb[k] as u32 * (31 - pa)) / 32) as u8);
            Pixel { rgb: c, alpha: alpha.max(fb.alpha) }
        } else {
            Pixel { rgb: rgb.map(|c| c as u8), alpha }
        };
        // Profondeur : toujours pour un pixel opaque ; pour un translucide,
        // seulement si POLYGON_ATTR bit 11.
        if alpha == 31 || ctx.attr & (1 << 11) != 0 {
            self.depth[i] = d;
        }
    }
}
