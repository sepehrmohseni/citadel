use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use rand::RngExt;
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

use crate::{Result, Session, TransportError};

const PROBE_EVERY: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub trait Dial: Send + Sync + 'static {
    fn dial(&self) -> BoxFuture<'_, Result<Arc<Session>>>;
}

impl<F, Fut> Dial for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Arc<Session>>> + Send + 'static,
{
    fn dial(&self) -> BoxFuture<'_, Result<Arc<Session>>> {
        Box::pin(self())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
    pub jitter: f64,
    pub stable_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(30),
            jitter: 0.2,
            stable_after: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(16);
        let raw = self.base.saturating_mul(1u32 << exp).min(self.cap);
        if self.jitter <= 0.0 {
            return raw;
        }
        let spread = self.jitter.min(1.0);
        let factor = rand::rng().random_range((1.0 - spread)..=(1.0 + spread));
        raw.mul_f64(factor).min(self.cap)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    Idle,
    Connecting {
        attempt: u32,
    },
    Up {
        attempt: u32,
        rtt_ms: Option<u64>,
    },
    Backoff {
        attempt: u32,
        retry_ms: u64,
        error: String,
    },
    Failed {
        attempt: u32,
        error: String,
    },
    Stopped,
}

struct Shared {
    session: watch::Sender<Option<Arc<Session>>>,
    state: watch::Sender<LinkState>,
    kick: watch::Sender<u64>,
}

#[derive(Clone)]
pub struct LinkHandle(Arc<Shared>);

impl LinkHandle {
    pub fn current(&self) -> Option<Arc<Session>> {
        self.0.session.borrow().clone().filter(|s| !s.is_closed())
    }

    pub fn sessions(&self) -> watch::Receiver<Option<Arc<Session>>> {
        self.0.session.subscribe()
    }

    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.0.state.subscribe()
    }

    pub fn reconnect_now(&self) {
        self.0.kick.send_modify(|g| *g = g.wrapping_add(1));
    }

    pub async fn retry(&self) {
        let failed = matches!(*self.0.state.borrow(), LinkState::Failed { .. });
        self.reconnect_now();
        if failed {
            let mut st = self.state();
            let _ = timeout(
                Duration::from_secs(2),
                st.wait_for(|s| !matches!(s, LinkState::Failed { .. })),
            )
            .await;
        }
    }

    pub async fn session(&self, wait: Duration) -> Result<Arc<Session>> {
        let mut rx = self.sessions();
        let mut st = self.state();
        let found = timeout(wait, async {
            loop {
                if let Some(s) = rx.borrow_and_update().clone().filter(|s| !s.is_closed()) {
                    return Ok(s);
                }
                if matches!(*st.borrow_and_update(), LinkState::Failed { .. }) {
                    return Err(TransportError::NotReady(self.describe()));
                }
                let gone = tokio::select! {
                    r = rx.changed() => r.is_err(),
                    r = st.changed() => r.is_err(),
                };
                if gone {
                    return Err(TransportError::Closed("link stopped".into()));
                }
            }
        })
        .await;
        match found {
            Ok(r) => r,
            Err(_) => Err(TransportError::NotReady(self.describe())),
        }
    }

    fn describe(&self) -> String {
        match &*self.0.state.borrow() {
            LinkState::Backoff {
                error, retry_ms, ..
            } => {
                format!(
                    "server unreachable: {error}; next attempt in {}s",
                    retry_ms.div_ceil(1000)
                )
            }
            LinkState::Connecting { attempt } if *attempt > 1 => {
                format!("the server has not answered yet (reconnect attempt {attempt})")
            }
            LinkState::Connecting { .. } => "the server has not answered yet".into(),
            LinkState::Failed { error, .. } => format!("{error}; retry to connect again"),
            LinkState::Up { .. } => "the connection was replaced while waiting".into(),
            LinkState::Idle => "not connected yet".into(),
            LinkState::Stopped => "the connection was stopped".into(),
        }
    }
}

pub struct Link {
    handle: LinkHandle,
    task: JoinHandle<()>,
}

impl Link {
    pub fn spawn(dial: impl Dial, backoff: Backoff) -> Self {
        let shared = Arc::new(Shared {
            session: watch::Sender::new(None),
            state: watch::Sender::new(LinkState::Idle),
            kick: watch::Sender::new(0),
        });
        let task = tokio::spawn(supervise(shared.clone(), Box::new(dial), backoff));
        Self {
            handle: LinkHandle(shared),
            task,
        }
    }

    pub fn handle(&self) -> LinkHandle {
        self.handle.clone()
    }

    pub async fn shutdown(&self) {
        self.task.abort();
        let sess = self.handle.0.session.send_replace(None);
        self.handle.0.state.send_replace(LinkState::Stopped);
        if let Some(s) = sess {
            s.disconnect().await;
        }
    }
}

impl std::ops::Deref for Link {
    type Target = LinkHandle;

    fn deref(&self) -> &LinkHandle {
        &self.handle
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
        let sess = self.handle.0.session.send_replace(None);
        self.handle.0.state.send_replace(LinkState::Stopped);
        if let (Some(s), Ok(rt)) = (sess, tokio::runtime::Handle::try_current()) {
            rt.spawn(async move { s.disconnect().await });
        }
    }
}

async fn supervise(shared: Arc<Shared>, dial: Box<dyn Dial>, backoff: Backoff) {
    let mut kick = shared.kick.subscribe();
    let mut attempt = 0u32;
    loop {
        attempt = attempt.saturating_add(1);
        shared.state.send_replace(LinkState::Connecting { attempt });
        let error = match dial.dial().await {
            Ok(sess) => {
                kick.borrow_and_update();
                let since = Instant::now();
                shared.session.send_replace(Some(sess.clone()));
                shared.state.send_replace(LinkState::Up {
                    attempt,
                    rtt_ms: None,
                });
                let mut probe = tokio::time::interval(PROBE_EVERY);
                probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let reason = loop {
                    tokio::select! {
                        reason = sess.closed() => break reason,
                        _ = kick.changed() => {
                            sess.disconnect().await;
                            break "reconnect requested".to_owned();
                        }
                        _ = probe.tick() => {
                            if let Some(rtt) = sess.rtt(PROBE_TIMEOUT).await {
                                let rtt_ms = Some(u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX));
                                shared.state.send_replace(LinkState::Up { attempt, rtt_ms });
                            }
                        }
                    }
                };
                shared.session.send_replace(None);
                if since.elapsed() >= backoff.stable_after {
                    attempt = 1;
                }
                tracing::info!(host = sess.host(), "link lost: {reason}");
                reason
            }
            Err(e) if e.is_permanent() => {
                tracing::info!("dial attempt {attempt} failed permanently: {e}");
                shared.state.send_replace(LinkState::Failed {
                    attempt,
                    error: e.to_string(),
                });
                if kick.changed().await.is_err() {
                    return;
                }
                attempt = 0;
                continue;
            }
            Err(e) => {
                tracing::debug!("dial attempt {attempt} failed: {e}");
                e.to_string()
            }
        };
        let delay = backoff.delay(attempt);
        shared.state.send_replace(LinkState::Backoff {
            attempt,
            retry_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            error,
        });
        tokio::select! {
            () = sleep(delay) => {}
            _ = kick.changed() => {}
        }
    }
}
