use citadel_vault::VaultError;
use thiserror::Error;

pub type Result<T, E = SyncError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("remote is sealed under another vault key; its password is required")]
    NeedsPassword,
    #[error("remote changed concurrently")]
    Conflict,
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("git: {0}")]
    Git(String),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("transport: {0}")]
    Transport(String),
    #[error("invalid sync target: {0}")]
    Config(String),
}

impl From<reqwest::Error> for SyncError {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e.without_url().to_string())
    }
}

pub(crate) fn vault(e: VaultError) -> SyncError {
    match e {
        VaultError::ForeignKey => SyncError::NeedsPassword,
        other => SyncError::Vault(other),
    }
}
