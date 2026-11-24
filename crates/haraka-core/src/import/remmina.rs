//! Import des fichiers `.remmina` (INI). Seul `protocol=SSH` est importé ; les
//! autres protocoles sont ignorés avec une note. `password=` (chiffré par la clé
//! locale de Remmina) n'est pas importable → credential `needs_reentry`.
//! Le déchiffrement via `remmina.pref` est hors périmètre.

use super::ssh_config::expand_path;
use super::{
    ImportPlan, ImportSource, PlanConnection, PlanCredential, PlanHop, clean, file_stem, parse_ini,
    split_host_port,
};

fn get<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// `files` : (nom de fichier, contenu texte déjà décodé).
pub fn parse(files: &[(String, String)], home: &str) -> ImportPlan {
    let mut plan = ImportPlan::new(ImportSource::Remmina);
    for (file, text) in files {
        parse_one(&mut plan, file, text, home);
    }
    if plan.connections.is_empty() && plan.warnings.is_empty() {
        plan.warn("nothing_found", &[]);
    }
    plan
}

fn parse_one(plan: &mut ImportPlan, file: &str, text: &str, home: &str) {
    let fname = clean(file);
    let sections = parse_ini(text, plan);
    let Some(sec) = sections
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case("remmina"))
    else {
        plan.skip("remmina_no_section", &[("file", &fname)]);
        return;
    };
    let e = &sec.entries;
    let protocol = get(e, "protocol").unwrap_or("");
    if !protocol.eq_ignore_ascii_case("ssh") {
        plan.skip(
            "remmina_non_ssh",
            &[("file", &fname), ("protocol", &clean(protocol))],
        );
        return;
    }
    let Some(server) = get(e, "server") else {
        plan.skip("remmina_no_server", &[("file", &fname)]);
        return;
    };
    let (host, port) = split_host_port(server);
    let host = clean(&host);
    if host.is_empty() {
        plan.skip("remmina_no_server", &[("file", &fname)]);
        return;
    }
    let name = get(e, "name").map_or_else(|| file_stem(&fname), clean);
    let user = get(e, "username").map(clean).filter(|u| !u.is_empty());
    let folder_path: Vec<String> = get(e, "group")
        .map(|g| g.split('/').map(clean).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    plan.add_folder_path(&folder_path);

    let key_file = get(e, "ssh_privatekey").map(|p| expand_path(p, home));
    let has_password = get(e, "password").is_some();
    let mut credential = None;
    if user.is_some() || has_password || key_file.is_some() {
        let uname = user.clone().unwrap_or_default();
        let key = format!("remmina:{uname}:{}", key_file.clone().unwrap_or_default());
        let cname = match (&key_file, &user) {
            (Some(k), _) => format!("{} (Remmina)", file_stem(k)),
            (None, Some(u)) => format!("{u} (Remmina)"),
            (None, None) => format!("{name} (Remmina)"),
        };
        let pw = has_password;
        // Un mot de passe attaché à un utilisateur vide est propre à la connexion.
        let key = if user.is_none() && has_password {
            format!("remmina-conn:{fname}:{name}")
        } else {
            key
        };
        plan.add_credential(PlanCredential {
            key: key.clone(),
            name: cname,
            username: uname,
            key_file: key_file.clone(),
            password_needs_reentry: pw,
        });
        if let Some(c) = plan.credentials.iter_mut().find(|c| c.key == key) {
            c.password_needs_reentry |= pw;
        }
        credential = Some(key);
    }

    let mut jump = None;
    let tunnel_on = get(e, "ssh_tunnel_enabled").or(get(e, "ssh_enabled")) == Some("1");
    if tunnel_on {
        match get(e, "ssh_tunnel_server").or(get(e, "ssh_server")) {
            Some(srv) => {
                let (h, p) = split_host_port(srv);
                let h = clean(&h);
                if h.is_empty() {
                    plan.warn("remmina_tunnel_invalid", &[("file", &fname)]);
                } else {
                    jump = Some(vec![PlanHop {
                        host: h,
                        port: p,
                        user: get(e, "ssh_tunnel_username")
                            .or(get(e, "ssh_username"))
                            .map(clean)
                            .filter(|u| !u.is_empty()),
                    }]);
                }
            }
            None => plan.warn("remmina_tunnel_invalid", &[("file", &fname)]),
        }
    }

    plan.connections.push(PlanConnection {
        name,
        host,
        port,
        user,
        folder_path,
        credential,
        jump_chain: jump,
        keepalive_s: None,
        tunnels: Vec::new(),
    });
}
