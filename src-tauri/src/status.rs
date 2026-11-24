//! État des serveurs (pastille verte/rouge de l'arbre, à la mRemoteNG), option désactivée par défaut.
//!
//! Sonde : simple **connexion TCP** vers `host:port` (celui de SSH), refermée aussitôt — pas
//! d'ICMP (qui demande des privilèges) et aucun échange SSH. Garde-fous anti-flood : intervalle
//! réglable (60 s par défaut, 15 s minimum), 8 sondes en parallèle au plus, délai de 2 s, un seul
//! test par couple hôte:port même partagé par plusieurs connexions, rien quand le coffre est
//! verrouillé ou l'option coupée, et les connexions derrière un rebond SSH ne sont pas testées
//! (état inconnu). Les résultats ne sont émis vers l'interface que lorsqu'ils changent.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::state::AppState;

const TICK: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PARALLEL: usize = 8;
const MAX_TARGETS: usize = 500;
const MIN_INTERVAL_S: u32 = 15;

static LAST: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static KICK: OnceLock<Notify> = OnceLock::new();

fn last() -> &'static Mutex<HashMap<String, bool>> {
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

fn kick_handle() -> &'static Notify {
    KICK.get_or_init(Notify::new)
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(run(app));
}

/// Dernier résultat connu : identifiant de connexion → joignable.
#[tauri::command]
pub fn status_snapshot() -> HashMap<String, bool> {
    last().lock().unwrap().clone()
}

/// Relance un cycle tout de suite (réglage modifié, déverrouillage).
#[tauri::command]
pub fn status_refresh() {
    kick_handle().notify_one();
}

struct Config {
    enabled: bool,
    interval: Duration,
    /// (identifiant de connexion, hôte, port) des connexions testables.
    targets: Vec<(String, String, u16)>,
}

async fn read_config(app: &AppHandle) -> Config {
    let state = app.state::<AppState>();
    let inner = state.inner.lock().await;
    let Some(vault) = inner.vault.as_ref() else {
        return Config {
            enabled: false,
            interval: TICK,
            targets: Vec::new(),
        };
    };
    let settings = &vault.store.settings;
    let interval = Duration::from_secs(u64::from(
        settings.server_status_interval_s.max(MIN_INTERVAL_S),
    ));
    if !settings.server_status {
        return Config {
            enabled: false,
            interval,
            targets: Vec::new(),
        };
    }
    let mut targets = Vec::new();
    for conn in &vault.store.connections {
        let eff = haraka_core::effective::resolve(&vault.store, conn);
        let hops = eff.jump_chain.value.as_ref().is_some_and(|h| !h.is_empty());
        if hops || eff.host.trim().is_empty() {
            continue;
        }
        targets.push((
            conn.id.clone(),
            eff.host.clone(),
            eff.port.value.unwrap_or(22),
        ));
    }
    Config {
        enabled: true,
        interval,
        targets,
    }
}

async fn probe(host: String, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::net::TcpStream::connect((host.as_str(), port))
        )
        .await,
        Ok(Ok(_))
    )
}

async fn run(app: AppHandle) {
    let mut last_run: Option<Instant> = None;
    loop {
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            () = kick_handle().notified() => { last_run = None; }
        }
        let cfg = read_config(&app).await;
        if !cfg.enabled {
            let had = {
                let mut l = last().lock().unwrap();
                let had = !l.is_empty();
                l.clear();
                had
            };
            if had {
                let _ = app.emit("status://update", HashMap::<String, bool>::new());
            }
            last_run = None;
            continue;
        }
        if last_run.is_some_and(|t| t.elapsed() < cfg.interval) {
            continue;
        }
        last_run = Some(Instant::now());

        // Un seul test par hôte:port, même partagé par plusieurs connexions.
        let mut uniq: Vec<(String, u16)> = Vec::new();
        let mut seen = HashSet::new();
        for (_, host, port) in &cfg.targets {
            if seen.insert((host.clone(), *port)) {
                uniq.push((host.clone(), *port));
            }
        }
        uniq.truncate(MAX_TARGETS);
        let results: HashMap<(String, u16), bool> = futures_util::stream::iter(uniq)
            .map(|(host, port)| async move {
                let up = probe(host.clone(), port).await;
                ((host, port), up)
            })
            .buffer_unordered(MAX_PARALLEL)
            .collect()
            .await;

        let map: HashMap<String, bool> = cfg
            .targets
            .iter()
            .filter_map(|(id, host, port)| {
                results
                    .get(&(host.clone(), *port))
                    .map(|up| (id.clone(), *up))
            })
            .collect();
        let changed = {
            let mut l = last().lock().unwrap();
            if *l == map {
                false
            } else {
                *l = map.clone();
                true
            }
        };
        if changed {
            let _ = app.emit("status://update", map);
        }
    }
}
