use thiserror::Error;

pub type Result<T, E = VaultError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault is locked")]
    Locked,
    #[error("vault has not been initialized")]
    Uninitialized,
    #[error("vault is already initialized")]
    AlreadyInitialized,
    #[error("wrong password")]
    BadPassword,
    #[error("ciphertext failed authentication")]
    Tampered,
    #[error("blob is sealed under a different vault key")]
    ForeignKey,
    #[error("not a citadel container")]
    BadMagic,
    #[error("unsupported container version {0}")]
    UnsupportedVersion(u8),
    #[error("container truncated")]
    Truncated,
    #[error("kdf parameters out of bounds: m={m_kib}KiB t={t} p={p}")]
    KdfBounds { m_kib: u32, t: u32, p: u32 },
    #[error("argon2: {0}")]
    Kdf(argon2::Error),
    #[error("payload of {0} bytes exceeds limit")]
    Oversize(u64),
    #[error("compression: {0}")]
    Codec(String),
    #[error("record encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("record decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("invalid identifier")]
    BadId,
    #[error("entropy source: {0}")]
    Entropy(#[from] getrandom::Error),
    #[error("storage: {0}")]
    Storage(#[from] Box<redb::Error>),
    #[error("this vault was written by a newer Citadel (format {0}); update the app to open it")]
    NewerFormat(u32),
    #[error(
        "{0} records could not be read by this version; update Citadel before re-encrypting the vault"
    )]
    Unreadable(usize),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

macro_rules! storage_from {
    ($($t:ty),+ $(,)?) => {$(
        impl From<$t> for VaultError {
            fn from(e: $t) -> Self {
                Self::Storage(Box::new(e.into()))
            }
        }
    )+};
}

storage_from!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);
