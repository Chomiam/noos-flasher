use serde::Serialize;
use thiserror::Error;

/// Erreurs typées du backend Noos Flasher.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Erreur d'entrée/sortie : {0}")]
    Io(#[from] std::io::Error),

    #[error("Erreur de sérialisation JSON : {0}")]
    Json(#[from] serde_json::Error),

    #[error("Sécurité disque : {0}")]
    UnsafeDevice(String),

    #[error("Périphérique introuvable : {0}")]
    DeviceNotFound(String),

    #[error("Image ISO/IMG invalide : {0}")]
    InvalidIso(String),

    #[error("La commande système '{cmd}' a échoué : {details}")]
    CommandFailed { cmd: String, details: String },

    #[error("Élévation de privilèges refusée ou interrompue (Polkit/pkexec) : {0}")]
    PrivilegeElevationDenied(String),

    #[error("Découpage WIM requis : {0}")]
    WimSplitRequired(String),

    #[error("Opération annulée par l'utilisateur")]
    Cancelled,

    #[error("Opération déjà en cours d'exécution")]
    AlreadyRunning,

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
