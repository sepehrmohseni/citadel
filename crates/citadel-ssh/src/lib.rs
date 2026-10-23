#![forbid(unsafe_code)]

mod agent;
mod auth;
mod client;
mod error;
mod forward;
mod keys;
mod link;
mod meter;
mod pty;
mod session;
mod sftp;
mod socks;
mod verify;

pub use auth::{
    AgentEndpoint, Credential, DynAgent, PromptRequest, Prompter, connect_agent, decode_key,
    default_identity_files, fingerprint, list_agent_keys, normalize_pem,
};
pub use client::Banner;
pub use error::{Result, TransportError};
pub use forward::{ForwardSpec, Tunnel, TunnelState};
pub use keys::{KeyFamily, KeyInfo, KeyKind, classify, generate, inspect};
pub use link::{Backoff, Dial, Link, LinkHandle, LinkState};
pub use meter::MeterSnapshot;
pub use pty::{Pty, PtyEvent, PtyRequest, PtyWriter};
pub use session::{ExecEvent, ExecOutput, ExecStream, Hop, Route, Session, connect};
pub use sftp::{DirEntry, FileStat, Progress, Sftp};
pub use verify::{HostKeyStatus, HostKeyVerifier, KnownHostsFile, UnknownHost};

pub use russh::keys::{Algorithm, Error as KeyError, HashAlg, PrivateKey, PublicKey};
