# AvxDS

Émulateur Nintendo DS écrit en Rust. Ca dépôt représente plus de 2 ans et demi de travail et de prise de tête.

| Dossier | Contenu |
|---|---|
| [`avxds-core`](avxds-core) | Le cœur : CPU ARM9/ARM7, mémoire, affichage 2D et 3D, son, cartouche, sauvegarde. Sans dépendance. |
| [`avxds-frontend`](avxds-frontend) | La fenêtre : affichage des deux écrans, clavier, stylet, son, sauvegardes. |

```
cd avxds-frontend
cargo run --release -- chemin\vers\jeu.nds
```

Aucune ROM, aucun BIOS ni firmware n'est fourni : le dépôt ne contient que
les ROMs de test écrites pour le projet (`avxds-core/tests/roms`).

## Licence

GNU General Public License, version 2 ou ultérieure (GPL-2.0-or-later) :
voir [`LICENSE`](LICENSE).
