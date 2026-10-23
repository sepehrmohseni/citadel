use secrecy::SecretString;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use self::sealed::Sealed as _;
use crate::{Id, Result};

pub const DEFAULT_SSH_PORT: u16 = 22;
pub const DEFAULT_KEEPALIVE_SECS: u32 = 15;

fn default_port() -> u16 {
    DEFAULT_SSH_PORT
}

fn default_keepalive() -> u32 {
    DEFAULT_KEEPALIVE_SECS
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Zeroize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub enum Auth {
    Identity(Id),
    Agent {
        #[serde(default)]
        fingerprint: Option<String>,
    },
    Password(#[serde(with = "crate::secret::plain")] SecretString),
    Interactive,
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct HostProfile {
    pub id: Id,
    pub label: String,
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    #[serde(default)]
    pub jump: Vec<Id>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    #[serde(default)]
    pub agent_forward: bool,
    #[serde(default)]
    pub legacy_crypto: bool,
    #[serde(default)]
    pub ssh_alias: Option<String>,
    #[serde(default)]
    pub canvas: Option<Point>,
    #[serde(default)]
    pub updated_ms: u64,
}

impl HostProfile {
    pub fn new(label: &str, hostname: &str, user: &str, auth: Auth) -> Result<Self> {
        Ok(Self {
            id: Id::random()?,
            label: label.to_owned(),
            hostname: hostname.to_owned(),
            port: DEFAULT_SSH_PORT,
            user: user.to_owned(),
            auth,
            jump: Vec::new(),
            workspace: None,
            tags: Vec::new(),
            keepalive_secs: DEFAULT_KEEPALIVE_SECS,
            agent_forward: false,
            legacy_crypto: false,
            ssh_alias: None,
            canvas: None,
            updated_ms: 0,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub enum KeyMaterial {
    Embedded {
        #[serde(with = "crate::secret::plain")]
        private_key: SecretString,
        #[serde(default, with = "crate::secret::optional")]
        passphrase: Option<SecretString>,
    },
    Agent,
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct IdentityKey {
    pub id: Id,
    pub label: String,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub material: KeyMaterial,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Zeroize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Forward {
    Local { bind: Endpoint, dest: Endpoint },
    Remote { bind: Endpoint, dest: Endpoint },
    Dynamic { bind: Endpoint },
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct TunnelSpec {
    pub id: Id,
    pub label: String,
    pub host: Id,
    pub forward: Forward,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub canvas: Option<Point>,
    #[serde(default)]
    pub updated_ms: u64,
}

impl TunnelSpec {
    pub fn new(label: &str, host: Id, forward: Forward) -> Result<Self> {
        Ok(Self {
            id: Id::random()?,
            label: label.to_owned(),
            host,
            forward,
            autostart: false,
            canvas: None,
            updated_ms: 0,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Snippet {
    pub id: Id,
    pub label: String,
    pub body: String,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct KnownHost {
    pub id: Id,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub key: String,
    pub fingerprint: String,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub enum SyncBackend {
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        object: String,
        access_key: String,
        #[serde(with = "crate::secret::plain")]
        secret_key: SecretString,
        #[serde(default)]
        path_style: bool,
    },
    WebDav {
        url: String,
        user: String,
        #[serde(with = "crate::secret::plain")]
        password: SecretString,
    },
    Git {
        remote: String,
        branch: String,
        path: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct SyncTarget {
    pub id: Id,
    pub label: String,
    pub backend: SyncBackend,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
pub struct Tombstone {
    pub id: Id,
    pub updated_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Entry {
    Host(HostProfile),
    Identity(IdentityKey),
    Tunnel(TunnelSpec),
    Snippet(Snippet),
    KnownHost(KnownHost),
    SyncTarget(SyncTarget),
    Tombstone(Tombstone),
}

impl Entry {
    pub fn id(&self) -> Id {
        match self {
            Self::Host(r) => r.id,
            Self::Identity(r) => r.id,
            Self::Tunnel(r) => r.id,
            Self::Snippet(r) => r.id,
            Self::KnownHost(r) => r.id,
            Self::SyncTarget(r) => r.id,
            Self::Tombstone(r) => r.id,
        }
    }

    pub fn stamp(&self) -> u64 {
        match self {
            Self::Host(r) => r.updated_ms,
            Self::Identity(r) => r.updated_ms,
            Self::Tunnel(r) => r.updated_ms,
            Self::Snippet(r) => r.updated_ms,
            Self::KnownHost(r) => r.updated_ms,
            Self::SyncTarget(r) => r.updated_ms,
            Self::Tombstone(r) => r.updated_ms,
        }
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Self::Tombstone(_))
    }
}

pub(crate) mod sealed {
    pub trait Sealed {
        const TABLE: &'static str;
    }
}

pub trait Record: sealed::Sealed + Serialize + DeserializeOwned + Sized {
    fn id(&self) -> Id;
    fn stamp(&self) -> u64;
    fn set_stamp(&mut self, ms: u64);
    fn into_entry(self) -> Entry;
    fn from_entry(entry: Entry) -> Option<Self>;
}

macro_rules! record {
    ($ty:ident, $variant:ident, $table:literal) => {
        impl sealed::Sealed for $ty {
            const TABLE: &'static str = $table;
        }

        impl Record for $ty {
            fn id(&self) -> Id {
                self.id
            }

            fn stamp(&self) -> u64 {
                self.updated_ms
            }

            fn set_stamp(&mut self, ms: u64) {
                self.updated_ms = ms;
            }

            fn into_entry(self) -> Entry {
                Entry::$variant(self)
            }

            fn from_entry(entry: Entry) -> Option<Self> {
                match entry {
                    Entry::$variant(r) => Some(r),
                    _ => None,
                }
            }
        }
    };
}

record!(HostProfile, Host, "hosts");
record!(IdentityKey, Identity, "identities");
record!(TunnelSpec, Tunnel, "tunnels");
record!(Snippet, Snippet, "snippets");
record!(KnownHost, KnownHost, "known_hosts");
record!(SyncTarget, SyncTarget, "sync_targets");
record!(Tombstone, Tombstone, "graveyard");

pub(crate) const LIVE_TABLES: [&str; 6] = [
    HostProfile::TABLE,
    IdentityKey::TABLE,
    TunnelSpec::TABLE,
    Snippet::TABLE,
    KnownHost::TABLE,
    SyncTarget::TABLE,
];
