# Noos Flasher

**Noos Flasher** est un créateur de clés USB bootables **Linux (ISO hybride / DD)** et **Windows 10/11 UEFI**, développé en **Rust** avec une interface native **GTK4 / Libadwaita**.

Il remplace le frontend Tauri v2 d'origine : le backend d'écriture, d'inspection ISO, de partitionnement et de debloat Windows est conservé, tandis que l'interface est désormais une application GNOME native, sans WebView, sans Node.js et sans IPC JavaScript.

---

## Fonctionnalités

### Périphériques et sécurité
- Détection des clés USB via `lsblk`, `/sys/block` et `/proc/mounts`.
- Exclusion automatique de `/`, `/boot`, `/boot/efi`, `/home`, `/nix`, des disques NVMe/SATA/PCIe internes et des périphériques virtuels.
- Double validation du périphérique cible côté utilisateur **et** dans le worker root lancé par `pkexec`.

### Images disque
- Inspection binaire des signatures **MBR** (`0x55AA`), **GPT** (`EFI PART`), **ISO 9660** (`CD001`), **El Torito** et **UDF** (`NSR02`/`NSR03`).
- Détection des installateurs Windows UEFI (`sources/install.wim`, `sources/boot.wim`, `bootmgr.efi`) et de la limite FAT32 de 4 Gio.
- Mode **DD** pour les ISO Linux hybrides et les images `.img`, avec streaming par blocs de 4 Mio, mesure de vitesse et `sync` matériel.

### Windows 10/11 UEFI
- Partitionnement **GPT** ou **MBR**, formatage **FAT32** ou **NTFS**.
- Découpage automatique de `install.wim` en `.swm` (3800 Mio max) via `wimlib-imagex` **embarqué** dans le binaire, avec ses bibliothèques `libwim`, `libntfs-3g` et `libfuse3`.
- Génération de `/autounattend.xml`, `noos_debloat.ps1` et `noos_firstlogon.ps1`.
- **Debloat Studio** : 22 patchs configurables (Copilot, Recall, Bing, Cortana, Widgets, OneDrive, télémétrie, bloatwares UWP, menu contextuel classique, etc.).
- Compte local administrateur pré-configuré, `BypassNRO`, bypass TPM/Secure Boot/RAM, langue française.

### Interface GTK4 / Libadwaita
- Fenêtre native GNOME avec `AdwHeaderBar`, `AdwPreferencesGroup`, `AdwSwitchRow`, `AdwComboRow`, `AdwBanner`, `AdwToastOverlay` et `AdwMessageDialog`.
- Sélection de l'ISO via `GtkFileDialog`, confirmation destructive native et aperçu des scripts dans un `AdwDialog`.
- Le volet **Personnalisation Windows / Debloat** n'apparaît que lorsqu'une ISO réellement détectée comme installateur Windows est sélectionnée.
- Progression pondérée par le volume total (copie + découpage WIM), vitesse, ETA, indicateur de dernière activité, journal périodique copiable et annulation.
- Aucune dépendance à WebKitGTK, Node.js, npm ou Tauri.

### Dépendances système
Noos Flasher s'appuie sur les outils système suivants, **livrés comme dépendances dans chaque paquet** :

- **`ntfs-3g`** : support NTFS et `mount.ntfs-3g` ;
- **`ntfsprogs`** : `mkfs.ntfs` / `mkntfs`, `ntfsfix` et utilitaires NTFS ;
- `parted`, `dosfstools`, `polkit` (`pkexec`), `util-linux` ;
- `7z` est optionnel (sonde rapide des métadonnées ISO) ;
- `wimtools` n'est pas requis : `wimlib-imagex` est embarqué.

> Sur Debian/Ubuntu, l'ancien paquet `ntfsprogs` a été fusionné dans `ntfs-3g` : ce dernier fournit bien `mkntfs` et `ntfsfix`. Sur Arch Linux et Fedora, `ntfs-3g` et `ntfsprogs` sont deux paquets distincts et sont déclarés séparément.

---

## Installation

### Arch Linux (AUR)

```bash
yay -S noos-flasher
# ou
paru -S noos-flasher
```

Le paquet fournit `/usr/bin/noos-flasher`, le fichier `.desktop` et les icônes hicolor. Il remplace `noos-win-installer`.

### Debian / Ubuntu / Pop!_OS

```bash
sudo apt install parted dosfstools ntfs-3g polkit-1 7zip
# puis installer le .deb fourni dans la release
sudo apt install ./noos-flasher_*.deb
```

### Fedora / openSUSE

```bash
sudo dnf install parted dosfstools ntfs-3g ntfsprogs polkit
sudo dnf install ./noos-flasher-*.rpm
```

### AppImage

L'AppImage est portable mais utilise les outils système de l'hôte (`pkexec`, `parted`, `mkfs.vfat`, `mkfs.ntfs`, `ntfs-3g`). Installez au minimum :

```bash
sudo apt install parted dosfstools ntfs-3g polkit-1
```

---

## Compilation depuis les sources

### Prérequis (Arch Linux)

```bash
sudo pacman -S --needed rust cargo gtk4 libadwaita \
  parted dosfstools ntfs-3g ntfsprogs polkit util-linux 7zip
```

### Prérequis (Debian / Ubuntu)

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  parted dosfstools ntfs-3g polkit-1 7zip
```

### Compilation

```bash
git clone https://github.com/Chomiam/noos-flasher.git
cd noos-flasher
cargo build --release

# Lancer l'interface
./target/release/noos-flasher

# Ouvrir directement une image (le volet Windows apparaît si l'ISO est un
# installateur Windows 10/11)
./target/release/noos-flasher ~/Téléchargements/Win11_24H2.iso
```

### Tests et qualité

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### Worker privilégié

L'application graphique ne tourne jamais en root. Au démarrage du flashage, elle appelle :

```bash
pkexec /usr/bin/noos-flasher --privileged-worker --job-json '<json>'
```

Le worker est le même binaire, sans initialisation GTK. En mode AppImage, il est copié hors du montage FUSE utilisateur (`/tmp/.mount_*`) pour que `pkexec` puisse l'exécuter.

---

## Structure du projet

```text
noos-flasher/
├── Cargo.toml                  # Crate Rust, métadonnées .deb et .rpm
├── assets/
│   └── icons/                  # Icônes hicolor (32 → 512 px)
├── packaging/
│   ├── PKGBUILD                # Paquet Arch Linux / AUR
│   ├── .SRCINFO
│   └── io.github.chomiam.NoosFlasher.desktop
├── resources/
│   └── wimtools/               # wimlib-imagex + bibliothèques embarquées
└── src/
    ├── main.rs                 # Entrée GTK4 + mode --privileged-worker
    ├── backend/
    │   ├── mod.rs
    │   ├── error.rs            # Erreurs typées (thiserror)
    │   ├── drive.rs            # Détection et validation des disques USB
    │   ├── writer.rs           # Inspection ISO, DD, Windows UEFI, pkexec
    │   └── debloat.rs          # autounattend.xml + scripts PowerShell
    └── ui/
        ├── mod.rs              # Application GTK4 / Libadwaita
        ├── window.rs           # Fenêtre, formulaires, progression, dialogues
        └── style.css           # Styles complémentaires
```

---

## Licence

Distribué sous licence **MIT**. Voir [LICENSE](LICENSE).
