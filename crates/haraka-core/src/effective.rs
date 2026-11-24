//! Résolution d'héritage dossier → connexion avec provenance
//! (ARCHITECTURE.md §4.1, jalon M2).
//!
//! `config_effective = fold(defaults_système, dossiers racine→feuille, connexion)`
//! Chaque champ porte sa source (système / dossier / connexion) pour l'UI
//! (« hérité de … » + surcharge explicite).

use serde::{Deserialize, Serialize};

use crate::model::{Connection, Folder, JumpHop, Store};

pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// "system" | "folder" | "connection"
    pub kind: String,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl Provenance {
    pub fn system() -> Self {
        Self {
            kind: "system".to_string(),
            id: None,
            name: None,
        }
    }

    pub fn folder(f: &Folder) -> Self {
        Self {
            kind: "folder".to_string(),
            id: Some(f.id.clone()),
            name: Some(f.name.clone()),
        }
    }

    pub fn connection() -> Self {
        Self {
            kind: "connection".to_string(),
            id: None,
            name: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved<T> {
    pub value: Option<T>,
    pub source: Provenance,
}

impl<T> Resolved<T> {
    fn system(value: Option<T>) -> Self {
        Self {
            value,
            source: Provenance::system(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub host: String,
    pub user: Resolved<String>,
    pub port: Resolved<u16>,
    pub credential_id: Resolved<String>,
    pub jump_chain: Resolved<Vec<JumpHop>>,
    pub clipboard_policy: Resolved<String>,
    pub keepalive_s: Resolved<u32>,
    pub encoding: Resolved<String>,
    pub auth_method_order: Resolved<Vec<String>>,
}

/// Chaîne de dossiers racine → feuille, avec garde-fou anti-cycle.
pub fn folder_chain<'a>(store: &'a Store, folder_id: &str) -> Vec<&'a Folder> {
    let mut chain = Vec::new();
    let mut current = store.folders.iter().find(|f| f.id == folder_id);
    let mut depth = 0;
    while let Some(folder) = current {
        chain.push(folder);
        depth += 1;
        if depth > MAX_DEPTH {
            break;
        }
        current = folder
            .parent_id
            .as_deref()
            .and_then(|pid| store.folders.iter().find(|f| f.id == pid));
    }
    chain.reverse();
    chain
}

/// Résolution complète avec provenance. Les défauts système viennent des
/// réglages du coffre quand ils existent (politique presse-papier).
pub fn resolve(store: &Store, conn: &Connection) -> EffectiveConfig {
    let chain = folder_chain(store, &conn.folder_id);

    macro_rules! fold {
        ($field:ident, $system:expr, $connfield:ident) => {{
            let mut r = Resolved::system($system);
            for f in &chain {
                if let Some(v) = &f.defaults.$field {
                    r = Resolved {
                        value: Some(v.clone()),
                        source: Provenance::folder(f),
                    };
                }
            }
            if let Some(v) = &conn.$connfield {
                r = Resolved {
                    value: Some(v.clone()),
                    source: Provenance::connection(),
                };
            }
            r
        }};
    }

    let user = fold!(user, None, user);
    let port = fold!(port, Some(22u16), port);
    let credential_id = fold!(credential_id, None, credential_id);
    let clipboard_policy = fold!(
        clipboard_policy,
        Some(store.settings.clipboard_default.clone()),
        clipboard_policy
    );
    let keepalive_s = fold!(keepalive_s, Some(30u32), keepalive_s);
    let encoding = fold!(encoding, Some("utf-8".to_string()), encoding);
    // auth_method_order : défauts système + dossiers uniquement (pas de
    // surcharge par connexion dans le schéma v1).
    let mut auth_method_order = Resolved::system(Some(vec![
        "agent".to_string(),
        "publickey".to_string(),
        "password".to_string(),
        "keyboard-interactive".to_string(),
    ]));
    for f in &chain {
        if let Some(v) = &f.defaults.auth_method_order {
            auth_method_order = Resolved {
                value: Some(v.clone()),
                source: Provenance::folder(f),
            };
        }
    }

    // jump_chain : défauts dossier = vecteur non vide ; connexion = Some(vec)
    // explicite (même vide = « pas de jump » surchargé).
    let mut jump_chain = Resolved::system(Some(Vec::new()));
    for f in &chain {
        if !f.defaults.jump_chain.is_empty() {
            jump_chain = Resolved {
                value: Some(f.defaults.jump_chain.clone()),
                source: Provenance::folder(f),
            };
        }
    }
    if let Some(v) = &conn.jump_chain {
        jump_chain = Resolved {
            value: Some(v.clone()),
            source: Provenance::connection(),
        };
    }

    EffectiveConfig {
        host: conn.host.clone(),
        user,
        port,
        credential_id,
        jump_chain,
        clipboard_policy,
        keepalive_s,
        encoding,
        auth_method_order,
    }
}

/// Credential effectif (raccourci utilisé par l'export ciblé).
pub fn credential_for(store: &Store, conn: &Connection) -> Option<String> {
    resolve(store, conn).credential_id.value
}

/// Identifiants de tous les dossiers d'un sous-arbre (BFS, anti-cycle).
pub fn subtree_folders(store: &Store, root_id: &str) -> Vec<String> {
    let mut out = vec![root_id.to_string()];
    let mut i = 0;
    while i < out.len() && out.len() <= MAX_DEPTH * 1024 {
        let parent = out[i].clone();
        for folder in &store.folders {
            if folder.parent_id.as_deref() == Some(parent.as_str()) && !out.contains(&folder.id) {
                out.push(folder.id.clone());
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionDefaults, Credential};

    fn folder(id: &str, parent: Option<&str>, cred: Option<&str>) -> Folder {
        Folder {
            id: id.into(),
            parent_id: parent.map(Into::into),
            name: id.into(),
            color: None,
            icon: None,
            sort_order: 0,
            tags: vec![],
            defaults: ConnectionDefaults {
                credential_id: cred.map(Into::into),
                ..Default::default()
            },
        }
    }

    fn conn(id: &str, folder_id: &str, cred: Option<&str>) -> Connection {
        let now = crate::crypto::now_rfc3339();
        Connection {
            id: id.into(),
            folder_id: folder_id.into(),
            name: id.into(),
            protocol: "ssh".into(),
            host: "h".into(),
            port: None,
            user: None,
            credential_id: cred.map(Into::into),
            jump_chain: None,
            clipboard_policy: None,
            keepalive_s: None,
            encoding: None,
            color: None,
            icon: None,
            tags: vec![],
            favorite: false,
            tunnels: vec![],
            startup_snippet_id: None,
            session_logging: false,
            sftp: Default::default(),
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now,
            last_used_at: None,
        }
    }

    fn store_sample() -> Store {
        let mut store = Store::new("v".into());
        store.folders = vec![
            folder("root", None, Some("cred-root")),
            folder("child", Some("root"), Some("cred-child")),
        ];
        store.credentials = vec![Credential {
            id: "cred-root".into(),
            name: "r".into(),
            username: "u".into(),
            password: None,
            private_key: None,
            passphrase: None,
            key_origin: None,
            needs_reentry: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }];
        store
    }

    #[test]
    fn inheritance_and_override() {
        let store = store_sample();
        let inherited = conn("c1", "child", None);
        assert_eq!(
            credential_for(&store, &inherited).as_deref(),
            Some("cred-child")
        );
        let overridden = conn("c2", "child", Some("cred-root"));
        assert_eq!(
            credential_for(&store, &overridden).as_deref(),
            Some("cred-root")
        );
        let root_conn = conn("c3", "root", None);
        assert_eq!(
            credential_for(&store, &root_conn).as_deref(),
            Some("cred-root")
        );
    }

    #[test]
    fn provenance_matrix() {
        let mut store = store_sample();
        store.folders[0].defaults.user = Some("deploy".into());
        store.folders[0].defaults.port = Some(2222);
        store.folders[1].defaults.clipboard_policy = Some("disabled".into());

        let mut c = conn("c1", "child", None);
        let eff = resolve(&store, &c);
        assert_eq!(eff.user.value.as_deref(), Some("deploy"));
        assert_eq!(eff.user.source.kind, "folder");
        assert_eq!(eff.user.source.id.as_deref(), Some("root"));
        assert_eq!(eff.port.value, Some(2222));
        assert_eq!(eff.clipboard_policy.value.as_deref(), Some("disabled"));
        assert_eq!(eff.clipboard_policy.source.id.as_deref(), Some("child"));
        assert_eq!(eff.keepalive_s.value, Some(30));
        assert_eq!(eff.keepalive_s.source.kind, "system");
        assert_eq!(eff.encoding.value.as_deref(), Some("utf-8"));

        c.user = Some("admin".into());
        c.port = Some(22);
        c.clipboard_policy = Some("bidirectional".into());
        c.jump_chain = Some(vec![]);
        let eff = resolve(&store, &c);
        assert_eq!(eff.user.source.kind, "connection");
        assert_eq!(eff.port.source.kind, "connection");
        assert_eq!(eff.clipboard_policy.source.kind, "connection");
        assert_eq!(eff.jump_chain.value, Some(vec![]));
        assert_eq!(eff.jump_chain.source.kind, "connection");
    }

    #[test]
    fn subtree_collects_descendants() {
        let mut store = Store::new("v".into());
        store.folders = vec![
            folder("a", None, None),
            folder("b", Some("a"), None),
            folder("c", Some("b"), None),
            folder("x", None, None),
        ];
        let sub = subtree_folders(&store, "a");
        assert_eq!(sub.len(), 3);
        assert!(sub.contains(&"c".to_string()));
        assert!(!sub.contains(&"x".to_string()));
    }
}
