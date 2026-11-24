//! Import `.mxtsessions` (MobaXterm, INI-like). Sections `[Bookmarks]`,
//! `[Bookmarks_N]` : `SubRep=` (chemin de dossiers séparé par `\`), `ImgNum=`,
//! entrées `nom=#109#0%hôte%port%utilisateur%…` (`#109#` = SSH). Les autres types
//! de session sont ignorés (note agrégée). Mots de passe non importables →
//! `needs_reentry`.

use std::collections::BTreeMap;

use super::{ImportPlan, ImportSource, PlanConnection, PlanCredential, clean, parse_ini};

const SSH_TYPE: u32 = 109;

pub fn parse(text: &str) -> ImportPlan {
    let mut plan = ImportPlan::new(ImportSource::Mobaxterm);
    let sections = parse_ini(text, &mut plan);
    let mut non_ssh: BTreeMap<String, usize> = BTreeMap::new();
    let mut malformed = 0usize;
    for sec in sections
        .iter()
        .filter(|s| s.name.to_lowercase().starts_with("bookmarks"))
    {
        let sub = sec
            .entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("SubRep"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let path: Vec<String> = sub
            .split(['\\', '/'])
            .map(clean)
            .filter(|s| !s.is_empty())
            .collect();
        for (name, value) in &sec.entries {
            if name.eq_ignore_ascii_case("SubRep") || name.eq_ignore_ascii_case("ImgNum") {
                continue;
            }
            let Some(rest) = value.strip_prefix('#') else {
                malformed += 1;
                continue;
            };
            let Some((ty, body)) = rest.split_once('#') else {
                malformed += 1;
                continue;
            };
            let Ok(ty) = ty.parse::<u32>() else {
                malformed += 1;
                continue;
            };
            if ty != SSH_TYPE {
                *non_ssh.entry(ty.to_string()).or_default() += 1;
                continue;
            }
            let fields: Vec<&str> = body.split('%').collect();
            let host = clean(fields.get(1).copied().unwrap_or(""));
            if host.is_empty() {
                plan.skip("mxt_no_host", &[("name", &clean(name))]);
                continue;
            }
            let port = fields
                .get(2)
                .and_then(|p| p.trim().parse::<u16>().ok())
                .filter(|p| *p != 0 && *p != 22);
            let user = fields
                .get(3)
                .map(|u| clean(u))
                .filter(|u| !u.is_empty() && !(u.starts_with('<') && u.ends_with('>')));
            plan.add_folder_path(&path);
            let credential = user.as_ref().map(|u| {
                let key = format!("mxt:{u}");
                plan.add_credential(PlanCredential {
                    key: key.clone(),
                    name: format!("{u} (MobaXterm)"),
                    username: u.clone(),
                    key_file: None,
                    password_needs_reentry: true,
                });
                key
            });
            plan.connections.push(PlanConnection {
                name: clean(name),
                host,
                port,
                user,
                folder_path: path.clone(),
                credential,
                jump_chain: None,
                keepalive_s: None,
                tunnels: Vec::new(),
            });
        }
    }
    if !non_ssh.is_empty() {
        let count: usize = non_ssh.values().sum();
        let types: Vec<String> = non_ssh.keys().map(|t| format!("#{t}#")).collect();
        plan.skipped += count;
        plan.warn(
            "mxt_non_ssh",
            &[("count", &count.to_string()), ("types", &types.join(", "))],
        );
    }
    if malformed > 0 {
        plan.skipped += malformed;
        plan.warn("mxt_malformed", &[("count", &malformed.to_string())]);
    }
    if plan.connections.is_empty() && plan.warnings.is_empty() {
        plan.warn("nothing_found", &[]);
    }
    plan
}
