//! Erreurs du backend SSH. Aucun message ne contient de matériau secret.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("échec d'authentification (toutes les méthodes disponibles ont été tentées)")]
    AuthFailed,
    #[error("authentification refusée : {0}")]
    AuthRejected(String),
    #[error("connexion impossible : {0}")]
    Connect(String),
    #[error("clé d'hôte refusée par l'utilisateur")]
    HostKeyRejected,
    #[error("clé d'hôte changée — connexion bloquée (vérification requise)")]
    HostKeyChanged,
    #[error("session introuvable : {0}")]
    NotFound(String),
    #[error("session fermée")]
    Closed,
    #[error("délai d'attente dépassé (prompt utilisateur)")]
    PromptTimeout,
    #[error("erreur SSH : {0}")]
    Russh(String),
    #[error("erreur d'entrée/sortie")]
    Io(#[from] std::io::Error),
    #[error("clé privée invalide ou passphrase incorrecte")]
    InvalidKey,
    #[error("SFTP : {0}")]
    Sftp(String),
    #[error("tunnel : {0}")]
    Tunnel(String),
    #[error("agent SSH indisponible")]
    AgentUnavailable,
}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Russh(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SshError>;
