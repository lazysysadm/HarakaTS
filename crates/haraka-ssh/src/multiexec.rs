//! Multi-exec (M7) : diffusion de la saisie d'une session vers d'autres sessions.
//!
//! `MultiExec` est la logique PURE (adhésions, cibles) ; `write_routed` écrit
//! les octets dans la session d'origine puis, si l'origine est membre d'un
//! multi-exec actif, dans chacune des AUTRES sessions membres.
//!
//! Anti-boucle : la copie miroir passe par `SessionManager::write` directement,
//! jamais par le chemin de saisie (`write_routed`) de la session cible ; une
//! écriture miroir ne peut donc pas être rediffusée. De plus `targets(origin)`
//! exclut toujours l'origine.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::SessionManager;

/// Nombre minimal de membres pour activer le multi-exec.
pub const MIN_MEMBERS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiExecError {
    /// Moins de `MIN_MEMBERS` sessions sélectionnées.
    NotEnoughMembers,
}

impl std::fmt::Display for MultiExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughMembers => {
                write!(f, "multi-exec : au moins 2 sessions sont requises")
            }
        }
    }
}

impl std::error::Error for MultiExecError {}

/// Instantané sérialisable (commande/événement `multiexec://state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MultiExecState {
    pub enabled: bool,
    pub sessions: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct MultiExec {
    enabled: bool,
    members: BTreeSet<String>,
}

impl MultiExec {
    pub fn new() -> Self {
        Self::default()
    }

    /// Active (≥ 2 membres) ou désactive. Désactiver vide la liste des membres.
    pub fn set(&mut self, enabled: bool, sessions: &[String]) -> Result<(), MultiExecError> {
        if !enabled {
            self.enabled = false;
            self.members.clear();
            return Ok(());
        }
        let members: BTreeSet<String> = sessions.iter().cloned().collect();
        if members.len() < MIN_MEMBERS {
            return Err(MultiExecError::NotEnoughMembers);
        }
        self.members = members;
        self.enabled = true;
        Ok(())
    }

    /// Retire une session (fermée). Retourne true si l'état a changé. Sous
    /// `MIN_MEMBERS` membres, le multi-exec se désactive (plus rien à diffuser).
    pub fn remove(&mut self, session: &str) -> bool {
        if !self.members.remove(session) {
            return false;
        }
        if self.members.len() < MIN_MEMBERS {
            self.enabled = false;
            self.members.clear();
        }
        true
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// `true` si la session est membre d'un multi-exec actif.
    pub fn is_member(&self, session: &str) -> bool {
        self.enabled && self.members.contains(session)
    }

    pub fn count(&self) -> usize {
        if self.enabled { self.members.len() } else { 0 }
    }

    /// Membres actifs (vide si désactivé).
    pub fn members(&self) -> Vec<String> {
        if self.enabled {
            self.members.iter().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Sessions à qui recopier la saisie de `origin` : les autres membres si
    /// `origin` est membre d'un multi-exec actif, sinon rien.
    pub fn targets(&self, origin: &str) -> Vec<String> {
        if !self.is_member(origin) {
            return Vec::new();
        }
        self.members
            .iter()
            .filter(|m| m.as_str() != origin)
            .cloned()
            .collect()
    }

    pub fn snapshot(&self) -> MultiExecState {
        MultiExecState {
            enabled: self.enabled,
            sessions: self.members(),
        }
    }
}

/// État partagé entre l'IPC et les tâches de saisie des sessions.
pub type SharedMultiExec = Arc<Mutex<MultiExec>>;

/// Écrit `bytes` dans `origin` puis, dans le même ordre, dans les autres
/// membres. Les octets sont identiques pour toutes les sessions. Une erreur
/// sur une cible ne bloque ni l'origine ni les autres (journalisée).
pub async fn write_routed(
    mgr: &SessionManager,
    multi: &SharedMultiExec,
    origin: &str,
    bytes: Vec<u8>,
) -> crate::Result<()> {
    let targets = multi.lock().map(|m| m.targets(origin)).unwrap_or_default();
    let result = mgr.write(origin, bytes.clone()).await;
    for target in targets {
        if let Err(e) = mgr.write(&target, bytes.clone()).await {
            tracing::warn!(session = %target, "multi-exec : écriture miroir : {e}");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn enabling_requires_two_members() {
        let mut m = MultiExec::new();
        assert_eq!(
            m.set(true, &ids(&["a"])),
            Err(MultiExecError::NotEnoughMembers)
        );
        assert_eq!(
            m.set(true, &ids(&["a", "a"])),
            Err(MultiExecError::NotEnoughMembers),
            "doublons comptés une fois"
        );
        assert!(!m.is_enabled());
        m.set(true, &ids(&["a", "b"])).unwrap();
        assert!(m.is_enabled());
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn targets_exclude_origin_and_non_members() {
        let mut m = MultiExec::new();
        m.set(true, &ids(&["a", "b", "c"])).unwrap();
        assert_eq!(m.targets("a"), ids(&["b", "c"]));
        assert_eq!(m.targets("b"), ids(&["a", "c"]));
        assert!(m.targets("zzz").is_empty(), "non membre : pas de diffusion");
        for o in ["a", "b", "c"] {
            assert!(!m.targets(o).contains(&o.to_string()), "jamais l'origine");
        }
    }

    #[test]
    fn disabled_routes_nothing_and_clears() {
        let mut m = MultiExec::new();
        m.set(true, &ids(&["a", "b"])).unwrap();
        m.set(false, &[]).unwrap();
        assert!(m.targets("a").is_empty());
        assert!(m.members().is_empty());
        assert_eq!(
            m.snapshot(),
            MultiExecState {
                enabled: false,
                sessions: vec![]
            }
        );
    }

    #[test]
    fn removing_members_and_auto_disable() {
        let mut m = MultiExec::new();
        m.set(true, &ids(&["a", "b", "c"])).unwrap();
        assert!(m.remove("c"));
        assert_eq!(m.targets("a"), ids(&["b"]));
        assert!(!m.remove("c"), "déjà retiré");
        assert!(!m.remove("inconnu"));
        assert!(m.remove("b"));
        assert!(!m.is_enabled(), "< 2 membres : désactivé");
        assert!(m.targets("a").is_empty());
    }

    #[test]
    fn mirrored_write_has_no_route_back() {
        // Modèle du chemin réel : la cible reçoit l'écriture miroir par
        // `mgr.write` (pas par `write_routed`) ; simuler un cycle complet de
        // diffusion depuis chaque origine ne produit jamais plus de n-1 copies.
        let mut m = MultiExec::new();
        m.set(true, &ids(&["a", "b", "c"])).unwrap();
        let mut deliveries = 0;
        for origin in ["a", "b", "c"] {
            deliveries += m.targets(origin).len();
        }
        assert_eq!(deliveries, 3 * 2);
    }
}
