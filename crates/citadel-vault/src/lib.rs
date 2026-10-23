#![forbid(unsafe_code)]

mod autolock;
mod codec;
mod crypto;
mod envelope;
mod error;
mod id;
mod kdf;
mod model;
mod secret;
mod store;

pub use autolock::AutoLock;
pub use envelope::{Bundle, FORMAT_VERSION, MAGIC};
pub use error::{Result, VaultError};
pub use id::Id;
pub use kdf::KdfParams;
pub use model::{
    Auth, DEFAULT_KEEPALIVE_SECS, DEFAULT_SSH_PORT, Endpoint, Entry, Forward, HostProfile,
    IdentityKey, KeyMaterial, KnownHost, Point, Record, Snippet, SyncBackend, SyncTarget,
    Tombstone, TunnelSpec,
};
pub use secrecy::{ExposeSecret, SecretString};
pub use secret::secret_from;
pub use store::{
    Absorbed, Backup, Health, ImportMode, LockState, MergeReport, STORE_FORMAT, Vault, list_backups,
};
