use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use citadel_ssh::{Backoff, Link, LinkHandle, LinkState, PtyWriter, TransportError, Tunnel};
use citadel_vault::{AutoLock, Id, LockState, Vault};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::error::{AppError, CmdResult};
use crate::plan;
use crate::prefs::Prefs;

const LINGER: Duration = Duration::from_secs(20);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

pub const EV_VAULT: &str = "citadel://vault";
const BACKUP_EVERY: Duration = Duration::from_secs(20 * 60);
pub const EV_LINK: &str = "citadel://link";
pub const EV_TUNNEL: &str = "citadel://tunnel";
pub const EV_PROMPT: &str = "citadel://prompt";
pub const EV_BANNER: &str = "citadel://banner";

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Dirs {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

pub struct Hub {
    pub link: Arc<Link>,
    pub users: usize,
    pub epoch: u64,
}

pub struct PtyEntry {
    pub writer: PtyWriter,
    pub task: AbortHandle,
    pub flow: Arc<Flow>,
}

#[derive(Default)]
pub struct Flow {
    pub acked: AtomicU64,
    pub wake: tokio::sync::Notify,
}

pub struct TunnelEntry {
    pub tunnel: Tunnel,
    pub host: Id,
    pub watch: AbortHandle,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptKind {
    HostKey {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        status: &'static str,
    },
    Interactive {
        host: String,
        user: String,
        name: String,
        instructions: String,
        prompts: Vec<PromptLine>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct PromptLine {
    pub text: String,
    pub echo: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PromptEvent {
    pub id: u64,
    #[serde(flatten)]
    pub kind: PromptKind,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    HostKey { accept: bool },
    Interactive { answers: Vec<String> },
    Cancel,
}

#[derive(Default)]
pub struct Prompts {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
}

impl Prompts {
    pub async fn ask(&self, app: &AppHandle, kind: PromptKind) -> Option<Answer> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if app.emit(EV_PROMPT, PromptEvent { id, kind }).is_err() {
            lock(&self.pending).remove(&id);
            return None;
        }
        let answer = tokio::time::timeout(PROMPT_TIMEOUT, rx)
            .await
            .ok()
            .and_then(Result::ok);
        lock(&self.pending).remove(&id);
        answer
    }

    pub fn answer(&self, id: u64, answer: Answer) -> bool {
        lock(&self.pending)
            .remove(&id)
            .is_some_and(|tx| tx.send(answer).is_ok())
    }
}

#[derive(Clone, Serialize)]
struct LinkEvent {
    host: String,
    #[serde(flatten)]
    state: LinkState,
}

pub struct Core {
    pub app: AppHandle,
    pub vault: Arc<Vault>,
    pub autolock: AutoLock,
    pub prefs: Mutex<Prefs>,
    pub dirs: Dirs,
    pub links: Mutex<HashMap<Id, Hub>>,
    pub ptys: Mutex<HashMap<u32, PtyEntry>>,
    pub tunnels: Mutex<HashMap<Id, TunnelEntry>>,
    pub monitors: Mutex<HashMap<u32, AbortHandle>>,
    pub prompts: Prompts,
    pub unlock_gate: tokio::sync::Mutex<u32>,
    seq: AtomicU32,
    epochs: AtomicU64,
}

impl Core {
    pub fn new(
        app: AppHandle,
        vault: Arc<Vault>,
        autolock: AutoLock,
        prefs: Prefs,
        dirs: Dirs,
    ) -> Self {
        Self {
            app,
            vault,
            autolock,
            prefs: Mutex::new(prefs),
            dirs,
            links: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            monitors: Mutex::new(HashMap::new()),
            prompts: Prompts::default(),
            unlock_gate: tokio::sync::Mutex::new(0),
            seq: AtomicU32::new(0),
            epochs: AtomicU64::new(0),
        }
    }

    pub fn next_id(&self) -> u32 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn prefs(&self) -> Prefs {
        lock(&self.prefs).clone()
    }

    pub fn acquire(self: &Arc<Self>, host: Id) -> LinkHandle {
        let mut links = lock(&self.links);
        if let Some(hub) = links.get_mut(&host) {
            hub.users += 1;
            return hub.link.handle();
        }
        let core = self.clone();
        let link = Arc::new(Link::spawn(
            move || {
                let core = core.clone();
                async move {
                    let planned = tokio::task::spawn_blocking({
                        let core = core.clone();
                        move || plan::route(&core, host)
                    })
                    .await;
                    let route = match planned {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => return Err(TransportError::Credential(e.to_string())),
                        Err(e) => return Err(TransportError::Credential(e.to_string())),
                    };
                    citadel_ssh::connect(&route).await
                }
            },
            Backoff::default(),
        ));
        let epoch = self.epochs.fetch_add(1, Ordering::Relaxed);
        self.watch_link(host, &link.handle());
        let handle = link.handle();
        links.insert(
            host,
            Hub {
                link,
                users: 1,
                epoch,
            },
        );
        handle
    }

    pub fn release(self: &Arc<Self>, host: Id) {
        let epoch = {
            let mut links = lock(&self.links);
            let Some(hub) = links.get_mut(&host) else {
                return;
            };
            hub.users = hub.users.saturating_sub(1);
            if hub.users > 0 {
                return;
            }
            hub.epoch
        };
        let core = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(LINGER).await;
            let gone = {
                let mut links = lock(&core.links);
                match links.get(&host) {
                    Some(hub) if hub.users == 0 && hub.epoch == epoch => links.remove(&host),
                    _ => None,
                }
            };
            if let Some(hub) = gone {
                hub.link.shutdown().await;
                let _ = core.app.emit(
                    EV_LINK,
                    LinkEvent {
                        host: host.to_string(),
                        state: LinkState::Stopped,
                    },
                );
            }
        });
    }

    pub async fn session(
        self: &Arc<Self>,
        host: Id,
        wait: Duration,
    ) -> CmdResult<(LinkHandle, Arc<citadel_ssh::Session>)> {
        let handle = self.acquire(host);
        match handle.session(wait).await {
            Ok(s) => Ok((handle, s)),
            Err(e) => {
                self.release(host);
                Err(e.into())
            }
        }
    }

    pub fn link_states(&self) -> Vec<(Id, LinkState)> {
        lock(&self.links)
            .iter()
            .map(|(id, hub)| (*id, hub.link.state().borrow().clone()))
            .collect()
    }

    pub fn retry_waiting(&self) {
        for hub in lock(&self.links).values() {
            let waiting = matches!(*hub.link.state().borrow(), LinkState::Backoff { .. });
            if waiting {
                hub.link.reconnect_now();
            }
        }
    }

    fn watch_link(&self, host: Id, handle: &LinkHandle) {
        let app = self.app.clone();
        let mut rx = handle.state();
        tauri::async_runtime::spawn(async move {
            loop {
                let state = rx.borrow_and_update().clone();
                let stopped = state == LinkState::Stopped;
                let _ = app.emit(
                    EV_LINK,
                    LinkEvent {
                        host: host.to_string(),
                        state,
                    },
                );
                if stopped || rx.changed().await.is_err() {
                    return;
                }
            }
        });
    }

    pub fn snapshot_soon(self: &Arc<Self>) {
        let vault = self.vault.clone();
        let dir = crate::safety::backups_dir(&self.dirs.data);
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = crate::safety::snapshot_if_changed(&vault, &dir) {
                tracing::warn!("automatic backup failed: {e}");
            }
        });
    }

    pub fn watch_vault(self: &Arc<Self>) {
        let core = self.clone();
        let mut rx = self.vault.subscribe();
        let periodic = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut every = tokio::time::interval(BACKUP_EVERY);
            every.tick().await;
            loop {
                every.tick().await;
                periodic.snapshot_soon();
            }
        });
        tauri::async_runtime::spawn(async move {
            loop {
                let state = *rx.borrow_and_update();
                let _ = core.app.emit(EV_VAULT, state);
                if state == LockState::Unlocked {
                    core.snapshot_soon();
                    core.retry_waiting();
                    crate::cmd::tunnels::autostart(&core).await;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        });
    }

    pub fn shutdown(&self) {
        for (_, e) in lock(&self.ptys).drain() {
            e.task.abort();
        }
        for (_, m) in lock(&self.monitors).drain() {
            m.abort();
        }
        for (_, t) in lock(&self.tunnels).drain() {
            t.watch.abort();
            t.tunnel.stop();
        }
        lock(&self.links).clear();
        self.vault.lock();
    }
}

pub fn parse_id(raw: &str) -> CmdResult<Id> {
    raw.parse()
        .map_err(|_| AppError::invalid(format!("malformed id {raw:?}")))
}
