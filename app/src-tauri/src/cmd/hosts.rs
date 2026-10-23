use std::sync::Arc;

use citadel_ssh::{KeyKind, decode_key, inspect, list_agent_keys};
use citadel_vault::{
    Auth, HostProfile, Id, IdentityKey, KeyMaterial, KnownHost, Point, SecretString, Snippet,
    TunnelSpec, Vault, secret_from,
};
use serde::Serialize;
use tauri::State;

use super::{AppState, blocking};
use crate::core::{Core, parse_id};
use crate::dto::{AuthInput, HostInput, HostView, IdentityView, KnownHostView, SnippetView};
use crate::error::{AppError, CmdResult};
use crate::plan;

const MAX_KEEPALIVE: u32 = 600;

fn sorted<T>(mut v: Vec<T>, key: impl Fn(&T) -> String) -> Vec<T> {
    v.sort_by_cached_key(|x| key(x).to_lowercase());
    v
}

fn clean(field: &str, value: &str, max: usize) -> CmdResult<String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(AppError::invalid(format!("{field} is required")));
    }
    if v.len() > max || v.chars().any(char::is_control) {
        return Err(AppError::invalid(format!("{field} is malformed")));
    }
    Ok(v.to_owned())
}

#[tauri::command]
pub async fn hosts_list(state: State<'_, AppState>) -> CmdResult<Vec<HostView>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let hosts = sorted(vault.list::<HostProfile>()?, |h| h.label.clone());
        Ok(hosts.iter().map(HostView::from).collect())
    })
    .await
}

fn build_host(vault: &Vault, input: HostInput) -> CmdResult<HostProfile> {
    let label = clean("label", &input.label, 128)?;
    let hostname = clean("hostname", &input.hostname, 253)?;
    if hostname.contains(char::is_whitespace) {
        return Err(AppError::invalid("hostname must not contain whitespace"));
    }
    let user = clean("user", &input.user, 64)?;
    if input.port == 0 {
        return Err(AppError::invalid("port must be between 1 and 65535"));
    }
    let existing = match &input.id {
        Some(raw) => Some(
            vault
                .get::<HostProfile>(parse_id(raw)?)?
                .ok_or(AppError::NotFound("host"))?,
        ),
        None => None,
    };
    let auth = match input.auth {
        AuthInput::Identity { identity } => {
            let kid = parse_id(&identity)?;
            vault
                .get::<IdentityKey>(kid)?
                .ok_or(AppError::NotFound("identity"))?;
            Auth::Identity(kid)
        }
        AuthInput::Agent { fingerprint } => Auth::Agent {
            fingerprint: fingerprint.filter(|f| !f.trim().is_empty()),
        },
        AuthInput::Password { password: Some(pw) } if !pw.is_empty() => {
            Auth::Password(secret_from(pw))
        }
        AuthInput::Password { .. } => match existing.as_ref().map(|h| &h.auth) {
            Some(Auth::Password(pw)) => Auth::Password(pw.clone()),
            _ => return Err(AppError::invalid("a password is required")),
        },
        AuthInput::Interactive => Auth::Interactive,
    };
    let mut host = match existing {
        Some(h) => h,
        None => HostProfile::new(&label, &hostname, &user, Auth::Interactive)?,
    };
    let jump = input
        .jump
        .iter()
        .map(|j| parse_id(j))
        .collect::<CmdResult<Vec<Id>>>()?;
    for jid in &jump {
        if *jid == host.id || plan::chain(vault, *jid)?.iter().any(|h| h.id == host.id) {
            return Err(AppError::Cycle(*jid));
        }
    }
    host.label = label;
    host.hostname = hostname;
    host.user = user;
    host.port = input.port;
    host.auth = auth;
    host.jump = jump;
    host.workspace = input
        .workspace
        .map(|w| w.trim().to_owned())
        .filter(|w| !w.is_empty());
    host.tags = input
        .tags
        .iter()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    host.keepalive_secs = input.keepalive_secs.unwrap_or(15).min(MAX_KEEPALIVE);
    host.agent_forward = input.agent_forward;
    host.legacy_crypto = input.legacy_crypto;
    host.ssh_alias = input
        .ssh_alias
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty());
    Ok(host)
}

#[tauri::command]
pub async fn host_save(state: State<'_, AppState>, input: HostInput) -> CmdResult<HostView> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let mut host = build_host(&vault, input)?;
        vault.put(&mut host)?;
        Ok(HostView::from(&host))
    })
    .await
}

#[tauri::command]
pub async fn host_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let core = state.0.clone();
    let id = parse_id(&id)?;
    let dead: Vec<Id> = {
        let vault = core.vault.clone();
        blocking(move || {
            let mut dead = Vec::new();
            for t in vault.list::<TunnelSpec>()? {
                if t.host == id {
                    dead.push(t.id);
                }
            }
            Ok(dead)
        })
        .await?
    };
    for t in &dead {
        super::tunnels::halt(&core, *t);
    }
    let vault = core.vault.clone();
    blocking(move || {
        for t in dead {
            vault.remove(t)?;
        }
        for mut h in vault.list::<HostProfile>()? {
            if h.jump.contains(&id) {
                h.jump.retain(|j| *j != id);
                vault.put(&mut h)?;
            }
        }
        vault.remove(id)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn canvas_move(state: State<'_, AppState>, id: String, x: f32, y: f32) -> CmdResult<()> {
    let id = parse_id(&id)?;
    if !x.is_finite() || !y.is_finite() {
        return Err(AppError::invalid("coordinates must be finite"));
    }
    let vault = state.0.vault.clone();
    blocking(move || {
        let at = Some(Point { x, y });
        if let Some(mut h) = vault.get::<HostProfile>(id)? {
            h.canvas = at;
            vault.put(&mut h)?;
        } else if let Some(mut t) = vault.get::<TunnelSpec>(id)? {
            t.canvas = at;
            vault.put(&mut t)?;
        } else {
            return Err(AppError::NotFound("node"));
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn identities_list(state: State<'_, AppState>) -> CmdResult<Vec<IdentityView>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let keys = sorted(vault.list::<IdentityKey>()?, |k| k.label.clone());
        Ok(keys.iter().map(IdentityView::from).collect())
    })
    .await
}

fn store_key(
    vault: &Vault,
    label: &str,
    pem: SecretString,
    passphrase: Option<SecretString>,
) -> CmdResult<IdentityView> {
    let label = clean("label", label, 128)?;
    let key = decode_key(&pem, passphrase.as_ref())?;
    let info = inspect(&key)?;
    if let Some(dup) = vault
        .list::<IdentityKey>()?
        .into_iter()
        .find(|k| k.fingerprint == info.fingerprint)
    {
        return Err(AppError::invalid(format!(
            "key already stored as \"{}\"",
            dup.label
        )));
    }
    let mut rec = IdentityKey {
        id: Id::random()?,
        label,
        algorithm: info.algorithm,
        public_key: info.public_key,
        fingerprint: info.fingerprint,
        material: KeyMaterial::Embedded {
            private_key: pem,
            passphrase,
        },
        updated_ms: 0,
    };
    vault.put(&mut rec)?;
    Ok(IdentityView::from(&rec))
}

#[tauri::command]
pub async fn identity_generate(
    state: State<'_, AppState>,
    label: String,
    kind: KeyKind,
) -> CmdResult<IdentityView> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let (pem, _) = citadel_ssh::generate(kind, &format!("{}@citadel", label.trim()))?;
        store_key(&vault, &label, pem, None)
    })
    .await
}

#[tauri::command]
pub async fn identity_import(
    state: State<'_, AppState>,
    label: String,
    pem: String,
    passphrase: Option<String>,
) -> CmdResult<IdentityView> {
    let vault = state.0.vault.clone();
    let pem = secret_from(pem);
    let pass = passphrase.filter(|p| !p.is_empty()).map(secret_from);
    blocking(move || store_key(&vault, &label, pem, pass)).await
}

#[tauri::command]
pub async fn identity_import_file(
    state: State<'_, AppState>,
    label: String,
    path: String,
    passphrase: Option<String>,
) -> CmdResult<IdentityView> {
    let vault = state.0.vault.clone();
    let pass = passphrase.filter(|p| !p.is_empty()).map(secret_from);
    blocking(move || {
        if std::fs::metadata(&path)?.len() > 64 * 1024 {
            return Err(AppError::invalid("file is too large to be a private key"));
        }
        let raw = std::fs::read_to_string(&path)?;
        store_key(&vault, &label, secret_from(raw), pass)
    })
    .await
}

#[derive(Serialize)]
pub struct AgentKey {
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub comment: String,
}

#[tauri::command]
pub async fn agent_keys(state: State<'_, AppState>) -> CmdResult<Vec<AgentKey>> {
    let ep = plan::agent_endpoint(state.0.prefs().agent.as_deref());
    let keys = list_agent_keys(&ep).await?;
    Ok(keys
        .iter()
        .map(|k| AgentKey {
            algorithm: k.algorithm().to_string(),
            public_key: k.to_openssh().unwrap_or_default(),
            fingerprint: citadel_ssh::fingerprint(k),
            comment: k.comment().to_string(),
        })
        .collect())
}

#[tauri::command]
pub async fn identity_agent(
    state: State<'_, AppState>,
    label: String,
    public_key: String,
) -> CmdResult<IdentityView> {
    let key = citadel_ssh::PublicKey::from_openssh(public_key.trim())
        .map_err(|e| AppError::invalid(e.to_string()))?;
    let vault = state.0.vault.clone();
    blocking(move || {
        let fingerprint = citadel_ssh::fingerprint(&key);
        if vault
            .list::<IdentityKey>()?
            .iter()
            .any(|k| k.fingerprint == fingerprint)
        {
            return Err(AppError::invalid("key already present"));
        }
        let mut rec = IdentityKey {
            id: Id::random()?,
            label: clean("label", &label, 128)?,
            algorithm: key.algorithm().to_string(),
            public_key: key
                .to_openssh()
                .map_err(|e| AppError::invalid(e.to_string()))?,
            fingerprint,
            material: KeyMaterial::Agent,
            updated_ms: 0,
        };
        vault.put(&mut rec)?;
        Ok(IdentityView::from(&rec))
    })
    .await
}

#[tauri::command]
pub async fn identity_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let id = parse_id(&id)?;
    let vault = state.0.vault.clone();
    blocking(move || {
        let users: Vec<String> = vault
            .list::<HostProfile>()?
            .into_iter()
            .filter(|h| matches!(h.auth, Auth::Identity(k) if k == id))
            .map(|h| h.label.clone())
            .collect();
        if !users.is_empty() {
            return Err(AppError::invalid(format!(
                "identity is used by: {}",
                users.join(", ")
            )));
        }
        vault.remove(id)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn snippets_list(state: State<'_, AppState>) -> CmdResult<Vec<SnippetView>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let all = sorted(vault.list::<Snippet>()?, |s| s.label.clone());
        Ok(all.iter().map(SnippetView::from).collect())
    })
    .await
}

#[tauri::command]
pub async fn snippet_save(
    state: State<'_, AppState>,
    id: Option<String>,
    label: String,
    body: String,
) -> CmdResult<SnippetView> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let label = clean("label", &label, 128)?;
        if body.is_empty() || body.len() > 64 * 1024 {
            return Err(AppError::invalid("snippet body must be 1 B to 64 KiB"));
        }
        let mut snip = match id {
            Some(raw) => vault
                .get::<Snippet>(parse_id(&raw)?)?
                .ok_or(AppError::NotFound("snippet"))?,
            None => Snippet {
                id: Id::random()?,
                label: String::new(),
                body: String::new(),
                updated_ms: 0,
            },
        };
        snip.label = label;
        snip.body = body;
        vault.put(&mut snip)?;
        Ok(SnippetView::from(&snip))
    })
    .await
}

#[tauri::command]
pub async fn snippet_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    remove_typed::<Snippet>(&state.0, &id).await
}

#[tauri::command]
pub async fn known_hosts_list(state: State<'_, AppState>) -> CmdResult<Vec<KnownHostView>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let all = sorted(vault.list::<KnownHost>()?, |k| {
            format!("{}:{}", k.host, k.port)
        });
        Ok(all.iter().map(KnownHostView::from).collect())
    })
    .await
}

#[tauri::command]
pub async fn known_host_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    remove_typed::<KnownHost>(&state.0, &id).await
}

async fn remove_typed<R: citadel_vault::Record + Send + 'static>(
    core: &Arc<Core>,
    raw: &str,
) -> CmdResult<()> {
    let id = parse_id(raw)?;
    let vault = core.vault.clone();
    blocking(move || {
        vault.get::<R>(id)?.ok_or(AppError::NotFound("record"))?;
        vault.remove(id)?;
        Ok(())
    })
    .await
}
