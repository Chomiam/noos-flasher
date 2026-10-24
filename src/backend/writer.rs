use crate::debloat::{write_unattend_and_debloat_to_usb, WindowsCustomConfig};
use crate::drive::{format_size, partition_path_for_disk, validate_target_device};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use walkdir::WalkDir;

/// Callback appelé à chaque étape du flashage. Il est exécuté depuis le thread
/// de travail : l'interface graphique relaie les événements vers sa boucle GTK.
pub type ProgressCallback = Arc<dyn Fn(FlashProgressEvent) + Send + Sync>;

/// Taille maximale d'un fichier sur un système de fichiers FAT32 (4 Gio - 1 octet).
pub const FAT32_MAX_FILE_SIZE: u64 = 4_294_967_295;

/// Préfixe de protocole utilisé par le worker privilégié (`pkexec`) sur `stdout`.
const PROGRESS_PREFIX: &str = "PROGRESS_JSON:";

/// Mode d'écriture sur la clé USB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashMode {
    LinuxDd,
    WindowsUefi,
}

impl FlashMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LinuxDd => "linux_dd",
            Self::WindowsUefi => "windows_uefi",
        }
    }

    pub fn from_arg(val: &str) -> AppResult<Self> {
        match val.trim().to_ascii_lowercase().as_str() {
            "linux_dd" | "dd" => Ok(Self::LinuxDd),
            "windows_uefi" | "windows" => Ok(Self::WindowsUefi),
            other => Err(AppError::Other(anyhow::anyhow!(
                "Mode de flashage inconnu : '{other}'"
            ))),
        }
    }
}

/// Schéma de table de partitions pour le mode Windows UEFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionScheme {
    Gpt,
    Mbr,
}

impl PartitionScheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Gpt => "gpt",
            Self::Mbr => "mbr",
        }
    }

    pub fn from_arg(val: &str) -> AppResult<Self> {
        match val.trim().to_ascii_lowercase().as_str() {
            "gpt" => Ok(Self::Gpt),
            "mbr" | "msdos" => Ok(Self::Mbr),
            other => Err(AppError::Other(anyhow::anyhow!(
                "Schéma de partitionnement inconnu : '{other}'"
            ))),
        }
    }
}

/// Système de fichiers cible pour le mode Windows UEFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystemType {
    Fat32,
    Ntfs,
}

impl FileSystemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fat32 => "fat32",
            Self::Ntfs => "ntfs",
        }
    }

    pub fn from_arg(val: &str) -> AppResult<Self> {
        match val.trim().to_ascii_lowercase().as_str() {
            "fat32" | "vfat" => Ok(Self::Fat32),
            "ntfs" => Ok(Self::Ntfs),
            other => Err(AppError::Other(anyhow::anyhow!(
                "Système de fichiers inconnu : '{other}'"
            ))),
        }
    }
}

/// Classification détectée lors de l'analyse d'un fichier `.iso` ou `.img`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsoKind {
    LinuxHybrid,
    WindowsUefi,
    StandardIso,
    RawImage,
}

/// Résultat complet de l'inspection d'une image ISO ou IMG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoInspection {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub kind: IsoKind,
    pub kind_label: String,
    pub volume_label: String,
    pub is_hybrid_mbr: bool,
    pub has_gpt_header: bool,
    pub has_el_torito: bool,
    pub is_udf: bool,
    pub is_windows_iso: bool,
    pub install_wim_size: Option<u64>,
    pub install_wim_formatted: Option<String>,
    pub requires_wim_split_for_fat32: bool,
    pub wimlib_available: bool,
    pub recommended_mode: FlashMode,
    pub recommended_scheme: PartitionScheme,
    pub recommended_fs: FileSystemType,
    pub details: String,
}

/// Paramètres d'une tâche d'écriture USB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashJobConfig {
    pub device_path: String,
    pub iso_path: String,
    pub mode: FlashMode,
    pub partition_scheme: PartitionScheme,
    pub fs_type: FileSystemType,
    pub volume_label: String,
    #[serde(default)]
    pub windows_custom: Option<WindowsCustomConfig>,
}

/// Événement de progression temps réel relayé à l'interface graphique.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashProgressEvent {
    pub stage: String,
    pub percent: f64,
    pub speed_mbps: f64,
    pub bytes_processed: u64,
    pub total_bytes: u64,
    pub log: Option<String>,
    pub completed: bool,
    pub error: Option<String>,
}

/// Résout le chemin absolu d'un binaire système à travers `PATH` et les dossiers système standard (`/usr/sbin`, `/sbin`, `/usr/bin`, `/bin`).
pub fn resolve_command_path(bin: &str) -> PathBuf {
    let p = Path::new(bin);
    if p.is_absolute() && p.is_file() {
        return p.to_path_buf();
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var
        .split(':')
        .filter(|d| !d.is_empty() && !d.starts_with("/tmp/.mount_"))
        .chain([
            "/usr/local/sbin",
            "/usr/local/bin",
            "/usr/sbin",
            "/sbin",
            "/usr/bin",
            "/bin",
        ])
    {
        let candidate = Path::new(dir).join(bin);
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(bin)
}

/// Vérifie la présence d'un exécutable dans le `PATH` ou dans `/usr/sbin` / `/sbin`.
pub fn has_command(bin: &str) -> bool {
    let resolved = resolve_command_path(bin);
    resolved.is_absolute() && resolved.is_file()
}

/// Vérifie qu'un outil système requis est présent avant de toucher au disque.
fn ensure_command(bin: &str) -> AppResult<()> {
    if has_command(bin) {
        Ok(())
    } else {
        Err(AppError::CommandFailed {
            cmd: bin.to_string(),
            details: format!(
                "L'outil requis '{bin}' est introuvable. Installez les dépendances du paquet noos-flasher (parted, dosfstools, ntfs-3g/ntfsprogs, util-linux)."
            ),
        })
    }
}

/// Binaires `wimtools` (`wimlib-imagex` + `libwim.so.15` + `libntfs-3g.so.89` + `libfuse3.so.3`) embarqués directement
/// dans l'exécutable Rust afin qu'aucun utilisateur Linux n'ait besoin d'installer `wimtools` ou `libfuse3.so.3` manuellement.
const EMBEDDED_WIMLIB_BIN: &[u8] = include_bytes!("../../resources/wimtools/wimlib-imagex");
const EMBEDDED_LIBWIM_SO: &[u8] = include_bytes!("../../resources/wimtools/libwim.so.15");
const EMBEDDED_LIBNTFS_SO: &[u8] = include_bytes!("../../resources/wimtools/libntfs-3g.so.89");
const EMBEDDED_LIBFUSE3_SO: &[u8] = include_bytes!("../../resources/wimtools/libfuse3.so.3");

/// Extrait ou localise le binaire `wimlib-imagex` embarqué et ses bibliothèques partagées,
/// et retourne `(chemin_wimlib_imagex, dossier_ld_library_path)`.
/// Utilise un dossier isolé par UID (`/tmp/noos_embedded_wimtools_<uid>`) pour éviter tout conflit
/// de permissions entre le processus graphique utilisateur et le worker privilégié `root` (`pkexec`).
pub fn ensure_embedded_wimtools() -> AppResult<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let uid = nix::unistd::geteuid().as_raw();
    let runtime_dir = PathBuf::from(format!("/tmp/noos_embedded_wimtools_{uid}"));
    fs::create_dir_all(&runtime_dir)?;

    let bin_path = runtime_dir.join("wimlib-imagex");
    let libwim_path = runtime_dir.join("libwim.so.15");
    let libntfs_path = runtime_dir.join("libntfs-3g.so.89");
    let libfuse3_path = runtime_dir.join("libfuse3.so.3");

    let needs_extract = !bin_path.exists()
        || fs::metadata(&bin_path)
            .map(|m| m.len() != EMBEDDED_WIMLIB_BIN.len() as u64)
            .unwrap_or(true);

    if needs_extract {
        fs::write(&bin_path, EMBEDDED_WIMLIB_BIN)?;
        let mut perms = fs::metadata(&bin_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin_path, perms)?;
    }

    for (path, bytes) in [
        (&libwim_path, EMBEDDED_LIBWIM_SO),
        (&libntfs_path, EMBEDDED_LIBNTFS_SO),
        (&libfuse3_path, EMBEDDED_LIBFUSE3_SO),
    ] {
        let needs_lib = !path.exists()
            || fs::metadata(path)
                .map(|m| m.len() != bytes.len() as u64)
                .unwrap_or(true);
        if needs_lib {
            fs::write(path, bytes)?;
        }
    }

    Ok((bin_path, runtime_dir))
}

/// Formate une taille en Gio pour les messages de progression compacts.
fn format_gib(bytes: u64) -> String {
    format!("{:.2} Gio", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Interpole un pourcentage dans une plage donnée.
fn progress_percent(processed: u64, total: u64, start: f64, end: f64) -> f64 {
    start + (end - start) * (processed as f64 / total.max(1) as f64).clamp(0.0, 1.0)
}

/// Additionne la taille de tous les volumes `.swm` produits par `wimlib-imagex split`.
fn sum_swm_parts(swm_dst: &str) -> u64 {
    let path = Path::new(swm_dst);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("swm");

    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(stem) && name.ends_with(ext) {
                total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Exécute `wimlib-imagex split` en surveillant la taille des fichiers `.swm`
/// produits pour émettre une progression réelle pendant le découpage.
#[allow(clippy::too_many_arguments)]
fn run_wimsplit_with_progress(
    mut command: Command,
    wim_src_str: &str,
    swm_dst_str: &str,
    wim_size: u64,
    total_iso_bytes: u64,
    base_bytes: u64,
    start_percent: f64,
    end_percent: f64,
    label: &str,
) -> AppResult<()> {
    let mut child = command
        .args(["split", wim_src_str, swm_dst_str, "3800"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let stderr = child.stderr.take().ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "Impossible de capturer stderr de wimlib-imagex split"
        ))
    })?;
    let stderr_reader = thread::spawn(move || {
        let mut reader = stderr;
        let mut output = String::new();
        let _ = reader.read_to_string(&mut output);
        output
    });

    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_log = Instant::now();
    let mut written = 0u64;

    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                written = written.max(sum_swm_parts(swm_dst_str));
                let now = Instant::now();
                if now.duration_since(last_emit) >= Duration::from_millis(350) {
                    let elapsed = started.elapsed().as_secs_f64().max(0.001);
                    let percent = progress_percent(written, wim_size, start_percent, end_percent);
                    let speed_mbps = (written as f64 / (1024.0 * 1024.0)) / elapsed;

                    let log = (now.duration_since(last_log) >= Duration::from_secs(2)).then(|| {
                        format!(
                            "{label} : {:.1}% — {} / {} — {:.0} Mo/s",
                            percent,
                            format_gib(written),
                            format_gib(wim_size),
                            speed_mbps,
                        )
                    });
                    if log.is_some() {
                        last_log = now;
                    }

                    emit_worker_event(&FlashProgressEvent {
                        stage: "Découpage de install.wim via wimlib (FAT32)…".to_string(),
                        percent,
                        speed_mbps,
                        bytes_processed: base_bytes.saturating_add(written),
                        total_bytes: total_iso_bytes,
                        log,
                        completed: false,
                        error: None,
                    });
                    last_emit = now;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }
    };

    let stderr_output = stderr_reader.join().unwrap_or_default();

    if status.success() {
        Ok(())
    } else {
        let details = stderr_output.trim().to_string();
        Err(AppError::CommandFailed {
            cmd: format!("{label} split"),
            details: if details.is_empty() {
                format!(
                    "wimlib-imagex s'est terminé avec le code {:?}",
                    status.code()
                )
            } else {
                details
            },
        })
    }
}

/// Exécute `wimlib-imagex split` en utilisant prioritairement la version embarquée autonome,
/// avec repli automatique sur `wimlib-imagex` système s'il est installé.
#[allow(clippy::too_many_arguments)]
fn run_embedded_wimsplit(
    wim_src_str: &str,
    swm_dst_str: &str,
    wim_size: u64,
    total_iso_bytes: u64,
    base_bytes: u64,
    start_percent: f64,
    end_percent: f64,
) -> AppResult<()> {
    let (bin_path, lib_dir) = ensure_embedded_wimtools()?;
    let mut command = Command::new(&bin_path);
    command.env("LD_LIBRARY_PATH", &lib_dir);

    match run_wimsplit_with_progress(
        command,
        wim_src_str,
        swm_dst_str,
        wim_size,
        total_iso_bytes,
        base_bytes,
        start_percent,
        end_percent,
        "wimlib-imagex",
    ) {
        Ok(()) => Ok(()),
        Err(embedded_error) => {
            if has_command("wimlib-imagex") {
                let sys_bin = resolve_command_path("wimlib-imagex");
                let mut command = Command::new(sys_bin);
                command.env_remove("LD_LIBRARY_PATH");
                if run_wimsplit_with_progress(
                    command,
                    wim_src_str,
                    swm_dst_str,
                    wim_size,
                    total_iso_bytes,
                    base_bytes,
                    start_percent,
                    end_percent,
                    "wimlib-imagex (système)",
                )
                .is_ok()
                {
                    return Ok(());
                }
            }
            Err(embedded_error)
        }
    }
}

/// Retourne toujours un binaire de découpage WIM valide (embarqué dans l'application ou système).
fn find_wimsplit_binary() -> Option<&'static str> {
    Some("wimlib-imagex (embarqué)")
}

/// Nettoie et normalise un label de volume pour FAT32 / NTFS (max 11 caractères ASCII majuscules).
pub fn sanitize_volume_label(raw: &str, default_label: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .map(|c| c.to_ascii_uppercase())
        .take(11)
        .collect();

    if cleaned.is_empty() {
        default_label.to_string()
    } else {
        cleaned
    }
}

/// Inspecte les métadonnées internes d'une image ISO via `7z l -slt` sur les fichiers clés Windows/UEFI.
fn probe_iso_entries_with_7z(iso_path: &str) -> (bool, Option<u64>, bool) {
    if !has_command("7z") {
        return (false, None, false);
    }

    let Ok(output) = Command::new(resolve_command_path("7z"))
        .env_remove("LD_LIBRARY_PATH")
        .args([
            "l",
            "-slt",
            iso_path,
            "sources/install.wim",
            "sources/install.esd",
            "sources/boot.wim",
            "sources\\install.wim",
            "sources\\install.esd",
            "sources\\boot.wim",
            "bootmgr.efi",
            "efi/boot/bootx64.efi",
        ])
        .output()
    else {
        return (false, None, false);
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut is_windows = false;
    let mut install_wim_size: Option<u64> = None;
    let mut has_efi_boot = false;

    let mut current_path = String::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(p) = trimmed.strip_prefix("Path = ") {
            current_path = p.trim().replace('\\', "/").to_ascii_lowercase();
            if current_path == "sources/install.wim"
                || current_path == "sources/install.esd"
                || current_path == "sources/boot.wim"
                || current_path == "bootmgr.efi"
            {
                is_windows = true;
            }
            if current_path == "efi/boot/bootx64.efi" || current_path == "bootmgr.efi" {
                has_efi_boot = true;
            }
        } else if let Some(s) = trimmed.strip_prefix("Size = ") {
            if current_path == "sources/install.wim" {
                if let Ok(sz) = s.trim().parse::<u64>() {
                    install_wim_size = Some(sz);
                }
            }
        }
    }

    (is_windows, install_wim_size, has_efi_boot)
}

/// Inspecte un fichier `.iso` ou `.img` pour détecter son format (Linux Hybrid ISO vs Windows UEFI ISO).
pub fn inspect_iso_file(iso_path: &str) -> AppResult<IsoInspection> {
    let trimmed_path = iso_path.trim();
    let path_obj = Path::new(trimmed_path);

    if !path_obj.exists() || !path_obj.is_file() {
        return Err(AppError::InvalidIso(format!(
            "Le fichier '{}' est introuvable.",
            trimmed_path
        )));
    }

    let file_name = path_obj
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image.iso")
        .to_string();

    let ext = path_obj
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext != "iso" && ext != "img" {
        return Err(AppError::InvalidIso(format!(
            "Extension '.{ext}' non supportée. Veuillez sélectionner un fichier .iso ou .img."
        )));
    }

    let metadata = fs::metadata(path_obj)?;
    let size_bytes = metadata.len();
    if size_bytes < 1024 * 1024 {
        return Err(AppError::InvalidIso(
            "Le fichier image est trop petit (< 1 Mio) ou corrompu.".to_string(),
        ));
    }

    let mut file = File::open(path_obj)?;

    // 1. Lecture du secteur 0 (MBR) et secteur 1 (GPT Header)
    let mut head_buf = [0u8; 1024];
    let head_read = file.read(&mut head_buf)?;
    let has_mbr_sig = head_read >= 512 && head_buf[510] == 0x55 && head_buf[511] == 0xAA;
    let has_part_entries = head_read >= 512 && head_buf[446..510].iter().any(|&b| b != 0);
    let is_hybrid_mbr = has_mbr_sig && has_part_entries;
    let has_gpt_header = head_read >= 520 && &head_buf[512..520] == b"EFI PART";

    // 2. Lecture des descripteurs ISO 9660 / El Torito / UDF aux secteurs 16..20 (offset 0x8000)
    let mut is_iso9660 = false;
    let mut is_udf = false;
    let mut has_el_torito = false;
    let mut raw_volume_label = String::new();

    if size_bytes >= 0xA000 && file.seek(SeekFrom::Start(0x8000)).is_ok() {
        let mut pvd_buf = [0u8; 8192];
        if file.read_exact(&mut pvd_buf).is_ok() {
            for sector in pvd_buf.chunks_exact(2048) {
                let ident = &sector[1..6];
                if ident == b"CD001" {
                    is_iso9660 = true;
                    if sector[0] == 1 && raw_volume_label.is_empty() {
                        raw_volume_label =
                            String::from_utf8_lossy(&sector[40..72]).trim().to_string();
                    } else if sector[0] == 0
                        && sector[7..30].starts_with(b"EL TORITO SPECIFICATION")
                    {
                        has_el_torito = true;
                    }
                } else if ident == b"BEA01" || ident == b"NSR02" || ident == b"NSR03" {
                    is_udf = true;
                }
            }
        }
    }

    // 3. Inspection rapide de la structure interne Windows/UEFI via 7z + heuristiques de nom/label
    let (probed_win, install_wim_size, _has_efi) = probe_iso_entries_with_7z(trimmed_path);

    let lower_name = file_name.to_ascii_lowercase();
    let lower_label = raw_volume_label.to_ascii_lowercase();
    let heuristic_win = lower_name.contains("win10")
        || lower_name.contains("win11")
        || lower_name.contains("windows")
        || lower_label.contains("cccoma")
        || lower_label.contains("esd-iso")
        || lower_label.contains("win10")
        || lower_label.contains("win11");

    let is_windows_iso = probed_win || (is_udf && heuristic_win && !is_hybrid_mbr);
    let wimlib_available = find_wimsplit_binary().is_some();
    let requires_wim_split_for_fat32 = install_wim_size
        .map(|sz| sz > FAT32_MAX_FILE_SIZE)
        .unwrap_or(is_windows_iso && size_bytes > 4_600_000_000);

    let (kind, kind_label, recommended_mode, default_label) = if ext == "img" {
        (
            IsoKind::RawImage,
            "Image Disque Brute (.img)".to_string(),
            FlashMode::LinuxDd,
            "NOOS_IMG",
        )
    } else if is_windows_iso {
        (
            IsoKind::WindowsUefi,
            "ISO Windows UEFI (UDF / Boot Manager)".to_string(),
            FlashMode::WindowsUefi,
            "WIN_USB",
        )
    } else if is_hybrid_mbr || has_gpt_header {
        (
            IsoKind::LinuxHybrid,
            "ISO Linux Hybride (isohybrid MBR/GPT)".to_string(),
            FlashMode::LinuxDd,
            "LINUX_USB",
        )
    } else if is_iso9660 || is_udf {
        (
            IsoKind::StandardIso,
            "Image ISO 9660 / UDF Standard".to_string(),
            FlashMode::LinuxDd,
            "BOOT_USB",
        )
    } else {
        return Err(AppError::InvalidIso(
            "Signature ISO 9660, UDF ou MBR introuvable dans l'image sélectionnée.".to_string(),
        ));
    };

    let volume_label = sanitize_volume_label(&raw_volume_label, default_label);
    let install_wim_formatted = install_wim_size.map(format_size);

    // Si install.wim > 4 Gio et que wimlib est présent, on peut utiliser FAT32 (avec split wimlib)
    // ou NTFS directement. Si wimlib est absent, on recommande NTFS par défaut.
    let recommended_fs = if is_windows_iso && requires_wim_split_for_fat32 && !wimlib_available {
        FileSystemType::Ntfs
    } else {
        FileSystemType::Fat32
    };

    let details = match kind {
        IsoKind::WindowsUefi => {
            if let Some(ref wim_fmt) = install_wim_formatted {
                if requires_wim_split_for_fat32 {
                    format!(
                        "Installateur Windows détecté (install.wim : {wim_fmt} > 4 Gio). En FAT32, découpage automatique via wimlib ; en NTFS, copie directe."
                    )
                } else {
                    format!("Installateur Windows détecté (install.wim : {wim_fmt} < 4 Gio). Compatible FAT32 et NTFS.")
                }
            } else {
                "Installateur Windows UEFI détecté. Partitionnement GPT/MBR et extraction des fichiers d'amorçage.".to_string()
            }
        }
        IsoKind::LinuxHybrid => {
            "Image ISO Linux hybride dotée d'une table de partition amorçable. Écriture directe par blocs (mode DD) recommandée.".to_string()
        }
        IsoKind::RawImage => {
            "Image disque brute (.img). Écriture directe bloc à bloc (mode DD) sur le périphérique cible.".to_string()
        }
        IsoKind::StandardIso => {
            "Image ISO 9660 standard détectée. Écriture directe (mode DD) sélectionnée.".to_string()
        }
    };

    Ok(IsoInspection {
        path: trimmed_path.to_string(),
        file_name,
        size_bytes,
        size_formatted: format_size(size_bytes),
        kind,
        kind_label,
        volume_label,
        is_hybrid_mbr,
        has_gpt_header,
        has_el_torito,
        is_udf,
        is_windows_iso,
        install_wim_size,
        install_wim_formatted,
        requires_wim_split_for_fat32,
        wimlib_available,
        recommended_mode,
        recommended_scheme: PartitionScheme::Gpt,
        recommended_fs,
        details,
    })
}

/// Garde RAII nettoyant automatiquement le binaire worker temporaire extrait hors du montage FUSE AppImage.
struct StagedWorkerGuard {
    path: Option<PathBuf>,
}

impl Drop for StagedWorkerGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.path {
            let _ = fs::remove_file(p);
        }
    }
}

/// Prépare la ligne de commande du worker privilégié pour `pkexec`.
///
/// Lorsqu'il est exécuté depuis une `.AppImage`, `std::env::current_exe()` pointe vers
/// `/tmp/.mount_Noos-FXXXXXX/usr/bin/noos-flasher`, un système de fichiers FUSE monté par
/// l'utilisateur courant auquel `root` (`pkexec`, `euid = 0`) n'a pas accès (`EACCES: Permission denied`).
/// Cette fonction copie alors l'exécutable dans `/tmp` (ou `/var/tmp` si `/tmp` est monté en `noexec`),
/// vérifie son exécution autonome via `--privileged-worker-check` et replie sur `--appimage-extract-and-run`
/// si les bibliothèques système hôtes sont absentes.
fn prepare_privileged_worker_invocation(
    current_exe: &Path,
    job_json: &str,
) -> AppResult<(Vec<String>, StagedWorkerGuard)> {
    use std::os::unix::fs::PermissionsExt;

    let exe_str = current_exe.to_str().ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "Le chemin de l'exécutable contient des caractères UTF-8 invalides"
        ))
    })?;

    let is_fuse_or_appimage = exe_str.starts_with("/tmp/.mount_")
        || std::env::var_os("APPDIR").is_some()
        || std::env::var_os("APPIMAGE").is_some()
        || !current_exe.exists()
        || (!exe_str.starts_with("/usr/")
            && !exe_str.starts_with("/opt/")
            && !exe_str.starts_with("/bin/")
            && !exe_str.starts_with("/sbin/"));

    if !is_fuse_or_appimage {
        return Ok((
            vec![
                exe_str.to_string(),
                "--privileged-worker".to_string(),
                "--job-json".to_string(),
                job_json.to_string(),
            ],
            StagedWorkerGuard { path: None },
        ));
    }

    // Si le binaire a été remplacé ou supprimé pendant que l'application
    // tournait (mise à jour, rebuild en développement), le chemin
    // `current_exe` n'existe plus. `/proc/self/exe` pointe toujours vers
    // l'inode du processus courant et permet de stager le worker.
    let proc_exe = Path::new("/proc/self/exe");
    let copy_source = if current_exe.exists() {
        current_exe
    } else if proc_exe.exists() {
        proc_exe
    } else {
        current_exe
    };

    let pid = std::process::id();
    for base_dir in ["/tmp", "/var/tmp"] {
        let staged_path = PathBuf::from(format!("{base_dir}/noos_privileged_worker_{pid}"));
        if fs::copy(copy_source, &staged_path).is_ok() {
            if let Ok(meta) = fs::metadata(&staged_path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&staged_path, perms);
            }

            let check_ok = Command::new(&staged_path)
                .arg("--privileged-worker-check")
                .current_dir("/tmp")
                .env_remove("LD_LIBRARY_PATH")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if check_ok {
                if let Some(staged_str) = staged_path.to_str() {
                    return Ok((
                        vec![
                            staged_str.to_string(),
                            "--privileged-worker".to_string(),
                            "--job-json".to_string(),
                            job_json.to_string(),
                        ],
                        StagedWorkerGuard {
                            path: Some(staged_path),
                        },
                    ));
                }
            }
            let _ = fs::remove_file(&staged_path);
        }
    }

    if let Ok(appimage_path) = std::env::var("APPIMAGE") {
        let trimmed = appimage_path.trim();
        if !trimmed.is_empty() && Path::new(trimmed).is_file() {
            return Ok((
                vec![
                    trimmed.to_string(),
                    "--appimage-extract-and-run".to_string(),
                    "--privileged-worker".to_string(),
                    "--job-json".to_string(),
                    job_json.to_string(),
                ],
                StagedWorkerGuard { path: None },
            ));
        }
    }

    Ok((
        vec![
            exe_str.to_string(),
            "--privileged-worker".to_string(),
            "--job-json".to_string(),
            job_json.to_string(),
        ],
        StagedWorkerGuard { path: None },
    ))
}

/// Lance le processus de flashage via `pkexec` et relaie la progression temps réel
/// à l'interface graphique au moyen du callback `progress`.
pub async fn execute_flash_with_pkexec(
    config: FlashJobConfig,
    cancel_flag: Arc<AtomicBool>,
    progress: ProgressCallback,
) -> AppResult<()> {
    // 1. Validation de sécurité côté utilisateur avant même d'appeler pkexec
    let validated_drive = validate_target_device(&config.device_path)?;
    let inspected_iso = inspect_iso_file(&config.iso_path)?;

    if inspected_iso.size_bytes > validated_drive.size_bytes {
        return Err(AppError::InvalidIso(format!(
            "L'image ({}) est plus volumineuse que la clé USB sélectionnée ({}).",
            inspected_iso.size_formatted, validated_drive.size_formatted
        )));
    }

    if config.mode == FlashMode::WindowsUefi
        && config.fs_type == FileSystemType::Fat32
        && inspected_iso.requires_wim_split_for_fat32
        && !inspected_iso.wimlib_available
    {
        return Err(AppError::WimSplitRequired(
            "Le fichier install.wim dépasse la limite FAT32 de 4 Gio et l'outil 'wimlib-imagex' (paquet wimtools) n'est pas installé sur ce système. Sélectionnez le format NTFS ou installez 'sudo apt install wimtools'.".to_string(),
        ));
    }

    let current_exe = std::env::current_exe()?;

    let safe_label = sanitize_volume_label(&config.volume_label, "NOOS_USB");
    let normalized_job = FlashJobConfig {
        device_path: validated_drive.path.clone(),
        iso_path: inspected_iso.path.clone(),
        mode: config.mode,
        partition_scheme: config.partition_scheme,
        fs_type: config.fs_type,
        volume_label: safe_label.clone(),
        windows_custom: config.windows_custom.clone(),
    };
    let job_json = serde_json::to_string(&normalized_job)?;

    let (pkexec_args, _staged_worker_guard) =
        prepare_privileged_worker_invocation(&current_exe, &job_json)?;

    progress(FlashProgressEvent {
        stage: "Demande d'élévation de privilèges PolicyKit (pkexec)...".to_string(),
        percent: 0.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: inspected_iso.size_bytes,
        log: Some(format!(
            "Authentification Polkit requise pour écrire sur {} ({} {})",
            validated_drive.path, validated_drive.vendor, validated_drive.model
        )),
        completed: false,
        error: None,
    });

    // 2. Exécution sécurisée sans sous-shell (zéro bash -c) via pkexec vers notre propre binaire en mode worker.
    // On fixe le dossier de travail sur /tmp et on purge les variables d'environnement AppImage/FUSE
    // pour éviter tout refus d'accès root (`EACCES`) sur `/tmp/.mount_*`.
    let pkexec_bin = resolve_command_path("pkexec");
    let mut child = tokio::process::Command::new(pkexec_bin)
        .current_dir("/tmp")
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("GIO_MODULE_DIR")
        .env_remove("GSETTINGS_SCHEMA_DIR")
        .env_remove("GI_TYPELIB_PATH")
        .env_remove("GTK_PATH")
        .env_remove("GTK_EXE_PREFIX")
        .env_remove("GTK_DATA_PREFIX")
        .env_remove("GDK_PIXBUF_MODULE_FILE")
        .env_remove("GTK_IM_MODULE_FILE")
        .args(&pkexec_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            AppError::PrivilegeElevationDenied(format!("Impossible de lancer pkexec : {e}"))
        })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "Impossible de capturer le flux stdout du worker privilégié"
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "Impossible de capturer le flux stderr du worker privilégié"
        ))
    })?;

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut last_error_msg = String::new();

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            let _ = child.kill().await;
            return Err(AppError::Cancelled);
        }

        tokio::select! {
            line_res = stdout_lines.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        if let Some(json_str) = line.strip_prefix(PROGRESS_PREFIX) {
                            if let Ok(event) = serde_json::from_str::<FlashProgressEvent>(json_str) {
                                if let Some(ref err) = event.error {
                                    last_error_msg = err.clone();
                                }
                                progress(event);
                            }
                        } else if !line.trim().is_empty() {
                            progress(FlashProgressEvent {
                                stage: "Opération en cours...".to_string(),
                                percent: 0.0,
                                speed_mbps: 0.0,
                                bytes_processed: 0,
                                total_bytes: inspected_iso.size_bytes,
                                log: Some(line),
                                completed: false,
                                error: None,
                            });
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        last_error_msg = format!("Erreur de lecture du flux worker : {e}");
                        break;
                    }
                }
            }
            err_res = stderr_lines.next_line() => {
                if let Ok(Some(err_line)) = err_res {
                    let trimmed = err_line.trim();
                    if !trimmed.is_empty() {
                        last_error_msg = trimmed.to_string();
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = child.kill().await;
                    return Err(AppError::Cancelled);
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        if code == 126 {
            return Err(AppError::PrivilegeElevationDenied(
                "Authentification PolicyKit annulée ou refusée.".to_string(),
            ));
        }
        if code == 127 {
            let detail = if last_error_msg.is_empty()
                || last_error_msg.contains("Request dismissed")
                || last_error_msg.contains("Not authorized")
            {
                "Authentification PolicyKit annulée ou refusée.".to_string()
            } else {
                format!("Authentification PolicyKit impossible ({last_error_msg}).")
            };
            return Err(AppError::PrivilegeElevationDenied(detail));
        }
        let reason = if last_error_msg.is_empty() {
            format!("Le worker privilégié s'est terminé avec le code {code}")
        } else {
            last_error_msg
        };
        return Err(AppError::CommandFailed {
            cmd: "privileged-worker".to_string(),
            details: reason,
        });
    }

    Ok(())
}

// ============================================================================
// LOGIQUE DU WORKER PRIVILÉGIÉ (EXÉCUTÉ EN ROOT VIA PKEXEC)
// ============================================================================

/// Émet un événement JSON structuré sur `stdout` à destination du processus parent.
fn emit_worker_event(event: &FlashProgressEvent) {
    if let Ok(serialized) = serde_json::to_string(event) {
        println!("{PROGRESS_PREFIX}{serialized}");
        let _ = std::io::stdout().flush();
    }
}

/// Point d'entrée du mode `--privileged-worker` invoqué par `pkexec`.
pub fn run_privileged_worker_from_args(args: &[String]) -> i32 {
    match parse_and_run_worker(args) {
        Ok(()) => 0,
        Err(err) => {
            emit_worker_event(&FlashProgressEvent {
                stage: "Erreur critique".to_string(),
                percent: 0.0,
                speed_mbps: 0.0,
                bytes_processed: 0,
                total_bytes: 0,
                log: Some(format!("ERREUR : {err}")),
                completed: false,
                error: Some(err.to_string()),
            });
            eprintln!("{err}");
            1
        }
    }
}

fn parse_and_run_worker(args: &[String]) -> AppResult<()> {
    let mut job_json_opt: Option<String> = None;
    let mut device_path = None;
    let mut iso_path = None;
    let mut mode = FlashMode::LinuxDd;
    let mut partition_scheme = PartitionScheme::Gpt;
    let mut fs_type = FileSystemType::Fat32;
    let mut volume_label = "NOOS_USB".to_string();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--job-json" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    job_json_opt = Some(v.clone());
                }
            }
            "--device" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    device_path = Some(v.clone());
                }
            }
            "--iso" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    iso_path = Some(v.clone());
                }
            }
            "--mode" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    mode = FlashMode::from_arg(v)?;
                }
            }
            "--scheme" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    partition_scheme = PartitionScheme::from_arg(v)?;
                }
            }
            "--fs" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    fs_type = FileSystemType::from_arg(v)?;
                }
            }
            "--label" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    volume_label = sanitize_volume_label(v, "NOOS_USB");
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mut config = if let Some(json_str) = job_json_opt {
        serde_json::from_str::<FlashJobConfig>(&json_str)?
    } else {
        let dev = device_path.ok_or_else(|| {
            AppError::Other(anyhow::anyhow!(
                "Argument --device manquant pour le worker privilégié"
            ))
        })?;
        let iso = iso_path.ok_or_else(|| {
            AppError::Other(anyhow::anyhow!(
                "Argument --iso manquant pour le worker privilégié"
            ))
        })?;
        FlashJobConfig {
            device_path: dev,
            iso_path: iso,
            mode,
            partition_scheme,
            fs_type,
            volume_label,
            windows_custom: None,
        }
    };

    // Double vérification de sécurité du périphérique cible dans le processus root
    let drive = validate_target_device(&config.device_path)?;
    config.device_path = drive.path.clone();
    config.volume_label = sanitize_volume_label(&config.volume_label, "NOOS_USB");

    match config.mode {
        FlashMode::LinuxDd => run_linux_dd_flash(&config, &drive.partitions),
        FlashMode::WindowsUefi => run_windows_uefi_flash(&config, &drive.partitions),
    }
}

/// Démonte proprement toutes les partitions montées d'un périphérique USB.
fn unmount_all_device_partitions(
    device_path: &str,
    partitions: &[crate::drive::DrivePartition],
) -> AppResult<()> {
    emit_worker_event(&FlashProgressEvent {
        stage: "Démontage des partitions existantes...".to_string(),
        percent: 1.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: 0,
        log: Some(format!(
            "Vérification et démontage des volumes montés sur {device_path}..."
        )),
        completed: false,
        error: None,
    });

    let umount_bin = resolve_command_path("umount");
    for part in partitions {
        if part.mountpoint.is_some() {
            let _ = Command::new(&umount_bin)
                .env_remove("LD_LIBRARY_PATH")
                .arg(&part.path)
                .status();
            let _ = Command::new(&umount_bin)
                .env_remove("LD_LIBRARY_PATH")
                .args(["-l", &part.path])
                .status();
        }
    }

    // Démontage éventuel du périphérique bloc racine s'il était monté en direct (ex: iso9660)
    let _ = Command::new(&umount_bin)
        .env_remove("LD_LIBRARY_PATH")
        .arg(device_path)
        .status();
    Ok(())
}

/// Écriture brute bloc par bloc (mode `dd`) avec calcul de vitesse en temps réel et `sync`.
fn run_linux_dd_flash(
    config: &FlashJobConfig,
    partitions: &[crate::drive::DrivePartition],
) -> AppResult<()> {
    ensure_command("umount")?;
    unmount_all_device_partitions(&config.device_path, partitions)?;

    let mut src_file = File::open(&config.iso_path)?;
    let total_bytes = src_file.metadata()?.len();

    emit_worker_event(&FlashProgressEvent {
        stage: "Ouverture du périphérique bloc en écriture directe...".to_string(),
        percent: 2.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes,
        log: Some(format!(
            "Début du flashage DD de '{}' ({}) vers '{}'",
            config.iso_path,
            format_size(total_bytes),
            config.device_path
        )),
        completed: false,
        error: None,
    });

    let mut dst_device = OpenOptions::new().write(true).open(&config.device_path)?;

    // Tampon de streaming de 4 Mio
    const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    const SYNC_INTERVAL_BYTES: u64 = 32 * 1024 * 1024;

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut bytes_written: u64 = 0;
    let mut bytes_since_last_sync: u64 = 0;
    let start_time = Instant::now();
    let mut last_emit = Instant::now();
    let mut last_log = Instant::now();

    loop {
        let read_count = src_file.read(&mut buffer)?;
        if read_count == 0 {
            break;
        }

        dst_device.write_all(&buffer[..read_count])?;
        bytes_written += read_count as u64;
        bytes_since_last_sync += read_count as u64;

        // Synchronisation périodique pour éviter l'accumulation dans le cache RAM du noyau
        // et fournir une mesure de vitesse matérielle réelle.
        if bytes_since_last_sync >= SYNC_INTERVAL_BYTES {
            dst_device.sync_data()?;
            bytes_since_last_sync = 0;
        }

        let now = Instant::now();
        if now.duration_since(last_emit) >= Duration::from_millis(120)
            || bytes_written == total_bytes
        {
            let elapsed_secs = start_time.elapsed().as_secs_f64().max(0.001);
            let speed_mbps = (bytes_written as f64 / (1024.0 * 1024.0)) / elapsed_secs;
            let raw_ratio = (bytes_written as f64 / total_bytes.max(1) as f64).clamp(0.0, 1.0);
            // Plage 2% -> 95% pour l'écriture, 95% -> 100% pour le sync final
            let percent = 2.0 + raw_ratio * 93.0;

            let log = (now.duration_since(last_log) >= Duration::from_secs(2)).then(|| {
                format!(
                    "DD : {:.1}% — {} / {} — {:.0} Mo/s",
                    percent,
                    format_gib(bytes_written),
                    format_gib(total_bytes),
                    speed_mbps,
                )
            });
            if log.is_some() {
                last_log = now;
            }

            emit_worker_event(&FlashProgressEvent {
                stage: "Écriture brute de l'image ISO (mode DD)...".to_string(),
                percent,
                speed_mbps,
                bytes_processed: bytes_written,
                total_bytes,
                log,
                completed: false,
                error: None,
            });
            last_emit = now;
        }
    }

    emit_worker_event(&FlashProgressEvent {
        stage: "Synchronisation matérielle des caches (sync)...".to_string(),
        percent: 96.0,
        speed_mbps: 0.0,
        bytes_processed: bytes_written,
        total_bytes,
        log: Some("Vidage des tampons d'écriture du noyau Linux (fsync + sync)...".to_string()),
        completed: false,
        error: None,
    });

    dst_device.sync_all()?;
    drop(dst_device);
    nix::unistd::sync();

    let _ = Command::new(resolve_command_path("partprobe"))
        .env_remove("LD_LIBRARY_PATH")
        .arg(&config.device_path)
        .status();
    let _ = Command::new(resolve_command_path("udevadm"))
        .env_remove("LD_LIBRARY_PATH")
        .arg("settle")
        .status();

    let total_elapsed = start_time.elapsed().as_secs_f64().max(0.001);
    let avg_speed = (bytes_written as f64 / (1024.0 * 1024.0)) / total_elapsed;

    emit_worker_event(&FlashProgressEvent {
        stage: "Clé USB bootable créée avec succès !".to_string(),
        percent: 100.0,
        speed_mbps: avg_speed,
        bytes_processed: bytes_written,
        total_bytes,
        log: Some(format!(
            "Flashage DD terminé avec succès : {} écrits à {:.1} Mo/s de moyenne.",
            format_size(bytes_written),
            avg_speed
        )),
        completed: true,
        error: None,
    });

    Ok(())
}

/// Garde RAII assurant le démontage et la suppression des dossiers temporaires en fin d'opération.
struct MountCleanupGuard {
    usb_mnt: PathBuf,
    iso_mnt: PathBuf,
}

impl Drop for MountCleanupGuard {
    fn drop(&mut self) {
        let umount_bin = resolve_command_path("umount");
        let _ = Command::new(&umount_bin)
            .env_remove("LD_LIBRARY_PATH")
            .arg(&self.usb_mnt)
            .status();
        let _ = Command::new(&umount_bin)
            .env_remove("LD_LIBRARY_PATH")
            .args(["-l", &self.usb_mnt.to_string_lossy()])
            .status();
        let _ = Command::new(&umount_bin)
            .env_remove("LD_LIBRARY_PATH")
            .arg(&self.iso_mnt)
            .status();
        let _ = Command::new(&umount_bin)
            .env_remove("LD_LIBRARY_PATH")
            .args(["-l", &self.iso_mnt.to_string_lossy()])
            .status();
        let _ = fs::remove_dir(&self.usb_mnt);
        let _ = fs::remove_dir(&self.iso_mnt);
    }
}

/// Exécute une commande système avec arguments vectorisés et retourne une erreur typée en cas d'échec.
fn run_checked_command(bin: &str, args: &[&str]) -> AppResult<()> {
    let output = Command::new(resolve_command_path(bin))
        .env_remove("LD_LIBRARY_PATH")
        .args(args)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let details = if !stderr.is_empty() { stderr } else { stdout };
        return Err(AppError::CommandFailed {
            cmd: format!("{bin} {}", args.join(" ")),
            details,
        });
    }
    Ok(())
}

/// Préparation complète d'une clé USB Windows UEFI :
/// Partitionnement GPT/MBR -> Formatage FAT32/NTFS -> Extraction -> Découpage `install.wim` (si FAT32 > 4 Gio).
fn run_windows_uefi_flash(
    config: &FlashJobConfig,
    partitions: &[crate::drive::DrivePartition],
) -> AppResult<()> {
    // Préflight : on refuse de commencer à effacer la clé si un outil
    // nécessaire au partitionnement/formatage est absent.
    ensure_command("parted")?;
    ensure_command("mount")?;
    ensure_command("umount")?;
    match config.fs_type {
        FileSystemType::Fat32 => ensure_command("mkfs.vfat")?,
        FileSystemType::Ntfs => {
            if !has_command("mkfs.ntfs") && !has_command("mkntfs") {
                return Err(AppError::CommandFailed {
                    cmd: "mkfs.ntfs / mkntfs".to_string(),
                    details: "Les outils NTFS de ntfsprogs sont introuvables.".to_string(),
                });
            }
        }
    }

    let total_iso_bytes = fs::metadata(&config.iso_path)?.len();

    // 1. Démontage et nettoyage des signatures existantes
    unmount_all_device_partitions(&config.device_path, partitions)?;

    emit_worker_event(&FlashProgressEvent {
        stage: "Effacement des signatures de partition existantes...".to_string(),
        percent: 3.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: total_iso_bytes,
        log: Some(format!("Exécution de wipefs -a sur {}", config.device_path)),
        completed: false,
        error: None,
    });

    if has_command("wipefs") {
        let _ = run_checked_command("wipefs", &["-a", &config.device_path]);
    }

    // 2. Partitionnement GPT ou MBR via parted
    let scheme_label = match config.partition_scheme {
        PartitionScheme::Gpt => "gpt",
        PartitionScheme::Mbr => "msdos",
    };
    let parted_fs = match config.fs_type {
        FileSystemType::Fat32 => "fat32",
        FileSystemType::Ntfs => "ntfs",
    };

    emit_worker_event(&FlashProgressEvent {
        stage: format!(
            "Partitionnement {} ({})...",
            config.partition_scheme.as_str().to_uppercase(),
            config.fs_type.as_str().to_uppercase()
        ),
        percent: 6.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: total_iso_bytes,
        log: Some(format!(
            "Création de la table de partitions {} et de la partition principale sur {}...",
            scheme_label.to_uppercase(),
            config.device_path
        )),
        completed: false,
        error: None,
    });

    run_checked_command(
        "parted",
        &["-s", &config.device_path, "mklabel", scheme_label],
    )?;
    run_checked_command(
        "parted",
        &[
            "-s",
            &config.device_path,
            "mkpart",
            "primary",
            parted_fs,
            "1MiB",
            "100%",
        ],
    )?;

    match config.partition_scheme {
        PartitionScheme::Gpt => {
            // Sur une clé USB Windows UEFI en GPT, la partition principale (même en FAT32)
            // DOIT avoir le GUID `msftdata` (Basic Data Partition : EBD0A0A2-B9E5-4433-87C0-68B6B72699C7)
            // et NON `esp` (C12A7328-F81F-11D2-BA4B-00A0C93EC93B).
            // Sous Windows PE (boot.wim), `mountmgr.sys` marque toute partition `esp` comme "Masqué"
            // sans lettre de lecteur (C:/D:), empêchant setup.exe de trouver `sources\install.swm`
            // et provoquant l'erreur "Un pilote multimédia dont votre ordinateur a besoin est manquant".
            let _ = run_checked_command(
                "parted",
                &["-s", &config.device_path, "set", "1", "esp", "off"],
            );
            let _ = run_checked_command(
                "parted",
                &["-s", &config.device_path, "set", "1", "msftdata", "on"],
            );
            if has_command("sfdisk") {
                let _ = run_checked_command(
                    "sfdisk",
                    &[
                        "--part-type",
                        &config.device_path,
                        "1",
                        "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
                    ],
                );
            }
        }
        PartitionScheme::Mbr => {
            let _ = run_checked_command(
                "parted",
                &["-s", &config.device_path, "set", "1", "boot", "on"],
            );
        }
    }

    let _ = Command::new(resolve_command_path("partprobe"))
        .env_remove("LD_LIBRARY_PATH")
        .arg(&config.device_path)
        .status();
    let _ = Command::new(resolve_command_path("udevadm"))
        .env_remove("LD_LIBRARY_PATH")
        .arg("settle")
        .status();

    let part_path = partition_path_for_disk(&config.device_path, 1);
    for _ in 0..25 {
        if Path::new(&part_path).exists() {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    if !Path::new(&part_path).exists() {
        return Err(AppError::DeviceNotFound(format!(
            "La partition nouvellement créée '{part_path}' n'est pas apparue dans /dev."
        )));
    }

    // 3. Formatage FAT32 ou NTFS
    emit_worker_event(&FlashProgressEvent {
        stage: format!(
            "Formatage de {} en {}...",
            part_path,
            config.fs_type.as_str().to_uppercase()
        ),
        percent: 12.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: total_iso_bytes,
        log: Some(format!(
            "Formatage du système de fichiers {} (Label: {}) sur {}...",
            config.fs_type.as_str().to_uppercase(),
            config.volume_label,
            part_path
        )),
        completed: false,
        error: None,
    });

    match config.fs_type {
        FileSystemType::Fat32 => {
            run_checked_command(
                "mkfs.vfat",
                &["-F", "32", "-n", &config.volume_label, &part_path],
            )?;
        }
        FileSystemType::Ntfs => {
            // `mkfs.ntfs` est fourni par ntfsprogs ; certains systèmes
            // n'exposent que `mkntfs` (même paquet, symlink absent).
            let mkfs_ntfs = if has_command("mkfs.ntfs") {
                "mkfs.ntfs"
            } else {
                "mkntfs"
            };
            run_checked_command(mkfs_ntfs, &["-Q", "-L", &config.volume_label, &part_path])?;
        }
    }

    // 4. Montage de l'ISO et de la partition USB
    let pid = std::process::id();
    let iso_mnt = PathBuf::from(format!("/tmp/noos_win_iso_{pid}"));
    let usb_mnt = PathBuf::from(format!("/tmp/noos_win_usb_{pid}"));
    fs::create_dir_all(&iso_mnt)?;
    fs::create_dir_all(&usb_mnt)?;

    let _cleanup_guard = MountCleanupGuard {
        usb_mnt: usb_mnt.clone(),
        iso_mnt: iso_mnt.clone(),
    };

    let iso_mnt_str = iso_mnt
        .to_str()
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("Chemin temporaire ISO invalide")))?;
    let usb_mnt_str = usb_mnt
        .to_str()
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("Chemin temporaire USB invalide")))?;

    emit_worker_event(&FlashProgressEvent {
        stage: "Montage de l'image ISO et du volume USB...".to_string(),
        percent: 16.0,
        speed_mbps: 0.0,
        bytes_processed: 0,
        total_bytes: total_iso_bytes,
        log: Some(format!(
            "Montage loop de l'ISO sur {iso_mnt_str} et de {part_path} sur {usb_mnt_str}"
        )),
        completed: false,
        error: None,
    });

    run_checked_command("mount", &["-o", "loop,ro", &config.iso_path, iso_mnt_str])?;
    run_checked_command("mount", &[&part_path, usb_mnt_str])?;

    // Pré-création explicite de /EFI/BOOT en majuscules sur FAT32/NTFS pour une compatibilité UEFI universelle
    let efi_boot_dir = usb_mnt.join("EFI").join("BOOT");
    fs::create_dir_all(&efi_boot_dir)?;

    // 5. Inspection des fichiers à copier et détection d'un éventuel install.wim > 4 Gio en FAT32
    let mut files_to_copy: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut split_wim_source: Option<(PathBuf, u64)> = None;
    let mut total_copy_bytes: u64 = 0;

    for entry_res in WalkDir::new(&iso_mnt).follow_links(false) {
        let entry =
            entry_res.map_err(|e| AppError::Other(anyhow::anyhow!("Erreur WalkDir: {e}")))?;
        let src_path = entry.path();
        let rel_path = src_path
            .strip_prefix(&iso_mnt)
            .map_err(|e| AppError::Other(anyhow::anyhow!("Erreur strip_prefix: {e}")))?;

        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let rel_lower = rel_path
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        let dst_path = if rel_lower == "efi/boot/bootx64.efi" {
            efi_boot_dir.join("BOOTX64.EFI")
        } else if rel_lower == "efi/boot/bootia32.efi" {
            efi_boot_dir.join("BOOTIA32.EFI")
        } else if rel_lower == "efi/boot/bootaa64.efi" {
            efi_boot_dir.join("BOOTAA64.EFI")
        } else {
            usb_mnt.join(rel_path)
        };

        if entry.file_type().is_dir() {
            fs::create_dir_all(&dst_path)?;
        } else if entry.file_type().is_file() {
            let file_len = entry.metadata().map(|m| m.len()).unwrap_or(0);

            if config.fs_type == FileSystemType::Fat32
                && rel_lower == "sources/install.wim"
                && file_len > FAT32_MAX_FILE_SIZE
            {
                split_wim_source = Some((src_path.to_path_buf(), file_len));
            } else {
                total_copy_bytes += file_len;
                files_to_copy.push((src_path.to_path_buf(), dst_path, file_len));
            }
        }
    }

    if split_wim_source.is_some() && find_wimsplit_binary().is_none() {
        return Err(AppError::WimSplitRequired(
            "install.wim dépasse 4 Gio sur une partition FAT32 et 'wimlib-imagex' est introuvable. Installez 'wimtools' ou choisissez NTFS.".to_string(),
        ));
    }

    // 6. Copie streamée des fichiers de l'ISO avec vitesse Mo/s et progression temps réel.
    // La progression est pondérée par le volume total à traiter (copie + découpage WIM)
    // afin d'éviter un saut brutal à 82 % avant les longues minutes de wimlib.
    let split_wim_size = split_wim_source
        .as_ref()
        .map(|(_, size)| *size)
        .unwrap_or(0);
    let total_work_bytes = total_copy_bytes.saturating_add(split_wim_size);
    let copy_percent = |processed: u64| progress_percent(processed, total_work_bytes, 18.0, 92.0);

    let mut copied_bytes: u64 = 0;
    let start_copy = Instant::now();
    let mut last_emit = Instant::now();
    let mut last_log = Instant::now();
    let mut copy_buf = vec![0u8; 1024 * 1024]; // 1 Mio

    for (src_file_path, dst_file_path, _file_len) in &files_to_copy {
        if let Some(parent) = dst_file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let rel_display = src_file_path
            .strip_prefix(&iso_mnt)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| src_file_path.to_string_lossy().to_string());

        let is_notable_file = {
            let low = rel_display.to_ascii_lowercase();
            low.ends_with("boot.wim")
                || low.ends_with("install.wim")
                || low.ends_with("install.esd")
                || low.ends_with("bootx64.efi")
                || low.ends_with("bootmgr.efi")
        };

        if is_notable_file {
            let elapsed = start_copy.elapsed().as_secs_f64().max(0.001);
            let speed = (copied_bytes as f64 / (1024.0 * 1024.0)) / elapsed;
            emit_worker_event(&FlashProgressEvent {
                stage: format!("Extraction : {rel_display}"),
                percent: copy_percent(copied_bytes),
                speed_mbps: speed,
                bytes_processed: copied_bytes,
                total_bytes: total_iso_bytes,
                log: Some(format!(
                    "Copie du fichier système : {rel_display} ({:.1}%)",
                    copy_percent(copied_bytes)
                )),
                completed: false,
                error: None,
            });
        }

        let mut reader = File::open(src_file_path)?;
        let mut writer = File::create(dst_file_path)?;

        loop {
            let n = reader.read(&mut copy_buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&copy_buf[..n])?;
            copied_bytes += n as u64;

            let now = Instant::now();
            if now.duration_since(last_emit) >= Duration::from_millis(150) {
                let elapsed = start_copy.elapsed().as_secs_f64().max(0.001);
                let speed_mbps = (copied_bytes as f64 / (1024.0 * 1024.0)) / elapsed;
                let percent = copy_percent(copied_bytes);

                let log = (now.duration_since(last_log) >= Duration::from_secs(2)).then(|| {
                    format!(
                        "Copie : {:.1}% — {} / {} — {:.0} Mo/s",
                        percent,
                        format_gib(copied_bytes),
                        format_gib(total_work_bytes),
                        speed_mbps,
                    )
                });
                if log.is_some() {
                    last_log = now;
                }

                emit_worker_event(&FlashProgressEvent {
                    stage: format!("Copie des fichiers Windows ({rel_display})..."),
                    percent,
                    speed_mbps,
                    bytes_processed: copied_bytes,
                    total_bytes: total_iso_bytes,
                    log,
                    completed: false,
                    error: None,
                });
                last_emit = now;
            }
        }
    }

    // 7. Découpage de install.wim via wimlib si FAT32 et fichier > 4 Gio
    if let Some((wim_src_path, wim_size)) = split_wim_source {
        let wim_bin = find_wimsplit_binary().ok_or_else(|| {
            AppError::WimSplitRequired(
                "Binaire 'wimlib-imagex' ou 'wimsplit' introuvable.".to_string(),
            )
        })?;

        let swm_dst_path = usb_mnt.join("sources").join("install.swm");
        if let Some(parent) = swm_dst_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let split_start_percent = copy_percent(copied_bytes);
        emit_worker_event(&FlashProgressEvent {
            stage: "Découpage de install.wim (> 4 Gio) via wimlib pour FAT32...".to_string(),
            percent: split_start_percent,
            speed_mbps: 0.0,
            bytes_processed: copied_bytes,
            total_bytes: total_iso_bytes,
            log: Some(format!(
                "Découpage de {} ({}) en volumes .swm de 3800 Mio max via {wim_bin}...",
                wim_src_path.display(),
                format_size(wim_size)
            )),
            completed: false,
            error: None,
        });

        let wim_src_str = wim_src_path
            .to_str()
            .ok_or_else(|| AppError::Other(anyhow::anyhow!("Chemin install.wim invalide")))?;
        let swm_dst_str = swm_dst_path
            .to_str()
            .ok_or_else(|| AppError::Other(anyhow::anyhow!("Chemin install.swm invalide")))?;

        run_embedded_wimsplit(
            wim_src_str,
            swm_dst_str,
            wim_size,
            total_iso_bytes,
            copied_bytes,
            split_start_percent,
            92.0,
        )?;

        copied_bytes += wim_size;

        emit_worker_event(&FlashProgressEvent {
            stage: "Découpage de install.wim terminé.".to_string(),
            percent: 92.0,
            speed_mbps: 0.0,
            bytes_processed: copied_bytes,
            total_bytes: total_iso_bytes,
            log: Some(format!(
                "Découpage terminé : {} de volumes .swm générés.",
                format_gib(wim_size)
            )),
            completed: false,
            error: None,
        });
    }

    // 7a. Vérification de secours : s'assurer que /EFI/BOOT/BOOTX64.EFI est bien présent
    let bootx64_dst = efi_boot_dir.join("BOOTX64.EFI");
    if !bootx64_dst.exists() {
        for fallback in ["bootmgfw.efi", "bootmgr.efi"] {
            let candidate = usb_mnt.join(fallback);
            if candidate.is_file() {
                let _ = fs::copy(&candidate, &bootx64_dst);
                break;
            }
        }
    }

    // 7b. Déploiement de autounattend.xml (Compte local Windows) et des scripts PowerShell Debloat ($OEM$)
    if let Some(ref win_custom) = config.windows_custom {
        if win_custom.enabled {
            emit_worker_event(&FlashProgressEvent {
                stage: "Injection d'autounattend.xml et des patchs Debloat Windows...".to_string(),
                percent: 94.0,
                speed_mbps: 0.0,
                bytes_processed: copied_bytes,
                total_bytes: total_iso_bytes,
                log: Some(format!(
                    "Génération de /autounattend.xml (Compte local : '{}', BypassNRO : {}) et de {} fonctions Debloat dans sources/$OEM$/$$/Setup/Scripts/noos_debloat.ps1",
                    win_custom.local_username,
                    win_custom.force_local_account,
                    win_custom.enabled_features.len()
                )),
                completed: false,
                error: None,
            });

            write_unattend_and_debloat_to_usb(&usb_mnt, win_custom)?;
        }
    }

    // 8. Synchronisation finale et démontage propre AVANT l'émission de l'événement 100%
    emit_worker_event(&FlashProgressEvent {
        stage: "Synchronisation des caches et démontage sécurisé...".to_string(),
        percent: 96.0,
        speed_mbps: 0.0,
        bytes_processed: copied_bytes,
        total_bytes: total_iso_bytes,
        log: Some(
            "Synchronisation des écritures et démontage propre de la clé USB (sync + umount)..."
                .to_string(),
        ),
        completed: false,
        error: None,
    });

    nix::unistd::sync();
    let umount_bin = resolve_command_path("umount");
    let _ = Command::new(&umount_bin)
        .env_remove("LD_LIBRARY_PATH")
        .arg(&usb_mnt)
        .status();
    let _ = Command::new(&umount_bin)
        .env_remove("LD_LIBRARY_PATH")
        .arg(&iso_mnt)
        .status();
    nix::unistd::sync();
    let _ = Command::new(resolve_command_path("partprobe"))
        .env_remove("LD_LIBRARY_PATH")
        .arg(&config.device_path)
        .status();
    let _ = Command::new(resolve_command_path("udevadm"))
        .env_remove("LD_LIBRARY_PATH")
        .arg("settle")
        .status();

    let elapsed_total = start_copy.elapsed().as_secs_f64().max(0.001);
    let avg_speed = (copied_bytes as f64 / (1024.0 * 1024.0)) / elapsed_total;

    emit_worker_event(&FlashProgressEvent {
        stage: "Clé USB Windows UEFI créée avec succès !".to_string(),
        percent: 100.0,
        speed_mbps: avg_speed,
        bytes_processed: total_iso_bytes,
        total_bytes: total_iso_bytes,
        log: Some(format!(
            "Clé USB Windows UEFI ({}/{}) prête à démarrer ({:.1} Mo/s).",
            config.partition_scheme.as_str().to_uppercase(),
            config.fs_type.as_str().to_uppercase(),
            avg_speed
        )),
        completed: true,
        error: None,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_resolve_command_path_finds_system_binaries() {
        let lsblk = resolve_command_path("lsblk");
        assert!(lsblk.is_absolute());
        assert!(lsblk.is_file());
    }

    #[test]
    fn test_ensure_embedded_wimtools_per_uid() {
        let (bin_path, lib_dir) = ensure_embedded_wimtools().expect("wimtools extraction");
        assert!(bin_path.is_file());
        assert!(lib_dir.is_dir());
        assert!(lib_dir.join("libwim.so.15").is_file());
        assert!(lib_dir.join("libntfs-3g.so.89").is_file());
        assert!(lib_dir.join("libfuse3.so.3").is_file());
        let uid = nix::unistd::geteuid().as_raw();
        assert!(lib_dir
            .to_string_lossy()
            .ends_with(&format!("noos_embedded_wimtools_{uid}")));

        let output = Command::new(&bin_path)
            .env("LD_LIBRARY_PATH", &lib_dir)
            .arg("--version")
            .output()
            .expect("execution wimlib-imagex --version");
        assert!(
            output.status.success(),
            "wimlib-imagex --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_ensure_command_detects_present_and_missing_binaries() {
        assert!(ensure_command("lsblk").is_ok());
        assert!(ensure_command("noos-flasher-command-that-does-not-exist").is_err());
    }

    #[test]
    fn test_progress_percent_is_linear_and_clamped() {
        assert_eq!(progress_percent(0, 1000, 18.0, 92.0), 18.0);
        assert_eq!(progress_percent(500, 1000, 18.0, 92.0), 55.0);
        assert_eq!(progress_percent(1000, 1000, 18.0, 92.0), 92.0);
        assert_eq!(progress_percent(2000, 1000, 18.0, 92.0), 92.0);
        assert_eq!(progress_percent(0, 0, 18.0, 92.0), 18.0);
    }

    #[test]
    fn test_sum_swm_parts_tracks_split_outputs() {
        let dir =
            std::env::temp_dir().join(format!("noos_flasher_swm_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("création dossier swm");
        fs::write(dir.join("install.swm"), vec![0u8; 1000]).expect("écriture swm 1");
        fs::write(dir.join("install2.swm"), vec![0u8; 2000]).expect("écriture swm 2");
        fs::write(dir.join("unrelated.swm"), vec![0u8; 5000]).expect("écriture swm autre");
        fs::write(dir.join("install.wim"), vec![0u8; 7000]).expect("écriture wim");

        let target = dir.join("install.swm");
        assert_eq!(sum_swm_parts(target.to_str().expect("chemin UTF-8")), 3000);
        fs::remove_dir_all(&dir).expect("nettoyage dossier swm");
    }

    #[test]
    fn test_sanitize_volume_label_filters_and_truncates() {
        assert_eq!(sanitize_volume_label("my usb!", "NOOS_USB"), "MYUSB");
        assert_eq!(
            sanitize_volume_label("abcdefghijklmnop", "NOOS_USB"),
            "ABCDEFGHIJK"
        );
        assert_eq!(sanitize_volume_label("", "NOOS_USB"), "NOOS_USB");
        assert_eq!(sanitize_volume_label("éàç", "NOOS_USB"), "NOOS_USB");
    }

    #[test]
    fn test_inspect_raw_img_detection() {
        let path = std::env::temp_dir().join(format!(
            "noos_flasher_raw_image_test_{}.img",
            std::process::id()
        ));
        let mut data = vec![0u8; 2 * 1024 * 1024];
        // Signature MBR + une entrée de partition non vide.
        data[510] = 0x55;
        data[511] = 0xAA;
        data[446] = 0x80;
        fs::write(&path, &data).expect("écriture image de test");

        let inspection =
            inspect_iso_file(path.to_str().expect("chemin UTF-8")).expect("inspection image brute");
        assert_eq!(inspection.kind, IsoKind::RawImage);
        assert_eq!(inspection.recommended_mode, FlashMode::LinuxDd);
        assert_eq!(inspection.recommended_fs, FileSystemType::Fat32);
        assert!(inspection.size_bytes >= 2 * 1024 * 1024);

        fs::remove_file(&path).expect("nettoyage image de test");
    }

    #[test]
    #[ignore = "nécessite NOOS_TEST_ISO=/chemin/vers/image.iso"]
    fn test_inspect_iso_from_env() {
        let Ok(path) = std::env::var("NOOS_TEST_ISO") else {
            eprintln!("NOOS_TEST_ISO non défini, test ignoré");
            return;
        };
        let inspection = inspect_iso_file(&path).expect("inspection de l'image réelle");
        println!("{inspection:#?}");
        assert!(!inspection.file_name.is_empty());
        assert!(inspection.size_bytes > 1024 * 1024);
    }

    #[test]
    fn test_inspect_iso_rejects_missing_and_tiny_files() {
        let missing = std::env::temp_dir().join("noos_flasher_missing_test.iso");
        let _ = fs::remove_file(&missing);
        assert!(inspect_iso_file(missing.to_str().expect("chemin UTF-8")).is_err());

        let tiny =
            std::env::temp_dir().join(format!("noos_flasher_tiny_test_{}.iso", std::process::id()));
        fs::write(&tiny, b"not an iso").expect("écriture fichier de test");
        assert!(inspect_iso_file(tiny.to_str().expect("chemin UTF-8")).is_err());
        fs::remove_file(&tiny).expect("nettoyage fichier de test");
    }

    #[test]
    fn test_prepare_privileged_worker_stages_outside_fuse_and_cleans_up() {
        let fake_fuse_dir = PathBuf::from(format!("/tmp/noos_test_fuse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&fake_fuse_dir);
        let _ = fs::remove_file(format!(
            "/tmp/noos_privileged_worker_{}",
            std::process::id()
        ));
        fs::create_dir_all(&fake_fuse_dir).unwrap();
        let fake_exe = fake_fuse_dir.join("fake-worker.sh");
        fs::write(&fake_exe, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&fake_exe).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_exe, perms).unwrap();

        let (args, guard) =
            prepare_privileged_worker_invocation(&fake_exe, r#"{"test":true}"#).unwrap();
        assert_eq!(args.len(), 4);
        assert_eq!(args[1], "--privileged-worker");
        assert_eq!(args[2], "--job-json");
        assert_eq!(args[3], r#"{"test":true}"#);

        let staged_path = PathBuf::from(&args[0]);
        assert!(staged_path
            .to_string_lossy()
            .starts_with("/tmp/noos_privileged_worker_"));
        assert!(staged_path.is_file());

        drop(guard);
        assert!(!staged_path.exists());
        let _ = fs::remove_dir_all(&fake_fuse_dir);
    }
}
