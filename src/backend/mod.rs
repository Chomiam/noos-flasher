//! Backend natif de Noos Flasher.
//!
//! Ce module ne dépend pas de Tauri : il expose des fonctions Rust pures et un
//! callback de progression, consommés par l'interface GTK4 / Libadwaita.

pub mod debloat;
pub mod drive;
pub mod error;
pub mod writer;
