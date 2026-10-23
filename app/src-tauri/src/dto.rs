use citadel_ssh::{LinkState, MeterSnapshot, TunnelState};
use citadel_vault::{
    Auth, Forward, HostProfile, IdentityKey, KeyMaterial, KnownHost, LockState, Point, Snippet,
    SyncBackend, SyncTarget, TunnelSpec,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct VaultStatus {
    pub state: LockState,
    pub path: String,
    pub autolock_minutes: u32,
    pub backups: usize,
    pub latest_backup_ms: Option<u64>,
    pub unreadable: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthView {
    Identity { identity: String },
    Agent { fingerprint: Option<String> },
    Password { stored: bool },
    Interactive,
}

#[derive(Serialize)]
pub struct HostView {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthView,
    pub jump: Vec<String>,
    pub workspace: Option<String>,
    pub tags: Vec<String>,
    pub keepalive_secs: u32,
    pub agent_forward: bool,
    #[serde(default)]
    pub legacy_crypto: bool,
    pub ssh_alias: Option<String>,
    pub canvas: Option<Point>,
    pub updated_ms: u64,
}

impl From<&HostProfile> for HostView {
    fn from(h: &HostProfile) -> Self {
        Self {
            id: h.id.to_string(),
            label: h.label.clone(),
            hostname: h.hostname.clone(),
            port: h.port,
            user: h.user.clone(),
            auth: match &h.auth {
                Auth::Identity(id) => AuthView::Identity {
                    identity: id.to_string(),
                },
                Auth::Agent { fingerprint } => AuthView::Agent {
                    fingerprint: fingerprint.clone(),
                },
                Auth::Password(_) => AuthView::Password { stored: true },
                Auth::Interactive => AuthView::Interactive,
            },
            jump: h.jump.iter().map(ToString::to_string).collect(),
            workspace: h.workspace.clone(),
            tags: h.tags.clone(),
            keepalive_secs: h.keepalive_secs,
            agent_forward: h.agent_forward,
            legacy_crypto: h.legacy_crypto,
            ssh_alias: h.ssh_alias.clone(),
            canvas: h.canvas,
            updated_ms: h.updated_ms,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthInput {
    Identity { identity: String },
    Agent { fingerprint: Option<String> },
    Password { password: Option<String> },
    Interactive,
}

#[derive(Deserialize)]
pub struct HostInput {
    pub id: Option<String>,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthInput,
    #[serde(default)]
    pub jump: Vec<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub keepalive_secs: Option<u32>,
    #[serde(default)]
    pub agent_forward: bool,
    #[serde(default)]
    pub legacy_crypto: bool,
    #[serde(default)]
    pub ssh_alias: Option<String>,
}

#[derive(Serialize)]
pub struct IdentityView {
    pub id: String,
    pub label: String,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub source: &'static str,
    pub encrypted: bool,
}

impl From<&IdentityKey> for IdentityView {
    fn from(k: &IdentityKey) -> Self {
        let (source, encrypted) = match &k.material {
            KeyMaterial::Embedded { passphrase, .. } => ("vault", passphrase.is_some()),
            KeyMaterial::Agent => ("agent", false),
        };
        Self {
            id: k.id.to_string(),
            label: k.label.clone(),
            algorithm: k.algorithm.clone(),
            public_key: k.public_key.clone(),
            fingerprint: k.fingerprint.clone(),
            source,
            encrypted,
        }
    }
}

#[derive(Serialize)]
pub struct TunnelView {
    pub id: String,
    pub label: String,
    pub host: String,
    pub forward: Forward,
    pub autostart: bool,
    pub canvas: Option<Point>,
    pub running: bool,
    pub state: Option<TunnelState>,
    pub stats: Option<MeterSnapshot>,
}

impl TunnelView {
    pub fn new(t: &TunnelSpec, live: Option<(TunnelState, MeterSnapshot)>) -> Self {
        let running = live.is_some();
        let (state, stats) = live.map_or((None, None), |(s, m)| (Some(s), Some(m)));
        Self {
            id: t.id.to_string(),
            label: t.label.clone(),
            host: t.host.to_string(),
            forward: t.forward.clone(),
            autostart: t.autostart,
            canvas: t.canvas,
            running,
            state,
            stats,
        }
    }
}

#[derive(Deserialize)]
pub struct TunnelInput {
    pub id: Option<String>,
    pub label: String,
    pub host: String,
    pub forward: Forward,
    #[serde(default)]
    pub autostart: bool,
}

#[derive(Serialize)]
pub struct SnippetView {
    pub id: String,
    pub label: String,
    pub body: String,
}

impl From<&Snippet> for SnippetView {
    fn from(s: &Snippet) -> Self {
        Self {
            id: s.id.to_string(),
            label: s.label.clone(),
            body: s.body.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct KnownHostView {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
}

impl From<&KnownHost> for KnownHostView {
    fn from(k: &KnownHost) -> Self {
        Self {
            id: k.id.to_string(),
            host: k.host.clone(),
            port: k.port,
            algorithm: k.algorithm.clone(),
            fingerprint: k.fingerprint.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncBackendView {
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        object: String,
        access_key: String,
        path_style: bool,
    },
    WebDav {
        url: String,
        user: String,
    },
    Git {
        remote: String,
        branch: String,
        path: String,
    },
}

#[derive(Serialize)]
pub struct SyncTargetView {
    pub id: String,
    pub label: String,
    pub backend: SyncBackendView,
}

impl From<&SyncTarget> for SyncTargetView {
    fn from(t: &SyncTarget) -> Self {
        let backend = match &t.backend {
            SyncBackend::S3 {
                endpoint,
                region,
                bucket,
                object,
                access_key,
                path_style,
                ..
            } => SyncBackendView::S3 {
                endpoint: endpoint.clone(),
                region: region.clone(),
                bucket: bucket.clone(),
                object: object.clone(),
                access_key: access_key.clone(),
                path_style: *path_style,
            },
            SyncBackend::WebDav { url, user, .. } => SyncBackendView::WebDav {
                url: url.clone(),
                user: user.clone(),
            },
            SyncBackend::Git {
                remote,
                branch,
                path,
            } => SyncBackendView::Git {
                remote: remote.clone(),
                branch: branch.clone(),
                path: path.clone(),
            },
        };
        Self {
            id: t.id.to_string(),
            label: t.label.clone(),
            backend,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncBackendInput {
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        object: String,
        access_key: String,
        secret_key: Option<String>,
        #[serde(default)]
        path_style: bool,
    },
    WebDav {
        url: String,
        user: String,
        password: Option<String>,
    },
    Git {
        remote: String,
        branch: String,
        path: String,
    },
}

#[derive(Deserialize)]
pub struct SyncTargetInput {
    pub id: Option<String>,
    pub label: String,
    pub backend: SyncBackendInput,
}

#[derive(Serialize)]
pub struct LinkView {
    pub host: String,
    #[serde(flatten)]
    pub state: LinkState,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PtyStatus {
    Exit { code: u32 },
    Signal { name: String },
    Closed,
}
