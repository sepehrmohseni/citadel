use citadel_config::ConfigError;
use citadel_ssh::TransportError;
use citadel_sync::SyncError;
use citadel_telemetry::TelemetryError;
use citadel_vault::{Id, VaultError};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

pub type CmdResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("jump chain loops back through host {0}")]
    Cycle(Id),
    #[error("background task failed: {0}")]
    Join(String),
}

impl AppError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Vault(VaultError::Locked) => "locked",
            Self::Vault(VaultError::BadPassword) => "bad_password",
            Self::Vault(VaultError::Uninitialized) => "uninitialized",
            Self::Vault(VaultError::Tampered) => "tampered",
            Self::Vault(_) => "vault",
            Self::Transport(TransportError::HostKeyRejected { .. }) => "host_key",
            Self::Transport(TransportError::AuthFailed { .. }) => "auth",
            Self::Transport(TransportError::Legacy { .. }) => "legacy",
            Self::Transport(TransportError::Key(citadel_ssh::KeyError::KeyIsEncrypted)) => {
                "key_encrypted"
            }
            Self::Transport(TransportError::Timeout { .. }) => "timeout",
            Self::Transport(_) => "transport",
            Self::Telemetry(_) => "telemetry",
            Self::Config(_) => "config",
            Self::Sync(SyncError::NeedsPassword) => "needs_password",
            Self::Sync(SyncError::Conflict) => "conflict",
            Self::Sync(_) => "sync",
            Self::Io(_) => "io",
            Self::NotFound(_) => "not_found",
            Self::Invalid(_) | Self::Cycle(_) => "invalid",
            Self::Join(_) => "internal",
        }
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Join(e.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        Self::Join(e.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}
