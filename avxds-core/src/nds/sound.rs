//! Contrôleur son de l'ARM7 : 16 canaux (PCM8, PCM16, IMA-ADPCM, PSG,
//! bruit), mixeur stéréo, deux unités de capture.
//!
//! Sources : GBATEK « DS Sound Channels 0..15 », « DS Sound Control
//! Registers », « DS Sound Capture », « DS Sound Notes »
//! (`docs/SOUND_SPEC.md`). Écrit simple et lisible : c'est la référence des
//! versions rapides à venir.
//!
//! **Cadence.** Le mixeur produit un échantillon stéréo toutes les 1024
//! périodes de 33,51 MHz (≈ 32 728 Hz ; GBATEK annonce 32,768 kHz en
//! sortie PWM). Entre deux échantillons, le timer de chaque canal avance de
//! 512 pas (il compte à 33,51 MHz / 2).

use crate::bus::Bus;

/// Cycles ARM9 entre deux échantillons de sortie (1024 cycles de bus).
pub const SAMPLE_CYCLES: u64 = 1024 * super::scheduler::BUS_CYCLE;
/// Fréquence de sortie : 33 513 982 Hz / 1024.
pub const SAMPLE_RATE: f64 = 33_513_982.0 / 1024.0;
/// Pas des timers de canal par échantillon de sortie.
const TICKS_PER_SAMPLE: u32 = 512;
/// Échantillons gardés au plus si personne ne les lit (≈ 1 s, stéréo).
const MAX_BUFFERED: usize = 2 * 32_768;

/// IndexTable de l'IMA-ADPCM (GBATEK « DS Sound Notes »).
const ADPCM_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// AdpcmTable, construite par le générateur que donne GBATEK :
/// X = 000776D2h ; Table[i] = X SHR 16 ; X += X / 10, puis Table[3] = 0Ah,
/// Table[4] = 0Bh, Table[88] = 7FFFh.
const fn adpcm_table() -> [i32; 89] {
    let mut t = [0i32; 89];
    let mut x: u32 = 0x0007_76D2;
    let mut i = 0;
    while i < 89 {
        t[i] = (x >> 16) as i32;
        x += x / 10;
        i += 1;
    }
    t[3] = 0x000A;
    t[4] = 0x000B;
    t[88] = 0x7FFF;
    t
}

const ADPCM_TABLE: [i32; 89] = adpcm_table();

/// Volume, panoramique et volume général : les valeurs 0..126 valent N,
/// 127 vaut 128 (GBATEK). Hypothèse : la même règle pour le panoramique.
fn level(v: u32) -> i64 {
    let v = (v & 0x7F) as i64;
    if v == 127 {
        128
    } else {
        v
    }
}

#[derive(Clone, Debug, Default)]
pub struct Channel {
    pub cnt: u32,
    pub sad: u32,
    pub tmr: u16,
    pub pnt: u16,
    pub len: u32,
    /// Compteur du timer (en pas de 33,51 MHz / 2).
    timer: u32,
    /// Périodes à attendre avant le premier échantillon (démarrage), et
    /// périodes déjà attendues.
    delay: u32,
    waited: u32,
    /// Position : octet depuis SAD (PCM), quartet de données (ADPCM), pas
    /// du cycle (PSG).
    pos: u32,
    /// Échantillon courant, PCM16.
    pub sample: i32,
    adpcm: (i32, i32),
    adpcm_loop: Option<(i32, i32)>,
    lfsr: u16,
}

impl Channel {
    fn busy(&self) -> bool {
        self.cnt & (1 << 31) != 0
    }

    fn format(&self) -> u32 {
        (self.cnt >> 29) & 3
    }

    fn repeat(&self) -> u32 {
        (self.cnt >> 27) & 3
    }

    /// Fin des données, en octets depuis SAD : PNT + LEN mots.
    fn end(&self) -> u32 {
        (self.pnt as u32 + self.len) * 4
    }

    /// Bit 31 passé de 0 à 1 : tout repart du début.
    fn start(&mut self, n: usize, bus: &mut impl Bus) {
        self.timer = 0;
        self.pos = 0;
        self.adpcm_loop = None;
        self.lfsr = 0x7FFF;
        // Délai avant le premier échantillon : PSG/bruit 1, PCM 3, ADPCM
        // 3 + 8 pour l'en-tête (GBATEK, « Timing »).
        self.delay = match self.format() {
            3 => 1,
            2 => 11,
            _ => 3,
        };
        self.waited = 0;
        if self.format() == 2 {
            let h = bus.read32(self.sad);
            self.adpcm = (h as u16 as i16 as i32, ((h >> 16) & 0x7F).min(88) as i32);
        }
        // Avec Hold, l'ancien niveau tient pendant le premier échantillon ;
        // sans, silence pendant le délai.
        if self.cnt & (1 << 15) == 0 {
            self.sample = 0;
        }
        let _ = n;
    }

    /// Avance le timer d'un échantillon de sortie ; renvoie la valeur.
    fn tick(&mut self, n: usize, bus: &mut impl Bus) -> i32 {
        if !self.busy() {
            // Arrêté : silence, ou le dernier échantillon tenu (Hold).
            if self.cnt & (1 << 15) == 0 {
                self.sample = 0;
            }
            return self.sample;
        }
        let period = 0x1_0000 - self.tmr as u32;
        self.timer += TICKS_PER_SAMPLE;
        while self.timer >= period && self.busy() {
            self.timer -= period;
            self.step(n, bus);
        }
        self.sample
    }

    /// Un pas du timer : l'échantillon suivant.
    fn step(&mut self, n: usize, bus: &mut impl Bus) {
        if self.delay > 0 {
            // Délai de démarrage. Avec Hold, l'ancien niveau tient pendant la
            // première période, puis zéro (GBATEK).
            self.delay -= 1;
            if self.waited >= 1 {
                self.sample = 0;
            }
            self.waited += 1;
            return;
        }
        match self.format() {
            0 => {
                self.sample = (bus.read8(self.sad + self.pos) as i8 as i32) << 8;
                self.pos += 1;
                self.check_end(4);
            }
            1 => {
                self.sample = bus.read16(self.sad + self.pos) as i16 as i32;
                self.pos += 2;
                self.check_end(4);
            }
            2 => self.adpcm_step(bus),
            _ => self.psg_step(n),
        }
    }

    /// Fin des données PCM : boucle sur PNT, ou arrêt (un coup ; manuel,
    /// hypothèse : comme un coup).
    fn check_end(&mut self, _unit: u32) {
        if self.pos < self.end() {
            return;
        }
        if self.repeat() == 1 {
            self.pos = self.pnt as u32 * 4;
        } else {
            self.stop();
        }
    }

    /// Fin d'un son joué une fois : le bit 31 retombe au début de la
    /// dernière période, mais le dernier échantillon sonne jusqu'à sa fin
    /// (GBATEK) ; ensuite silence, ou maintien avec Hold (voir `tick`).
    fn stop(&mut self) {
        self.cnt &= !(1 << 31);
    }

    /// IMA-ADPCM : quartet bas d'abord ; formule « pratique » de GBATEK
    /// (somme de Table/8, /4, /2, /1), bornée à ±7FFFh.
    fn adpcm_step(&mut self, bus: &mut impl Bus) {
        // Quartet `pos` des données (après le mot d'en-tête).
        let byte_off = 4 + self.pos / 2;
        // Début de boucle : on note l'état la première fois qu'on y passe.
        let loop_nibble = (self.pnt as u32).saturating_sub(1) * 8;
        if self.repeat() == 1 && self.pos == loop_nibble && self.adpcm_loop.is_none() {
            self.adpcm_loop = Some(self.adpcm);
        }
        let b = bus.read8(self.sad + byte_off);
        let code = if self.pos.is_multiple_of(2) { b & 15 } else { b >> 4 } as i32;
        let (mut pcm, mut index) = self.adpcm;
        let t = ADPCM_TABLE[index as usize];
        let mut diff = t / 8;
        if code & 1 != 0 {
            diff += t / 4;
        }
        if code & 2 != 0 {
            diff += t / 2;
        }
        if code & 4 != 0 {
            diff += t;
        }
        pcm = if code & 8 != 0 { (pcm - diff).max(-0x7FFF) } else { (pcm + diff).min(0x7FFF) };
        index = (index + ADPCM_INDEX[(code & 7) as usize]).clamp(0, 88);
        self.adpcm = (pcm, index);
        self.sample = pcm;
        self.pos += 1;
        if 4 + self.pos / 2 >= self.end() {
            if self.repeat() == 1 {
                self.pos = loop_nibble;
                if let Some(s) = self.adpcm_loop {
                    self.adpcm = s;
                }
            } else {
                self.stop();
            }
        }
    }

    /// PSG : canaux 8–13, rectangle de 8 pas commençant par le niveau bas ;
    /// bruit : canaux 14–15, compteur à rétroaction (X SHR 1, XOR 6000h si
    /// la retenue est à 1, sortie basse dans ce cas). Les canaux 0–7 en
    /// mode PSG restent muets.
    fn psg_step(&mut self, n: usize) {
        const HIGH: i32 = 0x7FFF;
        const LOW: i32 = -0x7FFF;
        match n {
            8..=13 => {
                let duty = (self.cnt >> 24) & 7;
                let high = duty != 7 && self.pos % 8 >= 7 - duty;
                self.sample = if high { HIGH } else { LOW };
                self.pos = (self.pos + 1) % 8;
            }
            14 | 15 => {
                let carry = self.lfsr & 1 != 0;
                self.lfsr >>= 1;
                if carry {
                    self.lfsr ^= 0x6000;
                }
                self.sample = if carry { LOW } else { HIGH };
            }
            _ => self.sample = 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub cnt: u8,
    pub dad: u32,
    pub len: u16,
    timer: u32,
    pos: u32,
}

#[derive(Clone, Debug)]
#[derive(Default)]
pub struct Sound {
    pub ch: [Channel; 16],
    pub soundcnt: u16,
    pub bias: u16,
    pub cap: [Capture; 2],
    /// Échantillons stéréo produits (gauche, droite), en attente du
    /// frontend : i16 signés, centrés sur 0.
    pub out: Vec<i16>,
}

impl Sound {
    /// Adresses ARM7 du contrôleur son.
    pub fn handles(addr: u32) -> bool {
        (0x0400_0400..=0x0400_051F).contains(&addr)
    }

    pub fn read(&self, addr: u32) -> u32 {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let c = &self.ch[((addr >> 4) & 15) as usize];
                // Seul SOUNDxCNT se relit ; le reste est en écriture seule.
                if addr & 0xF == 0 {
                    c.cnt
                } else {
                    0
                }
            }
            0x0400_0500 => self.soundcnt as u32,
            0x0400_0504 => self.bias as u32,
            0x0400_0508 => self.cap[0].cnt as u32 | (self.cap[1].cnt as u32) << 8,
            0x0400_0510 => self.cap[0].dad,
            0x0400_0518 => self.cap[1].dad,
            _ => 0,
        }
    }

    /// Écriture (adresse alignée sur 4, `mask` = octets écrits).
    pub fn write(&mut self, addr: u32, val: u32, mask: u32, bus: &mut impl Bus) {
        let merge = |old: u32, keep: u32| (old & !(mask & keep)) | (val & mask & keep);
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let n = ((addr >> 4) & 15) as usize;
                let c = &mut self.ch[n];
                match addr & 0xF {
                    0 => {
                        let was = c.busy();
                        c.cnt = merge(c.cnt, 0xFF7F_837F);
                        if c.busy() && !was {
                            c.start(n, bus);
                        }
                    }
                    4 => c.sad = merge(c.sad, 0x07FF_FFFC),
                    8 => {
                        let v = merge(c.tmr as u32 | (c.pnt as u32) << 16, 0xFFFF_FFFF);
                        c.tmr = v as u16;
                        c.pnt = (v >> 16) as u16;
                    }
                    _ => c.len = merge(c.len, 0x003F_FFFF),
                }
            }
            0x0400_0500 => self.soundcnt = merge(self.soundcnt as u32, 0xBF7F) as u16,
            0x0400_0504 => self.bias = merge(self.bias as u32, 0x3FF) as u16,
            0x0400_0508 => {
                let v = merge(self.cap[0].cnt as u32 | (self.cap[1].cnt as u32) << 8, 0x8F8F);
                for (i, cap) in self.cap.iter_mut().enumerate() {
                    let new = (v >> (8 * i)) as u8;
                    if new & 0x80 != 0 && cap.cnt & 0x80 == 0 {
                        cap.timer = 0;
                        cap.pos = 0;
                    }
                    cap.cnt = new;
                }
            }
            0x0400_0510 => self.cap[0].dad = merge(self.cap[0].dad, 0x07FF_FFFC),
            0x0400_0514 => self.cap[0].len = merge(self.cap[0].len as u32, 0xFFFF) as u16,
            0x0400_0518 => self.cap[1].dad = merge(self.cap[1].dad, 0x07FF_FFFC),
            0x0400_051C => self.cap[1].len = merge(self.cap[1].len as u32, 0xFFFF) as u16,
            _ => {}
        }
    }

    /// Produit un échantillon stéréo (appelé toutes les `SAMPLE_CYCLES`).
    pub fn sample(&mut self, bus: &mut impl Bus) {
        let enabled = self.soundcnt & 0x8000 != 0;
        // Valeurs de chaque canal après volume et panoramique, en 16.8
        // (GBATEK « Mixer pipeline », étapes 1 à 4).
        let mut left = [0i64; 16];
        let mut right = [0i64; 16];
        for n in 0..16 {
            let data = self.ch[n].tick(n, bus) as i64;
            if !enabled {
                continue;
            }
            let c = &self.ch[n];
            let shift = [0, 1, 2, 4][((c.cnt >> 8) & 3) as usize];
            let v = (data << 4) >> shift; // 16.4
            let v = v * level(c.cnt); // 16.11
            let pan = level(c.cnt >> 16);
            // 16.18, puis 10 bits de fraction retirés (arrondi par défaut).
            left[n] = (v * (128 - pan)) >> 10;
            right[n] = (v * pan) >> 10;
        }
        // Canaux 1 et 3 retirés du mixeur sur demande (SOUNDCNT bits 12–13).
        let mut mix_l = 0i64;
        let mut mix_r = 0i64;
        for n in 0..16 {
            if (n == 1 && self.soundcnt & (1 << 12) != 0) || (n == 3 && self.soundcnt & (1 << 13) != 0) {
                continue;
            }
            mix_l += left[n];
            mix_r += right[n];
        }
        self.capture(bus, mix_l, mix_r, &left, &right);
        // Sorties : mixeur, canal 1, canal 3 ou les deux (bits 8–11).
        let pick = |sel: u16, mix: i64, side: &[i64; 16]| match sel & 3 {
            0 => mix,
            1 => side[1],
            2 => side[3],
            _ => side[1] + side[3],
        };
        let l = pick(self.soundcnt >> 8, mix_l, &left);
        let r = pick(self.soundcnt >> 10, mix_r, &right);
        // Volume général (× N / 128 / 64, 21 bits de fraction), biais, bornes
        // 0..3FFh. Mixeur coupé : les canaux valent 0 plus haut, il reste le
        // biais seul.
        let master = level(self.soundcnt as u32);
        let out = |v: i64| -> i16 {
            // Sortie 10 bits bornée, puis recentrée sur le biais : les
            // condensateurs de sortie coupent la composante continue
            // (hypothèse, SOUND_SPEC §H1). Sans ça, un biais à 0 au
            // démarrage donnerait -32768 en continu puis un claquement.
            let bias = self.bias as i64 & 0x3FF;
            let s = ((v * master) >> 21) + bias;
            ((s.clamp(0, 0x3FF) - bias) << 6).clamp(-0x8000, 0x7FFF) as i16
        };
        // Personne ne lit : on jette la moitié la plus ancienne d'un coup
        // (jeter 2 valeurs à chaque fois recopierait tout le tampon).
        if self.out.len() >= MAX_BUFFERED {
            self.out.drain(..MAX_BUFFERED / 2);
        }
        self.out.push(out(l));
        self.out.push(out(r));
    }

    /// Unités de capture : au rythme du timer du canal 1 (capture 0) ou 3
    /// (capture 1), elles écrivent le mixeur gauche/droit (ou le canal 0/2)
    /// en mémoire. Données 16.8 bornées à 16 bits ; PCM8 = valeur / 256,
    /// arrondie vers zéro. Mode « addition » et défauts de capture : non
    /// émulés.
    fn capture(&mut self, bus: &mut impl Bus, mix_l: i64, mix_r: i64, left: &[i64; 16], right: &[i64; 16]) {
        for i in 0..2 {
            let cnt = self.cap[i].cnt;
            if cnt & 0x80 == 0 {
                continue;
            }
            let src = if cnt & 2 == 0 {
                if i == 0 {
                    mix_l
                } else {
                    mix_r
                }
            } else if i == 0 {
                left[0] + right[0]
            } else {
                left[2] + right[2]
            };
            let value = (src >> 8).clamp(-0x8000, 0x7FFF) as i32;
            let period = 0x1_0000 - self.ch[1 + 2 * i].tmr as u32;
            let cap = &mut self.cap[i];
            cap.timer += TICKS_PER_SAMPLE;
            while cap.timer >= period && cap.cnt & 0x80 != 0 {
                cap.timer -= period;
                let len = (cap.len.max(1) as u32) * 4;
                let addr = cap.dad + cap.pos;
                if cnt & 8 != 0 {
                    bus.write8(addr, (value / 256) as i8 as u8);
                    cap.pos += 1;
                } else {
                    bus.write16(addr, value as i16 as u16);
                    cap.pos += 2;
                }
                if cap.pos >= len {
                    cap.pos = 0;
                    if cnt & 4 != 0 {
                        cap.cnt &= 0x7F; // un coup : fin
                    }
                }
            }
        }
    }
}

/// SWI 1Ah GetSineTable : sin(i × π / 128) × 7FFFh, i = 0..63 (64 entrées
/// de 0 à 7FF5h, « environ 0 à 88,6° » d'après GBATEK). Formule retrouvée
/// d'après ces bornes : à vérifier.
pub fn sine_table(i: u32) -> u32 {
    ((i.min(63) as f64 * std::f64::consts::PI / 128.0).sin() * 32767.0).round() as u32
}

/// SWI 1Bh GetPitchTable : (2^(i/768) − 1) × 10000h, i = 0..2FFh (768
/// entrées de 0 à FF8Ah d'après GBATEK). Formule retrouvée d'après ces
/// bornes : à vérifier.
pub fn pitch_table(i: u32) -> u32 {
    // Arrondi : la troncature donnerait FF89h pour la dernière entrée.
    ((2f64.powf(i.min(0x2FF) as f64 / 768.0) - 1.0) * 65536.0).round() as u32
}

/// SWI 1Ch GetVolumeTable : 724 entrées de 0 à 7Fh (GBATEK) ; hypothèse
/// d'une courbe en dixièmes de décibel : 127 × 10^((i − 723) / 200).
pub fn volume_table(i: u32) -> u32 {
    (127.0 * 10f64.powf((i.min(0x2D3) as f64 - 723.0) / 200.0)).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mémoire plate de 1 Mo, vue comme le bus de l'ARM7.
    struct Mem(Vec<u8>);
    impl Bus for Mem {
        fn read8(&mut self, a: u32) -> u8 {
            self.0[a as usize & 0xF_FFFF]
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
        }
        fn read32(&mut self, a: u32) -> u32 {
            u32::from_le_bytes([0, 1, 2, 3].map(|k| self.read8(a + k)))
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.0[a as usize & 0xF_FFFF] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            for (k, b) in v.to_le_bytes().into_iter().enumerate() {
                self.write8(a + k as u32, b);
            }
        }
        fn write32(&mut self, a: u32, v: u32) {
            for (k, b) in v.to_le_bytes().into_iter().enumerate() {
                self.write8(a + k as u32, b);
            }
        }
    }

    fn mem() -> Mem {
        Mem(vec![0; 1 << 20])
    }

    const ALL: u32 = 0xFFFF_FFFF;
    /// TMR pour un pas du canal par échantillon de sortie (période 512).
    const ONE_PER_SAMPLE: u32 = 0x1_0000 - 512;

    /// Programme le canal `n` : SAD, TMR, PNT, LEN, puis CNT (qui démarre).
    #[allow(clippy::too_many_arguments)]
    fn play(s: &mut Sound, m: &mut Mem, n: u32, sad: u32, tmr: u32, pnt: u32, len: u32, cnt: u32) {
        let base = 0x0400_0400 + 16 * n;
        s.write(base + 4, sad, ALL, m);
        s.write(base + 8, tmr | pnt << 16, ALL, m);
        s.write(base + 12, len, ALL, m);
        s.write(base, cnt | 1 << 31, ALL, m);
    }

    /// Les `k` échantillons successifs du canal `n` (un par sortie).
    fn samples(s: &mut Sound, m: &mut Mem, n: usize, k: usize) -> Vec<i32> {
        (0..k)
            .map(|_| {
                s.sample(m);
                s.ch[n].sample
            })
            .collect()
    }

    const PCM8: u32 = 0;
    const PCM16: u32 = 1 << 29;
    const ADPCM: u32 = 2 << 29;
    const PSG: u32 = 3 << 29;
    const LOOP: u32 = 1 << 27;
    const ONESHOT: u32 = 2 << 27;

    #[test]
    fn adpcm_table_follows_the_gbatek_generator() {
        assert_eq!(&ADPCM_TABLE[..11], &[7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19]);
        assert_eq!((ADPCM_TABLE[87], ADPCM_TABLE[88]), (29794, 0x7FFF));
    }

    #[test]
    fn registers_keep_their_valid_bits() {
        let (mut s, mut m) = (Sound::default(), mem());
        s.write(0x0400_0410, !(1 << 31), ALL, &mut m);
        assert_eq!(s.read(0x0400_0410), 0x7F7F_837F, "bits inutilisés à zéro");
        s.write(0x0400_0414, 0xFFFF_FFFF, ALL, &mut m);
        assert_eq!(s.ch[1].sad, 0x07FF_FFFC, "SAD aligné sur 4");
        assert_eq!(s.read(0x0400_0414), 0, "SAD en écriture seule");
        s.write(0x0400_0418, 0x1234_5678, ALL, &mut m);
        assert_eq!((s.ch[1].tmr, s.ch[1].pnt), (0x5678, 0x1234));
        s.write(0x0400_041C, 0xFFFF_FFFF, ALL, &mut m);
        assert_eq!(s.ch[1].len, 0x003F_FFFF, "LEN sur 22 bits");
        // Écriture d'un seul octet (volume) : le reste ne bouge pas.
        s.write(0x0400_0410, 0x0000_0040, 0x0000_00FF, &mut m);
        assert_eq!(s.read(0x0400_0410), 0x7F7F_8340);
        s.write(0x0400_0500, 0xFFFF, ALL, &mut m);
        assert_eq!(s.read(0x0400_0500), 0xBF7F);
        s.write(0x0400_0504, 0xFFFF, ALL, &mut m);
        assert_eq!(s.read(0x0400_0504), 0x3FF);
        s.write(0x0400_0508, 0xFFFF, ALL, &mut m);
        assert_eq!(s.read(0x0400_0508), 0x8F8F);
    }

    #[test]
    fn pcm8_plays_after_three_samples_at_the_timer_rate() {
        let (mut s, mut m) = (Sound::default(), mem());
        m.0[0x1000..0x1008].copy_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]);
        play(&mut s, &mut m, 0, 0x1000, ONE_PER_SAMPLE, 0, 2, PCM8 | ONESHOT);
        let v = samples(&mut s, &mut m, 0, 11);
        assert_eq!(&v[..7], &[0, 0, 0, 0x1000, 0x2000, 0x3000, 0x4000], "délai de 3, PCM8 × 100h");
        assert_eq!(v[10], -0x8000, "octets signés");
        // Deux fois plus vite : deux pas par échantillon de sortie.
        let (mut s, mut m2) = (Sound::default(), mem());
        m2.0[0x1000..0x1008].copy_from_slice(&m.0[0x1000..0x1008]);
        play(&mut s, &mut m2, 0, 0x1000, 0x1_0000 - 256, 0, 2, PCM8 | ONESHOT);
        let v = samples(&mut s, &mut m2, 0, 4);
        assert_eq!(v, vec![0, 0x1000, 0x3000, 0x5000]);
    }

    #[test]
    fn loop_replays_from_pnt_and_one_shot_stops() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let (mut s, mut m) = (Sound::default(), mem());
        m.0[0x2000..0x2008].copy_from_slice(&data);
        play(&mut s, &mut m, 2, 0x2000, ONE_PER_SAMPLE, 1, 1, PCM8 | LOOP);
        let v: Vec<i32> = samples(&mut s, &mut m, 2, 15).into_iter().skip(3).map(|x| x >> 8).collect();
        assert_eq!(v, vec![1, 2, 3, 4, 5, 6, 7, 8, 5, 6, 7, 8], "PNT une fois, puis LEN en boucle");
        // Un coup : arrêt à la fin, bit 31 effacé, silence.
        let (mut s, mut m) = (Sound::default(), mem());
        m.0[0x2000..0x2008].copy_from_slice(&data);
        play(&mut s, &mut m, 2, 0x2000, ONE_PER_SAMPLE, 1, 1, PCM8 | ONESHOT);
        let v = samples(&mut s, &mut m, 2, 12);
        assert_eq!(v[10] >> 8, 8, "le dernier échantillon sonne sa période");
        assert_eq!(s.read(0x0400_0420) >> 31, 0, "plus occupé dès cette période");
        assert_eq!(v[11], 0, "puis silence");
        // Avec Hold : le dernier échantillon tient.
        let (mut s, mut m) = (Sound::default(), mem());
        m.0[0x2000..0x2008].copy_from_slice(&data);
        play(&mut s, &mut m, 2, 0x2000, ONE_PER_SAMPLE, 1, 1, PCM8 | ONESHOT | 1 << 15);
        let v = samples(&mut s, &mut m, 2, 12);
        assert_eq!(v[11] >> 8, 8, "tenu");
        // Redémarrage avec Hold : l'ancien niveau pendant la 1re période,
        // zéro pendant les 2e et 3e, le nouveau son à la 4e (GBATEK).
        s.write(0x0400_0420, (PCM8 | ONESHOT | 1 << 15) | 1 << 31, ALL, &mut m);
        let v = samples(&mut s, &mut m, 2, 4);
        assert_eq!(v, vec![8 << 8, 0, 0, 1 << 8]);
    }

    #[test]
    fn pcm16_samples_are_signed_halfwords() {
        let (mut s, mut m) = (Sound::default(), mem());
        m.write16(0x3000, 0x8000);
        m.write16(0x3002, 0x7FFF);
        play(&mut s, &mut m, 5, 0x3000, ONE_PER_SAMPLE, 0, 4, PCM16 | ONESHOT);
        assert_eq!(&samples(&mut s, &mut m, 5, 5)[3..], &[-0x8000, 0x7FFF]);
    }

    #[test]
    fn adpcm_decodes_and_restores_its_loop_state() {
        let (mut s, mut m) = (Sound::default(), mem());
        // En-tête : PCM 0, index 0 ; données : 7, F, 3, 0 (quartet bas
        // d'abord) ; valeurs calculées à la main d'après GBATEK.
        m.write32(0x4000, 0);
        m.write8(0x4004, 0xF7);
        m.write8(0x4005, 0x03);
        play(&mut s, &mut m, 3, 0x4000, ONE_PER_SAMPLE, 0, 3, ADPCM | ONESHOT);
        let v = samples(&mut s, &mut m, 3, 15);
        assert!(v[..11].iter().all(|&x| x == 0));
        assert_eq!(&v[11..15], &[11, -19, 10, 13], "délai de 11 (3 + en-tête)");
        // Boucle : au retour sur PNT, PCM et index reprennent l'état noté au
        // premier passage. Ici PNT = 1 (premier mot de données), LEN = 1 :
        // 8 quartets, puis les mêmes valeurs.
        let (mut s, mut m) = (Sound::default(), mem());
        m.write32(0x4000, 100 | 5 << 16);
        m.write32(0x4004, 0x1234_7F37);
        play(&mut s, &mut m, 3, 0x4000, ONE_PER_SAMPLE, 1, 1, ADPCM | LOOP);
        let v = samples(&mut s, &mut m, 3, 11 + 16);
        assert_eq!(&v[11..19], &v[19..27], "même suite après la boucle");
        // Bornes : index 88 qui ne monte plus, PCM borné à −7FFFh.
        // Code F : −(4095 + 8191 + 16383 + 32767) → −7FFFh ; puis code 0 :
        // + 7FFFh / 8 = 4095 → −28672 (index resté à 88).
        let (mut s, mut m) = (Sound::default(), mem());
        m.write32(0x4000, 88 << 16);
        m.write8(0x4004, 0x0F);
        play(&mut s, &mut m, 3, 0x4000, ONE_PER_SAMPLE, 0, 2, ADPCM | ONESHOT);
        assert_eq!(&samples(&mut s, &mut m, 3, 13)[11..13], &[-0x7FFF, -28672]);
    }

    #[test]
    fn psg_duty_cycles_and_noise() {
        let (mut s, mut m) = (Sound::default(), mem());
        const H: i32 = 0x7FFF;
        const L: i32 = -0x7FFF;
        play(&mut s, &mut m, 8, 0, ONE_PER_SAMPLE, 0, 0, PSG | 3 << 24);
        let v = samples(&mut s, &mut m, 8, 9);
        assert_eq!(&v[1..9], &[L, L, L, L, H, H, H, H], "50 % : bas d'abord");
        play(&mut s, &mut m, 9, 0, ONE_PER_SAMPLE, 0, 0, PSG);
        let v = samples(&mut s, &mut m, 9, 9);
        assert_eq!(&v[1..9], &[L, L, L, L, L, L, L, H], "12,5 %");
        play(&mut s, &mut m, 10, 0, ONE_PER_SAMPLE, 0, 0, PSG | 7 << 24);
        assert!(samples(&mut s, &mut m, 10, 9)[1..].iter().all(|&x| x == L), "duty 7 : toujours bas");
        play(&mut s, &mut m, 0, 0, ONE_PER_SAMPLE, 0, 4, PSG | 3 << 24);
        assert!(samples(&mut s, &mut m, 0, 6).iter().all(|&x| x == 0), "pas de PSG sur 0–7");
        // Bruit depuis 7FFFh : six bas, X = 40FFh ensuite (calcul à part).
        play(&mut s, &mut m, 14, 0, ONE_PER_SAMPLE, 0, 0, PSG);
        let v = samples(&mut s, &mut m, 14, 7);
        assert_eq!(&v[1..7], &[L; 6]);
        assert_eq!(s.ch[14].lfsr, 0x40FF);
    }

    #[test]
    fn mixer_follows_the_gbatek_pipeline() {
        let mix = |cnt_extra: u32, soundcnt: u32| {
            let (mut s, mut m) = (Sound::default(), mem());
            for o in (0..16).step_by(2) {
                m.write16(0x5000 + o, 0x1000);
            }
            s.write(0x0400_0500, soundcnt, ALL, &mut m);
            s.write(0x0400_0504, 0x200, ALL, &mut m);
            play(&mut s, &mut m, 4, 0x5000, ONE_PER_SAMPLE, 0, 4, PCM16 | LOOP | cnt_extra);
            for _ in 0..5 {
                s.sample(&mut m);
            }
            (s.out[s.out.len() - 2], s.out[s.out.len() - 1])
        };
        const ON: u32 = 0x8000 | 127; // activé, volume général 127 (= 128)
        // Volume 127, panoramique 64 : 1000h → (16.8) 800h × 128/128/64 =
        // 20h au-dessus du biais → (20h) × 64 = 2048 de chaque côté.
        assert_eq!(mix(127 | 64 << 16, ON), (2048, 2048));
        // Diviseur ÷16 : 2h → 128.
        assert_eq!(mix(127 | 3 << 8 | 64 << 16, ON), (128, 128));
        // Tout à droite (127 = 128).
        assert_eq!(mix(127 | 127 << 16, ON), (0, 4096));
        // Volume du canal 64 : moitié.
        assert_eq!(mix(64 | 64 << 16, ON), (1024, 1024));
        // Mixeur coupé : le biais seul, silence.
        assert_eq!(mix(127 | 64 << 16, 127), (0, 0));
        // Volume général 63 : 20h × 63/128 = 0Fh (arrondi par défaut).
        assert_eq!(mix(127 | 64 << 16, 0x8000 | 63), (15 * 64, 15 * 64));
    }

    #[test]
    fn soundcnt_routes_channels_1_and_3() {
        let (mut s, mut m) = (Sound::default(), mem());
        for o in (0..16).step_by(2) {
            m.write16(0x5000 + o, 0x1000);
        }
        s.write(0x0400_0504, 0x200, ALL, &mut m);
        play(&mut s, &mut m, 1, 0x5000, ONE_PER_SAMPLE, 0, 4, PCM16 | LOOP | 127 | 64 << 16);
        // Canal 1 hors mixeur (bit 12), mais sortie droite = canal 1.
        s.write(0x0400_0500, 0x8000 | 127 | 1 << 12 | 1 << 10, ALL, &mut m);
        for _ in 0..5 {
            s.sample(&mut m);
        }
        let n = s.out.len();
        assert_eq!((s.out[n - 2], s.out[n - 1]), (0, 2048));
    }

    #[test]
    fn capture_writes_the_mixer_at_channel_1_rate() {
        let (mut s, mut m) = (Sound::default(), mem());
        for o in (0..16).step_by(2) {
            m.write16(0x5000 + o, 0x1000);
        }
        s.write(0x0400_0500, 0x8000 | 127, ALL, &mut m);
        play(&mut s, &mut m, 0, 0x5000, ONE_PER_SAMPLE, 0, 4, PCM16 | LOOP | 127 | 64 << 16);
        // Le timer du canal 1 cadence la capture 0 (le canal n'a pas besoin
        // de jouer).
        s.write(0x0400_0418, 0x1_0000 - 1024, ALL, &mut m); // une capture pour deux échantillons
        s.write(0x0400_0510, 0x6000, ALL, &mut m);
        s.write(0x0400_0514, 2, ALL, &mut m); // 2 mots = 4 échantillons PCM16
        s.write(0x0400_0508, 0x80, ALL, &mut m); // mixeur gauche, boucle, PCM16
        for _ in 0..7 {
            s.sample(&mut m);
        }
        // Une capture tous les deux échantillons (2, 4, 6) ; le mixeur gauche
        // 16.8 vaut 800h.00 une fois le canal 0 parti → 0800h écrit.
        let got = |m: &mut Mem| (0..4).map(|k| m.read16(0x6000 + 2 * k)).collect::<Vec<_>>();
        assert_eq!(got(&mut m), vec![0, 0x0800, 0x0800, 0], "au rythme du canal 1");
        for _ in 0..4 {
            s.sample(&mut m);
        }
        assert_eq!(got(&mut m), vec![0x0800; 4], "4e capture, puis retour au début");
        assert_eq!(m.read16(0x6008), 0, "pas au-delà de LEN (boucle)");
        // Un coup + PCM8 : 0800h / 256 = 8 ; fin, bit 7 effacé.
        s.write(0x0400_0508, 0x80 | 4 | 8, ALL, &mut m);
        for _ in 0..12 {
            s.sample(&mut m);
        }
        assert_eq!(m.read8(0x6007), 8);
        assert_eq!(s.read(0x0400_0508) & 0x80, 0);
    }

    #[test]
    fn bios_sound_tables_match_the_gbatek_bounds() {
        assert_eq!((sine_table(0), sine_table(63)), (0, 0x7FF5));
        assert_eq!((pitch_table(0), pitch_table(0x2FF)), (0, 0xFF8A));
        assert_eq!(pitch_table(384), 0x6A0A, "demi-octave : (√2 − 1) × 10000h arrondi");
        assert_eq!((volume_table(0), volume_table(0x2D3)), (0, 0x7F));
    }
}
