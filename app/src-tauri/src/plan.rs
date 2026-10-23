use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use citadel_ssh::{
    AgentEndpoint, Credential, Hop, HostKeyStatus, HostKeyVerifier, KnownHostsFile, PromptRequest,
    Prompter, PublicKey, Route, UnknownHost, decode_key, default_identity_files, fingerprint,
};
use citadel_vault::{
    Auth, HostProfile, Id, IdentityKey, KeyMaterial, KnownHost, SecretString, Vault, secret_from,
};
use futures::future::BoxFuture;
use tauri::Emitter;

use crate::core::{Answer, Core, EV_BANNER, PromptKind, PromptLine};
use crate::error::{AppError, CmdResult};

const MAX_HOPS: usize = 8;

pub fn agent_endpoint(pref: Option<&str>) -> AgentEndpoint {
    match pref {
        Some(p) if citadel_config::agent::is_pipe(p) => AgentEndpoint::Pipe(p.to_owned()),
        Some(p) if !p.is_empty() => AgentEndpoint::Unix(p.into()),
        _ => AgentEndpoint::Env,
    }
}

pub fn chain(vault: &Vault, host: Id) -> CmdResult<Vec<HostProfile>> {
    let mut out = Vec::new();
    expand(vault, host, &mut HashSet::new(), &mut out)?;
    Ok(out)
}

fn expand(
    vault: &Vault,
    id: Id,
    seen: &mut HashSet<Id>,
    out: &mut Vec<HostProfile>,
) -> CmdResult<()> {
    if !seen.insert(id) {
        return Err(AppError::Cycle(id));
    }
    if seen.len() > MAX_HOPS {
        return Err(AppError::invalid(format!(
            "jump chain exceeds {MAX_HOPS} hops"
        )));
    }
    let h = vault
        .get::<HostProfile>(id)?
        .ok_or(AppError::NotFound("host"))?;
    for jid in h.jump.clone() {
        expand(vault, jid, seen, out)?;
    }
    out.push(h);
    if out.len() > MAX_HOPS {
        return Err(AppError::invalid(format!(
            "jump chain exceeds {MAX_HOPS} hops"
        )));
    }
    Ok(())
}

fn credential(core: &Arc<Core>, host: &HostProfile) -> CmdResult<Credential> {
    let agent = || agent_endpoint(core.prefs().agent.as_deref());
    Ok(match &host.auth {
        Auth::Identity(kid) => {
            let key = core
                .vault
                .get::<IdentityKey>(*kid)?
                .ok_or(AppError::NotFound("identity"))?;
            match &key.material {
                KeyMaterial::Embedded {
                    private_key,
                    passphrase,
                } => Credential::Key(decode_key(private_key, passphrase.as_ref())?),
                KeyMaterial::Agent => Credential::Agent {
                    endpoint: agent(),
                    fingerprint: Some(key.fingerprint.clone()),
                },
            }
        }
        Auth::Agent {
            fingerprint: Some(fp),
        } => Credential::Agent {
            endpoint: agent(),
            fingerprint: Some(fp.clone()),
        },
        Auth::Agent { fingerprint: None } => Credential::Auto {
            endpoint: agent(),
            files: dirs::home_dir()
                .map(|home| default_identity_files(&home))
                .unwrap_or_default(),
        },
        Auth::Password(pw) => Credential::Password(pw.clone()),
        Auth::Interactive => Credential::Interactive(Arc::new(UiPrompter { core: core.clone() })),
    })
}

pub fn route(core: &Arc<Core>, host: Id) -> CmdResult<Route> {
    let hosts = chain(&core.vault, host)?;
    let mut hops = Vec::with_capacity(hosts.len());
    for h in &hosts {
        hops.push(Hop {
            host: h.hostname.clone(),
            port: h.port,
            user: h.user.clone(),
            credential: credential(core, h)?,
            legacy: h.legacy_crypto,
        });
    }
    let target = hosts.last().ok_or(AppError::NotFound("host"))?;
    let app = core.app.clone();
    let label = target.label.clone();
    Ok(Route {
        hops,
        keepalive: Duration::from_secs(u64::from(target.keepalive_secs)),
        connect_timeout: Duration::from_secs(15),
        agent_forward: target.agent_forward,
        verifier: Arc::new(VaultVerifier { core: core.clone() }),
        banner: Some(Arc::new(move |text: &str| {
            let _ = app.emit(EV_BANNER, (label.clone(), text.to_owned()));
        })),
        prompter: Some(Arc::new(UiPrompter { core: core.clone() })),
        agent_gate: Some(core.agent_gate.clone()),
    })
}

struct UiPrompter {
    core: Arc<Core>,
}

impl Prompter for UiPrompter {
    fn prompt(&self, req: PromptRequest) -> BoxFuture<'static, Option<Vec<SecretString>>> {
        let core = self.core.clone();
        Box::pin(async move {
            let kind = PromptKind::Interactive {
                host: req.host,
                user: req.user,
                name: req.name,
                instructions: req.instructions,
                prompts: req
                    .prompts
                    .into_iter()
                    .map(|(text, echo)| PromptLine { text, echo })
                    .collect(),
            };
            match core.prompts.ask(&core.app, kind).await? {
                Answer::Interactive { answers } => {
                    Some(answers.into_iter().map(secret_from).collect())
                }
                _ => None,
            }
        })
    }
}

struct VaultVerifier {
    core: Arc<Core>,
}

fn key_parts(key: &PublicKey) -> (String, String) {
    let line = key.to_openssh().unwrap_or_default();
    let mut it = line.split_whitespace();
    let alg = it.next().unwrap_or_default().to_owned();
    let blob = it.next().unwrap_or_default().to_owned();
    (alg, blob)
}

enum Trust {
    Known,
    Unknown,
    Changed(Option<String>),
}

fn assess(core: &Core, host: &str, port: u16, key: &PublicKey) -> Trust {
    let (alg, blob) = key_parts(key);
    let stored = core.vault.list::<KnownHost>().unwrap_or_default();
    let mut previous = None;
    for k in stored.iter().filter(|k| k.host == host && k.port == port) {
        if k.algorithm == alg && k.key == blob {
            return Trust::Known;
        }
        previous.get_or_insert_with(|| k.fingerprint.clone());
    }
    if previous.is_some() {
        return Trust::Changed(previous);
    }
    match KnownHostsFile::system(UnknownHost::Reject).map(|f| f.status(host, port, key)) {
        Some(Ok(HostKeyStatus::Known)) => Trust::Known,
        Some(Ok(HostKeyStatus::Changed { .. })) => Trust::Changed(None),
        _ => Trust::Unknown,
    }
}

fn remember(vault: &Vault, host: &str, port: u16, key: &PublicKey) -> CmdResult<()> {
    let (alg, blob) = key_parts(key);
    for old in vault.list::<KnownHost>()? {
        if old.host == host && old.port == port {
            vault.remove(old.id)?;
        }
    }
    let mut rec = KnownHost {
        id: Id::random()?,
        host: host.to_owned(),
        port,
        algorithm: alg,
        key: blob,
        fingerprint: fingerprint(key),
        updated_ms: 0,
    };
    vault.put(&mut rec)?;
    Ok(())
}

impl HostKeyVerifier for VaultVerifier {
    fn verify<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, citadel_ssh::Result<bool>> {
        Box::pin(async move {
            let core = self.core.clone();
            let (h, k) = (host.to_owned(), key.clone());
            let trust = tokio::task::spawn_blocking(move || assess(&core, &h, port, &k))
                .await
                .unwrap_or(Trust::Unknown);
            let (status, previous) = match trust {
                Trust::Known => return Ok(true),
                Trust::Unknown => ("unknown", None),
                Trust::Changed(prev) => ("changed", prev),
            };
            let kind = PromptKind::HostKey {
                host: host.to_owned(),
                port,
                algorithm: key.algorithm().to_string(),
                fingerprint: fingerprint(key),
                status,
                previous,
            };
            let accepted = matches!(
                self.core.prompts.ask(&self.core.app, kind).await,
                Some(Answer::HostKey { accept: true })
            );
            if accepted {
                let vault = self.core.vault.clone();
                let (h, k) = (host.to_owned(), key.clone());
                let saved =
                    tokio::task::spawn_blocking(move || remember(&vault, &h, port, &k)).await;
                if !matches!(saved, Ok(Ok(()))) {
                    tracing::warn!(host, port, "accepted host key could not be persisted");
                }
            }
            Ok(accepted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_vault::KdfParams;

    fn host(v: &Vault, label: &str, jump: Vec<Id>) -> Id {
        let mut h = HostProfile::new(label, label, "u", Auth::Interactive).unwrap();
        h.jump = jump;
        v.put(&mut h).unwrap();
        h.id
    }

    #[test]
    fn chain_flattens_nested_bastions_and_rejects_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path().join("v.redb")).unwrap();
        v.initialize(
            &SecretString::from("pw"),
            KdfParams {
                m_kib: 64,
                t: 1,
                p: 1,
            },
        )
        .unwrap();
        let edge = host(&v, "edge", vec![]);
        let mid = host(&v, "mid", vec![edge]);
        let db = host(&v, "db", vec![mid]);
        let labels: Vec<String> = chain(&v, db)
            .unwrap()
            .into_iter()
            .map(|h| h.label.clone())
            .collect();
        assert_eq!(labels, ["edge", "mid", "db"]);

        let mut e = v.get::<HostProfile>(edge).unwrap().unwrap();
        e.jump = vec![db];
        v.put(&mut e).unwrap();
        assert!(matches!(chain(&v, db), Err(AppError::Cycle(_))));
    }
}
