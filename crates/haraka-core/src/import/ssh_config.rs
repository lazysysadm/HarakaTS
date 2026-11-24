//! Parseur `~/.ssh/config` maison (tolérant, pur sur des chaînes).
//!
//! Supporté : `Host` (motifs `*` `?`, négation `!`), `Include` (glob, relatif à
//! `~/.ssh`, profondeur/cycles bornés), `HostName`, `User`, `Port`, `IdentityFile`,
//! `ProxyJump` (chaînes), `ServerAliveInterval`, `Local/Remote/DynamicForward`.
//! Non supporté, signalé : `Match` (bloc ignoré), `ProxyCommand`. Autres options :
//! une note agrégée. Sémantique OpenSSH : première valeur obtenue gagne.

use std::collections::{BTreeSet, HashSet};

use super::{
    ImportPlan, ImportSource, PlanConnection, PlanCredential, PlanHop, PlanTunnel, clean,
    file_stem, numbered_lines, split_host_port,
};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDED_FILES: usize = 200;

/// Fournit le contenu des fichiers inclus (le noyau reste pur).
pub trait IncludeProvider {
    /// Développe un motif d'`Include` (déjà débarrassé de `~`, éventuellement relatif
    /// à `~/.ssh`) en couples (chemin canonique, contenu).
    fn expand(&mut self, pattern: &str) -> Vec<(String, String)>;
}

/// Provider vide (aucun Include résolu).
pub struct NoIncludes;
impl IncludeProvider for NoIncludes {
    fn expand(&mut self, _pattern: &str) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Provider sur système de fichiers (glob `*` `?` par composant de chemin).
pub struct FsIncludes {
    pub ssh_dir: String,
}

impl IncludeProvider for FsIncludes {
    fn expand(&mut self, pattern: &str) -> Vec<(String, String)> {
        let full = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{}", self.ssh_dir.trim_end_matches('/'), pattern)
        };
        let mut found: Vec<String> = vec!["/".to_string()];
        for comp in full.split('/').filter(|c| !c.is_empty()) {
            let mut next = Vec::new();
            for base in &found {
                if comp.contains(['*', '?']) {
                    let Ok(rd) = std::fs::read_dir(base) else {
                        continue;
                    };
                    let mut names: Vec<String> = rd
                        .flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|n| !n.starts_with('.') || comp.starts_with('.'))
                        .filter(|n| glob_match(comp, n, false))
                        .collect();
                    names.sort();
                    for n in names {
                        next.push(join(base, &n));
                    }
                } else {
                    next.push(join(base, comp));
                }
                if next.len() > 1000 {
                    break;
                }
            }
            found = next;
        }
        let mut out = Vec::new();
        for p in found.into_iter().take(MAX_INCLUDED_FILES) {
            let Ok(meta) = std::fs::metadata(&p) else {
                continue;
            };
            if !meta.is_file() || meta.len() > super::MAX_INPUT_BYTES as u64 {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&p) {
                let (text, _) = super::decode_text(&bytes);
                let canon = std::fs::canonicalize(&p)
                    .map(|c| c.display().to_string())
                    .unwrap_or(p);
                out.push((canon, text));
            }
        }
        out
    }
}

fn join(base: &str, comp: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{comp}")
    } else {
        format!("{base}/{comp}")
    }
}

/// Glob `*` / `?` (sans classes), insensible à la casse si demandé.
pub fn glob_match(pat: &str, text: &str, ci: bool) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = if ci {
        (
            pat.to_lowercase().chars().collect(),
            text.to_lowercase().chars().collect(),
        )
    } else {
        (pat.chars().collect(), text.chars().collect())
    };
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// `~`, `~/…`, `%d` → home ; `%%` → `%`.
pub fn expand_path(s: &str, home: &str) -> String {
    let mut out = String::new();
    let rest = if s == "~" {
        out.push_str(home);
        ""
    } else if let Some(r) = s.strip_prefix("~/") {
        out.push_str(home);
        out.push('/');
        r
    } else {
        s
    };
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('d') => out.push_str(home),
                Some('%') => out.push('%'),
                Some(o) => {
                    out.push('%');
                    out.push(o);
                }
                None => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Découpe une ligne en (mot-clé minuscule, arguments), guillemets gérés.
fn tokenize(line: &str) -> Option<(String, Vec<String>)> {
    let l = line.trim_start();
    if l.is_empty() || l.starts_with('#') {
        return None;
    }
    let kw_end = l
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(l.len());
    let keyword = l[..kw_end].to_lowercase();
    let mut rest = l[kw_end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    let mut prev_ws = true;
    for c in rest.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            '#' if !in_q && prev_ws => break,
            c if c.is_whitespace() && !in_q => {
                if has {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
        prev_ws = c.is_whitespace();
    }
    if has {
        args.push(cur);
    }
    Some((keyword, args))
}

struct Block {
    patterns: Vec<String>,
    entries: Vec<(String, Vec<String>)>,
}

impl Block {
    fn matches(&self, host: &str) -> bool {
        let mut positive = false;
        for p in &self.patterns {
            if let Some(neg) = p.strip_prefix('!') {
                if glob_match(neg, host, true) {
                    return false;
                }
            } else if glob_match(p, host, true) {
                positive = true;
            }
        }
        positive
    }

    fn is_pure_star(&self) -> bool {
        self.patterns.len() == 1 && self.patterns[0] == "*"
    }
}

struct Parser<'a> {
    provider: &'a mut dyn IncludeProvider,
    ssh_dir: &'a str,
    home: &'a str,
    blocks: Vec<Block>,
    ignored: BTreeSet<String>,
    included_files: usize,
}

const KNOWN: &[&str] = &[
    "hostname",
    "user",
    "port",
    "identityfile",
    "proxyjump",
    "proxycommand",
    "serveraliveinterval",
    "localforward",
    "remoteforward",
    "dynamicforward",
];

impl Parser<'_> {
    fn parse_text(
        &mut self,
        plan: &mut ImportPlan,
        text: &str,
        depth: usize,
        stack: &mut Vec<String>,
        mut cur: Option<usize>,
    ) {
        for (lineno, line) in numbered_lines(text, plan) {
            let Some((kw, args)) = tokenize(line) else {
                continue;
            };
            match kw.as_str() {
                "host" => {
                    self.blocks.push(Block {
                        patterns: args,
                        entries: Vec::new(),
                    });
                    cur = Some(self.blocks.len() - 1);
                }
                "match" => {
                    plan.skip("ssh_match_unsupported", &[("line", &lineno.to_string())]);
                    cur = None;
                }
                "include" => {
                    if depth >= MAX_INCLUDE_DEPTH {
                        plan.warn("ssh_include_depth", &[("line", &lineno.to_string())]);
                        continue;
                    }
                    for pat in &args {
                        let pattern = expand_path(pat, self.home);
                        for (path, content) in self.provider.expand(&pattern) {
                            if stack.contains(&path) {
                                plan.warn("ssh_include_cycle", &[("path", &path)]);
                                continue;
                            }
                            self.included_files += 1;
                            if self.included_files > MAX_INCLUDED_FILES {
                                plan.warn("ssh_include_limit", &[]);
                                return;
                            }
                            stack.push(path);
                            self.parse_text(plan, &content, depth + 1, stack, cur);
                            stack.pop();
                        }
                    }
                }
                k => {
                    if !KNOWN.contains(&k) {
                        self.ignored.insert(k.to_string());
                        continue;
                    }
                    if let Some(i) = cur {
                        self.blocks[i].entries.push((kw, args));
                    }
                }
            }
        }
    }
}

#[derive(Default, Clone)]
struct Eff {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<String>,
    identity: Vec<String>,
    proxyjump: Option<String>,
    proxycommand: bool,
    keepalive: Option<String>,
    forwards: Vec<(String, Vec<String>)>,
}

fn effective<F: Fn(&Block) -> bool>(blocks: &[Block], pred: F) -> Eff {
    let mut e = Eff::default();
    for b in blocks.iter().filter(|b| pred(b)) {
        for (k, a) in &b.entries {
            let first = a.first().cloned();
            match k.as_str() {
                "hostname" if e.hostname.is_none() => e.hostname = first,
                "user" if e.user.is_none() => e.user = first,
                "port" if e.port.is_none() => e.port = first,
                "proxyjump" if e.proxyjump.is_none() => e.proxyjump = first,
                "serveraliveinterval" if e.keepalive.is_none() => e.keepalive = first,
                "proxycommand" => {
                    if !e.proxycommand && first.as_deref().is_some_and(|v| v != "none") {
                        e.proxycommand = true;
                    }
                }
                "identityfile" => {
                    if let Some(f) = first
                        && !e.identity.contains(&f)
                    {
                        e.identity.push(f);
                    }
                }
                "localforward" | "remoteforward" | "dynamicforward" => {
                    e.forwards.push((k.clone(), a.clone()));
                }
                _ => {}
            }
        }
    }
    e
}

fn parse_port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

fn parse_hop(spec: &str, aliases: &[String], blocks: &[Block]) -> Option<PlanHop> {
    let spec = spec.trim();
    let spec = spec.strip_prefix("ssh://").unwrap_or(spec);
    let spec = spec.trim_end_matches('/');
    let (user, hostport) = match spec.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()), h),
        None => (None, spec),
    };
    let (host, port) = split_host_port(hostport);
    if host.is_empty() {
        return None;
    }
    // Alias connu : on reprend son HostName / User / Port.
    if aliases.iter().any(|a| a.eq_ignore_ascii_case(&host)) {
        let e = effective(blocks, |b| b.matches(&host));
        return Some(PlanHop {
            host: e
                .hostname
                .map_or_else(|| host.clone(), |h| h.replace("%h", &host)),
            port: port.or_else(|| e.port.as_deref().and_then(parse_port)),
            user: user.or(e.user),
        });
    }
    Some(PlanHop { host, port, user })
}

fn parse_chain(spec: &str, aliases: &[String], blocks: &[Block]) -> Option<Vec<PlanHop>> {
    if spec.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let hops: Vec<PlanHop> = spec
        .split(',')
        .filter_map(|h| parse_hop(h, aliases, blocks))
        .collect();
    if hops.is_empty() { None } else { Some(hops) }
}

fn norm_bind(b: &str) -> String {
    match b {
        "*" | "" => "0.0.0.0".to_string(),
        other => other.trim_matches(['[', ']']).to_string(),
    }
}

fn parse_forward(kind: &str, args: &[String]) -> Option<PlanTunnel> {
    let first = args.first()?;
    let (bind, port) = match first.rsplit_once(':') {
        Some((b, p)) => (norm_bind(b), parse_port(p)?),
        None => ("127.0.0.1".to_string(), parse_port(first)?),
    };
    if kind == "dynamicforward" {
        return Some(PlanTunnel {
            kind: "dynamic".into(),
            bind_addr: bind,
            bind_port: port,
            dest_host: None,
            dest_port: None,
        });
    }
    let dest = args.get(1)?;
    let (dh, dp) = split_host_port(dest);
    let dp = dp.or_else(|| args.get(2).and_then(|p| parse_port(p)))?;
    if dh.is_empty() {
        return None;
    }
    Some(PlanTunnel {
        kind: if kind == "localforward" {
            "local"
        } else {
            "remote"
        }
        .into(),
        bind_addr: bind,
        bind_port: port,
        dest_host: Some(dh),
        dest_port: Some(dp),
    })
}

fn is_concrete(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('!') && !p.contains(['*', '?', ','])
}

/// Analyse le contenu d'un `~/.ssh/config`. `ssh_dir` : base des `Include` relatifs.
pub fn parse(
    text: &str,
    home: &str,
    ssh_dir: &str,
    provider: &mut dyn IncludeProvider,
) -> ImportPlan {
    let mut plan = ImportPlan::new(ImportSource::SshConfig);
    let mut parser = Parser {
        provider,
        ssh_dir,
        home,
        blocks: vec![Block {
            patterns: vec!["*".into()],
            entries: Vec::new(),
        }],
        ignored: BTreeSet::new(),
        included_files: 0,
    };
    let _ = parser.ssh_dir;
    let mut stack = vec!["<main>".to_string()];
    parser.parse_text(&mut plan, text, 0, &mut stack, Some(0));
    let Parser {
        blocks, ignored, ..
    } = parser;

    // Alias concrets, dans l'ordre d'apparition.
    let mut aliases: Vec<String> = Vec::new();
    for b in blocks.iter().skip(1) {
        for p in b.patterns.iter().filter(|p| is_concrete(p)) {
            if !aliases.iter().any(|a| a.eq_ignore_ascii_case(p)) {
                aliases.push(p.clone());
            }
        }
    }
    // Motifs joker n'agissant sur aucun hôte.
    for b in blocks.iter().skip(1) {
        let wild = b.patterns.iter().any(|p| !is_concrete(p));
        if wild && !b.is_pure_star() && !aliases.iter().any(|a| b.matches(a)) {
            let pats = b.patterns.join(" ");
            plan.warn("ssh_wildcard_unused", &[("pattern", &clean(&pats))]);
        }
    }

    // Défauts du dossier racine : bloc global + `Host *`.
    let root = effective(&blocks, |b| b.is_pure_star());
    let root_user = root.user.clone();
    let root_port = root.port.as_deref().and_then(parse_port);
    let root_keep = root
        .keepalive
        .as_deref()
        .and_then(|v| v.parse::<u32>().ok());
    let root_identity = root.identity.first().map(|p| expand_path(p, home));
    plan.root_defaults.user = root_user.clone();
    plan.root_defaults.port = root_port;
    plan.root_defaults.keepalive_s = root_keep.filter(|k| *k > 0);
    if let Some(spec) = &root.proxyjump
        && let Some(chain) = parse_chain(spec, &aliases, &blocks)
    {
        plan.root_defaults.jump_chain = chain;
    }
    if let Some(path) = &root_identity {
        plan.add_credential(PlanCredential {
            key: path.clone(),
            name: format!("{} (ssh_config)", file_stem(path)),
            username: root_user.clone().unwrap_or_default(),
            key_file: Some(path.clone()),
            password_needs_reentry: false,
        });
        plan.root_defaults.credential = Some(path.clone());
    }
    if root.proxycommand {
        plan.skip("ssh_proxycommand", &[("hosts", "*")]);
    }

    let mut proxycmd_hosts: Vec<String> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for alias in &aliases {
        let e = effective(&blocks, |b| b.matches(alias));
        let host = clean(&e.hostname.as_deref().map_or_else(
            || alias.clone(),
            |h| h.replace("%h", alias).replace("%%", "%"),
        ));
        let port = match &e.port {
            Some(p) => match parse_port(p) {
                Some(v) => Some(v),
                None => {
                    plan.warn("ssh_invalid_port", &[("host", alias), ("value", &clean(p))]);
                    None
                }
            },
            None => None,
        };
        let user_in = e.user.clone().filter(|u| !u.is_empty());
        let credential = e
            .identity
            .first()
            .map(|p| expand_path(p, home))
            .filter(|p| Some(p) != root_identity.as_ref());
        if e.identity.len() > 1 {
            plan.warn("ssh_extra_identities", &[("host", alias)]);
        }
        if let Some(path) = &credential {
            plan.add_credential(PlanCredential {
                key: path.clone(),
                name: format!("{} (ssh_config)", file_stem(path)),
                username: user_in.clone().or(root_user.clone()).unwrap_or_default(),
                key_file: Some(path.clone()),
                password_needs_reentry: false,
            });
        }
        let jump = match &e.proxyjump {
            Some(spec) if Some(spec) == root.proxyjump.as_ref() => None,
            Some(spec) => match parse_chain(spec, &aliases, &blocks) {
                Some(c) => Some(c),
                None => {
                    plan.warn("ssh_invalid_proxyjump", &[("host", alias)]);
                    None
                }
            },
            None => None,
        };
        if e.proxycommand && !root.proxycommand {
            proxycmd_hosts.push(alias.clone());
        }
        let mut tunnels = Vec::new();
        for (k, a) in &e.forwards {
            match parse_forward(k, a) {
                Some(t) => tunnels.push(t),
                None => plan.warn(
                    "ssh_forward_invalid",
                    &[("host", alias), ("spec", &clean(&a.join(" ")))],
                ),
            }
        }
        let mut name = clean(alias);
        if name.is_empty() || !seen_names.insert(name.to_lowercase()) {
            name = format!("{name} ({host})");
        }
        plan.connections.push(PlanConnection {
            name,
            host,
            port: port.filter(|p| Some(*p) != root_port),
            user: user_in.filter(|u| Some(u) != root_user.as_ref()),
            folder_path: Vec::new(),
            credential,
            jump_chain: jump,
            keepalive_s: e
                .keepalive
                .as_deref()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|k| *k > 0 && Some(*k) != root_keep),
            tunnels,
        });
    }
    if !proxycmd_hosts.is_empty() {
        plan.skipped += proxycmd_hosts.len();
        plan.warn(
            "ssh_proxycommand",
            &[("hosts", &clean(&proxycmd_hosts.join(", ")))],
        );
    }
    if !ignored.is_empty() {
        let list: Vec<String> = ignored.into_iter().take(40).collect();
        plan.warn("ssh_options_ignored", &[("options", &list.join(", "))]);
    }
    if plan.connections.is_empty() && plan.warnings.is_empty() {
        plan.warn("nothing_found", &[]);
    }
    plan
}
