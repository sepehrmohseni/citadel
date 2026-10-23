use std::borrow::Cow;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use russh::client::{Config, Handle, Msg};
use russh::keys::PublicKey;
use russh::{ChannelMsg, ChannelStream, Disconnect, Preferred, SshId, cipher, kex, mac};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::timeout;

use crate::agent::ForwardAgent;
use crate::auth::{self, Credential, Prompter};
use crate::client::{Banner, Client, RemoteDest, RemoteRoutes};
use crate::meter::Meter;
use crate::{HostKeyVerifier, Result, TransportError};

const CLIENT_ID: &str = concat!("SSH-2.0-Citadel_", env!("CARGO_PKG_VERSION"));
const KEEPALIVE_MAX: usize = 3;
const WINDOW: u32 = 8 << 20;
const PACKET: u32 = 32 * 1024;
const EXEC_CAP: usize = 16 << 20;
const PROMPT_WAIT: Duration = Duration::from_secs(180);

const KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G16_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

const LEGACY_KEX: &[kex::Name] = &[
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G16_SHA512,
    kex::DH_G14_SHA256,
    kex::DH_G14_SHA1,
    kex::DH_GEX_SHA1,
    kex::DH_G1_SHA1,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

const LEGACY_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
    cipher::AES_256_CBC,
    cipher::AES_192_CBC,
    cipher::AES_128_CBC,
];

const LEGACY_MACS: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
    mac::HMAC_SHA1_ETM,
    mac::HMAC_SHA1,
];

#[derive(Clone, Debug)]
pub struct Hop {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub credential: Credential,
    pub legacy: bool,
}

#[derive(Clone)]
pub struct Route {
    pub hops: Vec<Hop>,
    pub keepalive: Duration,
    pub connect_timeout: Duration,
    pub agent_forward: bool,
    pub verifier: Arc<dyn HostKeyVerifier>,
    pub banner: Option<Banner>,
    pub prompter: Option<Arc<dyn Prompter>>,
}

impl Route {
    pub fn direct(hop: Hop, verifier: Arc<dyn HostKeyVerifier>) -> Self {
        Self {
            hops: vec![hop],
            keepalive: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(15),
            agent_forward: false,
            verifier,
            banner: None,
            prompter: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecEvent {
    Stdout(Bytes),
    Stderr(Bytes),
    Exit(u32),
}

pub struct ExecStream {
    ch: russh::Channel<Msg>,
}

impl ExecStream {
    pub async fn next(&mut self) -> Option<ExecEvent> {
        loop {
            match self.ch.wait().await? {
                ChannelMsg::Data { data } => return Some(ExecEvent::Stdout(data)),
                ChannelMsg::ExtendedData { data, .. } => return Some(ExecEvent::Stderr(data)),
                ChannelMsg::ExitStatus { exit_status } => {
                    return Some(ExecEvent::Exit(exit_status));
                }
                ChannelMsg::Close => return None,
                _ => {}
            }
        }
    }

    pub async fn close(&self) -> Result<()> {
        Ok(self.ch.close().await?)
    }
}

pub struct Session {
    pub(crate) handle: Handle<Client>,
    routes: Arc<RemoteRoutes>,
    closed: watch::Receiver<Option<String>>,
    host_key: Arc<OnceLock<PublicKey>>,
    via: Option<Arc<Session>>,
    pub(crate) agent_forward: bool,
    host: String,
    port: u16,
    user: String,
}

fn algorithm_kind(kind: &russh::AlgorithmKind) -> &'static str {
    match kind {
        russh::AlgorithmKind::Kex => "key exchange",
        russh::AlgorithmKind::Key => "host key",
        russh::AlgorithmKind::Cipher => "cipher",
        russh::AlgorithmKind::Mac => "MAC",
        russh::AlgorithmKind::Compression => "compression",
    }
}

fn preferred(legacy: bool) -> Preferred {
    if legacy {
        Preferred {
            kex: Cow::Borrowed(LEGACY_KEX),
            cipher: Cow::Borrowed(LEGACY_CIPHERS),
            mac: Cow::Borrowed(LEGACY_MACS),
            ..Preferred::DEFAULT
        }
    } else {
        Preferred {
            kex: Cow::Borrowed(KEX),
            ..Preferred::DEFAULT
        }
    }
}

fn config(keepalive: Duration, legacy: bool) -> Arc<Config> {
    Arc::new(Config {
        preferred: preferred(legacy),
        client_id: SshId::Standard(Cow::Borrowed(CLIENT_ID)),
        keepalive_interval: (!keepalive.is_zero()).then_some(keepalive),
        keepalive_max: KEEPALIVE_MAX,
        inactivity_timeout: None,
        window_size: WINDOW,
        maximum_packet_size: PACKET,
        nodelay: true,
        ..Config::default()
    })
}

async fn bounded<T>(
    limit: Duration,
    stage: &'static str,
    addr: &str,
    fut: impl Future<Output = Result<T>>,
) -> Result<T> {
    timeout(limit, fut)
        .await
        .map_err(|_| TransportError::Timeout {
            stage,
            addr: addr.to_owned(),
        })?
}

pub async fn connect(route: &Route) -> Result<Arc<Session>> {
    let last = route
        .hops
        .len()
        .checked_sub(1)
        .ok_or(TransportError::EmptyRoute)?;
    let mut prev: Option<Arc<Session>> = None;
    for (i, hop) in route.hops.iter().enumerate() {
        let agent = (i == last && route.agent_forward)
            .then(|| Arc::new(ForwardAgent::for_credential(&hop.credential)));
        let addr = format!("{}:{}", hop.host, hop.port);
        let sess = match prev.take() {
            None => {
                let tcp = bounded(route.connect_timeout, "tcp connect", &addr, async {
                    Ok(TcpStream::connect((hop.host.as_str(), hop.port)).await?)
                })
                .await?;
                tcp.set_nodelay(true)?;
                Box::pin(Session::establish(route, hop, agent, tcp, None)).await?
            }
            Some(bastion) => {
                let stream = bounded(route.connect_timeout, "bastion channel", &addr, async {
                    bastion.direct_tcpip(&hop.host, hop.port).await
                })
                .await?;
                Box::pin(Session::establish(route, hop, agent, stream, Some(bastion))).await?
            }
        };
        prev = Some(Arc::new(sess));
    }
    prev.ok_or(TransportError::EmptyRoute)
}

impl Session {
    async fn establish<S>(
        route: &Route,
        hop: &Hop,
        agent: Option<Arc<ForwardAgent>>,
        stream: S,
        via: Option<Arc<Session>>,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let addr = format!("{}:{}", hop.host, hop.port);
        let (closed_tx, closed) = watch::channel(None);
        let routes = Arc::new(RemoteRoutes::default());
        let host_key = Arc::new(OnceLock::new());
        let agent_forward = agent.is_some();
        let client = Client {
            host: hop.host.clone(),
            port: hop.port,
            verifier: route.verifier.clone(),
            routes: routes.clone(),
            agent,
            closed: Arc::new(closed_tx),
            host_key: host_key.clone(),
            banner: route.banner.clone(),
        };
        let handshake =
            russh::client::connect_stream(config(route.keepalive, hop.legacy), stream, client);
        let mut handle = bounded(route.connect_timeout, "handshake", &addr, async {
            handshake.await.map_err(|e| match e {
                TransportError::Ssh(russh::Error::UnknownKey) => TransportError::HostKeyRejected {
                    host: hop.host.clone(),
                    port: hop.port,
                },
                TransportError::Ssh(russh::Error::NoCommonAlgo { kind, theirs, .. })
                    if !hop.legacy =>
                {
                    TransportError::Legacy {
                        host: hop.host.clone(),
                        kind: algorithm_kind(&kind).to_owned(),
                        offered: theirs.join(", "),
                    }
                }
                other => other,
            })
        })
        .await?;
        let interactive =
            route.prompter.is_some() || matches!(hop.credential, Credential::Interactive(_));
        let auth_wait = if interactive {
            PROMPT_WAIT
        } else {
            route.connect_timeout.saturating_mul(4)
        };
        bounded(
            auth_wait,
            "authentication",
            &addr,
            auth::authenticate(
                &mut handle,
                &hop.user,
                &hop.host,
                &hop.credential,
                route.prompter.clone(),
            ),
        )
        .await?;
        Ok(Self {
            handle,
            routes,
            closed,
            host_key,
            via,
            agent_forward,
            host: hop.host.clone(),
            port: hop.port,
            user: hop.user.clone(),
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn host_key(&self) -> Option<&PublicKey> {
        self.host_key.get()
    }

    pub fn depth(&self) -> usize {
        self.via.as_ref().map_or(0, |v| v.depth() + 1)
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
            || self.closed.borrow().is_some()
            || self.via.as_ref().is_some_and(|v| v.is_closed())
    }

    pub async fn closed(&self) -> String {
        let mut rx = self.closed.clone();
        loop {
            if let Some(reason) = rx.borrow_and_update().clone() {
                return reason;
            }
            if self.handle.is_closed() {
                return "connection lost".to_owned();
            }
            if rx.changed().await.is_err() {
                return "session dropped".to_owned();
            }
        }
    }

    pub async fn rtt(&self, limit: Duration) -> Option<Duration> {
        if self.is_closed() {
            return None;
        }
        let start = tokio::time::Instant::now();
        let replied = timeout(limit, self.handle.send_ping()).await;
        let took = start.elapsed();
        match replied {
            Ok(Ok(())) if !self.is_closed() => Some(took),
            _ => None,
        }
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
        if let Some(via) = &self.via {
            Box::pin(via.disconnect()).await;
        }
    }

    pub async fn direct_tcpip(&self, host: &str, port: u16) -> Result<ChannelStream<Msg>> {
        self.direct_tcpip_from(host, port, "127.0.0.1", 0).await
    }

    pub async fn direct_tcpip_from(
        &self,
        host: &str,
        port: u16,
        orig_addr: &str,
        orig_port: u16,
    ) -> Result<ChannelStream<Msg>> {
        let ch = self
            .handle
            .channel_open_direct_tcpip(host, u32::from(port), orig_addr, u32::from(orig_port))
            .await?;
        Ok(ch.into_stream())
    }

    pub async fn exec(&self, cmd: &str) -> Result<ExecOutput> {
        let mut stream = self.exec_stream(cmd).await?;
        let mut out = ExecOutput::default();
        while let Some(ev) = stream.next().await {
            match ev {
                ExecEvent::Stdout(d) if out.stdout.len() + d.len() <= EXEC_CAP => {
                    out.stdout.extend_from_slice(&d);
                }
                ExecEvent::Stderr(d) if out.stderr.len() + d.len() <= EXEC_CAP => {
                    out.stderr.extend_from_slice(&d);
                }
                ExecEvent::Exit(code) => out.status = Some(code),
                ExecEvent::Stdout(_) | ExecEvent::Stderr(_) => {}
            }
        }
        Ok(out)
    }

    pub async fn exec_stream(&self, cmd: &str) -> Result<ExecStream> {
        let ch = self.handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        Ok(ExecStream { ch })
    }

    pub(crate) async fn open_session_channel(&self) -> Result<russh::Channel<Msg>> {
        Ok(self.handle.channel_open_session().await?)
    }

    pub async fn remote_forward(
        &self,
        bind_addr: &str,
        bind_port: u16,
        dest_host: &str,
        dest_port: u16,
        meter: Arc<Meter>,
    ) -> Result<u16> {
        let dest = RemoteDest {
            host: dest_host.to_owned(),
            port: dest_port,
            meter,
        };
        self.routes
            .insert(bind_addr, u32::from(bind_port), dest.clone());
        match self
            .handle
            .tcpip_forward(bind_addr, u32::from(bind_port))
            .await
        {
            Ok(assigned) => {
                let assigned = if bind_port == 0 {
                    u16::try_from(assigned).unwrap_or(0)
                } else {
                    bind_port
                };
                if assigned != bind_port {
                    self.routes.remove(bind_addr, u32::from(bind_port));
                    self.routes.insert(bind_addr, u32::from(assigned), dest);
                }
                Ok(assigned)
            }
            Err(russh::Error::RequestDenied) => {
                self.routes.remove(bind_addr, u32::from(bind_port));
                Err(TransportError::ForwardDenied {
                    addr: bind_addr.to_owned(),
                    port: bind_port,
                })
            }
            Err(e) => {
                self.routes.remove(bind_addr, u32::from(bind_port));
                Err(e.into())
            }
        }
    }

    pub async fn cancel_remote_forward(&self, bind_addr: &str, bind_port: u16) -> Result<()> {
        self.routes.remove(bind_addr, u32::from(bind_port));
        Ok(self
            .handle
            .cancel_tcpip_forward(bind_addr, u32::from(bind_port))
            .await?)
    }
}
