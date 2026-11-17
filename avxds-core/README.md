# avxds-core

Cœur de l'émulateur Nintendo DS **AvxDS**, réécrit de zéro en novembre 2026.

Cette crate contient les deux processeurs de la DS (un seul interpréteur
générique) et la machine qui les relie (`Nds`) :

| Type    | Processeur | Architecture | Spécificités gérées |
|---------|------------|--------------|---------------------|
| `Arm9`  | ARM946E-S  | ARMv5TE      | BLX, CLZ, QADD/QSUB/QDADD/QDSUB, SMLAxy/SMULxy/SMLAWy/SMLALxy, LDRD/STRD, PLD, BKPT, MRC/MCR p15, interworking sur LDR/LDM/POP |
| `Arm7`  | ARM7TDMI   | ARMv4T       | Pas de CP15, condition NV = jamais, LDRH/LDRSH impairs, writeback LDM/STM de l'ARMv4 |

Pas de `unsafe` (`#![forbid(unsafe_code)]`), pas de pointeurs bruts, pas de
`static mut`, aucune dépendance.

## Utilisation

```rust
use avxds_core::{Arm9, Bus};

fn demarrer(bus: &mut impl Bus) {
    let mut cpu = Arm9::new();
    cpu.reset(bus);          // PC = vecteur de reset du bus
    loop {
        cpu.step(bus);       // une instruction (ou une IRQ si irq_line)
    }
}
```

La machine implémente le trait [`Bus`](src/bus.rs) : lectures/écritures 8/16/32
bits (toujours alignées, les rotations sont faites par le CPU), base des vecteurs
d'exception, CP15, et un point d'accroche `hle_swi` pour émuler les appels BIOS
sans BIOS.

## Structure

```
src/bus.rs          trait Bus, Cp15Op
src/cpu/mod.rs      registres, modes et banques, exceptions, boucle step()
src/cpu/alu.rs      additions avec retenue, barrel shifter, conditions
src/cpu/arm.rs      jeu ARM, décodage par table de 4096 entrées (const fn)
src/cpu/thumb.rs    jeu Thumb, décodage par table de 1024 entrées
src/cpu/tests.rs    39 tests unitaires, instruction par instruction
src/nds/mod.rs      struct Nds, matériel, vues Arm9Bus / Arm7Bus, registres I/O
src/nds/cp15.rs     CP15 : contrôle, ITCM/DTCM, registres d'identification
src/nds/vram.rs     banques A–I, VRAMCNT, zones BG/OBJ/LCDC/ARM7
src/nds/irq.rs      contrôleur IME/IE/IF (un par CPU)
src/nds/math.rs     division et racine carrée matérielles
src/nds/scheduler.rs  ordonnanceur d'événements (temps en cycles ARM9)
src/nds/lcd.rs      timing de l'écran : lignes, HBlank, VBlank, DISPSTAT, VCOUNT
src/nds/timers.rs   4 timers par CPU (prescaler, cascade, IRQ)
src/nds/ipc.rs      FIFO IPC (16 mots dans chaque sens)
src/nds/cart.rs     en-tête .nds, CRC16
src/nds/boot.rs     direct boot (état laissé par le firmware)
src/nds/hle_bios.rs mini-BIOS d'AvxDS (vecteurs, relais d'IRQ, IntrWait, Halt)
src/nds/hle_swi.rs  appels BIOS en Rust (Div, CpuSet, LZ77…)
src/nds/gpu.rs      moteurs 2D A et B (fonds texte, affines, bitmaps…)
src/nds/dma.rs      DMA (4 canaux par CPU) et DMA fill
src/nds/tests.rs    54 tests de la machine
src/png.rs          encodeur PNG sans dépendance (captures)
tests/boot.rs       direct boot d'une vraie ROM compilée pour le projet
tests/roms/         sources et binaire de cette ROM (C, ARM9 + ARM7)
tests/programs.rs   tests d'intégration : programmes C réels
tests/programs/     suite.c + binaires compilés + valeurs attendues
examples/bench.rs   débit de l'interpréteur et de la machine
examples/boot_report.rs  démarre une ROM et liste ce qui manque
examples/screenshot.rs   capture les deux écrans d'une ROM en PNG
tools/asm.py        réassemble les blocs `// asm:` des tests unitaires
tools/build_programs.sh   recompile les programmes de test
tools/build_test_roms.sh  recompile les ROMs de test (tests/roms/)
tools/make_nds.py   assemble une ROM .nds depuis deux binaires
```

## Tests

```
cargo test
cargo run --release --example bench
```

* **Tests unitaires** : chaque bloc de code est écrit en assembleur dans les
  commentaires (`0xE1A0000F, // mov r0, pc`). Après modification d'un
  commentaire : `python3 tools/asm.py src/cpu/tests.rs` (nécessite `llvm-mc`,
  `ld.lld` et `llvm-objdump`).
* **Tests d'intégration** : `tests/programs/suite.c` (tri, CRC32, division
  logicielle, `switch` en table de sauts, récursion, multiplications 64 bits,
  pointeurs de fonction, accès 8/16 bits signés, appels ARM ↔ Thumb…) est
  compilé en 8 variantes : ARMv4T/ARMv5TE × ARM/Thumb × -O0/-O2. Les valeurs
  attendues viennent du même code compilé sur la machine hôte (vérifié sous
  UBSan). Pour régénérer : `sh tools/build_programs.sh` (clang + lld).

Les binaires sont versionnés : `cargo test` n'a besoin d'aucun outil LLVM.

## La machine `Nds`

```rust
use avxds_core::Nds;
let mut nds = Nds::new();
let header = nds.direct_boot(std::fs::read("jeu.nds")?)?; // sans BIOS Nintendo
loop { nds.run_frame(); } // jusqu'au début exact du prochain VBlank
```

Une capture des deux écrans (avec, ici, un appui sur A à l'image 60) :

```
cargo run --release --example screenshot -- jeu.nds 900 capture.png A@60
```

Pour jouer, voir le crate voisin `../avxds-frontend`.

Pour savoir ce qui manque à une ROM :

```
cargo run --release --example boot_report -- jeu.nds 120
```

Déjà émulé :

| Partie | Détail |
|---|---|
| Mémoire | Main RAM 4 Mo (miroirs), WRAM partagée selon WRAMCNT, WRAM ARM7 64 Ko, palette, OAM, BIOS en lecture seule |
| CP15 | Registre de contrôle (vecteurs hauts), ITCM/DTCM avec taille virtuelle, base et mode « load », ID |
| VRAM | 9 banques, tous les modes MST/OFS, chevauchements (OU en lecture), C/D pour l'ARM7, VRAMSTAT |
| IRQ | IME/IE/IF séparés pour chaque CPU, réveil du Halt même avec IME = 0 |
| Écran | 263 lignes × 4260 cycles ARM9, HBlank/VBlank, VCOUNT, DISPSTAT par CPU, IRQ VBlank/HBlank/VCount (LYC sur 9 bits) |
| Timers | 4 par CPU, F/1 à F/1024, cascade, IRQ, compteur calculé à la lecture |
| IPC | IPCSYNC, IPCFIFO avec drapeaux, erreurs et IRQ |
| Affichage | Moteurs 2D A et B : modes éteint/graphique/VRAM, fonds texte, affines, étendus et bitmaps, priorités, master brightness, échange des écrans |
| DMA | 4 canaux par CPU : immédiat, VBlank, HBlank, répétition, IRQ ; DMA fill |
| Boot | Direct boot : binaires, en-tête en 0x027FFE00, CP15, piles ; mini-BIOS maison ; SWI en Rust |
| Divers | DIVCNT/SQRTCNT, KEYINPUT/EXTKEYIN (touches relâchées), POWCNT, POSTFLG, HALTCNT |

Les registres I/O pas encore émulés sont notés dans `nds.hw.unhandled_io`
(CPU et adresse) : c'est la liste de ce qu'il reste à faire pour un jeu donné.

## Limites connues

* Timing : `step()` renvoie un nombre de cycles **approximatif**. Les vrais
  timings (attente mémoire, cache de l'ARM9) viendront avec le scheduler.
* Ordonnancement : les CPU avancent par tranches de 64 cycles ARM9 au plus
  (`MAX_SLICE`), donc jusqu'à 64 cycles de décalage entre eux.
* Pas encore : sprites, fenêtres, mélange, 3D, cartouche, SPI (tactile), son, RTC.
* Simplifications : division instantanée (pas de drapeau « occupé »), MPU et
  caches mémorisés sans effet, protection en lecture du BIOS ARM7 absente,
  slot GBA vide lu à 0, HALTCNT ne gère que le mode Halt.

## Prochaines étapes

1. Sprites (OBJ), puis fenêtres et mélange alpha.
2. Commandes de cartouche (lecture de la ROM par le jeu) ; SPI (écran tactile).
3. ROMs de test CPU (armwrestler, rockwrestler) en plus de cette suite.
