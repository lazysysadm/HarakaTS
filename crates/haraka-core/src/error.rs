//! Erreurs typées de HarakaTS. Aucun message ne contient de matériau secret.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("format de fichier invalide : {0}")]
    BadFormat(&'static str),
    #[error("version de format non supportée : {0} (mettez HarakaTS à jour)")]
    UnsupportedVersion(u16),
    #[error("schéma interne non supporté : {0}")]
    UnsupportedSchema(u32),
    #[error("données altérées ou corrompues (échec d'authentification)")]
    Tampered,
    #[error("mot de passe ou passphrase incorrect")]
    WrongPassword,
    #[error("erreur cryptographique interne")]
    Crypto,
    #[error("erreur d'entrée/sortie")]
    Io(#[from] std::io::Error),
    #[error("erreur de sérialisation")]
    Json(#[from] serde_json::Error),
    #[error("élément introuvable : {0}")]
    NotFound(&'static str),
    #[error("référence invalide : {0}")]
    InvalidReference(String),
    #[error("coffre verrouillé")]
    Locked,
    #[error("un coffre existe déjà dans ce répertoire de données")]
    AlreadyExists,
    #[error("paramètres invalides : {0}")]
    InvalidParams(String),
}

pub type Result<T> = std::result::Result<T, Error>;
