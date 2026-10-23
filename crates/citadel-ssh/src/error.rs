use thiserror::Error;

pub type Result<T, E = TransportError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("ssh: {0}")]
    Ssh(#[from] russh::Error),
    #[error("key: {0}")]
    Key(#[from] russh::keys::Error),
    #[error("key encoding: {0}")]
    KeyFormat(#[from] russh::keys::ssh_key::Error),
    #[error("wire encoding: {0}")]
    Encoding(#[from] russh::keys::ssh_encoding::Error),
    #[error("sftp: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("host key for {host}:{port} was rejected")]
    HostKeyRejected { host: String, port: u16 },
    #[error("authentication failed for {user}@{host}: {detail}")]
    AuthFailed {
        user: String,
        host: String,
        detail: String,
    },
    #[error(
        "{host} only offers legacy {kind} algorithms ({offered}); enable legacy algorithms for this host to connect"
    )]
    Legacy {
        host: String,
        kind: String,
        offered: String,
    },
    #[error("ssh agent: {0}")]
    Agent(String),
    #[error("agent does not hold key {0}")]
    AgentKeyMissing(String),
    #[error("route has no hops")]
    EmptyRoute,
    #[error("{stage} timed out for {addr}")]
    Timeout { stage: &'static str, addr: String },
    #[error("session closed: {0}")]
    Closed(String),
    #[error("{0}")]
    NotReady(String),
    #[error("remote forward {addr}:{port} denied")]
    ForwardDenied { addr: String, port: u16 },
    #[error("socks: {0}")]
    Socks(&'static str),
    #[error("credential unavailable: {0}")]
    Credential(String),
    #[error("interactive authentication cancelled")]
    Cancelled,
    #[error("remote path {0} is not valid UTF-8")]
    BadPath(String),
}
