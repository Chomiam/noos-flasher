//! Noos Flasher — créateur de clés USB bootables Linux et Windows UEFI.
//!
//! Le binaire sert à la fois d'interface graphique GTK4 / Libadwaita et de
//! worker privilégié lancé par `pkexec` (`--privileged-worker`).

mod backend;
mod ui;

// Compatibilité des chemins historiques du backend (`crate::drive`, etc.).
pub use backend::{debloat, drive, error, writer};

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|arg| arg == "--privileged-worker-check") {
        std::process::exit(0);
    }

    if args.iter().any(|arg| arg == "--privileged-worker") {
        let exit_code = backend::writer::run_privileged_worker_from_args(&args);
        std::process::exit(exit_code);
    }

    ui::run()
}
