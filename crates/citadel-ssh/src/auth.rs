use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{MethodKind, MethodSet};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::client::Client;
use crate::{Result, TransportError};

const MAX_KBD_ROUNDS: usize = 8;
const MAX_FACTORS: usize = 3;
const MAX_AGENT_KEYS: usize = 8;
const MAX_KEY_FILE: u64 = 64 * 1024;
const PEM_WIDTH: usize = 70;

pub type DynAgent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AgentEndpoint {
    Env,
    Unix(PathBuf),
    Pipe(String),
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    pub user: String,
    pub host: String,
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<(String, bool)>,
}

pub trait Prompter: Send + Sync + 'static {
    fn prompt(&self, req: PromptRequest) -> BoxFuture<'static, Option<Vec<SecretString>>>;
}

#[derive(Clone)]
pub enum Credential {
    Key(Arc<PrivateKey>),
    Agent {
        endpoint: AgentEndpoint,
        fingerprint: Option<String>,
    },
    Auto {
        endpoint: AgentEndpoint,
        files: Vec<PathBuf>,
    },
    Password(SecretString),
    Interactive(Arc<dyn Prompter>),
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(k) => write!(f, "Key({})", fingerprint(k.public_key())),
            Self::Agent {
                endpoint,
                fingerprint,
            } => f
                .debug_struct("Agent")
                .field("endpoint", endpoint)
                .field("fingerprint", fingerprint)
                .finish(),
            Self::Auto { endpoint, files } => f
                .debug_struct("Auto")
                .field("endpoint", endpoint)
                .field("files", files)
                .finish(),
            Self::Password(_) => f.write_str("Password([REDACTED])"),
            Self::Interactive(_) => f.write_str("Interactive"),
        }
    }
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub fn default_identity_files(home: &Path) -> Vec<PathBuf> {
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|n| home.join(".ssh").join(n))
        .collect()
}

fn armor(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("-----BEGIN ")?;
    let head_end = rest.find("-----")? + 16;
    let foot = line.get(head_end..)?.find("-----END ")? + head_end;
    line.ends_with("-----").then_some((head_end, foot))
}

pub fn normalize_pem(raw: &str) -> Zeroizing<String> {
    let src = raw.trim_start_matches('\u{feff}');
    let mut text = Zeroizing::new(String::with_capacity(src.len()));
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            text.push('\n');
        } else {
            text.push(c);
        }
    }
    let trimmed = text.trim();
    let mut out = Zeroizing::new(String::with_capacity(trimmed.len() + 2));
    if !trimmed.contains('\n')
        && let Some((head_end, foot_start)) = armor(trimmed)
    {
        let (head, foot) = (&trimmed[..head_end], &trimmed[foot_start..]);
        let body: Zeroizing<String> = Zeroizing::new(
            trimmed[head_end..foot_start]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect(),
        );
        if !body.contains(':') {
            out.push_str(head);
            out.push('\n');
            for chunk in body.as_bytes().chunks(PEM_WIDTH) {
                out.push_str(&String::from_utf8_lossy(chunk));
                out.push('\n');
            }
            out.push_str(foot);
            out.push('\n');
            return out;
        }
    }
    for line in trimmed.lines() {
        out.push_str(line.trim());
        out.push('\n');
    }
    out
}

pub fn decode_key(
    pem: &SecretString,
    passphrase: Option<&SecretString>,
) -> Result<Arc<PrivateKey>> {
    let text = normalize_pem(pem.expose_secret());
    if text.starts_with("-----BEGIN DSA PRIVATE KEY-----") {
        return Err(TransportError::Credential(
            "DSA keys are disabled in OpenSSH 7.0 and later; generate an ed25519 key instead"
                .into(),
        ));
    }
    if text.contains("DEK-Info:") && !text.contains("DEK-Info: AES-128-CBC,") {
        return Err(TransportError::Credential(
            "this PEM key uses a legacy cipher; convert it with `ssh-keygen -p -f <file>`".into(),
        ));
    }
    let key = russh::keys::decode_secret_key(&text, passphrase.map(ExposeSecret::expose_secret))?;
    Ok(Arc::new(key))
}

pub async fn connect_agent(endpoint: &AgentEndpoint) -> Result<DynAgent> {
    match endpoint {
        #[cfg(unix)]
        AgentEndpoint::Env => Ok(AgentClient::connect_env().await?.dynamic()),
        #[cfg(unix)]
        AgentEndpoint::Unix(path) => Ok(AgentClient::connect_uds(path).await?.dynamic()),
        #[cfg(windows)]
        AgentEndpoint::Env => {
            let pipe = std::env::var("SSH_AUTH_SOCK")
                .unwrap_or_else(|_| r"\\.\pipe\openssh-ssh-agent".to_owned());
            Ok(AgentClient::connect_named_pipe(pipe).await?.dynamic())
        }
        #[cfg(windows)]
        AgentEndpoint::Pipe(name) => Ok(AgentClient::connect_named_pipe(name).await?.dynamic()),
        #[allow(unreachable_patterns)]
        other => Err(TransportError::Agent(format!(
            "{other:?} is not supported on this platform"
        ))),
    }
}

pub(crate) async fn agent_keys(agent: &mut DynAgent, want: Option<&str>) -> Result<Vec<PublicKey>> {
    let keys: Vec<PublicKey> = agent
        .request_identities()
        .await?
        .into_iter()
        .filter_map(|id| match id {
            AgentIdentity::PublicKey { key, .. } => Some(key),
            AgentIdentity::Certificate { .. } => None,
        })
        .filter(|k| want.is_none_or(|fp| fingerprint(k) == fp))
        .collect();
    match want {
        Some(fp) if keys.is_empty() => Err(TransportError::AgentKeyMissing(fp.to_owned())),
        _ => Ok(keys),
    }
}

pub async fn list_agent_keys(endpoint: &AgentEndpoint) -> Result<Vec<PublicKey>> {
    let mut agent = connect_agent(endpoint).await?;
    agent_keys(&mut agent, None).await
}

enum Step {
    Done,
    Partial(MethodSet),
    Failed,
}

pub(crate) enum Used {
    File(Arc<PrivateKey>),
    Agent {
        endpoint: AgentEndpoint,
        fingerprint: String,
    },
}

fn short(key: &PublicKey) -> String {
    let fp = fingerprint(key);
    let tail = fp.strip_prefix("SHA256:").unwrap_or(&fp);
    format!(
        "{} {}",
        key.algorithm().as_str(),
        tail.get(..12).unwrap_or(tail)
    )
}

fn signed(what: &str, hash: Option<HashAlg>, rsa: bool) -> String {
    match (rsa, hash) {
        (true, Some(HashAlg::Sha512)) => format!("{what} as rsa-sha2-512"),
        (true, Some(HashAlg::Sha256)) => format!("{what} as rsa-sha2-256"),
        (true, _) => format!("{what} as ssh-rsa"),
        (false, _) => what.to_owned(),
    }
}

fn method_name(m: &MethodKind) -> &'static str {
    m.into()
}

struct Ctx<'a> {
    h: &'a mut Handle<Client>,
    user: &'a str,
    host: &'a str,
    prompter: Option<Arc<dyn Prompter>>,
    rsa: Option<Vec<Option<HashAlg>>>,
    tried: Vec<String>,
    methods: Option<MethodSet>,
    used: Option<Used>,
}

impl Ctx<'_> {
    fn record(&mut self, what: &str, res: AuthResult) -> Step {
        match res {
            AuthResult::Success => Step::Done,
            AuthResult::Failure {
                remaining_methods,
                partial_success,
            } => {
                self.methods = Some(remaining_methods.clone());
                if partial_success {
                    self.tried.push(format!("{what} accepted"));
                    Step::Partial(remaining_methods)
                } else {
                    self.tried.push(format!("{what} rejected"));
                    Step::Failed
                }
            }
        }
    }

    fn offers(&self, kind: MethodKind) -> bool {
        self.methods.as_ref().is_some_and(|m| m.contains(&kind))
    }

    fn detail(&self) -> String {
        let mut out = if self.tried.is_empty() {
            "no credential could be offered".to_owned()
        } else {
            self.tried.join("; ")
        };
        if let Some(m) = &self.methods {
            let names: Vec<&str> = m.iter().map(method_name).collect();
            if !names.is_empty() {
                out.push_str(&format!("; server accepts {}", names.join(", ")));
            }
        }
        out
    }

    async fn hashes(&mut self, rsa: bool) -> Vec<Option<HashAlg>> {
        if !rsa {
            return vec![None];
        }
        if let Some(cached) = &self.rsa {
            return cached.clone();
        }
        let list = match self.h.best_supported_rsa_hash().await {
            Ok(Some(alg)) => vec![alg],
            _ => vec![Some(HashAlg::Sha512), Some(HashAlg::Sha256), None],
        };
        self.rsa = Some(list.clone());
        list
    }

    async fn ask(
        &self,
        name: &str,
        lines: Vec<(String, bool)>,
    ) -> Result<Option<Vec<SecretString>>> {
        let Some(p) = self.prompter.clone() else {
            return Ok(None);
        };
        let req = PromptRequest {
            user: self.user.to_owned(),
            host: self.host.to_owned(),
            name: name.to_owned(),
            instructions: String::new(),
            prompts: lines,
        };
        Ok(Some(p.prompt(req).await.ok_or(TransportError::Cancelled)?))
    }

    async fn key(&mut self, key: &Arc<PrivateKey>) -> Result<Step> {
        let rsa = key.algorithm().is_rsa();
        let what = short(key.public_key());
        let mut step = Step::Failed;
        for hash in self.hashes(rsa).await {
            let res = self
                .h
                .authenticate_publickey(self.user, PrivateKeyWithHashAlg::new(key.clone(), hash))
                .await?;
            step = self.record(&signed(&what, hash, rsa), res);
            if !matches!(step, Step::Failed) {
                break;
            }
        }
        Ok(step)
    }

    async fn agent(&mut self, endpoint: &AgentEndpoint, want: Option<&str>) -> Result<Step> {
        let mut agent = match connect_agent(endpoint).await {
            Ok(a) => a,
            Err(e) if want.is_none() => {
                self.tried.push(format!("ssh agent unavailable ({e})"));
                return Ok(Step::Failed);
            }
            Err(e) => return Err(e),
        };
        let ids = agent.request_identities().await?;
        let ids: Vec<AgentIdentity> = ids
            .into_iter()
            .filter(|id| {
                want.is_none_or(|fp| match id {
                    AgentIdentity::PublicKey { key, .. } => fingerprint(key) == fp,
                    AgentIdentity::Certificate { certificate, .. } => {
                        PublicKey::from(certificate.public_key().clone())
                            .fingerprint(HashAlg::Sha256)
                            .to_string()
                            == fp
                    }
                })
            })
            .take(MAX_AGENT_KEYS)
            .collect();
        if ids.is_empty() {
            if let Some(fp) = want {
                return Err(TransportError::AgentKeyMissing(fp.to_owned()));
            }
            self.tried.push("ssh agent holds no keys".into());
            return Ok(Step::Failed);
        }
        let mut step = Step::Failed;
        for id in ids {
            match id {
                AgentIdentity::PublicKey { key, .. } => {
                    let rsa = key.algorithm().is_rsa();
                    let what = format!("agent {}", short(&key));
                    for hash in self.hashes(rsa).await {
                        let res = self
                            .h
                            .authenticate_publickey_with(self.user, key.clone(), hash, &mut agent)
                            .await
                            .map_err(|e| TransportError::Agent(e.to_string()))?;
                        step = self.record(&signed(&what, hash, rsa), res);
                        if !matches!(step, Step::Failed) {
                            self.used = Some(Used::Agent {
                                endpoint: endpoint.clone(),
                                fingerprint: fingerprint(&key),
                            });
                            return Ok(step);
                        }
                    }
                }
                AgentIdentity::Certificate { certificate, .. } => {
                    let rsa = certificate.algorithm().is_rsa();
                    let what = format!("agent certificate {}", certificate.key_id());
                    for hash in self.hashes(rsa).await {
                        let res = self
                            .h
                            .authenticate_certificate_with(
                                self.user,
                                certificate.clone(),
                                hash,
                                &mut agent,
                            )
                            .await
                            .map_err(|e| TransportError::Agent(e.to_string()))?;
                        step = self.record(&signed(&what, hash, rsa), res);
                        if !matches!(step, Step::Failed) {
                            return Ok(step);
                        }
                    }
                }
            }
        }
        Ok(step)
    }

    async fn files(&mut self, files: &[PathBuf]) -> Result<Step> {
        for path in files {
            let Ok(meta) = tokio::fs::metadata(path).await else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_KEY_FILE {
                continue;
            }
            let Ok(raw) = tokio::fs::read_to_string(path).await else {
                continue;
            };
            let pem = SecretString::from(raw);
            let key = match decode_key(&pem, None) {
                Ok(k) => k,
                Err(TransportError::Key(russh::keys::Error::KeyIsEncrypted)) => {
                    let asked = self
                        .ask(
                            "Key passphrase",
                            vec![(format!("Passphrase for {}", path.display()), false)],
                        )
                        .await?;
                    let Some(pass) = asked.and_then(|mut a| a.pop()) else {
                        self.tried
                            .push(format!("{} needs a passphrase", path.display()));
                        continue;
                    };
                    match decode_key(&pem, Some(&pass)) {
                        Ok(k) => k,
                        Err(e) => {
                            self.tried
                                .push(format!("{} not decrypted ({e})", path.display()));
                            continue;
                        }
                    }
                }
                Err(e) => {
                    self.tried
                        .push(format!("{} unreadable ({e})", path.display()));
                    continue;
                }
            };
            let step = self.key(&key).await?;
            if !matches!(step, Step::Failed) {
                self.used = Some(Used::File(key));
                return Ok(step);
            }
        }
        Ok(Step::Failed)
    }

    async fn password(&mut self, pw: &SecretString) -> Result<Step> {
        let res = self
            .h
            .authenticate_password(self.user, pw.expose_secret())
            .await?;
        let step = self.record("password", res);
        if matches!(step, Step::Failed) && self.offers(MethodKind::KeyboardInteractive) {
            return self.interactive(Some(pw)).await;
        }
        Ok(step)
    }

    async fn interactive(&mut self, auto: Option<&SecretString>) -> Result<Step> {
        let mut resp = self
            .h
            .authenticate_keyboard_interactive_start(self.user, None)
            .await?;
        let mut auto = auto;
        for _ in 0..MAX_KBD_ROUNDS {
            match resp {
                KeyboardInteractiveAuthResponse::Success => {
                    self.tried.push("keyboard-interactive accepted".into());
                    return Ok(Step::Done);
                }
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods,
                    partial_success,
                } => {
                    let res = AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    };
                    return Ok(self.record("keyboard-interactive", res));
                }
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let answers: Vec<SecretString> = if prompts.is_empty() {
                        Vec::new()
                    } else if let (Some(pw), [only]) = (auto, prompts.as_slice())
                        && !only.echo
                    {
                        auto = None;
                        vec![pw.clone()]
                    } else if let Some(p) = self.prompter.clone() {
                        let req = PromptRequest {
                            user: self.user.to_owned(),
                            host: self.host.to_owned(),
                            name,
                            instructions,
                            prompts: prompts.into_iter().map(|p| (p.prompt, p.echo)).collect(),
                        };
                        p.prompt(req).await.ok_or(TransportError::Cancelled)?
                    } else {
                        self.tried.push(
                            "keyboard-interactive needs input but no prompt is available".into(),
                        );
                        return Ok(Step::Failed);
                    };
                    let wire = answers
                        .iter()
                        .map(|a| a.expose_secret().to_owned())
                        .collect();
                    resp = self
                        .h
                        .authenticate_keyboard_interactive_respond(wire)
                        .await?;
                }
            }
        }
        Ok(Step::Failed)
    }

    async fn second(&mut self, methods: &MethodSet) -> Result<Step> {
        if methods.contains(&MethodKind::KeyboardInteractive) && self.prompter.is_some() {
            return self.interactive(None).await;
        }
        if methods.contains(&MethodKind::Password) {
            let line = format!("Password for {}@{}", self.user, self.host);
            if let Some(pw) = self
                .ask("Password", vec![(line, false)])
                .await?
                .and_then(|mut a| a.pop())
            {
                return self.password(&pw).await;
            }
        }
        Ok(Step::Failed)
    }
}

pub(crate) async fn authenticate(
    h: &mut Handle<Client>,
    user: &str,
    host: &str,
    cred: &Credential,
    prompter: Option<Arc<dyn Prompter>>,
) -> Result<Option<Used>> {
    let prompter = match cred {
        Credential::Interactive(p) => Some(p.clone()),
        _ => prompter,
    };
    let mut cx = Ctx {
        h,
        user,
        host,
        prompter,
        rsa: None,
        tried: Vec::new(),
        methods: None,
        used: None,
    };
    let mut step = match cred {
        Credential::Key(key) => cx.key(key).await?,
        Credential::Agent {
            endpoint,
            fingerprint,
        } => cx.agent(endpoint, fingerprint.as_deref()).await?,
        Credential::Auto { endpoint, files } => match cx.agent(endpoint, None).await? {
            Step::Failed => cx.files(files).await?,
            other => other,
        },
        Credential::Password(pw) => cx.password(pw).await?,
        Credential::Interactive(_) => cx.interactive(None).await?,
    };
    for _ in 0..MAX_FACTORS {
        step = match step {
            Step::Done => return Ok(cx.used),
            Step::Failed => break,
            Step::Partial(methods) => cx.second(&methods).await?,
        };
    }
    if matches!(step, Step::Done) {
        return Ok(cx.used);
    }
    Err(TransportError::AuthFailed {
        user: user.to_owned(),
        host: host.to_owned(),
        detail: cx.detail(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACD3V9lJY29MNi81VWD5Jwn+mDMuiZiZe8HLV6yvwPz+/QAAAJh0ri47dK4u\nOwAAAAtzc2gtZWQyNTUxOQAAACD3V9lJY29MNi81VWD5Jwn+mDMuiZiZe8HLV6yvwPz+/Q\nAAAEC8kQtJ9jAgfyBYoMB73HuyBHcdNq9GpaIPYT2bSMJeivdX2Uljb0w2LzVVYPknCf6Y\nMy6JmJl7wctXrK/A/P79AAAAD2NpdGFkZWwtZml4dHVyZQECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    fn decodes(text: &str) -> bool {
        decode_key(&SecretString::from(text), None).is_ok()
    }

    #[test]
    fn clean_keys_decode() {
        assert!(decodes(KEY));
    }

    #[test]
    fn mangled_pastes_are_repaired() {
        let crlf = KEY.replace('\n', "\r\n");
        let indented: String = KEY.lines().map(|l| format!("    {l}  \n")).collect();
        let bom = format!("\u{feff}\n\n{KEY}\n\n");
        let collapsed = KEY.trim().replace('\n', " ");
        let unwrapped = {
            let mut lines = KEY.trim().lines();
            let head = lines.next().unwrap_or_default();
            let mut rest: Vec<&str> = lines.collect();
            let foot = rest.pop().unwrap_or_default();
            format!("{head}{}{foot}", rest.concat())
        };
        for variant in [crlf, indented, bom, collapsed, unwrapped] {
            assert!(decodes(&variant), "failed to decode {variant:?}");
        }
    }

    #[test]
    fn unsupported_formats_explain_themselves() {
        let dsa = "-----BEGIN DSA PRIVATE KEY-----\nAAAA\n-----END DSA PRIVATE KEY-----\n";
        let err = decode_key(&SecretString::from(dsa), None)
            .err()
            .map(|e| e.to_string());
        assert!(err.is_some_and(|e| e.contains("ed25519")));
    }

    #[test]
    fn default_identity_files_prefer_modern_keys() {
        let files = default_identity_files(Path::new("/home/u"));
        let names: Vec<_> = files.iter().filter_map(|p| p.file_name()).collect();
        assert_eq!(names, ["id_ed25519", "id_ecdsa", "id_rsa"]);
    }
}
