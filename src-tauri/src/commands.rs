use crate::debloat::{
    self, DebloatFeatureDescriptor, WindowsCustomConfig,
};
use crate::drive::{self, RemovableDrive};
use crate::error::{AppError, AppResult};
use crate::writer::{
    self, FileSystemType, FlashJobConfig, FlashMode, FlashProgressEvent, IsoInspection,
    PartitionScheme, SystemCapabilities,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

/// État partagé de l'application Tauri.
pub struct AppState {
    pub is_flashing: Arc<AtomicBool>,
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            is_flashing: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Aperçu des fichiers générés pour la personnalisation Windows (`autounattend.xml` + `noos_debloat.ps1`).
#[derive(Debug, Clone, Serialize)]
pub struct WindowsScriptsPreview {
    pub autounattend_xml: String,
    pub debloat_ps1: String,
}

/// Retourne la liste des périphériques USB/amovibles sécurisés (excluant `/` et disques internes).
#[tauri::command]
pub async fn get_removable_drives() -> AppResult<Vec<RemovableDrive>> {
    tokio::task::spawn_blocking(drive::list_removable_drives)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("Erreur de tâche lsblk : {e}")))?
}

/// Inspecte un fichier `.iso` ou `.img` et détecte s'il s'agit d'un ISO Linux hybride ou Windows UEFI.
#[tauri::command]
pub async fn inspect_iso(path: String) -> AppResult<IsoInspection> {
    tokio::task::spawn_blocking(move || writer::inspect_iso_file(&path))
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("Erreur d'inspection ISO : {e}")))?
}

/// Retourne l'état des outils système installés sur l'hôte (`pkexec`, `parted`, `mkfs.vfat`, `mkfs.ntfs`, `wimlib-imagex`).
#[tauri::command]
pub async fn get_system_capabilities() -> AppResult<SystemCapabilities> {
    tokio::task::spawn_blocking(writer::check_system_capabilities)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("Erreur de vérification système : {e}")))
}

/// Retourne le catalogue complet des fonctions de debloat et de personnalisation Windows.
#[tauri::command]
pub async fn get_windows_debloat_catalog() -> AppResult<Vec<DebloatFeatureDescriptor>> {
    Ok(debloat::get_debloat_catalog())
}

/// Génère un aperçu en direct du fichier `autounattend.xml` et du script PowerShell `noos_debloat.ps1`.
#[tauri::command]
pub async fn preview_windows_scripts(
    windows_custom: WindowsCustomConfig,
) -> AppResult<WindowsScriptsPreview> {
    tokio::task::spawn_blocking(move || WindowsScriptsPreview {
        autounattend_xml: debloat::generate_autounattend_xml(&windows_custom),
        debloat_ps1: debloat::generate_debloat_powershell_script(&windows_custom),
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("Erreur de génération d'aperçu : {e}")))
}

/// Minimise la fenêtre sans bordure.
#[tauri::command]
pub fn window_minimize(window: WebviewWindow) -> AppResult<()> {
    window.minimize()?;
    Ok(())
}

/// Bascule entre l'état maximisé et restauré de la fenêtre sans bordure.
#[tauri::command]
pub fn window_toggle_maximize(window: WebviewWindow) -> AppResult<bool> {
    let is_max = window.is_maximized()?;
    if is_max {
        window.unmaximize()?;
        Ok(false)
    } else {
        window.maximize()?;
        Ok(true)
    }
}

/// Ferme proprement la fenêtre principale.
#[tauri::command]
pub fn window_close(window: WebviewWindow) -> AppResult<()> {
    window.close()?;
    Ok(())
}

/// Démarre l'opération de flashage avec élévation de privilèges `pkexec` et événements temps réel.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_flash(
    app: AppHandle,
    state: State<'_, AppState>,
    device_path: String,
    iso_path: String,
    mode: FlashMode,
    partition_scheme: PartitionScheme,
    fs_type: FileSystemType,
    volume_label: String,
    windows_custom: Option<WindowsCustomConfig>,
) -> AppResult<()> {
    if state
        .is_flashing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::AlreadyRunning);
    }

    state.cancel_flag.store(false, Ordering::SeqCst);

    let is_flashing_guard = Arc::clone(&state.is_flashing);
    let cancel_flag = Arc::clone(&state.cancel_flag);

    let config = FlashJobConfig {
        device_path,
        iso_path,
        mode,
        partition_scheme,
        fs_type,
        volume_label,
        windows_custom,
    };

    let result = writer::execute_flash_with_pkexec(app.clone(), config, cancel_flag).await;
    is_flashing_guard.store(false, Ordering::SeqCst);

    if let Err(ref err) = result {
        let _ = app.emit(
            "flash-progress",
            FlashProgressEvent {
                stage: "Échec de l'opération".to_string(),
                percent: 0.0,
                speed_mbps: 0.0,
                bytes_processed: 0,
                total_bytes: 0,
                eta_seconds: None,
                log: Some(format!("ERREUR : {err}")),
                completed: false,
                error: Some(err.to_string()),
            },
        );
    }

    result
}

/// Demande l'annulation immédiate du flashage en cours.
#[tauri::command]
pub async fn cancel_flash(state: State<'_, AppState>) -> AppResult<()> {
    if state.is_flashing.load(Ordering::SeqCst) {
        state.cancel_flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}
