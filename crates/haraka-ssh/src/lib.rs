//! haraka-ssh — backend de session SSH de HarakaTS (jalon M3).
//! Dépend de haraka-core (modèle, coffre, known_hosts) ; aucune dépendance
//! Tauri/GUI : testable headless contre un serveur russh in-process.

pub mod edit;
pub mod error;
pub mod keyinfo;
pub mod multiexec;
pub mod osc7;
pub mod session;
pub mod sftp;
pub mod transfer;
pub mod tunnel;

pub use edit::{
    EditDocInfo, EditEvent, EditManager, LineEnding, RemoteState, SaveOutcome, build_editor_argv,
};
pub use error::{Result, SshError};
pub use keyinfo::{KeyInfo, check_key_passphrase, inspect_private_key};
pub use multiexec::{MultiExec, MultiExecError, MultiExecState, SharedMultiExec, write_routed};
pub use russh_sftp::client::SftpSession;
pub use session::{
    AgentKey, AuthMaterial, HopConfig, HostKeyDecision, OpenRequest, PromptDto, ReconnectPolicy,
    SessionEvent, SessionManager, agent_keys,
};
pub use transfer::{
    Conflict, Direction, SymlinkPolicy, TransferEvent, TransferInfo, TransferManager, TransferSpec,
    TransferState,
};
pub use tunnel::{TunnelDef, TunnelEvent, TunnelInfo, TunnelKind, TunnelManager, TunnelState};
