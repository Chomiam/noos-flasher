# Noos Flasher (`noos_win_installer`)

Outil desktop moderne, rapide et sécurisé de création de clés USB bootables **Linux (ISO Hybride / DD)** et **Windows 10/11 UEFI**, développé en **Rust + Tauri v2** pour Pop!_OS / Linux.

## Architecture du Projet

```text
noos_win_installer/
├── package.json
├── ui/                          # Frontend Vanilla ES6+ (Catppuccin Mocha)
│   ├── index.html               # Interface 2 colonnes + modale de confirmation
│   ├── styles.css               # Design système sombre réactif
│   └── app.js                   # Client IPC Tauri v2 & suivi temps réel
└── src-tauri/                   # Backend natif Rust (Tauri v2)
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/
    │   └── default.json         # Permissions Tauri v2 (core, event, dialog)
    └── src/
        ├── main.rs              # Point d'entrée binaire
        ├── lib.rs               # Initialisation Tauri & interception --privileged-worker
        ├── error.rs             # Gestion d'erreurs typée (thiserror + anyhow)
        ├── drive.rs             # Détection USB (lsblk + sysfs + /proc/mounts) & exclusion /
        ├── writer.rs            # Inspection ISO, flash DD, Windows UEFI, wimlib & pkexec
        └── commands.rs          # Commandes Tauri asynchrones (spawn_blocking)
```

## Fonctionnalités Clés

1. **Isolation stricte des disques système (`drive.rs`)** :
   - Double vérification via `lsblk --json`, `/sys/block/<dev>/removable` et `/proc/mounts`.
   - Exclusion inconditionnelle de `/`, `/boot`, `/boot/efi`, `/recovery`, `/home`, `/nix`, `[SWAP]` et des bus internes (`nvme`, `sata`, `pcie`).
2. **Inspection intelligente des images `.iso` / `.img` (`writer.rs`)** :
   - Analyse binaire des en-têtes MBR (`0x55AA`), GPT (`EFI PART`), ISO 9660 (`CD001`), El Torito et UDF (`NSR02`/`NSR03`).
   - Détection des installateurs Windows UEFI (`sources/install.wim`, `sources/boot.wim`, `bootmgr.efi`) et vérification de la taille de `install.wim` vis-à-vis de la limite FAT32 (`4 Gio - 1 octet`).
3. **Deux moteurs d'écriture avec progression temps réel (`tauri::Emitter`)** :
   - **Mode DD (Linux Hybride / `.img`)** : Streaming direct par blocs de 4 Mio avec `sync_data()` périodique (mesure réelle de la vitesse en Mo/s) et `sync_all()` + `sync` final.
   - **Mode Windows UEFI** : Nettoyage `wipefs`, partitionnement `GPT` ou `MBR` (`parted`), formatage `FAT32` (`mkfs.vfat`) ou `NTFS` (`mkfs.ntfs`), extraction streamée des fichiers et découpage automatique de `sources/install.wim` en fichiers `.swm` (3800 Mio max) via `wimlib-imagex split` lorsque FAT32 est sélectionné et que `install.wim > 4 Gio`.
4. **Élévation de privilèges PolicyKit (`pkexec`) sans lancer l'UI en root (compatible `.deb`, `.rpm`, `.AppImage` et `AUR`)** :
   - L'interface graphique Tauri s'exécute avec les droits utilisateur standard.
   - Au démarrage du flash, le backend invoque `pkexec <binaire> --privileged-worker ...` avec des arguments vectorisés (aucun appel `bash -c` ou `sh -c`) et récupère les événements JSON en temps réel sur `stdout`.
   - En mode **`.AppImage`**, le binaire worker est automatiquement extrait et vérifié hors du point de montage FUSE utilisateur (`/tmp/.mount_*`) afin de permettre son exécution par `root` (`pkexec`) sans erreur `EACCES`.

## Installation sur Arch Linux (AUR)

Le paquet officiel **[`noos-win-installer`](https://aur.archlinux.org/packages/noos-win-installer)** (alias `noos-flasher`) est disponible sur l'**Arch User Repository (AUR)** :

```bash
# Avec yay
yay -S noos-win-installer

# Ou avec paru
paru -S noos-win-installer

# Ou manuellement via makepkg
git clone https://aur.archlinux.org/noos-win-installer.git
cd noos-win-installer
makepkg -si
```

## Prérequis Système (Pop!_OS / Ubuntu / Debian)

```bash
sudo apt install parted dosfstools ntfs-3g p7zip-full wimtools policykit-1
```

## Développement & Exécution

```bash
# Vérification syntaxique et tests unitaires
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
node -c ui/app.js

# Lancement en mode développement Tauri v2
cargo tauri dev
```
