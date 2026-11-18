# avxds-frontend

Fenêtre de l'émulateur AvxDS (les deux écrans l'un au-dessus de l'autre).

```
cargo run --release -- ..\mandel.nds
cargo run --release -- --firmware ..\firmware.bin ..\mandel.nds
```

La sauvegarde est `jeu.sav`, à côté de la ROM : écrite une seconde après
la dernière écriture du jeu et en quittant (Échap). Avant d'écraser un
fichier existant, AvxDS en garde une copie `jeu.sav.bak`. Le type de puce est
détecté tout seul ; `--save-type flash512k` (ou eeprom512, eeprom8k,
eeprom64k, fram32k, eeprom128k, flash256k, flash1m, flash8m) l'impose.

`--firmware` (facultatif) : dump de 256 Ko du firmware de sa console ; les
réglages utilisateur et le calibrage du tactile en viennent. Sans lui, AvxDS
utilise un firmware de remplacement.

Son : sortie audio par défaut du système. Le son de la DS (≈ 32 728 Hz) est
rééchantillonné à la fréquence de la carte son, avec une réserve de 60 ms.
L'image donne la cadence et le son suit. Le titre de la fenêtre signale
« sans son », « son coupé » ou le nombre de coupures (famines du tampon).

| Touche clavier | Touche DS |
|---|---|
| Flèches | Croix |
| X / Z | A / B |
| S / A | X / Y |
| Q / W | L / R |
| Entrée / Retour arrière | Start / Select |
| Tab (maintenu) | Accéléré (sans son) |
| M | Couper / rétablir le son |
| F11 | Diagnostic 3D (`diag_N_*`) |
| F12 | Capture PNG (`capture_N.png`) |
| Clic gauche sur l'écran du bas | Stylet |
| Échap | Quitter |

Dépend de `../avxds-core`, de winit 0.28 / pixels 0.13 /
winit_input_helper 0.14 (les versions de l'ancien frontend) et de cpal 0.18
pour le son.
