pub mod commands;
pub mod debloat;
pub mod drive;
pub mod error;
pub mod writer;

use commands::AppState;

/// Point d'entrée principal de la bibliothèque Tauri.
/// Gère également le mode `--privileged-worker` lorsqu'il est invoqué par `pkexec`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--privileged-worker") {
        let exit_code = writer::run_privileged_worker_from_args(&args);
        std::process::exit(exit_code);
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_removable_drives,
            commands::inspect_iso,
            commands::get_system_capabilities,
            commands::get_windows_debloat_catalog,
            commands::preview_windows_scripts,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::start_flash,
            commands::cancel_flash,
        ]);

    if let Err(err) = builder.run(tauri::generate_context!()) {
        eprintln!("Erreur fatale lors de l'exécution de l'application Tauri : {err}");
        std::process::exit(1);
    }
}
