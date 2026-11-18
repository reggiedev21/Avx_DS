//! File d'attente audio entre l'émulateur et la carte son.
//!
//! L'émulateur pousse des échantillons stéréo à la fréquence de la DS
//! (≈ 32 728 Hz) ; le rappel de la carte son en tire à la sienne (souvent
//! 48 000 Hz). Entre les deux :
//!
//! * **rééchantillonnage linéaire** : la sortie de la DS est déjà en
//!   marches de 10 bits, une interpolation plus fine n'apporterait rien ;
//! * **réserve** d'environ 60 ms : la lecture ne commence (ou ne reprend,
//!   après une famine) qu'une fois la réserve pleine ;
//! * **régulation douce** : l'image cadence l'émulation, donc les deux
//!   horloges dérivent un peu. Le pas de lecture est corrigé d'au plus
//!   ±0,5 % selon l'écart à la réserve visée, ce qui ne s'entend pas.
//!
//! Aucune dépendance : le frontend partage une `AudioQueue` derrière un
//! `Mutex` et l'appelle depuis le rappel de sa bibliothèque audio.

use std::collections::VecDeque;

/// Réserve visée, en secondes.
pub const TARGET_SECONDS: f64 = 0.060;
/// Au-delà (accéléré, rappel bloqué…), on jette les plus anciens.
pub const MAX_SECONDS: f64 = 0.250;
/// Correction maximale du pas de lecture.
pub const MAX_CORRECTION: f64 = 0.005;

pub struct AudioQueue {
    /// Échantillons source (gauche, droite) pas encore consommés ; `src[0]`
    /// est l'échantillon courant, `src[1]` le suivant.
    src: VecDeque<[i16; 2]>,
    /// Position entre `src[0]` et `src[1]`, dans [0, 1).
    frac: f64,
    /// Pas de lecture nominal : échantillons source par échantillon de
    /// sortie.
    base_step: f64,
    target: usize,
    max: usize,
    /// Lecture en cours (sinon : silence, en attendant la réserve).
    playing: bool,
    /// Nombre de famines, pour le diagnostic.
    pub underruns: u32,
}

impl AudioQueue {
    pub fn new(src_rate: f64, dst_rate: f64) -> Self {
        AudioQueue {
            src: VecDeque::new(),
            frac: 0.0,
            base_step: src_rate / dst_rate,
            target: (src_rate * TARGET_SECONDS) as usize,
            max: (src_rate * MAX_SECONDS) as usize,
            playing: false,
            underruns: 0,
        }
    }

    /// Échantillons source en réserve.
    pub fn len(&self) -> usize {
        self.src.len()
    }

    pub fn is_empty(&self) -> bool {
        self.src.is_empty()
    }

    /// Ajoute des échantillons entrelacés (gauche, droite, …), tels que
    /// `Sound::out`.
    pub fn push(&mut self, interleaved: &[i16]) {
        self.src.extend(interleaved.chunks_exact(2).map(|p| [p[0], p[1]]));
        if self.src.len() > self.max {
            let extra = self.src.len() - self.max;
            self.src.drain(..extra);
        }
    }

    /// Pas de lecture corrigé selon la réserve : plus pleine, on lit un peu
    /// plus vite ; plus vide, un peu plus lentement.
    fn step(&self) -> f64 {
        let t = self.target.max(1) as f64;
        let error = ((self.src.len() as f64 - t) / t).clamp(-1.0, 1.0);
        self.base_step * (1.0 + MAX_CORRECTION * error)
    }

    /// Remplit un tampon de sortie entrelacé de `channels` canaux, en
    /// valeurs dans [−1, 1]. Mono : moyenne des deux côtés ; au-delà de
    /// deux canaux, les suivants restent muets.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        if !self.playing && self.src.len() >= self.target.max(2) {
            self.playing = true;
        }
        for frame in out.chunks_mut(channels) {
            let [l, r] = self.next_frame();
            match frame {
                [m] => *m = (l + r) / 2.0,
                [a, b, rest @ ..] => {
                    *a = l;
                    *b = r;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
    }

    /// Un échantillon de sortie (gauche, droite).
    fn next_frame(&mut self) -> [f32; 2] {
        if !self.playing {
            return [0.0; 2];
        }
        // Il faut l'échantillon courant et le suivant pour interpoler.
        if self.src.len() < 2 {
            self.playing = false;
            self.underruns += 1;
            return [0.0; 2];
        }
        let (a, b) = (self.src[0], self.src[1]);
        let t = self.frac as f32;
        let mix = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) / 32768.0;
        let out = [mix(0), mix(1)];
        self.frac += self.step();
        while self.frac >= 1.0 && !self.src.is_empty() {
            self.frac -= 1.0;
            self.src.pop_front();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File déjà amorcée, de fréquences `src` → `dst`, contenant `n`
    /// échantillons valant (i, −i).
    fn queue(src: f64, dst: f64, n: i16) -> AudioQueue {
        let mut q = AudioQueue::new(src, dst);
        q.target = 2;
        let data: Vec<i16> = (0..n).flat_map(|i| [i, -i]).collect();
        q.push(&data);
        q
    }

    fn left(out: &[f32]) -> Vec<f32> {
        out.chunks(2).map(|f| f[0] * 32768.0).collect()
    }

    #[test]
    fn same_rate_passes_samples_through() {
        // Réserve pile à la cible : pas de correction.
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.target = 8;
        q.push(&(0..8i16).flat_map(|i| [i, -i]).collect::<Vec<_>>());
        let mut out = [0.0; 2];
        q.fill(&mut out, 2);
        assert_eq!(out, [0.0, 0.0]);
        q.fill(&mut out, 2);
        assert_eq!(out[0] * 32768.0, 1.0);
        assert_eq!(out[1] * 32768.0, -1.0, "droite séparée de la gauche");
    }

    #[test]
    fn upsampling_interpolates_between_samples() {
        // ×2 : un échantillon sur deux tombe à mi-chemin. La réserve est
        // très au-dessus de la cible ; on neutralise la correction.
        let mut q = queue(1000.0, 2000.0, 100);
        q.target = 100;
        let mut out = [0.0; 10];
        q.fill(&mut out, 2);
        let v = left(&out);
        for (got, want) in v.iter().zip([0.0, 0.5, 1.0, 1.5, 2.0]) {
            assert!((got - want).abs() < 0.02, "{v:?}");
        }
    }

    #[test]
    fn waits_for_the_reserve_and_reprimes_after_an_underrun() {
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.target = 4;
        q.push(&[100, 100, 200, 200, 300, 300]);
        let mut out = [1.0; 4];
        q.fill(&mut out, 2);
        assert_eq!(out, [0.0; 4], "réserve pas encore pleine : silence");
        q.push(&[400, 400]);
        let mut out = [0.0; 16];
        q.fill(&mut out, 2);
        let v = left(&out);
        assert!(v[0] > 99.0 && v[0] < 101.0, "{v:?}");
        assert_eq!(v[7], 0.0, "famine : silence");
        assert_eq!(q.underruns, 1);
        q.push(&[500, 500]);
        let mut out = [1.0; 2];
        q.fill(&mut out, 2);
        assert_eq!(out, [0.0; 2], "pas de reprise avant la réserve");
    }

    #[test]
    fn a_fuller_queue_is_read_slightly_faster() {
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.target = 100;
        q.push(&[0; 2 * 200]);
        assert!((q.step() - 1.005).abs() < 1e-9, "réserve double : +0,5 %");
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.target = 100;
        q.push(&[0; 2 * 50]);
        assert!((q.step() - 0.9975).abs() < 1e-9, "moitié : −0,25 %");
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.target = 50;
        q.push(&[0; 2 * 240]);
        assert!((q.step() - 1.005).abs() < 1e-9, "correction bornée à +0,5 %");
    }

    #[test]
    fn the_queue_drops_the_oldest_beyond_its_maximum() {
        let mut q = AudioQueue::new(1000.0, 1000.0);
        q.push(&(0..600i16).flat_map(|i| [i, i]).collect::<Vec<_>>());
        assert_eq!(q.len(), 250);
        assert_eq!(q.src[0], [350, 350], "les plus anciens jetés");
    }

    #[test]
    fn mono_and_extra_channels() {
        let mut q = queue(1000.0, 1000.0, 10);
        q.target = 10;
        let mut out = [9.0; 6];
        q.fill(&mut out, 3);
        assert_eq!(&out[3..6], &[1.0 / 32768.0, -1.0 / 32768.0, 0.0]);
        let mut q = queue(1000.0, 1000.0, 10);
        q.push(&[]);
        let mut out = [9.0; 2];
        q.fill(&mut out, 1);
        assert_eq!(out[1], 0.0, "mono : (i − i) / 2");
    }

    #[test]
    fn long_run_keeps_the_reserve_near_its_target() {
        // Horloges décalées de ±0,2 % (bien plus que l'écart réel entre
        // l'horloge du PC et celle de la DS) : ni famine, ni débordement,
        // sur une minute.
        for drift in [1.002, 0.998] {
            let mut q = AudioQueue::new(32_728.0, 48_000.0);
            let mut out = vec![0.0; 2 * 800]; // 1/60 s à 48 kHz
            let mut owed = 0.0;
            for _ in 0..3600 {
                owed += 32_728.0 / 60.0 * drift;
                let n = owed as usize;
                owed -= n as f64;
                q.push(&vec![1000; 2 * n]);
                q.fill(&mut out, 2);
            }
            assert_eq!(q.underruns, 0, "dérive {drift}");
            assert!(q.len() < q.max && q.len() > q.target / 4, "dérive {drift} : {} / {}", q.len(), q.target);
        }
    }
}
