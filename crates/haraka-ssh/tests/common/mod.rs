//! Serveur SSH de test in-process (russh server) — hermétique, sans réseau
//! externe, pour les tests d'authentification, host keys, jump hosts et shell.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Config, Handler, Msg, Session};
use russh::{Channel, ChannelId, Disconnect, MethodSet};
use tokio::net::{TcpListener, TcpStream};

pub struct TestServerSpec {
    pub password: Option<String>,
    pub authorized_key: Option<PublicKey>,
    pub otp: Option<String>,
    pub host_key: PrivateKey,
    /// Déconnexion serveur après N ms (test de reconnexion).
    pub close_after_ms: Option<u64>,
}

impl TestServerSpec {
    pub fn password(password: &str) -> Self {
        Self {
            password: Some(password.to_string()),
            authorized_key: None,
            otp: None,
            host_key: PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap(),
            close_after_ms: None,
        }
    }
}

pub struct TestServer {
    pub addr: std::net::SocketAddr,
    pub window: Arc<(AtomicU32, AtomicU32)>,
    pub host_key_b64: String,
}

struct TestHandler {
    spec: Arc<TestServerSpec>,
    window: Arc<(AtomicU32, AtomicU32)>,
    shell_channels: std::collections::HashSet<ChannelId>,
}

impl Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == "test" && self.spec.password.as_deref() == Some(password) {
            return Ok(Auth::Accept);
        }
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::server_supported()),
            partial_success: false,
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        if user == "test" && self.spec.authorized_key.as_ref() == Some(key.public_key) {
            return Ok(Auth::Accept);
        }
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::server_supported()),
            partial_success: false,
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if user != "test" || self.spec.otp.is_none() {
            return Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::server_supported()),
                partial_success: false,
            });
        }
        match response {
            None => Ok(Auth::Partial {
                name: "OTP".into(),
                instructions: "Code de vérification".into(),
                prompts: vec![("Code : ".into(), false)].into(),
            }),
            Some(resp) => {
                let answers: Vec<String> = resp
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .collect();
                if answers.first().map(String::as_str) == self.spec.otp.as_deref() {
                    Ok(Auth::Accept)
                } else {
                    Ok(Auth::Reject {
                        proceed_with_methods: Some(MethodSet::server_supported()),
                        partial_success: false,
                    })
                }
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shell_channels.insert(channel.id());
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"bienvenue-test\r\n".to_vec())?;
        if let Some(ms) = self.spec.close_after_ms {
            let handle = session.handle();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let _ = handle
                    .disconnect(Disconnect::ByApplication, "test-drop".into(), "".into())
                    .await;
            });
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Écho uniquement sur les canaux shell — jamais sur direct-tcpip
        // (sinon le flux du serveur cible est dupliqué vers le client imbriqué).
        if self.shell_channels.contains(&channel) {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.window.0.store(col_width, Ordering::SeqCst);
        self.window.1.store(row_height, Ordering::SeqCst);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        let target = format!("{host_to_connect}:{port_to_connect}");
        tokio::spawn(async move {
            if let Ok(mut tcp) = TcpStream::connect(&target).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
            }
        });
        Ok(())
    }
}

impl TestServer {
    pub async fn start(spec: TestServerSpec) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_key_b64 = B64.encode(spec.host_key.public_key().to_bytes().unwrap());
        let window = Arc::new((AtomicU32::new(0), AtomicU32::new(0)));
        let config = Arc::new(Config {
            keys: vec![spec.host_key.clone()],
            methods: MethodSet::server_supported(),
            auth_rejection_time: Duration::from_millis(20),
            ..Default::default()
        });
        let spec = Arc::new(spec);
        let window_clone = window.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = TestHandler {
                    spec: spec.clone(),
                    window: window_clone.clone(),
                    shell_channels: std::collections::HashSet::new(),
                };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, stream, handler).await {
                        let _ = running.await;
                    }
                });
            }
        });
        Self {
            addr,
            window,
            host_key_b64,
        }
    }
}
