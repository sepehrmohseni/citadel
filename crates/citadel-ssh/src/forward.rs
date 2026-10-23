use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::link::LinkHandle;
use crate::meter::{Meter, MeterSnapshot, Metered};
use crate::socks::{self, Reply};
use crate::{Result, Session};

const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
const DEFAULT_WAIT: Duration = Duration::from_secs(20);
const SOCKS_HANDSHAKE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ForwardSpec {
    Local {
        bind: String,
        bind_port: u16,
        dest: String,
        dest_port: u16,
    },
    Remote {
        bind: String,
        bind_port: u16,
        dest: String,
        dest_port: u16,
    },
    Dynamic {
        bind: String,
        bind_port: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TunnelState {
    Listening { addr: String },
    Waiting { reason: String },
    Forwarding { remote_port: u16 },
    Failed { error: String },
    Stopped,
}

pub struct Tunnel {
    spec: ForwardSpec,
    local: Option<SocketAddr>,
    meter: Arc<Meter>,
    state: watch::Receiver<TunnelState>,
    task: JoinHandle<()>,
}

impl Tunnel {
    pub async fn open(spec: ForwardSpec, link: LinkHandle) -> Result<Self> {
        Self::open_with(spec, link, DEFAULT_WAIT).await
    }

    pub async fn open_with(spec: ForwardSpec, link: LinkHandle, wait: Duration) -> Result<Self> {
        let meter = Arc::new(Meter::default());
        match &spec {
            ForwardSpec::Local {
                bind,
                bind_port,
                dest,
                dest_port,
            } => {
                let listener = TcpListener::bind((bind.as_str(), *bind_port)).await?;
                let addr = listener.local_addr()?;
                let (tx, state) = watch::channel(TunnelState::Listening {
                    addr: addr.to_string(),
                });
                let target = Target::Fixed {
                    host: dest.clone(),
                    port: *dest_port,
                };
                let task = tokio::spawn(accept_loop(
                    listener,
                    addr,
                    link,
                    target,
                    meter.clone(),
                    wait,
                    tx,
                ));
                Ok(Self {
                    spec,
                    local: Some(addr),
                    meter,
                    state,
                    task,
                })
            }
            ForwardSpec::Dynamic { bind, bind_port } => {
                let listener = TcpListener::bind((bind.as_str(), *bind_port)).await?;
                let addr = listener.local_addr()?;
                let (tx, state) = watch::channel(TunnelState::Listening {
                    addr: addr.to_string(),
                });
                let task = tokio::spawn(accept_loop(
                    listener,
                    addr,
                    link,
                    Target::Socks,
                    meter.clone(),
                    wait,
                    tx,
                ));
                Ok(Self {
                    spec,
                    local: Some(addr),
                    meter,
                    state,
                    task,
                })
            }
            ForwardSpec::Remote {
                bind,
                bind_port,
                dest,
                dest_port,
            } => {
                let (tx, state) = watch::channel(TunnelState::Waiting {
                    reason: "awaiting session".into(),
                });
                let remote = RemoteBinding {
                    bind: bind.clone(),
                    bind_port: *bind_port,
                    dest: dest.clone(),
                    dest_port: *dest_port,
                };
                let task = tokio::spawn(remote_loop(remote, link, meter.clone(), tx));
                Ok(Self {
                    spec,
                    local: None,
                    meter,
                    state,
                    task,
                })
            }
        }
    }

    pub fn spec(&self) -> &ForwardSpec {
        &self.spec
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }

    pub fn stats(&self) -> MeterSnapshot {
        self.meter.snapshot()
    }

    pub fn state(&self) -> watch::Receiver<TunnelState> {
        self.state.clone()
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
enum Target {
    Fixed { host: String, port: u16 },
    Socks,
}

async fn accept_loop(
    listener: TcpListener,
    listen: SocketAddr,
    link: LinkHandle,
    target: Target,
    meter: Arc<Meter>,
    wait: Duration,
    state: watch::Sender<TunnelState>,
) {
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        let _ = sock.set_nodelay(true);
        let link = link.clone();
        let target = target.clone();
        let meter = meter.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let lease = meter.open();
            if let Err(e) = relay(sock, peer, listen, &link, target, &meter, wait, &state).await {
                meter.fail();
                tracing::debug!(%peer, "forward closed: {e}");
            }
            drop(lease);
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn relay(
    mut sock: TcpStream,
    peer: SocketAddr,
    listen: SocketAddr,
    link: &LinkHandle,
    target: Target,
    meter: &Arc<Meter>,
    wait: Duration,
    state: &watch::Sender<TunnelState>,
) -> Result<()> {
    let (host, port, socks) = match target {
        Target::Fixed { host, port } => (host, port, false),
        Target::Socks => {
            let (h, p) = tokio::time::timeout(SOCKS_HANDSHAKE, socks::handshake(&mut sock))
                .await
                .map_err(|_| crate::TransportError::Socks("handshake timed out"))??;
            (h, p, true)
        }
    };
    let sess = match link.session(wait).await {
        Ok(s) => s,
        Err(e) => {
            state.send_replace(TunnelState::Waiting {
                reason: e.to_string(),
            });
            if socks {
                let _ = socks::reply(&mut sock, Reply::GeneralFailure).await;
            }
            return Err(e);
        }
    };
    let opened = sess
        .direct_tcpip_from(&host, port, &peer.ip().to_string(), peer.port())
        .await;
    let mut remote = match opened {
        Ok(ch) => ch,
        Err(e) => {
            if socks {
                let _ = socks::reply(&mut sock, Reply::HostUnreachable).await;
            }
            return Err(e);
        }
    };
    if socks {
        socks::reply(&mut sock, Reply::Succeeded).await?;
    }
    state.send_if_modified(|s| {
        if matches!(s, TunnelState::Waiting { .. }) {
            *s = TunnelState::Listening {
                addr: listen.to_string(),
            };
            return true;
        }
        false
    });
    let mut local = Metered::new(sock, meter.clone());
    tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
    Ok(())
}

struct RemoteBinding {
    bind: String,
    bind_port: u16,
    dest: String,
    dest_port: u16,
}

struct Registered {
    sess: Arc<Session>,
    bind: String,
    port: u16,
}

impl Drop for Registered {
    fn drop(&mut self) {
        if self.sess.is_closed() {
            return;
        }
        let sess = self.sess.clone();
        let bind = std::mem::take(&mut self.bind);
        let port = self.port;
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = sess.cancel_remote_forward(&bind, port).await;
            });
        }
    }
}

async fn remote_loop(
    remote: RemoteBinding,
    link: LinkHandle,
    meter: Arc<Meter>,
    state: watch::Sender<TunnelState>,
) {
    let mut sessions = link.sessions();
    let mut active: Option<Registered> = None;
    loop {
        let current = sessions
            .borrow_and_update()
            .clone()
            .filter(|s| !s.is_closed());
        match current {
            Some(sess) if active.as_ref().is_none_or(|r| !Arc::ptr_eq(&r.sess, &sess)) => {
                active = None;
                match sess
                    .remote_forward(
                        &remote.bind,
                        remote.bind_port,
                        &remote.dest,
                        remote.dest_port,
                        meter.clone(),
                    )
                    .await
                {
                    Ok(port) => {
                        state.send_replace(TunnelState::Forwarding { remote_port: port });
                        active = Some(Registered {
                            sess,
                            bind: remote.bind.clone(),
                            port,
                        });
                    }
                    Err(e) => {
                        state.send_replace(TunnelState::Failed {
                            error: e.to_string(),
                        });
                    }
                }
            }
            Some(_) => {}
            None => {
                active = None;
                state.send_replace(TunnelState::Waiting {
                    reason: "session down".into(),
                });
            }
        }
        if sessions.changed().await.is_err() {
            state.send_replace(TunnelState::Stopped);
            return;
        }
    }
}
