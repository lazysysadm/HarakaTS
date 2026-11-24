//! Verrouillage automatique : inactivité (timer), mise en veille et
//! verrouillage de session (logind / ScreenSaver via zbus, Linux).
//! Aucun signal système n'est bloquant : en cas d'échec D-Bus, l'app continue.

use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(inactivity_loop(app.clone()));
    #[cfg(target_os = "linux")]
    {
        tauri::async_runtime::spawn(retry_loop(app.clone(), system_signals));
        tauri::async_runtime::spawn(retry_loop(app, session_signals));
    }
}

async fn inactivity_loop(app: AppHandle) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        let should = {
            let state = app.state::<AppState>();
            let inner = state.inner.lock().await;
            inner.vault.is_some() && inner.autolock.should_lock()
        };
        if should {
            lock_now(&app, "inactivity").await;
        }
    }
}

async fn lock_now(app: &AppHandle, reason: &str) {
    let state = app.state::<AppState>();
    let mut inner = state.inner.lock().await;
    let Some(vault) = inner.vault.take() else {
        return;
    };
    let settings = &vault.store.settings;
    let allowed = match reason {
        "suspend" => settings.lock_on_suspend,
        "session-lock" => settings.lock_on_session_lock,
        _ => true,
    };
    if !allowed {
        inner.vault = Some(vault);
        return;
    }
    let disconnect = vault.store.settings.disconnect_on_lock;
    vault.lock();
    inner.previews.clear();
    drop(inner);
    if disconnect {
        crate::sessions::close_all_sessions(&state).await;
    } else {
        crate::sessions::lock_keep_sessions(&state).await;
    }
    tracing::info!(disconnect, "coffre verrouillé ({reason})");
    let _ = app.emit(
        "vault://state",
        serde_json::json!({ "state": "locked", "reason": reason }),
    );
}

#[cfg(target_os = "linux")]
#[allow(clippy::type_complexity)]
async fn retry_loop(
    app: AppHandle,
    watch: fn(
        AppHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = zbus::Result<()>> + Send>>,
) {
    loop {
        if let Err(err) = watch(app.clone()).await {
            tracing::warn!("signaux D-Bus indisponibles (non bloquant) : {err}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::type_complexity)]
fn system_signals(
    app: AppHandle,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = zbus::Result<()>> + Send>> {
    Box::pin(async move {
        use futures_util::TryStreamExt;
        let conn = zbus::Connection::system().await?;
        let sleep_rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.login1.Manager")
            .member("PrepareForSleep")
            .build()?;
        let lock_rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.login1.Session")
            .member("Lock")
            .build()?;
        let mut sleep_stream = zbus::MessageStream::for_match_rule(sleep_rule, &conn, None).await?;
        let mut lock_stream = zbus::MessageStream::for_match_rule(lock_rule, &conn, None).await?;
        loop {
            tokio::select! {
                msg = sleep_stream.try_next() => {
                    match msg? {
                        Some(m) => {
                            let active: bool = m.body().deserialize().unwrap_or(false);
                            if active {
                                lock_now(&app, "suspend").await;
                            }
                        }
                        None => return Ok(()),
                    }
                }
                msg = lock_stream.try_next() => {
                    match msg? {
                        Some(_) => lock_now(&app, "session-lock").await,
                        None => return Ok(()),
                    }
                }
            }
        }
    })
}

#[cfg(target_os = "linux")]
#[allow(clippy::type_complexity)]
fn session_signals(
    app: AppHandle,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = zbus::Result<()>> + Send>> {
    Box::pin(async move {
        use futures_util::TryStreamExt;
        let conn = zbus::Connection::session().await?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.ScreenSaver")
            .member("Lock")
            .build()?;
        let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
        while let Some(_msg) = stream.try_next().await? {
            lock_now(&app, "session-lock").await;
        }
        Ok(())
    })
}
