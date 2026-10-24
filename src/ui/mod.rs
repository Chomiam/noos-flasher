mod style;
mod window;

use gtk::prelude::*;

/// Identifiant d'application utilisé par GTK, Polkit et le fichier .desktop.
pub const APP_ID: &str = "io.github.chomiam.NoosFlasher";

pub fn run() -> glib::ExitCode {
    // Permet d'ouvrir directement une image depuis le gestionnaire de fichiers :
    // `noos-flasher /chemin/vers/image.iso`.
    let initial_iso = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));

    let app = adw::Application::builder().application_id(APP_ID).build();
    gtk::Window::set_default_icon_name(APP_ID);

    app.connect_startup(|_| style::load_css());
    app.connect_activate(move |app| window::present(app, initial_iso.clone()));

    // On ne transmet pas le chemin de l'ISO à GApplication : il est déjà extrait
    // ci-dessus et l'application n'utilise pas le mode HANDLES_OPEN de GIO.
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "noos-flasher".to_string());
    app.run_with_args(&[program])
}
