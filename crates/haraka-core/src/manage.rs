//! CRUD validé de l'arbre, des connexions et des credentials (jalon M2).
//! Intégrité référentielle, anti-cycles, suppression avec détachement.

use uuid::Uuid;

use crate::crypto::now_rfc3339;
use crate::effective::{self, MAX_DEPTH};
use crate::error::{Error, Result};
use crate::model::{Connection, Folder, JumpHop, Store};

fn check_credential_ref(store: &Store, id: &Option<String>) -> Result<()> {
    if let Some(id) = id {
        store.credential(id)?;
    }
    Ok(())
}

fn check_hops(store: &Store, hops: &[JumpHop]) -> Result<()> {
    for hop in hops {
        if hop.host.trim().is_empty() {
            return Err(Error::InvalidParams("hôte de jump vide".into()));
        }
        check_credential_ref(store, &hop.credential_id)?;
    }
    Ok(())
}

fn check_parent(store: &Store, folder_id: &str, parent_id: Option<&str>) -> Result<()> {
    let Some(pid) = parent_id else {
        return Ok(());
    };
    if pid == folder_id {
        return Err(Error::InvalidReference(
            "un dossier ne peut pas être son propre parent".into(),
        ));
    }
    store.folder(pid)?;
    if effective::subtree_folders(store, folder_id).contains(&pid.to_string()) {
        return Err(Error::InvalidReference(
            "déplacement créerait un cycle (cible dans le sous-arbre)".into(),
        ));
    }
    if effective::folder_chain(store, pid).len() >= MAX_DEPTH {
        return Err(Error::InvalidParams("profondeur maximale atteinte".into()));
    }
    Ok(())
}

pub fn upsert_folder(store: &mut Store, mut folder: Folder) -> Result<()> {
    if folder.name.trim().is_empty() {
        return Err(Error::InvalidParams("nom de dossier vide".into()));
    }
    if folder.id.is_empty() {
        folder.id = Uuid::new_v4().to_string();
    }
    check_parent(store, &folder.id, folder.parent_id.as_deref())?;
    check_credential_ref(store, &folder.defaults.credential_id)?;
    check_hops(store, &folder.defaults.jump_chain)?;
    match store.folders.iter_mut().find(|f| f.id == folder.id) {
        Some(existing) => *existing = folder,
        None => store.folders.push(folder),
    }
    Ok(())
}

/// Suppression d'un dossier. Sans `cascade`, refuse si non vide.
/// Retourne le nombre de connexions supprimées.
pub fn delete_folder(store: &mut Store, id: &str, cascade: bool) -> Result<usize> {
    store.folder(id)?;
    let sub = effective::subtree_folders(store, id);
    let conns: Vec<String> = store
        .connections
        .iter()
        .filter(|c| sub.contains(&c.folder_id))
        .map(|c| c.id.clone())
        .collect();
    let has_children = store
        .folders
        .iter()
        .any(|f| f.parent_id.as_deref() == Some(id));
    if !cascade && (has_children || !conns.is_empty()) {
        return Err(Error::InvalidReference(format!(
            "dossier non vide ({} connexion(s), sous-dossiers) : cascade=true ou déplacer le contenu",
            conns.len()
        )));
    }
    store.folders.retain(|f| !sub.contains(&f.id));
    store.connections.retain(|c| !sub.contains(&c.folder_id));
    store.recents.retain(|r| !conns.contains(&r.connection_id));
    Ok(conns.len())
}

pub fn move_folder(
    store: &mut Store,
    id: &str,
    new_parent: Option<&str>,
    sort_order: Option<i32>,
) -> Result<()> {
    store.folder(id)?;
    check_parent(store, id, new_parent)?;
    let folder = store
        .folders
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or(Error::NotFound("dossier"))?;
    folder.parent_id = new_parent.map(str::to_string);
    if let Some(order) = sort_order {
        folder.sort_order = order;
    }
    Ok(())
}

pub fn upsert_connection(store: &mut Store, mut conn: Connection) -> Result<()> {
    if conn.name.trim().is_empty() {
        return Err(Error::InvalidParams("nom de connexion vide".into()));
    }
    if conn.host.trim().is_empty() {
        return Err(Error::InvalidParams("hôte vide".into()));
    }
    if conn.protocol != "ssh" {
        return Err(Error::InvalidParams(format!(
            "protocole non supporté en v1 : {}",
            conn.protocol
        )));
    }
    store.folder(&conn.folder_id)?;
    check_credential_ref(store, &conn.credential_id)?;
    if let Some(hops) = &conn.jump_chain {
        check_hops(store, hops)?;
    }
    if let Some(id) = &conn.startup_snippet_id {
        store.macro_by_id(id)?;
    }
    if conn.id.is_empty() {
        conn.id = Uuid::new_v4().to_string();
        conn.created_at = now_rfc3339();
    }
    conn.updated_at = now_rfc3339();
    match store.connections.iter_mut().find(|c| c.id == conn.id) {
        Some(existing) => *existing = conn,
        None => store.connections.push(conn),
    }
    Ok(())
}

pub fn delete_connection(store: &mut Store, id: &str) -> Result<()> {
    if !store.connections.iter().any(|c| c.id == id) {
        return Err(Error::NotFound("connexion"));
    }
    store.connections.retain(|c| c.id != id);
    store.recents.retain(|r| r.connection_id != id);
    Ok(())
}

pub fn duplicate_connection(store: &mut Store, id: &str) -> Result<String> {
    let original = store
        .connections
        .iter()
        .find(|c| c.id == id)
        .ok_or(Error::NotFound("connexion"))?;
    let mut copy = original.clone();
    copy.id = Uuid::new_v4().to_string();
    copy.name = format!("{} (copie)", original.name);
    copy.favorite = false;
    copy.last_used_at = None;
    copy.created_at = now_rfc3339();
    copy.updated_at = copy.created_at.clone();
    let new_id = copy.id.clone();
    store.connections.push(copy);
    Ok(new_id)
}

/// Chaîne de jumps héritée des dossiers (si la connexion ne surcharge pas).
fn inherited_jumps(store: &Store, conn: &Connection) -> Vec<JumpHop> {
    effective::folder_chain(store, &conn.folder_id)
        .iter()
        .rev()
        .find(|f| !f.defaults.jump_chain.is_empty())
        .map(|f| f.defaults.jump_chain.clone())
        .unwrap_or_default()
}

/// Connexions referencing un credential (direct, hérité, ou via jump).
pub fn credential_refs(store: &Store, cred_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in &store.connections {
        let direct = c.credential_id.as_deref() == Some(cred_id);
        let inherited = effective::credential_for(store, c).as_deref() == Some(cred_id);
        let hops = c
            .jump_chain
            .clone()
            .unwrap_or_else(|| inherited_jumps(store, c));
        let via_hop = hops
            .iter()
            .any(|h| h.credential_id.as_deref() == Some(cred_id));
        if (direct || inherited || via_hop) && !out.contains(&c.id) {
            out.push(c.id.clone());
        }
    }
    out
}

/// Suppression d'un credential. Sans `detach`, refuse s'il est référencé.
/// Retourne le nombre de connexions qui étaient référencées.
pub fn delete_credential(store: &mut Store, id: &str, detach: bool) -> Result<usize> {
    store.credential(id)?;
    let refs = credential_refs(store, id);
    if !refs.is_empty() && !detach {
        return Err(Error::InvalidReference(format!(
            "credential référencé par {} connexion(s) : detach=true pour détacher",
            refs.len()
        )));
    }
    if detach {
        for c in &mut store.connections {
            if c.credential_id.as_deref() == Some(id) {
                c.credential_id = None;
            }
            if let Some(hops) = &mut c.jump_chain {
                for h in hops {
                    if h.credential_id.as_deref() == Some(id) {
                        h.credential_id = None;
                    }
                }
            }
        }
        for f in &mut store.folders {
            if f.defaults.credential_id.as_deref() == Some(id) {
                f.defaults.credential_id = None;
            }
            for h in &mut f.defaults.jump_chain {
                if h.credential_id.as_deref() == Some(id) {
                    h.credential_id = None;
                }
            }
        }
    }
    store.credentials.retain(|c| c.id != id);
    Ok(refs.len())
}

/// Métadonnées d'un credential (les secrets passent par Vault::set_secret).
pub fn upsert_credential_meta(
    store: &mut Store,
    id: Option<&str>,
    name: &str,
    username: &str,
) -> Result<String> {
    if name.trim().is_empty() {
        return Err(Error::InvalidParams("nom de credential vide".into()));
    }
    let now = now_rfc3339();
    match id {
        Some(id) => {
            let c = store.credential_mut(id)?;
            c.name = name.to_string();
            c.username = username.to_string();
            c.updated_at = now;
            Ok(id.to_string())
        }
        None => {
            let id = Uuid::new_v4().to_string();
            store.credentials.push(crate::model::Credential {
                id: id.clone(),
                name: name.to_string(),
                username: username.to_string(),
                password: None,
                private_key: None,
                passphrase: None,
                key_origin: Some("pasted".into()),
                needs_reentry: vec![],
                created_at: now.clone(),
                updated_at: now,
            });
            Ok(id)
        }
    }
}

pub fn touch_recent(store: &mut Store, conn_id: &str) -> Result<()> {
    if !store.connections.iter().any(|c| c.id == conn_id) {
        return Err(Error::NotFound("connexion"));
    }
    store.recents.retain(|r| r.connection_id != conn_id);
    store.recents.insert(
        0,
        crate::model::Recent {
            connection_id: conn_id.to_string(),
            last_used_at: now_rfc3339(),
        },
    );
    store.recents.truncate(50);
    Ok(())
}

/// Validation globale d'intégrité (tests, post-import).
pub fn validate_store(store: &Store) -> Result<()> {
    for f in &store.folders {
        check_parent(store, &f.id, f.parent_id.as_deref())?;
        check_credential_ref(store, &f.defaults.credential_id)?;
        check_hops(store, &f.defaults.jump_chain)?;
    }
    for c in &store.connections {
        store.folder(&c.folder_id)?;
        check_credential_ref(store, &c.credential_id)?;
        if let Some(hops) = &c.jump_chain {
            check_hops(store, hops)?;
        }
        if let Some(id) = &c.startup_snippet_id {
            store.macro_by_id(id)?;
        }
    }
    for r in &store.recents {
        if !store.connections.iter().any(|c| c.id == r.connection_id) {
            return Err(Error::InvalidReference("récente orpheline".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionDefaults, Credential};

    fn folder(id: &str, parent: Option<&str>) -> Folder {
        Folder {
            id: id.into(),
            parent_id: parent.map(Into::into),
            name: id.into(),
            color: None,
            icon: None,
            sort_order: 0,
            tags: vec![],
            defaults: ConnectionDefaults::default(),
        }
    }

    fn conn(id: &str, folder_id: &str) -> Connection {
        let now = now_rfc3339();
        Connection {
            id: id.into(),
            folder_id: folder_id.into(),
            name: id.into(),
            protocol: "ssh".into(),
            host: "h".into(),
            port: None,
            user: None,
            credential_id: None,
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

    fn cred(id: &str) -> Credential {
        Credential {
            id: id.into(),
            name: id.into(),
            username: "u".into(),
            password: None,
            private_key: None,
            passphrase: None,
            key_origin: None,
            needs_reentry: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn sample() -> Store {
        let mut store = Store::new("v".into());
        store.folders = vec![folder("a", None), folder("b", Some("a"))];
        store.connections = vec![conn("c1", "b"), conn("c2", "a")];
        store.credentials = vec![cred("k1")];
        store
    }

    #[test]
    fn folder_cycle_rejected() {
        let mut store = sample();
        let mut a = store.folders[0].clone();
        a.parent_id = Some("b".into()); // b est enfant de a → cycle
        assert!(upsert_folder(&mut store, a).is_err());
        assert!(move_folder(&mut store, "a", Some("b"), None).is_err());
        assert!(move_folder(&mut store, "a", Some("a"), None).is_err());
    }

    #[test]
    fn delete_folder_refuses_non_empty_then_cascades() {
        let mut store = sample();
        assert!(delete_folder(&mut store, "a", false).is_err());
        let removed = delete_folder(&mut store, "a", true).unwrap();
        assert_eq!(removed, 2);
        assert!(store.folders.is_empty());
        assert!(store.connections.is_empty());
    }

    #[test]
    fn connection_validation_and_duplicate() {
        let mut store = sample();
        let mut bad = conn("x", "inexistant");
        assert!(upsert_connection(&mut store, bad.clone()).is_err());
        bad.folder_id = "a".into();
        bad.host = " ".into();
        assert!(upsert_connection(&mut store, bad.clone()).is_err());
        bad.host = "h".into();
        bad.credential_id = Some("fantome".into());
        assert!(upsert_connection(&mut store, bad).is_err());

        let new_id = duplicate_connection(&mut store, "c1").unwrap();
        assert_ne!(new_id, "c1");
        let copy = store.connections.iter().find(|c| c.id == new_id).unwrap();
        assert_eq!(copy.host, "h");
        assert!(copy.name.contains("(copie)"));
        assert!(!copy.favorite);
    }

    #[test]
    fn credential_refs_direct_inherited_hop() {
        let mut store = sample();
        store.folders[0].defaults.credential_id = Some("k1".into()); // a → k1 hérité
        store.connections[0].credential_id = Some("k1".into()); // c1 direct
        store.connections[1].jump_chain = Some(vec![JumpHop {
            host: "j".into(),
            port: None,
            user: None,
            credential_id: Some("k1".into()),
        }]);
        let refs = credential_refs(&store, "k1");
        assert_eq!(refs.len(), 2); // c1 (direct+hérité), c2 (hop)
    }

    #[test]
    fn credential_delete_refuse_then_detach() {
        let mut store = sample();
        store.connections[0].credential_id = Some("k1".into());
        assert!(delete_credential(&mut store, "k1", false).is_err());
        let n = delete_credential(&mut store, "k1", true).unwrap();
        assert_eq!(n, 1);
        assert!(store.credentials.is_empty());
        assert!(store.connections[0].credential_id.is_none());
    }

    #[test]
    fn recents_order_and_cap() {
        let mut store = sample();
        touch_recent(&mut store, "c1").unwrap();
        touch_recent(&mut store, "c2").unwrap();
        touch_recent(&mut store, "c1").unwrap();
        assert_eq!(store.recents.len(), 2);
        assert_eq!(store.recents[0].connection_id, "c1");
        assert!(touch_recent(&mut store, "fantome").is_err());
    }

    #[test]
    fn validate_store_ok_and_broken() {
        let mut store = sample();
        assert!(validate_store(&store).is_ok());
        store.connections[0].folder_id = "fantome".into();
        assert!(validate_store(&store).is_err());
    }
}
