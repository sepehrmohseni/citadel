use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use russh::keys::ssh_encoding::Encode;
use russh::keys::ssh_key::Signature;
use russh::keys::ssh_key::private::KeypairData;
use russh::keys::{HashAlg, PrivateKey, PublicKey, signature};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::auth::{self, AgentEndpoint, Credential, Used};
use crate::{Result, TransportError};

const MAX_FRAME: usize = 256 * 1024;
const FAILURE: u8 = 5;
const REQUEST_IDENTITIES: u8 = 11;
const IDENTITIES_ANSWER: u8 = 12;
const SIGN_REQUEST: u8 = 13;
const SIGN_RESPONSE: u8 = 14;
const FLAG_RSA_SHA2_256: u32 = 2;
const FLAG_RSA_SHA2_512: u32 = 4;

pub(crate) enum ForwardAgent {
    Key(Arc<PrivateKey>),
    Upstream {
        endpoint: AgentEndpoint,
        fingerprint: Option<String>,
    },
    Empty,
}

impl ForwardAgent {
    pub fn for_credential(cred: &Credential) -> Self {
        match cred {
            Credential::Key(k) => Self::Key(k.clone()),
            Credential::Agent {
                endpoint,
                fingerprint,
            } => Self::Upstream {
                endpoint: endpoint.clone(),
                fingerprint: fingerprint.clone(),
            },
            Credential::Auto { .. } | Credential::Password(_) | Credential::Interactive(_) => {
                Self::Empty
            }
        }
    }
}

pub(crate) struct ForwardSlot {
    agent: RwLock<Arc<ForwardAgent>>,
    gate: Option<Arc<AtomicBool>>,
}

impl ForwardSlot {
    pub fn new(cred: &Credential, gate: Option<Arc<AtomicBool>>) -> Self {
        Self {
            agent: RwLock::new(Arc::new(ForwardAgent::for_credential(cred))),
            gate,
        }
    }

    pub fn settle(&self, used: &Used) {
        let next = match used {
            Used::File(k) => ForwardAgent::Key(k.clone()),
            Used::Agent {
                endpoint,
                fingerprint,
            } => ForwardAgent::Upstream {
                endpoint: endpoint.clone(),
                fingerprint: Some(fingerprint.clone()),
            },
        };
        *self.agent.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
    }

    fn current(&self) -> Arc<ForwardAgent> {
        self.agent
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(self: Arc<Self>, mut io: S) {
        let mut frame = Vec::new();
        loop {
            let mut len = [0u8; 4];
            if io.read_exact(&mut len).await.is_err() {
                return;
            }
            let len = u32::from_be_bytes(len) as usize;
            if len == 0 || len > MAX_FRAME {
                return;
            }
            frame.resize(len, 0);
            if io.read_exact(&mut frame).await.is_err() {
                return;
            }
            let open = self.gate.as_ref().is_none_or(|g| g.load(Ordering::SeqCst));
            let reply = if open {
                self.current().respond(&frame).await
            } else {
                Err(TransportError::Agent("vault is locked".into()))
            };
            let reply = reply.unwrap_or_else(|e| {
                tracing::debug!("forwarded agent request refused: {e}");
                vec![FAILURE]
            });
            let Ok(reply_len) = u32::try_from(reply.len()) else {
                return;
            };
            let mut out = Vec::with_capacity(4 + reply.len());
            out.extend_from_slice(&reply_len.to_be_bytes());
            out.extend_from_slice(&reply);
            if io.write_all(&out).await.is_err() || io.flush().await.is_err() {
                return;
            }
        }
    }
}

impl ForwardAgent {
    async fn exposed(&self) -> Result<Vec<PublicKey>> {
        match self {
            Self::Key(k) => Ok(vec![k.public_key().clone()]),
            Self::Upstream {
                endpoint,
                fingerprint,
            } => {
                let mut agent = auth::connect_agent(endpoint).await?;
                auth::agent_keys(&mut agent, fingerprint.as_deref()).await
            }
            Self::Empty => Ok(Vec::new()),
        }
    }

    async fn respond(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let (&kind, mut body) = frame
            .split_first()
            .ok_or(TransportError::Agent("empty frame".into()))?;
        match kind {
            REQUEST_IDENTITIES => {
                let keys = self.exposed().await?;
                let mut out = vec![IDENTITIES_ANSWER];
                out.extend_from_slice(&u32::try_from(keys.len()).unwrap_or(0).to_be_bytes());
                for key in keys {
                    put_string(&mut out, &key.to_bytes()?);
                    put_string(&mut out, key.comment().as_bytes());
                }
                Ok(out)
            }
            SIGN_REQUEST => {
                let blob = take_string(&mut body)?;
                let data = take_string(&mut body)?;
                let flags = take_u32(&mut body).unwrap_or(0);
                let hash = if flags & FLAG_RSA_SHA2_512 != 0 {
                    Some(HashAlg::Sha512)
                } else if flags & FLAG_RSA_SHA2_256 != 0 {
                    Some(HashAlg::Sha256)
                } else {
                    None
                };
                let sig = self.sign(blob, data, hash).await?;
                let mut out = vec![SIGN_RESPONSE];
                put_string(&mut out, &sig.encode_vec()?);
                Ok(out)
            }
            _ => Ok(vec![FAILURE]),
        }
    }

    async fn sign(&self, blob: &[u8], data: &[u8], hash: Option<HashAlg>) -> Result<Signature> {
        match self {
            Self::Key(k) => {
                if k.public_key().to_bytes()? != blob {
                    return Err(TransportError::Agent("key not exposed".into()));
                }
                let sig = match k.key_data() {
                    KeypairData::Rsa(rsa) => signature::Signer::try_sign(&(rsa, hash), data),
                    _ => signature::Signer::try_sign(k.as_ref(), data),
                };
                sig.map_err(|e| TransportError::Agent(e.to_string()))
            }
            Self::Upstream {
                endpoint,
                fingerprint,
            } => {
                let mut agent = auth::connect_agent(endpoint).await?;
                let keys = auth::agent_keys(&mut agent, fingerprint.as_deref()).await?;
                let mut target = None;
                for key in keys {
                    if key.to_bytes()? == blob {
                        target = Some(key);
                        break;
                    }
                }
                let key = target.ok_or(TransportError::Agent("key not exposed".into()))?;
                Ok(agent.sign_request_signature(&key, hash, data).await?)
            }
            Self::Empty => Err(TransportError::Agent("no identity exposed".into())),
        }
    }
}

fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn take_u32(buf: &mut &[u8]) -> Result<u32> {
    let (head, rest) = buf
        .split_first_chunk::<4>()
        .ok_or(TransportError::Agent("short frame".into()))?;
    *buf = rest;
    Ok(u32::from_be_bytes(*head))
}

fn take_string<'a>(buf: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = take_u32(buf)? as usize;
    if buf.len() < len {
        return Err(TransportError::Agent("short frame".into()));
    }
    let (s, rest) = buf.split_at(len);
    *buf = rest;
    Ok(s)
}
