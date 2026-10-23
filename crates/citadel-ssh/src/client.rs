use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use russh::ChannelOpenFailure;
use russh::client::{ChannelOpenHandle, DisconnectReason, Handler, Msg, Session};
use russh::keys::{PublicKey, PublicKeyOrCertificate};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::agent::ForwardSlot;
use crate::meter::{Meter, Metered};
use crate::{TransportError, auth};

pub type Banner = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub(crate) struct RemoteDest {
    pub host: String,
    pub port: u16,
    pub meter: Arc<Meter>,
}

#[derive(Default)]
pub(crate) struct RemoteRoutes(Mutex<HashMap<(String, u32), RemoteDest>>);

impl RemoteRoutes {
    pub fn insert(&self, addr: &str, port: u32, dest: RemoteDest) {
        self.map().insert((addr.to_owned(), port), dest);
    }

    pub fn remove(&self, addr: &str, port: u32) {
        self.map().remove(&(addr.to_owned(), port));
    }

    fn lookup(&self, addr: &str, port: u32) -> Option<RemoteDest> {
        let map = self.map();
        map.get(&(addr.to_owned(), port)).cloned().or_else(|| {
            let mut by_port = map.iter().filter(|((_, p), _)| *p == port);
            match (by_port.next(), by_port.next()) {
                (Some((_, d)), None) => Some(d.clone()),
                _ => None,
            }
        })
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<(String, u32), RemoteDest>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) struct Client {
    pub host: String,
    pub port: u16,
    pub verifier: Arc<dyn crate::HostKeyVerifier>,
    pub routes: Arc<RemoteRoutes>,
    pub agent: Option<Arc<ForwardSlot>>,
    pub closed: Arc<watch::Sender<Option<String>>>,
    pub host_key: Arc<OnceLock<PublicKey>>,
    pub banner: Option<Banner>,
    pub waiting: Arc<AtomicUsize>,
}

struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn new(n: &'a AtomicUsize) -> Self {
        n.fetch_add(1, Ordering::SeqCst);
        Self(n)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Handler for Client {
    type Error = TransportError;

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(cb) = &self.banner {
            cb(banner);
        }
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => PublicKey::from(cert.public_key().clone()),
        };
        let ok = {
            let _waiting = Waiting::new(&self.waiting);
            self.verifier.verify(&self.host, self.port, &key).await?
        };
        if ok {
            let _ = self.host_key.set(key);
        } else {
            tracing::warn!(
                host = %self.host,
                port = self.port,
                fingerprint = %auth::fingerprint(&key),
                "host key rejected"
            );
        }
        Ok(ok)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(dest) = self.routes.lookup(connected_address, connected_port) else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let _lease = dest.meter.open();
            let local = match TcpStream::connect((dest.host.as_str(), dest.port)).await {
                Ok(s) => s,
                Err(e) => {
                    dest.meter.fail();
                    tracing::debug!(host = %dest.host, port = dest.port, "remote forward target refused: {e}");
                    return;
                }
            };
            let _ = local.set_nodelay(true);
            let mut local = Metered::new(local, dest.meter.clone());
            let mut remote = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
        });
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        match &self.agent {
            Some(agent) => {
                reply.accept().await;
                tokio::spawn(agent.clone().serve(channel.into_stream()));
            }
            None => {
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await
            }
        }
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let (msg, out) = match reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                (format!("{:?}: {}", info.reason_code, info.message), Ok(()))
            }
            DisconnectReason::Error(e) => (e.to_string(), Err(e)),
        };
        self.closed.send_replace(Some(msg));
        out
    }
}
