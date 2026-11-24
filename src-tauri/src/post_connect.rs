//! Actions déclenchées à chaque (re)connexion d'une session native : suivi du
//! répertoire (OSC 7, opt-in), tunnels « démarrage automatique », snippet de
//! démarrage. Point d'extension unique appelé par `forward_native` à l'état
//! `connected` ; chaque action est indépendante et ne doit jamais faire échouer
//! la session (erreurs journalisées).

use tauri::{AppHandle, Manager};

use crate::state::AppState;

pub async fn run(app: AppHandle, sid: String) {
    follow_hook(&app, &sid).await;
    crate::tunnels::auto_start(&app, &sid).await;
    crate::macros::run_startup_snippet(&app, &sid).await;
}

/// Suivi du répertoire (OSC 7) : injection opt-in à chaque (re)connexion.
async fn follow_hook(app: &AppHandle, sid: &str) {
    let state = app.state::<AppState>();
    let enabled = state
        .inner
        .lock()
        .await
        .vault
        .as_ref()
        .is_some_and(|v| v.store.settings.sftp_follow_terminal);
    if enabled {
        // Laisse le shell afficher son premier prompt (jusqu'à 5 s : bannière, MOTD lents).
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let tail = state
            .session_io
            .lock()
            .await
            .get(sid)
            .map(|io| io.clip.tail.clone());
        if let Some(tail) = tail {
            // Attend que la sortie se calme (2 relevés identiques et non vides, 5 s max).
            let mut prev = String::new();
            let mut stable = 0;
            for _ in 0..20 {
                let now = tail.lock().map(|t| t.clone()).unwrap_or_default();
                if !now.trim().is_empty() && now == prev {
                    stable += 1;
                    if stable >= 2 {
                        break;
                    }
                } else {
                    stable = 0;
                }
                prev = now;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            // Le hook est une syntaxe bash/zsh : seulement vers un serveur OpenSSH. Cisco, MikroTik
            // (ROSSSH), Dropbear/BusyBox, etc. ne l'interprètent pas et peuvent se bloquer.
            let ident = state.sessions.server_ident(sid).await.unwrap_or_default();
            if !ident_accepts_hook(&ident) {
                tracing::debug!(session = %sid, ident = %ident, "serveur non OpenSSH : hook de suivi non injecté");
                return;
            }
            let last = tail.lock().map(|t| t.clone()).unwrap_or_default();
            if !prompt_accepts_hook(&last) {
                tracing::debug!(session = %sid, "prompt non POSIX : hook de suivi non injecté");
                return;
            }
        }
        if let Err(e) = crate::sftp::inject_follow_hook(&state, sid).await {
            tracing::debug!(session = %sid, "suivi du répertoire non activé : {e}");
        }
    }
}

const TAIL_MAX: usize = 256;

/// Garde la fin de la sortie (sans séquences d'échappement CSI/OSC) pour reconnaître le prompt.
pub fn remember_tail(tail: &std::sync::Mutex<String>, text: &str) {
    let clean = strip_escapes(text);
    if clean.is_empty() {
        return;
    }
    let Ok(mut t) = tail.lock() else { return };
    t.push_str(&clean);
    if t.len() > TAIL_MAX {
        let mut cut = t.len() - TAIL_MAX;
        while !t.is_char_boundary(cut) {
            cut += 1;
        }
        t.drain(..cut);
    }
}

fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            if c != '\x07' && c != '\r' {
                out.push(c);
            }
            continue;
        }
        match it.next() {
            Some('[') => {
                for n in it.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(n) = it.next() {
                    if n == '\x07' {
                        break;
                    }
                    if n == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Serveur OpenSSH uniquement (`SSH-2.0-OpenSSH_9.6p1 …`). Identification inconnue : refus.
pub fn ident_accepts_hook(ident: &str) -> bool {
    ident
        .trim()
        .to_ascii_lowercase()
        .split_once('-')
        .and_then(|(_, rest)| rest.split_once('-'))
        .is_some_and(|(_, software)| software.starts_with("openssh"))
}

/// Le hook (syntaxe bash/zsh) est injecté seulement si le prompt ne ressemble pas à celui d'un
/// équipement non POSIX : RouterOS (`[admin@MikroTik] > `), routeurs, shells à prompt `>`.
/// Prompt inconnu : on injecte (comportement historique).
pub fn prompt_accepts_hook(tail: &str) -> bool {
    if tail.contains("RouterOS") || tail.contains("MikroTik") {
        return false;
    }
    let last = tail.lines().rev().find(|l| !l.trim().is_empty());
    match last {
        Some(l) => !l.trim_end().ends_with('>'),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routeros_prompt_refused() {
        assert!(!prompt_accepts_hook("\n[admin@MikroTik] > "));
        assert!(!prompt_accepts_hook("Switch>"));
        assert!(!prompt_accepts_hook(
            "MikroTik RouterOS 7.16\n[admin@x] \u{1b}"
        ));
    }

    #[test]
    fn only_openssh_idents_accepted() {
        assert!(ident_accepts_hook("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3"));
        assert!(!ident_accepts_hook("SSH-2.0-ROSSSH"));
        assert!(!ident_accepts_hook("SSH-2.0-Cisco-1.25"));
        assert!(!ident_accepts_hook("SSH-2.0-dropbear_2022.83"));
        assert!(!ident_accepts_hook(""));
    }

    #[test]
    fn posix_prompts_accepted() {
        assert!(prompt_accepts_hook("user@host:~$ "));
        assert!(prompt_accepts_hook("root@host:/# "));
        assert!(prompt_accepts_hook("host% "));
        assert!(prompt_accepts_hook(""));
    }

    #[test]
    fn escapes_removed_from_tail() {
        let t = std::sync::Mutex::new(String::new());
        remember_tail(&t, "\x1b[1;32muser@h\x1b[0m:~$ \x1b]0;title\x07");
        assert_eq!(t.lock().unwrap().as_str(), "user@h:~$ ");
    }
}
