//! Mesure du rendu 3D seul, sur une scène proche de l'écran titre de
//! Pokémon N2 : ~1 600 polygones texturés (16 couleurs), qui couvrent
//! l'écran plusieurs fois. `cargo run --release --example bench3d`
use avxds_core::nds::gpu3d::polygon::{PolyRam, Polygon, Vertex};
use avxds_core::nds::gpu3d::render::{RenderParams, Renderer};
use avxds_core::nds::gpu3d::texture::TexMem;
use std::time::Instant;

/// Générateur pseudo-aléatoire minimal (scène reproductible).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: i32) -> i32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as i32
    }
}

fn scene(n: usize, size: i32) -> PolyRam {
    let mut rng = Lcg(42);
    let mut list = PolyRam::default();
    for _ in 0..n {
        let (x, y) = (rng.next(256 - size), rng.next(192 - size));
        let w = 4096 + rng.next(8192);
        let z = rng.next(4096) - 2048;
        let v = |dx: i32, dy: i32, s: i16, t: i16| Vertex {
            clip: [0, 0, z, w],
            color: [63, 50, 40],
            tex: [s << 4, t << 4],
            screen: [x + dx, y + dy],
        };
        let p = Polygon {
            vertices: vec![v(0, 0, 0, 0), v(size, 0, 31, 0), v(size, size, 31, 31), v(0, size, 0, 31)],
            attr: 0xC0 | 31 << 16,
            teximage: 3 << 26 | 2 << 20 | 2 << 23 | 1 << 16 | 1 << 17, // 16 couleurs, 32×32
            pltt_base: 0,
            front: true,
        };
        list.store(p, 4);
    }
    list
}

fn main() {
    let mut mem = TexMem::default();
    for (i, b) in mem.tex.iter_mut().take(32 * 32 / 2).enumerate() {
        *b = (i * 7) as u8;
    }
    for (i, b) in mem.pal.iter_mut().take(32).enumerate() {
        *b = (i * 13) as u8;
    }
    let params = RenderParams { clear_depth: 0x7FFF, disp3dcnt: 1, ..RenderParams::default() };
    // `bench3d profil` : trois images seulement (pour valgrind --tool=callgrind).
    if std::env::args().nth(1).as_deref() == Some("profil") {
        let list = scene(1600, 24);
        let mut r = Renderer::default();
        for _ in 0..3 {
            r.render(&list, &mem, &params);
        }
        return;
    }
    for (n, size, label) in [(1600, 24, "écran titre (1 600 quads de 24×24)"), (200, 80, "200 grands quads de 80×80")] {
        let list = scene(n, size);
        let pixels: i64 = n as i64 * (size as i64 * size as i64);
        let mut r = Renderer::default();
        r.render(&list, &mem, &params); // échauffement
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed().as_secs_f64() < 2.0 {
            r.render(&list, &mem, &params);
            frames += 1;
        }
        let per = start.elapsed().as_secs_f64() / frames as f64;
        println!(
            "{label} : {:.2} ms par image ({:.0} images/s), {:.1} Mpixels/s",
            per * 1e3,
            1.0 / per,
            pixels as f64 / per / 1e6
        );
    }
    let start = Instant::now();
    let mut k = 0;
    while start.elapsed().as_secs_f64() < 1.0 {
        std::hint::black_box(TexMem::default());
        k += 1;
    }
    println!("copie de la mémoire des textures (608 Ko) : {:.3} ms", 1e3 / k as f64);
}
