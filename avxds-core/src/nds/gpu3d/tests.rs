//! Tests du moteur de géométrie (étape 3D-2).

use super::geometry::Geometry;
use super::math::{Mat4, ONE};
use super::polygon::{self, Clip, Vertex, MAX_POLYGONS};

/// Coordonnée 4.12 en paramètre 16 bits.
fn f(x: f64) -> u32 {
    ((x * ONE as f64) as i32 as u32) & 0xFFFF
}

fn xy(x: f64, y: f64) -> u32 {
    f(x) | f(y) << 16
}

/// Commandes de base : matrices identité, viewport plein écran, faces
/// avant et arrière affichées, alpha plein.
fn geo() -> Geometry {
    let mut g = Geometry::default();
    g.execute(0x29, &[0xC0 | 31 << 16]); // POLYGON_ATTR
    g
}

fn v16(g: &mut Geometry, x: f64, y: f64, z: f64) {
    g.execute(0x23, &[xy(x, y), f(z)]);
}

fn begin(g: &mut Geometry, prim: u32) {
    g.execute(0x40, &[prim]);
}

#[test]
fn matrices_multiply_new_times_current() {
    // C = M × C : avec des vecteurs-lignes, la dernière matrice envoyée
    // s'applique la première au sommet.
    let mut g = geo();
    g.execute(0x10, &[1]); // position
    g.execute(0x1C, &[f(1.0), f(2.0), f(3.0)]); // translation
    g.execute(0x1B, &[f(2.0), f(2.0), f(2.0)]); // échelle
    g.execute(0x71, &[xy(1.0, 0.0), f(0.0)]); // POS_TEST
    assert_eq!(g.pos_result, [3 * ONE, 2 * ONE, 3 * ONE, ONE], "(1,0,0) × échelle × translation");
    // Dans l'autre ordre : la translation s'applique d'abord.
    for cmd in [0x1C, 0x18] {
        let mut g = geo();
        g.execute(0x10, &[1]);
        g.execute(0x1B, &[f(2.0), f(2.0), f(2.0)]);
        if cmd == 0x1C {
            g.execute(0x1C, &[f(1.0), 0, 0]);
        } else {
            let m = [f(1.0), 0, 0, 0, 0, f(1.0), 0, 0, 0, 0, f(1.0), 0, f(1.0), 0, 0, f(1.0)];
            g.execute(0x18, &m); // MTX_MULT_4x4 d'une translation
        }
        g.execute(0x71, &[xy(1.0, 0.0), f(0.0)]);
        assert_eq!(g.pos_result[0], 4 * ONE, "commande {cmd:#x} : (1 + 1) × 2");
    }
}

#[test]
fn matrix_types_fill_the_missing_elements() {
    let p: Vec<u32> = (1..=12).collect();
    let m = Mat4::from_4x3(&p);
    assert_eq!(m.0, [[1, 2, 3, 0], [4, 5, 6, 0], [7, 8, 9, 0], [10, 11, 12, ONE]]);
    let m = Mat4::from_3x3(&p[..9]);
    assert_eq!(m.0, [[1, 2, 3, 0], [4, 5, 6, 0], [7, 8, 9, 0], [0, 0, 0, ONE]]);
    let m = Mat4::translation(&[5, 6, 7]);
    assert_eq!(m.0[3], [5, 6, 7, ONE]);
    // Produit : cumul en 64 bits, >> 12 à la fin seulement.
    let a = Mat4([[ONE / 2 + 1; 4]; 4]);
    let b = Mat4([[3; 4]; 4]);
    assert_eq!(a.mul(&b).0[0][0], ((4 * (ONE as i64 / 2 + 1) * 3) >> 12) as i32);
    // Troncature (décalage arithmétique), pas d'arrondi : 0.75 → 0,
    // −0.75 → −1 (choix d'AvxDS, GBATEK ne tranche pas).
    let one = Mat4([[1, 0, 0, 0]; 4]);
    assert_eq!(one.mul(&Mat4([[768; 4]; 4])).0[0][0], 0);
    assert_eq!(one.mul(&Mat4([[-768; 4]; 4])).0[0][0], -1);
}

#[test]
fn clip_matrix_is_position_times_projection() {
    let mut g = geo();
    g.execute(0x10, &[0]);
    g.execute(0x1B, &[f(2.0), f(2.0), f(2.0)]); // projection : échelle 2
    g.execute(0x10, &[1]);
    g.execute(0x1C, &[f(1.0), 0, 0]); // position : translation x + 1
    assert_eq!(g.clip, g.position.mul(&g.projection));
    assert_eq!(g.clip.at(12), 2 * ONE, "la translation passe par la projection");
}

#[test]
fn mode2_scale_spares_the_vector_matrix() {
    let mut g = geo();
    g.execute(0x10, &[2]);
    g.execute(0x1C, &[f(1.0), 0, 0]);
    g.execute(0x1B, &[f(2.0), f(2.0), f(2.0)]);
    g.execute(0x1A, &[f(0.0), f(1.0), 0, f(-1.0), 0, 0, 0, 0, f(1.0)]); // rotation
    assert_eq!(g.vector.0[0][1], ONE, "rotation appliquée au vecteur");
    assert_eq!(g.vecmtx(0), 0);
    assert_eq!(g.position.0[0][1], 2 * ONE, "échelle appliquée à la position seule");
    g.execute(0x10, &[1]);
    g.execute(0x15, &[]);
    assert_eq!(g.position, Mat4::IDENTITY);
    assert_ne!(g.vector, Mat4::IDENTITY, "mode 1 : la position seule");
}

#[test]
fn position_stack_levels_errors_and_mirrors() {
    let mut g = geo();
    g.execute(0x10, &[1]);
    g.execute(0x1C, &[f(1.0), 0, 0]);
    g.execute(0x11, &[]); // PUSH : niveau 1
    g.execute(0x15, &[]);
    g.execute(0x11, &[]); // niveau 2
    assert_eq!(g.stack_status() >> 8 & 31, 2);
    g.execute(0x12, &[2]); // POP 2 : retour au niveau 0, la translation
    assert_eq!(g.stack_status() >> 8 & 31, 0);
    assert_eq!(g.position.0[3][0], ONE);
    g.execute(0x12, &[0x3F]); // POP −1 : « réalloue » le niveau 1 (identité)
    assert_eq!(g.position, Mat4::IDENTITY);
    assert_eq!(g.stack_status() & (1 << 15), 0);
    // STORE / RESTORE à une adresse ; 31 pose l'erreur.
    g.execute(0x1C, &[f(5.0), 0, 0]);
    g.execute(0x13, &[7]);
    g.execute(0x15, &[]);
    g.execute(0x14, &[7]);
    assert_eq!(g.position.0[3][0], 5 * ONE);
    g.execute(0x13, &[31]);
    assert_ne!(g.stack_status() & (1 << 15), 0, "entrée 31");
    g.ack_stack_error();
    assert_eq!(g.stack_status() & (1 << 15), 0);
    // 31 PUSH depuis 0 : le 32e (pointeur 31) déborde.
    let mut g = geo();
    g.execute(0x10, &[2]);
    for _ in 0..31 {
        g.execute(0x11, &[]);
    }
    assert_eq!(g.stack_status() & (1 << 15), 0);
    g.execute(0x11, &[]);
    assert_ne!(g.stack_status() & (1 << 15), 0);
}

#[test]
fn projection_stack_has_one_entry() {
    let mut g = geo();
    g.execute(0x10, &[0]);
    g.execute(0x1B, &[f(3.0), f(3.0), f(3.0)]);
    g.execute(0x11, &[]);
    assert_eq!(g.stack_status() >> 13 & 1, 1);
    g.execute(0x15, &[]);
    g.execute(0x12, &[5]); // paramètre ignoré : toujours 1
    assert_eq!(g.projection.0[0][0], 3 * ONE);
    assert_eq!(g.stack_status() & (3 << 13), 0);
    g.execute(0x11, &[]);
    g.execute(0x11, &[]);
    assert_ne!(g.stack_status() & (1 << 15), 0, "2e PUSH sans place");
    g.ack_stack_error();
    g.execute(0x12, &[1]); // POP de trop : erreur, pointeur à 1
    assert_eq!(g.stack_status() & (1 << 13 | 1 << 15), 1 << 13 | 1 << 15);
    g.ack_stack_error();
    assert_eq!(g.stack_status() >> 13 & 1, 0, "l'acquittement remet le pointeur à 0");
}

#[test]
fn vertex_formats_keep_or_offset_the_previous_coordinates() {
    let mut g = geo();
    begin(&mut g, 0);
    v16(&mut g, 0.5, -0.25, 0.75);
    g.execute(0x25, &[xy(0.125, 0.5)]); // VTX_XY : z gardé
    g.execute(0x28, &[0x3FF | 1 << 10]); // VTX_DIFF : x − 1/4096, y + 1/4096
    let p = &g.building.polygons[0].vertices;
    assert_eq!(p[0].clip, [ONE / 2, -ONE / 4, 3 * ONE / 4, ONE]);
    assert_eq!(p[1].clip[..3], [ONE / 8, ONE / 2, 3 * ONE / 4]);
    assert_eq!(p[2].clip[..3], [ONE / 8 - 1, ONE / 2 + 1, 3 * ONE / 4]);
    // VTX_10 : 6 bits de fraction ; VTX_XZ, VTX_YZ.
    begin(&mut g, 0);
    g.execute(0x24, &[32 | (0x3FF & -16i32 as u32) << 10 | 64 << 20]);
    g.execute(0x26, &[xy(0.25, 0.5)]);
    g.execute(0x27, &[xy(-0.5, 0.0)]);
    let p = &g.building.polygons[1].vertices;
    assert_eq!(p[0].clip[..3], [ONE / 2, -ONE / 4, ONE]);
    assert_eq!(p[1].clip[..3], [ONE / 4, -ONE / 4, ONE / 2]);
    assert_eq!(p[2].clip[..3], [ONE / 4, -ONE / 2, 0]);
}

#[test]
fn primitives_strips_and_shared_vertices() {
    let mut g = geo();
    g.execute(0x29, &[0x80 | 31 << 16]); // face avant seulement
    // Bande de 5 sommets en zigzag : 3 triangles, tous face avant une fois
    // l'alternance corrigée ; ils partagent leurs sommets (3 + 1 + 1).
    begin(&mut g, 2);
    for (x, y) in [(0.0, 0.5), (0.0, 0.0), (0.25, 0.5), (0.25, 0.0), (0.5, 0.5)] {
        v16(&mut g, x - 0.25, y - 0.25, 0.0);
    }
    assert_eq!(g.building.polygons.len(), 3);
    assert!(g.building.polygons.iter().all(|p| p.front));
    assert_eq!(g.building.vertex_count, 5);
    // Bande de quadrilatères, 6 sommets haut-bas : 2 quadrilatères, 4 + 2.
    let mut g = geo();
    begin(&mut g, 3);
    for (x, y) in [(0.0, 0.0), (0.0, 0.5), (0.25, 0.0), (0.25, 0.5), (0.5, 0.0), (0.5, 0.5)] {
        v16(&mut g, x - 0.25, y - 0.25, 0.0);
    }
    assert_eq!(g.building.polygons.len(), 2);
    let q = &g.building.polygons[0].vertices;
    assert_eq!([q[2].clip[0], q[2].clip[1]], [ONE / 4 - ONE / 4, ONE / 2 - ONE / 4], "ordre 0 1 3 2");
    assert_eq!(g.building.vertex_count, 6);
    assert_eq!(g.ram_count(), 2 | 6 << 16);
    // Triangles et quadrilatères séparés.
    let mut g = geo();
    begin(&mut g, 1);
    for (x, y) in [(0.0, 0.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5), (0.1, 0.1)] {
        v16(&mut g, x, y, 0.0);
    }
    assert_eq!(g.building.polygons.len(), 1, "le 5e sommet attend les 3 suivants");
    assert_eq!(g.building.polygons[0].vertices.len(), 4);
}

#[test]
fn attributes_apply_at_begin_and_hide_back_faces() {
    let mut g = geo();
    g.execute(0x29, &[0x80 | 31 << 16]); // face avant seulement
    begin(&mut g, 0);
    // Sens horaire : face arrière, cachée.
    v16(&mut g, 0.0, 0.0, 0.0);
    v16(&mut g, 0.0, 0.5, 0.0);
    v16(&mut g, 0.5, 0.0, 0.0);
    assert!(g.building.polygons.is_empty());
    g.execute(0x29, &[0x40 | 31 << 16]); // arrière seulement : pas avant BEGIN
    v16(&mut g, 0.0, 0.0, 0.0);
    v16(&mut g, 0.0, 0.5, 0.0);
    v16(&mut g, 0.5, 0.0, 0.0);
    assert!(g.building.polygons.is_empty(), "l'attribut attend BEGIN_VTXS");
    begin(&mut g, 0);
    v16(&mut g, 0.0, 0.0, 0.0);
    v16(&mut g, 0.0, 0.5, 0.0);
    v16(&mut g, 0.5, 0.0, 0.0);
    assert_eq!(g.building.polygons.len(), 1);
    assert!(!g.building.polygons[0].front);
    assert_eq!(g.building.polygons[0].attr, 0x40 | 31 << 16);
    // Segment (sommet répété) : toujours affiché.
    g.execute(0x29, &[31 << 16]);
    begin(&mut g, 0);
    v16(&mut g, 0.0, 0.0, 0.0);
    v16(&mut g, 0.5, 0.5, 0.0);
    v16(&mut g, 0.5, 0.5, 0.0);
    assert_eq!(g.building.polygons.len(), 2);
}

fn tri(g: &mut Geometry, pts: [(f64, f64, f64); 3]) {
    begin(g, 0);
    for (x, y, z) in pts {
        v16(g, x, y, z);
    }
}

#[test]
fn clipping_against_the_view_volume() {
    let mut g = geo();
    // Un sommet au-delà de x = w : remplacé par deux sommets.
    tri(&mut g, [(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    let p = &g.building.polygons[0].vertices;
    assert_eq!(p.len(), 4);
    assert!(p.iter().all(|v| v.clip[0] <= v.clip[3]));
    assert!(p.iter().any(|v| v.clip[0] == ONE), "point sur le plan x = w");
    // Entièrement dehors : rien.
    tri(&mut g, [(2.0, 0.0, 0.0), (3.0, 0.0, 0.0), (2.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons.len(), 1);
    // Coupé par le plan lointain : caché si POLYGON_ATTR bit 12 = 0.
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 2.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons.len(), 1);
    g.execute(0x29, &[0xC0 | 1 << 12 | 31 << 16]);
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 2.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons.len(), 2, "rendu une fois coupé");
    // Le plan proche, lui, découpe toujours.
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, -2.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons.len(), 3);
    // Les couleurs sont interpolées au point de coupe.
    let a = Vertex { clip: [0, 0, 0, ONE], color: [0; 3], ..Vertex::default() };
    let b = Vertex { clip: [3 * ONE, 0, 0, ONE], color: [60; 3], ..Vertex::default() };
    let c = Vertex { clip: [0, ONE / 2, 0, ONE], color: [0; 3], ..Vertex::default() };
    let Clip::Clipped(out) = polygon::clip(&[a, b, c], false) else { panic!() };
    let cut = out.iter().find(|v| v.clip[0] == ONE && v.clip[1] == 0).unwrap();
    assert_eq!(cut.color, [20; 3], "un tiers du chemin");
}

#[test]
fn viewport_maps_clip_space_to_the_screen() {
    let mut g = geo();
    tri(&mut g, [(-1.0, -1.0, 0.0), (0.999, -1.0, 0.0), (0.0, 0.0, 0.0)]);
    let p = &g.building.polygons[0].vertices;
    assert_eq!(p[0].screen, [0, 191], "bas gauche");
    assert_eq!(p[2].screen, [128, 95], "centre (y compté depuis le bas)");
    // Viewport réduit à la moitié gauche.
    g.execute(0x60, &[127 << 16 | 191 << 24]);
    tri(&mut g, [(-1.0, -1.0, 0.0), (0.999, -1.0, 0.0), (0.0, 0.0, 0.0)]);
    assert_eq!(g.building.polygons[1].vertices[2].screen, [64, 95]);
}

#[test]
fn swap_hands_the_list_to_the_renderer() {
    let mut g = geo();
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    g.request_swap(2);
    g.swap();
    assert_eq!(g.rendering.polygons.len(), 1);
    assert!(g.building.polygons.is_empty());
    assert_eq!(g.ram_count(), 0);
    assert_eq!(g.building.swap_flags, 2, "bits pour la liste suivante");
}

#[test]
fn polygon_ram_overflows_at_2048() {
    // Une bande : 1 sommet par triangle, la limite des polygones arrive
    // avant celle des 6144 sommets.
    let mut g = geo();
    begin(&mut g, 2);
    for i in 0..MAX_POLYGONS + 3 {
        let (x, y) = if i % 2 == 0 { (0.0, 0.5) } else { (0.0, 0.0) };
        v16(&mut g, x + (i / 2) as f64 / 4096.0, y, 0.0);
    }
    assert_eq!(g.building.polygons.len(), MAX_POLYGONS);
    assert!(g.building.overflow);
    // Triangles séparés : la limite des 6144 sommets (2048 × 3) arrive
    // en même temps ; un de plus est refusé.
    let mut g = geo();
    for _ in 0..MAX_POLYGONS + 1 {
        tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    }
    assert_eq!(g.building.vertex_count, 6144);
}

#[test]
fn colors_expand_to_six_bits_and_lighting_uses_the_normal() {
    let mut g = geo();
    g.execute(0x20, &[31 | 1 << 5]);
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[0].vertices[0].color, [63, 3, 0]);
    // DIF_AMB bit 15 : la couleur diffuse devient la couleur du sommet.
    g.execute(0x30, &[10 | 1 << 15]);
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[1].vertices[0].color, [21, 0, 0]);
    g.execute(0x30, &[20]); // sans le bit 15 : la couleur ne change pas
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[2].vertices[0].color, [21, 0, 0]);
    // Lumière 0 vers −z, blanche ; matière : diffus blanc, émission rouge 5.
    g.execute(0x10, &[2]);
    g.execute(0x32, &[0x201 << 20]); // z = −511/512
    g.execute(0x33, &[31 | 31 << 5 | 31 << 10]);
    g.execute(0x30, &[31 | 31 << 5 | 31 << 10]);
    g.execute(0x31, &[5 << 16]);
    let lit = |g: &Geometry, attr: u32, normal: u32| {
        let mut g = g.clone();
        g.execute(0x29, &[attr | 31 << 16]);
        begin(&mut g, 0);
        g.execute(0x21, &[normal]);
        tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
        g.building.polygons.last().unwrap().vertices[0].color
    };
    // Normale face à la lumière : émission + diffus × lumière × ≈ 1.
    let c = lit(&g, 0xC1, 0x1FF << 20);
    assert!(c.iter().all(|&x| x >= 55), "fortement éclairé : {c:?}");
    assert!(c[0] > c[1], "émission rouge en plus : {c:?}");
    // Normale à l'opposé : émission seule (5 → 11 sur 6 bits).
    assert_eq!(lit(&g, 0xC1, 0x201 << 20), [11, 0, 0]);
    // Lumière éteinte dans POLYGON_ATTR : émission seule.
    assert_eq!(lit(&g, 0xC0, 0x1FF << 20), [11, 0, 0]);
}

#[test]
fn position_vector_and_box_tests() {
    let mut g = geo();
    g.execute(0x10, &[2]);
    g.execute(0x72, &[0x1FF]); // VEC_TEST x = 511/512
    assert_eq!(g.vec_result, [0x0FF8, 0, 0]);
    g.execute(0x1B, &[f(4.0), f(4.0), f(4.0)]); // échelle : position seule
    g.execute(0x72, &[0x1FF]);
    assert_eq!(g.vec_result[0], 0x0FF8, "VEC_TEST passe par la matrice de vecteur");
    g.execute(0x1A, &[f(2.0), 0, 0, 0, f(1.0), 0, 0, 0, f(1.0)]); // x × 2 aussi au vecteur
    g.execute(0x72, &[0x100]); // x = 0.5 → 1.0 : déborde
    assert_eq!(g.vec_result[0] as u16, 0xF000, "+1.0 se relit F000h");
    // BOX_TEST : pavé dans le volume, puis loin dehors.
    let mut g = geo();
    g.execute(0x70, &[xy(-0.25, -0.25), f(0.0) | f(0.5) << 16, f(0.5) | f(0.5) << 16]);
    assert!(g.box_result);
    g.execute(0x70, &[xy(3.0, 3.0), f(0.0) | f(0.5) << 16, f(0.5) | f(0.5) << 16]);
    assert!(!g.box_result);
    // Barre qui traverse le volume, coins tous dehors : ses faces entrent.
    g.execute(0x70, &[xy(-3.0, -0.1), f(-0.1) | f(6.0) << 16, f(0.2) | f(0.2) << 16]);
    assert!(g.box_result, "faces, pas seulement les coins");
    // Volume entièrement dans le pavé : les faces seules comptent.
    g.execute(0x70, &[xy(-4.0, -4.0), f(-4.0) | f(7.9) << 16, f(7.9) | f(7.9) << 16]);
    assert!(!g.box_result);
}

// ---- Rendu (étape 3D-3) ----

use super::polygon::{PolyRam, Polygon};
use super::render::{expand_depth, Pixel, RenderParams, Renderer, WIDTH};
use super::texture::TexMem;

/// Sommet déjà projeté : position à l'écran, profondeur z/w, couleur.
fn sv(x: i32, y: i32, z: f64, rgb: [u8; 3]) -> Vertex {
    Vertex { clip: [0, 0, (z * ONE as f64) as i32, ONE], color: rgb, tex: [0; 2], screen: [x, y] }
}

fn poly(vs: Vec<Vertex>, alpha: u32) -> Polygon {
    Polygon { vertices: vs, attr: 0xC0 | alpha << 16, teximage: 0, pltt_base: 0, front: true }
}

fn render(polys: Vec<Polygon>, params: RenderParams) -> Renderer {
    let mut list = PolyRam::default();
    for p in polys {
        list.store(p, 3);
    }
    let mut r = Renderer::default();
    r.render(&list, &TexMem::default(), &params);
    r
}

fn px(r: &Renderer, x: usize, y: usize) -> Pixel {
    r.color[y * WIDTH + x]
}

const RED: [u8; 3] = [63, 0, 0];
const BLUE: [u8; 3] = [0, 0, 63];

#[test]
fn rear_plane_clears_color_alpha_and_depth() {
    assert_eq!(expand_depth(0), 0);
    assert_eq!(expand_depth(1), 0x200);
    assert_eq!(expand_depth(0x7FFF), 0xFF_FFFF, "7FFFh : le plus loin");
    let r = render(vec![], RenderParams { clear_color: 31 | 4 << 5 | 9 << 10 | 16 << 16, clear_depth: 0x1234, ..RenderParams::default() });
    assert_eq!(px(&r, 10, 10), Pixel { rgb: [63, 9, 19], alpha: 16 });
    assert_eq!(r.depth[0], expand_depth(0x1234));
}

#[test]
fn opaque_triangle_follows_the_top_left_rule() {
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    let r = render(vec![poly(vec![sv(10, 10, 0.0, RED), sv(20, 10, 0.0, RED), sv(10, 20, 0.0, RED)], 31)], p);
    for (x, y) in [(10, 10), (19, 10), (10, 19), (12, 12), (15, 14)] {
        assert_eq!(px(&r, x, y), Pixel { rgb: RED, alpha: 31 }, "({x}, {y}) dedans");
    }
    // Dernière colonne et dernière ligne exclues.
    for (x, y) in [(9, 10), (20, 10), (10, 20), (15, 15), (100, 100)] {
        assert_eq!(px(&r, x, y).alpha, 0, "({x}, {y}) dehors");
    }
    // Deux carrés voisins (x 0..10 et 10..20) : la colonne 10 n'est dessinée
    // qu'une fois, par le second.
    let sq = |x0: i32, rgb| poly(vec![sv(x0, 30, 0.0, rgb), sv(x0 + 10, 30, 0.0, rgb), sv(x0 + 10, 40, 0.0, rgb), sv(x0, 40, 0.0, rgb)], 31);
    let r = render(vec![sq(10, BLUE), sq(0, RED)], p);
    assert_eq!(px(&r, 9, 35).rgb, RED);
    assert_eq!(px(&r, 10, 35).rgb, BLUE, "pas recouverte par le carré de gauche");
    assert_eq!(px(&r, 5, 39).rgb, RED);
    assert_eq!(px(&r, 5, 40).alpha, 0, "ligne du bas exclue");
    // Segment et point : au moins un pixel.
    let r = render(vec![poly(vec![sv(50, 50, 0.0, RED), sv(60, 50, 0.0, RED), sv(60, 50, 0.0, RED)], 31)], p);
    assert_eq!((px(&r, 50, 50).alpha, px(&r, 59, 50).alpha), (31, 31), "segment horizontal");
    let r = render(vec![poly(vec![sv(70, 70, 0.0, RED); 3], 31)], p);
    assert_eq!(px(&r, 70, 70).alpha, 31, "point");
}

#[test]
fn nearer_polygon_wins_whatever_the_order() {
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    let near = poly(vec![sv(0, 0, -0.5, RED), sv(30, 0, -0.5, RED), sv(0, 30, -0.5, RED)], 31);
    let far = poly(vec![sv(0, 0, 0.5, BLUE), sv(30, 0, 0.5, BLUE), sv(0, 30, 0.5, BLUE)], 31);
    for order in [vec![near.clone(), far.clone()], vec![far.clone(), near.clone()]] {
        let r = render(order, p);
        assert_eq!(px(&r, 5, 5).rgb, RED);
    }
    // Plan arrière plus proche que le polygone : il le cache.
    let r = render(vec![far.clone()], RenderParams { clear_depth: 0x1000, ..p });
    assert_eq!(px(&r, 5, 5).alpha, 0);
    // Même profondeur : « inférieur » refuse le second, « égal » l'accepte.
    let r = render(vec![near.clone(), poly(near.vertices.iter().map(|v| Vertex { color: BLUE, ..*v }).collect(), 31)], p);
    assert_eq!(px(&r, 5, 5).rgb, RED);
    let mut eq = poly(near.vertices.iter().map(|v| Vertex { color: BLUE, ..*v }).collect(), 31);
    eq.attr |= 1 << 14;
    let r = render(vec![near.clone(), eq.clone()], p);
    assert_eq!(px(&r, 5, 5).rgb, BLUE);
    // Tolérance de ±200h, pas plus.
    let shifted = |dz: i32| {
        let mut q = eq.clone();
        q.vertices.iter_mut().for_each(|v| v.clip[2] += dz);
        q
    };
    // z/w × 800000h : 1 unité de z (1/4096) ≈ 800h de profondeur.
    let r = render(vec![near.clone(), shifted(1)], p);
    assert_eq!(px(&r, 5, 5).rgb, RED, "800h d'écart : hors tolérance");
}

#[test]
fn colors_are_interpolated_across_the_polygon() {
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    let r = render(vec![poly(vec![sv(0, 0, 0.0, [0; 3]), sv(60, 0, 0.0, [60, 0, 0]), sv(0, 10, 0.0, [0; 3])], 31)], p);
    assert_eq!(px(&r, 30, 0).rgb, [30, 0, 0], "milieu du bord du haut");
    assert_eq!(px(&r, 15, 0).rgb, [15, 0, 0]);
}

#[test]
fn wireframe_draws_only_the_edges_and_translucent_waits() {
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    let tri = vec![sv(10, 10, 0.0, RED), sv(40, 10, 0.0, RED), sv(10, 40, 0.0, RED)];
    let r = render(vec![poly(tri.clone(), 0)], p);
    assert_eq!(px(&r, 25, 10), Pixel { rgb: RED, alpha: 31 }, "bord du haut, alpha 31");
    assert_eq!(px(&r, 10, 25).alpha, 31, "bord gauche");
    assert_eq!(px(&r, 24, 25).alpha, 31, "bord oblique (dernier pixel de la ligne 25)");
    assert_eq!(px(&r, 15, 15).alpha, 0, "intérieur vide");
    // Translucide (3D-4) : dessiné après les opaques, avec son alpha.
    let r = render(vec![poly(tri, 16)], p);
    assert_eq!(px(&r, 15, 15), Pixel { rgb: RED, alpha: 16 });
    assert_eq!(r.translucent, 1);
}

#[test]
fn w_buffer_uses_w_as_depth() {
    // Même z/w, w différents : seul le tampon W les départage.
    let mk = |w: i32, rgb| {
        let v = |x, y| Vertex { clip: [0, 0, 0, w], color: rgb, tex: [0; 2], screen: [x, y] };
        poly(vec![v(0, 0), v(30, 0), v(0, 30)], 31)
    };
    let p = RenderParams { clear_depth: 0x7FFF, w_buffer: true, ..RenderParams::default() };
    let r = render(vec![mk(2 * ONE, BLUE), mk(ONE, RED)], p);
    assert_eq!(px(&r, 5, 5).rgb, RED, "w plus petit = plus proche");
    assert_eq!(r.depth[5 * WIDTH + 5], ONE as u32);
    let r = render(vec![mk(2 * ONE, BLUE), mk(ONE, RED)], RenderParams { w_buffer: false, ..p });
    assert_eq!(px(&r, 5, 5).rgb, BLUE, "tampon Z : égalité, le premier reste");
}

// ---- Textures (étape 3D-4) ----

use super::texture::{TexParams, Texel};

/// TEXIMAGE_PARAM : adresse/8, taille 8 << n, format, options.
fn teximage(addr: u32, ws: u32, hs: u32, format: u32, extra: u32) -> u32 {
    (addr / 8) | ws << 20 | hs << 23 | format << 26 | extra
}

fn put16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn texture_params_decode_teximage_and_pltt_base() {
    let t = TexParams::new(teximage(0x1238, 1, 3, 3, 1 << 16 | 1 << 19 | 1 << 29), 5);
    assert_eq!((t.addr, t.width, t.height, t.format), (0x1238, 16, 64, 3));
    assert_eq!((t.repeat, t.flip, t.color0_transparent), ([true, false], [false, true], true));
    assert_eq!(t.pal_base, 5 * 16, "pas de 16 octets");
    assert_eq!(TexParams::new(teximage(0, 0, 0, 2, 0), 5).pal_base, 5 * 8, "4 couleurs : pas de 8");
}

#[test]
fn texture_formats_decode_their_texels() {
    let mut m = TexMem::default();
    for i in 0..8u16 {
        put16(&mut m.pal, 0x100 + 2 * i as usize, 0x0400 * i + i); // couleur repérable
    }
    let col = |i: u16| [(i & 31) as u8, ((0x0400 * i + i) >> 5 & 31) as u8, ((0x0400 * i + i) >> 10 & 31) as u8];
    let pb = 0x100 / 16;
    // A3I5 : index 3, alpha 7 → 31 ; alpha 2 → 9.
    m.tex[0] = 0b111_00011;
    m.tex[1] = 0b010_00001;
    let t = TexParams::new(teximage(0, 0, 0, 1, 0), pb);
    assert_eq!(t.sample(&m, 0, 0), Texel { rgb: col(3), alpha: 31 });
    assert_eq!(t.sample(&m, 1, 0).alpha, 2 * 4 + 1);
    // 4 couleurs : 2 bits par texel, le premier en bits de poids faible.
    m.tex[0x10] = 0b11_10_01_00;
    let t = TexParams::new(teximage(0x10, 0, 0, 2, 0), 0x100 / 8);
    assert_eq!([0, 1, 2, 3].map(|s| t.sample(&m, s, 0).rgb), [col(0), col(1), col(2), col(3)]);
    let t = TexParams::new(teximage(0x10, 0, 0, 2, 1 << 29), 0x100 / 8);
    assert_eq!(t.sample(&m, 0, 0).alpha, 0, "couleur 0 transparente");
    assert_eq!(t.sample(&m, 1, 0).alpha, 31);
    // 16 couleurs : 4 bits par texel.
    m.tex[0x20] = 0x52;
    let t = TexParams::new(teximage(0x20, 0, 0, 3, 0), pb);
    assert_eq!([t.sample(&m, 0, 0).rgb, t.sample(&m, 1, 0).rgb], [col(2), col(5)]);
    // 256 couleurs, ligne suivante (largeur 8).
    m.tex[0x30 + 8] = 6;
    let t = TexParams::new(teximage(0x30, 0, 0, 4, 0), pb);
    assert_eq!(t.sample(&m, 0, 1).rgb, col(6));
    // A5I3 : index 3 bits, alpha 5 bits tel quel.
    m.tex[0x40] = 13 << 3 | 4;
    let t = TexParams::new(teximage(0x40, 0, 0, 6, 0), pb);
    assert_eq!(t.sample(&m, 0, 0), Texel { rgb: col(4), alpha: 13 });
    // Couleur directe : bit 15 = alpha.
    put16(&mut m.tex, 0x50, 0x7C1F);
    put16(&mut m.tex, 0x52, 0x801F);
    let t = TexParams::new(teximage(0x50, 0, 0, 7, 0), 0);
    assert_eq!(t.sample(&m, 0, 0).alpha, 0);
    assert_eq!(t.sample(&m, 1, 0), Texel { rgb: [31, 0, 0], alpha: 31 });
}

#[test]
fn compressed_4x4_texture_uses_slot1_palette_info() {
    let mut m = TexMem::default();
    // Couleurs 0–3 de la palette du bloc : rouge 8, rouge 16, vert 31, bleu 31.
    for (i, c) in [8u16, 16, 31 << 5, 31 << 10].into_iter().enumerate() {
        put16(&mut m.pal, 0x40 + 2 * i, c);
    }
    // Bloc 1 (texels 4..7 de la 1re rangée) d'une texture 8×8 en slot 0 :
    // rangée 0 = texels 0, 1, 2, 3.
    m.tex[4] = 0b11_10_01_00;
    let sample = |m: &TexMem, mode: u16, s: i32| {
        let mut m = m.clone();
        put16(&mut m.tex, 0x2_0000 + 4 / 2, (mode << 14) | (0x40 / 4)); // slot 1 : adresse / 2
        TexParams::new(teximage(0, 0, 0, 5, 0), 0).sample(&m, s, 0)
    };
    let red = |r: u8| Texel { rgb: [r, 0, 0], alpha: 31 };
    assert_eq!(sample(&m, 0, 4), red(8));
    assert_eq!(sample(&m, 0, 5), red(16));
    assert_eq!(sample(&m, 0, 6).rgb, [0, 31, 0]);
    assert_eq!(sample(&m, 0, 7).alpha, 0, "mode 0, texel 3 : transparent");
    assert_eq!(sample(&m, 1, 6), red(12), "mode 1 : (c0 + c1) / 2");
    assert_eq!(sample(&m, 1, 7).alpha, 0);
    assert_eq!(sample(&m, 2, 7).rgb, [0, 0, 31], "mode 2 : couleur 3");
    assert_eq!(sample(&m, 3, 6), red(11), "mode 3 : (5 c0 + 3 c1) / 8");
    assert_eq!(sample(&m, 3, 7), red(13), "(3 c0 + 5 c1) / 8");
    // Bloc en slot 2 : infos dans la moitié haute du slot 1.
    let mut m2 = m.clone();
    m2.tex[0x4_0000] = 0b01;
    put16(&mut m2.tex, 0x2_0000 + 0x1_0000, 0x40 / 4);
    assert_eq!(TexParams::new(teximage(0x4_0000, 0, 0, 5, 0), 0).sample(&m2, 0, 0), red(16));
}

#[test]
fn texture_coordinates_clamp_repeat_and_flip() {
    let mut m = TexMem::default();
    for i in 0..8u8 {
        m.tex[i as usize] = i; // 256 couleurs, ligne 0 : index = s
        put16(&mut m.pal, 2 * i as usize, i as u16);
    }
    let r = |extra: u32, s: i32| TexParams::new(teximage(0, 0, 0, 4, extra), 0).sample(&m, s, 0).rgb[0];
    assert_eq!([r(0, -3), r(0, 3), r(0, 20)], [0, 3, 7], "bornage");
    assert_eq!([r(1 << 16, 9), r(1 << 16, -1)], [1, 7], "répétition");
    assert_eq!([r(1 << 16 | 1 << 18, 9), r(1 << 16 | 1 << 18, 17)], [6, 1], "miroir une fois sur deux");
}

#[test]
fn texture_memory_follows_vram_slots() {
    use crate::nds::vram::Vram;
    let mut v = Vram::default();
    v.banks[0][5] = 0xAA; // A : texture, slot 1
    v.banks[4][7] = 0xBB; // E : palettes, slots 0–3
    v.banks[5][9] = 0xCC; // F : palettes, OFS 2 → slot 4
    v.set_cnt(0, 0x80 | 3 | 1 << 3);
    v.set_cnt(4, 0x80 | 3);
    v.set_cnt(5, 0x80 | 3 | 2 << 3);
    let m = TexMem::from_vram(&v);
    assert_eq!(m.tex[0x2_0000 + 5], 0xAA);
    assert_eq!(m.pal[7], 0xBB);
    assert_eq!(m.pal[4 * 0x4000 + 9], 0xCC);
}

/// Texture 8×8 en 256 couleurs, unie, d'index 1 ; palette[1] = `color`.
fn solid_texture(color: u16) -> TexMem {
    let mut m = TexMem::default();
    m.tex[..64].fill(1);
    put16(&mut m.pal, 2, color);
    m
}

fn textured(alpha: u32, mode: u32, fmt_extra: u32, rgb: [u8; 3]) -> Polygon {
    let mut p = poly(vec![sv(0, 0, 0.0, rgb), sv(30, 0, 0.0, rgb), sv(0, 30, 0.0, rgb)], alpha);
    p.attr |= mode << 4;
    p.teximage = teximage(0, 0, 0, 4, fmt_extra);
    p
}

fn render_tex(polys: Vec<Polygon>, mem: &TexMem, params: RenderParams) -> Renderer {
    let mut list = PolyRam::default();
    for p in polys {
        list.store(p, 3);
    }
    let mut r = Renderer::default();
    r.render(&list, mem, &params);
    r
}

const TEX_ON: RenderParams = RenderParams {
    clear_color: 0,
    clear_depth: 0x7FFF,
    w_buffer: false,
    manual_sort: false,
    disp3dcnt: 1,
    alpha_ref: 0,
    toon: [0; 32],
};

#[test]
fn texture_blending_modes_follow_gbatek() {
    let mem = solid_texture(20 | 10 << 5 | 31 << 10); // texel 6 bits : 41, 21, 63
    let px5 = |p: Polygon, params: RenderParams| render_tex(vec![p], &mem, params).color[5 * WIDTH + 5];
    // Modulation : ((t + 1)(v + 1) − 1) / 64 ; vertex blanc → texel.
    assert_eq!(px5(textured(31, 0, 0, [63; 3]), TEX_ON).rgb, [41, 21, 63]);
    assert_eq!(px5(textured(31, 0, 0, [31, 0, 63]), TEX_ON).rgb, [20, 0, 63] /* (42 × 32 − 1) / 64 */);
    // Textures désactivées (DISP3DCNT bit 0) : couleur du sommet.
    assert_eq!(px5(textured(31, 0, 0, [7, 8, 9]), RenderParams { disp3dcnt: 0, ..TEX_ON }).rgb, [7, 8, 9]);
    // Décalque d'un texel opaque : le texel ; d'un texel transparent (couleur
    // 0 transparente, index 1 → on passe par l'index 0) : le sommet.
    assert_eq!(px5(textured(31, 1, 0, [7, 8, 9]), TEX_ON).rgb, [41, 21, 63]);
    let mut clear = solid_texture(0x7FFF);
    clear.tex[..64].fill(0);
    let p = textured(31, 1, 1 << 29, [7, 8, 9]);
    assert_eq!(render_tex(vec![p], &clear, TEX_ON).color[5 * WIDTH + 5].rgb, [7, 8, 9]);
    // Toon : R du sommet indexe la table ; highlight y ajoute la valeur.
    let mut toon = [0u16; 32];
    toon[10] = 5 | 6 << 5 | 7 << 10; // 6 bits : 11, 13, 15
    let white = solid_texture(0x7FFF);
    let p = |mode| textured(31, mode, 0, [20, 0, 0]); // R = 20 → index 10
    let r = render_tex(vec![p(2)], &white, RenderParams { toon, ..TEX_ON });
    assert_eq!(r.color[5 * WIDTH + 5].rgb, [11, 13, 15], "toon × texel blanc");
    let r = render_tex(vec![p(2)], &white, RenderParams { toon, disp3dcnt: 1 | 2, ..TEX_ON });
    assert_eq!(r.color[5 * WIDTH + 5].rgb, [22, 26, 30], "highlight : + valeur de la table");
}

#[test]
fn alpha_test_and_transparent_texels() {
    // A3I5 d'alpha 2 → 9 ; polygone opaque : alpha final = 9 (modulation).
    let mut m = TexMem::default();
    m.tex[..64].fill(0b010_00001);
    put16(&mut m.pal, 2, 0x7FFF);
    let mut p = textured(31, 0, 0, [63; 3]);
    p.teximage = teximage(0, 0, 0, 1, 0);
    let at = |params| render_tex(vec![p.clone()], &m, params).color[5 * WIDTH + 5];
    assert_eq!(at(TEX_ON).alpha, 9);
    assert_eq!(at(RenderParams { disp3dcnt: 1 | 4, alpha_ref: 9, ..TEX_ON }).alpha, 0, "9 n'est pas > 9");
    assert_eq!(at(RenderParams { disp3dcnt: 1 | 4, alpha_ref: 8, ..TEX_ON }).alpha, 9);
    // Texel d'alpha 0 : rien n'est écrit, pas même la profondeur.
    m.tex[..64].fill(1);
    let r = render_tex(vec![p.clone()], &m, TEX_ON);
    assert_eq!(r.color[5 * WIDTH + 5].alpha, 0);
    assert_eq!(r.depth[5 * WIDTH + 5], expand_depth(0x7FFF));
    // Sur un plan arrière opaque, mélange coupé : le texel d'alpha 0 ne
    // remplace pas le pixel (il n'est pas dessiné du tout).
    let r = render_tex(vec![p.clone()], &m, RenderParams { clear_color: 31 | 31 << 16, ..TEX_ON });
    assert_eq!(r.color[5 * WIDTH + 5], Pixel { rgb: [63, 0, 0], alpha: 31 });
}

#[test]
fn a3i5_and_a5i3_textures_make_a_polygon_translucent() {
    // Polygone A3I5 (alpha de texel 3 → 13) devant un opaque rouge, envoyé
    // avant lui. Translucide, il est dessiné après l'opaque et le recouvre ;
    // dessiné en premier, il serait recouvert (il n'écrit pas la profondeur).
    let mut m = TexMem::default();
    m.tex[..64].fill(0b011_00001);
    put16(&mut m.pal, 2, 31 << 10);
    let mut glass = poly(vec![sv(0, 0, -0.5, [63; 3]), sv(30, 0, -0.5, [63; 3]), sv(0, 30, -0.5, [63; 3])], 31);
    glass.teximage = teximage(0, 0, 0, 1, 0);
    let back = poly(vec![sv(0, 0, 0.5, RED), sv(30, 0, 0.5, RED), sv(0, 30, 0.5, RED)], 31);
    let r = render_tex(vec![glass, back], &m, TEX_ON);
    assert_eq!(r.color[5 * WIDTH + 5], Pixel { rgb: [0, 0, 63], alpha: 13 });
    assert_eq!(r.translucent, 1);
}

#[test]
fn translucent_polygons_blend_after_the_opaque_ones() {
    let back = |rgb| poly(vec![sv(0, 0, 0.5, rgb), sv(30, 0, 0.5, rgb), sv(0, 30, 0.5, rgb)], 31);
    let glass = |alpha, attr: u32| {
        let mut p = poly(vec![sv(0, 0, -0.5, BLUE), sv(30, 0, -0.5, BLUE), sv(0, 30, -0.5, BLUE)], alpha);
        p.attr |= attr;
        p
    };
    let blend_on = RenderParams { disp3dcnt: 8, ..TEX_ON };
    // Translucide envoyé avant l'opaque : dessiné quand même après.
    let r = render_tex(vec![glass(15, 0), back(RED)], &TexMem::default(), blend_on);
    let p = r.color[5 * WIDTH + 5];
    // (poly × (A + 1) + fb × (31 − A)) / 32, alpha = max.
    assert_eq!(p.rgb, [31, 0, 31], "63 × 16 / 32 de chaque côté");
    assert_eq!(p.alpha, 31);
    // La profondeur n'est mise à jour par un translucide que si bit 11.
    assert_eq!(r.depth[5 * WIDTH + 5], 0xC0_0000, "celle de l'opaque (z/w = 0.5)");
    let r = render_tex(vec![back(RED), glass(15, 1 << 11)], &TexMem::default(), blend_on);
    assert_eq!(r.depth[5 * WIDTH + 5], 0x40_0000);
    // Mélange désactivé : le translucide remplace (alpha compris).
    let r = render_tex(vec![back(RED), glass(15, 0)], &TexMem::default(), RenderParams { disp3dcnt: 0, ..TEX_ON });
    assert_eq!(r.color[5 * WIDTH + 5], Pixel { rgb: BLUE, alpha: 15 });
    // Sur le plan arrière d'alpha 0 : pas de mélange.
    let r = render_tex(vec![glass(15, 0)], &TexMem::default(), blend_on);
    assert_eq!(r.color[5 * WIDTH + 5], Pixel { rgb: BLUE, alpha: 15 });
}

#[test]
fn translucent_auto_sort_by_y_unless_manual() {
    // Deux translucides superposés : le tri par y du haut décide de l'ordre.
    let at = |y0: i32, rgb| poly(vec![sv(0, y0, 0.0, rgb), sv(40, y0, 0.0, rgb), sv(0, 40, 0.0, rgb)], 16);
    let list = vec![at(0, RED), at(5, BLUE)];
    let p = RenderParams { disp3dcnt: 0, ..TEX_ON };
    let r = render_tex(list.clone(), &TexMem::default(), p);
    // Les translucides n'écrivent pas la profondeur (bit 11 à 0) : le
    // dernier dessiné recouvre l'autre.
    assert_eq!(r.color[10 * WIDTH + 5].rgb, BLUE, "y = 0 trié avant y = 5");
    let r = render_tex(vec![list[1].clone(), list[0].clone()], &TexMem::default(), p);
    assert_eq!(r.color[10 * WIDTH + 5].rgb, BLUE, "tri automatique, quel que soit l'ordre reçu");
    let r = render_tex(vec![list[1].clone(), list[0].clone()], &TexMem::default(), RenderParams { manual_sort: true, ..p });
    assert_eq!(r.color[10 * WIDTH + 5].rgb, RED, "tri manuel : ordre reçu");
}

#[test]
fn textures_follow_perspective() {
    // Bord du haut de x = 0 (w = 1) à x = 60 (w = 3), S de 0 à 50 texels :
    // au milieu de l'écran, la perspective donne 12,5 texels (et non 25).
    // (Pas pile sur une frontière de texel : l'arrondi entier n'y joue pas.)
    let mut m = TexMem::default();
    for s in 0..64usize {
        m.tex[s] = s as u8; // 256 couleurs, largeur 64 : index = s
    }
    for i in 0..64u16 {
        put16(&mut m.pal, 2 * i as usize, i);
    }
    let v = |x: i32, y: i32, w: i32, s: i16| Vertex { clip: [0, 0, 0, w * ONE], color: [63; 3], tex: [s << 4, 0], screen: [x, y] };
    let mut p = poly(vec![v(0, 0, 1, 0), v(60, 0, 3, 50), v(0, 10, 1, 0)], 31);
    p.teximage = teximage(0, 3, 0, 4, 0);
    let r = render_tex(vec![p], &m, TEX_ON);
    let s_mid = r.color[30].rgb[0];
    assert_eq!(s_mid, to6_test(12), "1/w interpolé : s = 12,5 au milieu");
}

/// 5 bits → 6 bits du mélange (X × 2 + 1 si X > 0), avec un texel
/// modulé par du blanc.
fn to6_test(x: u8) -> u8 {
    if x > 0 { x * 2 + 1 } else { 0 }
}

#[test]
fn texture_coordinate_transforms() {
    let mut g = geo();
    g.execute(0x10, &[3]); // matrice de texture
    g.execute(0x1B, &[f(2.0), f(3.0), f(1.0)]); // S × 2, T × 3
    // Mode 0 : TEXCOORD tel quel.
    g.execute(0x2A, &[0]);
    g.execute(0x22, &[16 | 32 << 16]);
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[0].vertices[0].tex, [16, 32]);
    // Mode 1 : (S, T, 1/16, 1/16) × matrice.
    g.execute(0x2A, &[1 << 30]);
    g.execute(0x22, &[16 | 32 << 16]);
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[1].vertices[0].tex, [32, 96]);
    // Mode 3 : (x, y, z) du sommet × matrice + S, T du dernier TEXCOORD.
    g.execute(0x2A, &[3 << 30]);
    g.execute(0x22, &[16 | 32 << 16]);
    tri(&mut g, [(0.5, 0.25, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    // x = 0.5 × 2 = 1.0 → 16 (4 bits de fraction) ; y = 0.25 × 3 → 12.
    assert_eq!(g.building.polygons[2].vertices[0].tex, [16 + 16, 32 + 12]);
    // Mode 2 : la normale × matrice + S, T, calculé au NORMAL.
    g.execute(0x2A, &[2 << 30]);
    g.execute(0x22, &[16 | 32 << 16]);
    g.execute(0x21, &[0x100]); // normale x = 0.5
    tri(&mut g, [(0.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.0, 0.5, 0.0)]);
    assert_eq!(g.building.polygons[3].vertices[0].tex, [16 + 16, 32]);
}

#[test]
fn span_interpolation_details() {
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    // Couleur constante sous un w qui n'est pas une puissance de 2 : la
    // reconstruction (c/w) × w ne doit pas perdre une unité.
    let v = |x: i32, y: i32| Vertex { clip: [0, 0, 0, 3 * ONE], color: [30, 45, 61], tex: [0; 2], screen: [x, y] };
    let r = render(vec![poly(vec![v(0, 0), v(40, 0), v(0, 40)], 31)], p);
    for x in 0..30 {
        assert_eq!(px(&r, x, 5).rgb, [30, 45, 61], "x = {x}");
    }
    // Profondeur qui varie le long de la ligne : z/w de −0,5 à +0,5.
    let vz = |x: i32, z: f64| sv(x, 50, z, RED);
    let mut q = poly(vec![vz(0, -0.5), vz(64, 0.5), sv(0, 60, -0.5, RED)], 31);
    q.vertices[2].screen = [0, 60];
    let r = render(vec![q], p);
    let d = |x: usize| r.depth[50 * WIDTH + x];
    assert_eq!(d(0), 0x40_0000);
    assert!((d(32) as i64 - 0x80_0000).abs() <= 1, "milieu : {:X}", d(32));
    assert!(d(16) < d(32) && d(32) < d(48));
}

#[test]
fn negative_texture_coordinates_round_down() {
    // Texture 8 texels (256 couleurs, index = s), répétée. S va de −8 à 0
    // sur 16 pixels : s = −8 + x/2. À x = 3, s = −6,5 → texel −7 → 1
    // (une troncature donnerait −6 → 2).
    let mut m = TexMem::default();
    for s in 0..8usize {
        m.tex[s] = s as u8;
        put16(&mut m.pal, 2 * s, (s as u16) << 1);
    }
    let v = |x: i32, y: i32, s: i16| Vertex { clip: [0, 0, 0, ONE], color: [63; 3], tex: [s << 4, 0], screen: [x, y] };
    let mut q = poly(vec![v(0, 0, -8), v(16, 0, 0), v(0, 4, -8)], 31);
    q.teximage = teximage(0, 0, 0, 4, 1 << 16);
    let r = render_tex(vec![q], &m, TEX_ON);
    let texel = |x: usize| (r.color[x].rgb[0] - 1) / 4; // 2s → 6 bits : 4s + 1
    assert_eq!(texel(3), 1);
    assert_eq!(texel(4), 2, "s = −6 exactement");
}

#[test]
fn constant_colors_survive_any_w() {
    // Couleur constante, w quelconque : chaque pixel doit rendre la couleur
    // exacte (balayage de w et des 64 niveaux).
    let p = RenderParams { clear_depth: 0x7FFF, ..RenderParams::default() };
    for w in (1..400).map(|k| k * 97 + 13) {
        let polys: Vec<Polygon> = (0..64u8)
            .map(|c| {
                let y = c as i32 * 3;
                let v = |x: i32, y: i32| Vertex { clip: [0, 0, 0, w], color: [c; 3], tex: [0; 2], screen: [x, y] };
                poly(vec![v(0, y), v(250, y), v(250, y + 2), v(0, y + 2)], 31)
            })
            .collect();
        let r = render(polys, p);
        for c in 0..64usize {
            for x in (0..249).step_by(7) {
                assert_eq!(px(&r, x, c * 3).rgb, [c as u8; 3], "w = {w}, couleur {c}, x = {x}");
            }
        }
    }
}
