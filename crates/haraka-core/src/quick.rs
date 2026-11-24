//! Barre de connexion rapide : parse `[user@]host[:port]` (jalon M2).

use serde::{Deserialize, Serialize};

use crate::crypto::now_rfc3339;
use crate::error::{Error, Result};
use crate::model::Connection;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Parse `user@host:port`, `host:port`, `user@host`, `host`,
/// et les littéraux IPv6 entre crochets `user@[::1]:22`.
pub fn parse(input: &str) -> Result<QuickSpec> {
    let s = input.trim();
    if s.is_empty() {
        return Err(Error::InvalidParams("saisie vide".into()));
    }
    if s.contains("://") || s.contains(char::is_whitespace) {
        return Err(Error::InvalidParams(
            "format attendu : [user@]host[:port]".into(),
        ));
    }
    let (user, rest) = match s.split_once('@') {
        Some((u, r)) => {
            if u.is_empty() {
                return Err(Error::InvalidParams("utilisateur vide avant @".into()));
            }
            (Some(u.to_string()), r)
        }
        None => (None, s),
    };
    let (host, port) = if let Some(stripped) = rest.strip_prefix('[') {
        // IPv6 littéral : [adresse] ou [adresse]:port
        let (addr, tail) = stripped
            .split_once(']')
            .ok_or_else(|| Error::InvalidParams("IPv6 : crochet fermant manquant".into()))?;
        if addr.is_empty() {
            return Err(Error::InvalidParams("adresse IPv6 vide".into()));
        }
        let port = match tail {
            "" => None,
            t => Some(parse_port(t.strip_prefix(':').ok_or_else(|| {
                Error::InvalidParams("format attendu : [adresse]:port".into())
            })?)?),
        };
        (addr.to_string(), port)
    } else {
        match rest.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(parse_port(p)?)),
            None => (rest.to_string(), None),
        }
    };
    if host.is_empty() {
        return Err(Error::InvalidParams("hôte vide".into()));
    }
    Ok(QuickSpec { user, host, port })
}

fn parse_port(p: &str) -> Result<u16> {
    let port: u16 = p
        .parse()
        .map_err(|_| Error::InvalidParams(format!("port invalide : {p}")))?;
    if port == 0 {
        return Err(Error::InvalidParams("port 0 invalide".into()));
    }
    Ok(port)
}

/// Convertit une saisie rapide en connexion sauvegardable.
pub fn to_connection(spec: &QuickSpec, name: &str, folder_id: &str) -> Connection {
    let now = now_rfc3339();
    Connection {
        id: String::new(), // upsert_connection génère l'UUID
        folder_id: folder_id.to_string(),
        name: name.to_string(),
        protocol: "ssh".to_string(),
        host: spec.host.clone(),
        port: spec.port,
        user: spec.user.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        assert_eq!(
            parse("deploy@web-01:2222").unwrap(),
            QuickSpec {
                user: Some("deploy".into()),
                host: "web-01".into(),
                port: Some(2222)
            }
        );
        assert_eq!(
            parse("host").unwrap(),
            QuickSpec {
                user: None,
                host: "host".into(),
                port: None
            }
        );
        assert_eq!(parse("host:22").unwrap().port, Some(22));
        assert_eq!(parse("u@host").unwrap().user.as_deref(), Some("u"));
        assert_eq!(parse("  u@host:22  ").unwrap().host, "host");
        let v6 = parse("u@[::1]:22").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.port, Some(22));
        assert_eq!(parse("[::1]").unwrap().host, "::1");
    }

    #[test]
    fn parse_rejects_invalid() {
        for bad in [
            "",
            "   ",
            "a b",
            "ssh://host",
            "host:abc",
            "host:0",
            "host:99999",
            "@host",
            "user@",
            "[::1",
            "[]",
        ] {
            assert!(parse(bad).is_err(), "accepté à tort : {bad}");
        }
    }
}
