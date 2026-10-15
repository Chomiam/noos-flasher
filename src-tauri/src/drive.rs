use crate::error::{AppError, AppResult};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Points de montage système critiques : tout disque hébergeant l'un de ces points
/// (directement ou via une partition/LVM/LUKS enfant) est inconditionnellement banni.
const CRITICAL_MOUNT_PREFIXES: &[&str] = &[
    "/",
    "/boot",
    "/boot/efi",
    "/efi",
    "/recovery",
    "/home",
    "/root",
    "/usr",
    "/var",
    "/etc",
    "/opt",
    "/srv",
    "/nix",
    "[SWAP]",
];

/// Représente une partition existante sur le périphérique amovible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrivePartition {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub mountpoint: Option<String>,
}

/// Représente un périphérique bloc amovible sécurisé éligible au flashage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovableDrive {
    pub name: String,
    pub path: String,
    pub model: String,
    pub vendor: String,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub transport: String,
    pub removable: bool,
    pub hotplug: bool,
    pub read_only: bool,
    pub mountpoints: Vec<String>,
    pub mountpoint_display: String,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub partitions: Vec<DrivePartition>,
}

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    #[serde(default)]
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Debug, Clone, Deserialize)]
struct LsblkDevice {
    name: String,
    kname: Option<String>,
    path: Option<String>,
    model: Option<String>,
    vendor: Option<String>,
    #[serde(default, deserialize_with = "deserialize_u64_flexible")]
    size: u64,
    #[serde(default, deserialize_with = "deserialize_bool_flexible")]
    rm: bool,
    #[serde(default, deserialize_with = "deserialize_bool_flexible")]
    hotplug: bool,
    #[serde(rename = "type")]
    dev_type: Option<String>,
    tran: Option<String>,
    mountpoint: Option<String>,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_bool_flexible")]
    ro: bool,
    fstype: Option<String>,
    label: Option<String>,
    #[serde(default)]
    children: Vec<LsblkDevice>,
}

fn deserialize_u64_flexible<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum U64OrString {
        U64(u64),
        Str(String),
        Null,
    }
    match U64OrString::deserialize(deserializer)? {
        U64OrString::U64(v) => Ok(v),
        U64OrString::Str(s) => Ok(s.trim().parse::<u64>().unwrap_or(0)),
        U64OrString::Null => Ok(0),
    }
}

fn deserialize_bool_flexible<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrIntOrStr {
        Bool(bool),
        Int(u64),
        Str(String),
        Null,
    }
    match BoolOrIntOrStr::deserialize(deserializer)? {
        BoolOrIntOrStr::Bool(b) => Ok(b),
        BoolOrIntOrStr::Int(i) => Ok(i != 0),
        BoolOrIntOrStr::Str(s) => {
            let trimmed = s.trim().to_ascii_lowercase();
            Ok(trimmed == "1" || trimmed == "true" || trimmed == "yes")
        }
        BoolOrIntOrStr::Null => Ok(false),
    }
}

/// Formate une taille en octets en chaîne lisible (Go / Mo + GB décimal).
pub fn format_size(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GB_DEC: f64 = 1_000_000_000.0;

    if bytes >= 1024 * 1024 * 1024 {
        let gib = bytes as f64 / GIB;
        let gb = bytes as f64 / GB_DEC;
        format!("{gib:.1} Gio ({gb:.1} Go)")
    } else {
        let mib = bytes as f64 / MIB;
        format!("{mib:.1} Mio")
    }
}

/// Collecte récursivement tous les points de montage d'un disque et de ses partitions/volumes enfants.
fn collect_all_mountpoints(dev: &LsblkDevice, out: &mut Vec<String>) {
    if let Some(mp) = &dev.mountpoint {
        let trimmed = mp.trim();
        if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    for mp in dev.mountpoints.iter().flatten() {
        let trimmed = mp.trim();
        if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    for child in &dev.children {
        collect_all_mountpoints(child, out);
    }
}

/// Vérifie si un point de montage correspond à une partition système critique.
pub fn is_critical_mountpoint(mp: &str) -> bool {
    let normalized = mp.trim();
    if normalized.is_empty() {
        return false;
    }
    if CRITICAL_MOUNT_PREFIXES.contains(&normalized) {
        return true;
    }
    // Exclure également tout sous-dossier système critique (/boot/*, /nix/*, /usr/*, /var/*)
    // mais autoriser les montages utilisateur amovibles (/media/*, /run/media/*, /mnt/*)
    if normalized.starts_with("/media/")
        || normalized.starts_with("/run/media/")
        || normalized.starts_with("/mnt/")
    {
        return false;
    }
    for prefix in &[
        "/boot/",
        "/recovery/",
        "/nix/",
        "/usr/",
        "/var/",
        "/etc/",
        "/home/",
    ] {
        if normalized.starts_with(prefix) {
            return true;
        }
    }
    false
}

/// Vérifie dans `/proc/mounts` qu'aucune partition du disque `/dev/<kname>` n'est montée sur un chemin critique.
fn check_proc_mounts_safety(kname: &str) -> bool {
    let Ok(content) = fs::read_to_string("/proc/mounts") else {
        return true;
    };
    let dev_prefix = format!("/dev/{kname}");
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(source) = parts.next() else {
            continue;
        };
        let Some(target) = parts.next() else {
            continue;
        };
        if (source == dev_prefix || source.starts_with(&dev_prefix))
            && is_critical_mountpoint(target)
        {
            return false;
        }
    }
    true
}

/// Inspecte `/sys/block/<kname>` pour confirmer que le périphérique est bien amovible ou sur bus USB/MMC.
fn check_sysfs_removable_or_usb(kname: &str) -> (bool, bool) {
    let sys_base = format!("/sys/block/{kname}");
    let removable_flag = fs::read_to_string(format!("{sys_base}/removable"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false);

    let is_usb_or_mmc_bus = fs::read_link(format!("{sys_base}/device"))
        .or_else(|_| fs::canonicalize(&sys_base))
        .map(|p| {
            let path_str = p.to_string_lossy().to_ascii_lowercase();
            path_str.contains("/usb") || path_str.contains("/mmc")
        })
        .unwrap_or(false);

    (removable_flag, is_usb_or_mmc_bus)
}

/// Évalue de manière stricte si un périphérique `lsblk` est un disque externe/amovible sûr.
fn is_safe_removable_disk(dev: &LsblkDevice) -> bool {
    let dev_type = dev.dev_type.as_deref().unwrap_or("");
    if dev_type != "disk" {
        return false;
    }

    let kname = dev.kname.as_deref().unwrap_or(&dev.name);

    // 1. Exclusion stricte des périphériques virtuels, boucles, RAM, RAID, optique et NVMe internes
    let excluded_prefixes = [
        "loop", "ram", "zram", "sr", "fd", "dm-", "md", "mtdblock", "nbd", "zd",
    ];
    if excluded_prefixes.iter().any(|p| kname.starts_with(p)) {
        return false;
    }

    // 2. Ignorer les lecteurs de cartes vides (taille 0) ou trop petits (< 64 Mio)
    if dev.size < 64 * 1024 * 1024 {
        return false;
    }

    // 3. Vérification du bus de transport : bloquer NVMe / SATA / ATA / PCIe internes
    let tran = dev.tran.as_deref().unwrap_or("").to_ascii_lowercase();
    if kname.starts_with("nvme") && tran != "usb" {
        return false;
    }
    if matches!(tran.as_str(), "nvme" | "sata" | "ata" | "pcie" | "sas") && !dev.rm {
        return false;
    }

    // 4. Vérification croisée sysfs + flags lsblk (rm / hotplug / usb)
    let (sys_rm, sys_usb) = check_sysfs_removable_or_usb(kname);
    let is_usb_tran = tran == "usb" || tran == "mmc";
    let is_removable = dev.rm || dev.hotplug || is_usb_tran || sys_rm || sys_usb;
    if !is_removable {
        return false;
    }

    // 5. Exclusion inconditionnelle si le disque ou l'une de ses partitions est monté sur un chemin système
    let mut mountpoints = Vec::new();
    collect_all_mountpoints(dev, &mut mountpoints);
    if mountpoints.iter().any(|mp| is_critical_mountpoint(mp)) {
        return false;
    }

    if !check_proc_mounts_safety(kname) {
        return false;
    }

    true
}

/// Convertit un `LsblkDevice` validé en `RemovableDrive`.
fn to_removable_drive(dev: LsblkDevice) -> RemovableDrive {
    let kname = dev.kname.clone().unwrap_or_else(|| dev.name.clone());
    let path = dev
        .path
        .clone()
        .unwrap_or_else(|| format!("/dev/{kname}"));

    let vendor = dev
        .vendor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Générique")
        .to_string();

    let model = dev
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Périphérique USB")
        .to_string();

    let transport = dev
        .tran
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("usb")
        .to_uppercase();

    let mut mountpoints = Vec::new();
    collect_all_mountpoints(&dev, &mut mountpoints);

    let mountpoint_display = if mountpoints.is_empty() {
        "Non monté".to_string()
    } else {
        mountpoints.join(", ")
    };

    let mut partitions = Vec::new();
    for child in &dev.children {
        let child_path = child
            .path
            .clone()
            .unwrap_or_else(|| format!("/dev/{}", child.name));
        let child_mp = child.mountpoint.clone().or_else(|| {
            child
                .mountpoints
                .iter()
                .flatten()
                .find(|m| !m.trim().is_empty())
                .cloned()
        });
        partitions.push(DrivePartition {
            name: child.name.clone(),
            path: child_path,
            size_bytes: child.size,
            size_formatted: format_size(child.size),
            fstype: child.fstype.clone(),
            label: child.label.clone(),
            mountpoint: child_mp,
        });
    }

    RemovableDrive {
        name: dev.name,
        path,
        model,
        vendor,
        size_bytes: dev.size,
        size_formatted: format_size(dev.size),
        transport,
        removable: dev.rm,
        hotplug: dev.hotplug,
        read_only: dev.ro,
        mountpoints,
        mountpoint_display,
        fstype: dev.fstype,
        label: dev.label,
        partitions,
    }
}

/// Énumère de manière sécurisée tous les périphériques blocs amovibles présents sur le système.
pub fn list_removable_drives() -> AppResult<Vec<RemovableDrive>> {
    let output = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,KNAME,PATH,MODEL,VENDOR,SIZE,RM,HOTPLUG,TYPE,TRAN,MOUNTPOINT,MOUNTPOINTS,RO,FSTYPE,LABEL",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(AppError::CommandFailed {
            cmd: "lsblk".to_string(),
            details: stderr,
        });
    }

    let parsed: LsblkOutput = serde_json::from_slice(&output.stdout)?;
    let drives = parsed
        .blockdevices
        .into_iter()
        .filter(is_safe_removable_disk)
        .map(to_removable_drive)
        .collect();

    Ok(drives)
}

/// Vérifie impérativement qu'un chemin de périphérique (`/dev/sdX`) est valide,
/// existe dans la liste des disques amovibles autorisés et n'est pas en lecture seule.
pub fn validate_target_device(device_path: &str) -> AppResult<RemovableDrive> {
    let trimmed = device_path.trim();
    if !trimmed.starts_with("/dev/") || trimmed.contains("..") || trimmed.contains(' ') {
        return Err(AppError::UnsafeDevice(format!(
            "Chemin de périphérique invalide ou suspect : '{trimmed}'"
        )));
    }

    if !Path::new(trimmed).exists() {
        return Err(AppError::DeviceNotFound(format!(
            "Le périphérique '{trimmed}' n'existe pas sur le système."
        )));
    }

    let drives = list_removable_drives()?;
    let matched = drives
        .into_iter()
        .find(|d| d.path == trimmed || format!("/dev/{}", d.name) == trimmed)
        .ok_or_else(|| {
            AppError::UnsafeDevice(format!(
                "Refus de sécurité : '{trimmed}' n'est pas un périphérique USB/amovible autorisé ou contient une partition système."
            ))
        })?;

    if matched.read_only {
        return Err(AppError::UnsafeDevice(format!(
            "Le périphérique '{}' ({}) est verrouillé en lecture seule matériellement.",
            matched.path, matched.model
        )));
    }

    Ok(matched)
}

/// Retourne le chemin de la nième partition d'un disque bloc Linux.
/// Exemple : `/dev/sda` + `1` -> `/dev/sda1`, `/dev/mmcblk0` + `1` -> `/dev/mmcblk0p1`.
pub fn partition_path_for_disk(disk_path: &str, part_num: u32) -> String {
    let ends_with_digit = disk_path
        .chars()
        .last()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false);
    if ends_with_digit {
        format!("{disk_path}p{part_num}")
    } else {
        format!("{disk_path}{part_num}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critical_mountpoint_detection() {
        assert!(is_critical_mountpoint("/"));
        assert!(is_critical_mountpoint("/boot/efi"));
        assert!(is_critical_mountpoint("/recovery"));
        assert!(is_critical_mountpoint("/home"));
        assert!(is_critical_mountpoint("[SWAP]"));
        assert!(!is_critical_mountpoint("/media/chomiam/USB_KEY"));
        assert!(!is_critical_mountpoint("/run/media/chomiam/WIN11"));
    }

    #[test]
    fn test_partition_path_naming() {
        assert_eq!(partition_path_for_disk("/dev/sda", 1), "/dev/sda1");
        assert_eq!(partition_path_for_disk("/dev/mmcblk0", 1), "/dev/mmcblk0p1");
    }

    #[test]
    fn test_internal_nvme_rejected() {
        let nvme_root = LsblkDevice {
            name: "nvme1n1".to_string(),
            kname: Some("nvme1n1".to_string()),
            path: Some("/dev/nvme1n1".to_string()),
            model: Some("Fanxiang S790 2TB".to_string()),
            vendor: None,
            size: 2_000_398_934_016,
            rm: false,
            hotplug: false,
            dev_type: Some("disk".to_string()),
            tran: Some("nvme".to_string()),
            mountpoint: None,
            mountpoints: vec![],
            ro: false,
            fstype: None,
            label: None,
            children: vec![LsblkDevice {
                name: "nvme1n1p3".to_string(),
                kname: Some("nvme1n1p3".to_string()),
                path: Some("/dev/nvme1n1p3".to_string()),
                model: None,
                vendor: None,
                size: 1_990_733_159_936,
                rm: false,
                hotplug: false,
                dev_type: Some("part".to_string()),
                tran: Some("nvme".to_string()),
                mountpoint: Some("/".to_string()),
                mountpoints: vec![Some("/".to_string())],
                ro: false,
                fstype: Some("ext4".to_string()),
                label: None,
                children: vec![],
            }],
        };
        assert!(!is_safe_removable_disk(&nvme_root));
    }

    #[test]
    fn test_usb_drive_accepted() {
        let usb_drive = LsblkDevice {
            name: "sda".to_string(),
            kname: Some("sda".to_string()),
            path: Some("/dev/sda".to_string()),
            model: Some("USB Flash Drive".to_string()),
            vendor: Some("Lexar".to_string()),
            size: 31_037_849_600,
            rm: true,
            hotplug: true,
            dev_type: Some("disk".to_string()),
            tran: Some("usb".to_string()),
            mountpoint: None,
            mountpoints: vec![],
            ro: false,
            fstype: Some("iso9660".to_string()),
            label: Some("Ubuntu-Server".to_string()),
            children: vec![],
        };
        assert!(is_safe_removable_disk(&usb_drive));
    }
}
