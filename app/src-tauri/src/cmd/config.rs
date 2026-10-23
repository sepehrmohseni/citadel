use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use citadel_config::agent::{AgentSocket, discover_with};
use citadel_config::{SshConfig, SshHost};
use citadel_ssh::{decode_key, inspect};
use citadel_vault::{Auth, HostProfile, Id, IdentityKey, KeyMaterial, SecretString, Vault};
use serde::Serialize;
use tauri::State;

use super::{AppState, blocking};
use crate::core::parse_id;
use crate::error::{AppError, CmdResult};

#[derive(Serialize)]
pub struct ConfigEntry {
    pub host: SshHost,
    pub imported: Option<String>,
}

#[derive(Serialize, Default)]
pub struct ImportSummary {
    pub hosts: usize,
    pub keys: usize,
    pub skipped: Vec<String>,
}

fn config_path() -> CmdResult<PathBuf> {
    SshConfig::default_path().ok_or(AppError::NotFound("home directory"))
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest)),
        None => PathBuf::from(path),
    }
}

fn local_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

fn aliased(vault: &Vault) -> CmdResult<HashMap<String, Id>> {
    Ok(vault
        .list::<HostProfile>()?
        .iter()
        .filter_map(|h| h.ssh_alias.clone().map(|a| (a, h.id)))
        .collect())
}

#[tauri::command]
pub async fn sshconfig_hosts(state: State<'_, AppState>) -> CmdResult<Vec<ConfigEntry>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let cfg = SshConfig::load(&config_path()?)?;
        let known = aliased(&vault)?;
        Ok(cfg
            .hosts()
            .into_iter()
            .map(|h| ConfigEntry {
                imported: known.get(&h.alias).map(ToString::to_string),
                host: h,
            })
            .collect())
    })
    .await
}

fn identity_for(vault: &Vault, files: &[String], summary: &mut ImportSummary) -> CmdResult<Auth> {
    let known: Vec<IdentityKey> = vault.list::<IdentityKey>()?;
    for f in files {
        let path = expand(f);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let pem = SecretString::from(raw.as_str());
        drop(zeroize::Zeroizing::new(raw));
        match decode_key(&pem, None) {
            Ok(key) => {
                let info = inspect(&key)?;
                if let Some(k) = known.iter().find(|k| k.fingerprint == info.fingerprint) {
                    return Ok(Auth::Identity(k.id));
                }
                let label = path.file_name().map_or_else(
                    || "imported key".to_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let mut rec = IdentityKey {
                    id: Id::random()?,
                    label,
                    algorithm: info.algorithm,
                    public_key: info.public_key,
                    fingerprint: info.fingerprint,
                    material: KeyMaterial::Embedded {
                        private_key: pem,
                        passphrase: None,
                    },
                    updated_ms: 0,
                };
                vault.put(&mut rec)?;
                summary.keys += 1;
                return Ok(Auth::Identity(rec.id));
            }
            Err(_) => {
                let pub_path = PathBuf::from(format!("{}.pub", path.display()));
                if let Ok(line) = std::fs::read_to_string(pub_path)
                    && let Ok(pk) = citadel_ssh::PublicKey::from_openssh(line.trim())
                {
                    return Ok(Auth::Agent {
                        fingerprint: Some(citadel_ssh::fingerprint(&pk)),
                    });
                }
            }
        }
    }
    Ok(Auth::Agent { fingerprint: None })
}

fn adhoc(spec: &str, made: &mut BTreeMap<String, Id>, vault: &Vault) -> CmdResult<Id> {
    if let Some(id) = made.get(spec) {
        return Ok(*id);
    }
    let (user, rest) = match spec.split_once('@') {
        Some((u, r)) => (u.to_owned(), r),
        None => (local_user(), spec),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => {
            (h.trim_matches(['[', ']']), p.parse().unwrap_or(22))
        }
        _ => (rest, 22),
    };
    let mut h = HostProfile::new(spec, host, &user, Auth::Agent { fingerprint: None })?;
    h.port = port;
    vault.put(&mut h)?;
    made.insert(spec.to_owned(), h.id);
    Ok(h.id)
}

fn import_alias(
    vault: &Vault,
    cfg: &SshConfig,
    alias: &str,
    made: &mut BTreeMap<String, Id>,
    depth: usize,
    summary: &mut ImportSummary,
) -> CmdResult<Id> {
    if let Some(id) = made.get(alias) {
        return Ok(*id);
    }
    if depth > 8 {
        return Err(AppError::invalid(format!(
            "ProxyJump chain for {alias} is too deep"
        )));
    }
    let entry = cfg.host(alias);
    let declared = cfg.aliases().iter().any(|a| a == alias);
    if !declared {
        return adhoc(alias, made, vault);
    }
    let mut jump = Vec::new();
    for j in &entry.proxy_jump {
        jump.push(import_alias(vault, cfg, j, made, depth + 1, summary)?);
    }
    let auth = identity_for(vault, &entry.identity_files, summary)?;
    let hostname = entry.hostname.clone().unwrap_or_else(|| alias.to_owned());
    let user = entry.user.clone().unwrap_or_else(local_user);
    let mut h = HostProfile::new(alias, &hostname, &user, auth)?;
    h.port = entry.port.unwrap_or(22);
    h.jump = jump;
    h.agent_forward = entry.forward_agent.unwrap_or(false);
    h.ssh_alias = Some(alias.to_owned());
    vault.put(&mut h)?;
    summary.hosts += 1;
    made.insert(alias.to_owned(), h.id);
    Ok(h.id)
}

#[tauri::command]
pub async fn sshconfig_import(
    state: State<'_, AppState>,
    aliases: Vec<String>,
) -> CmdResult<ImportSummary> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let cfg = SshConfig::load(&config_path()?)?;
        let mut made: BTreeMap<String, Id> = aliased(&vault)?.into_iter().collect();
        let mut summary = ImportSummary::default();
        for alias in aliases {
            if made.contains_key(&alias) {
                summary.skipped.push(alias);
                continue;
            }
            if let Err(e) = import_alias(&vault, &cfg, &alias, &mut made, 0, &mut summary) {
                summary.skipped.push(format!("{alias}: {e}"));
            }
        }
        Ok(summary)
    })
    .await
}

fn slug(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "host".into() } else { s }
}

fn export_one(vault: &Vault, cfg: &mut SshConfig, id: Id, depth: usize) -> CmdResult<String> {
    if depth > 8 {
        return Err(AppError::Cycle(id));
    }
    let mut h = vault
        .get::<HostProfile>(id)?
        .ok_or(AppError::NotFound("host"))?;
    let alias = h.ssh_alias.clone().unwrap_or_else(|| slug(&h.label));
    let unsafe_field =
        |v: &str| v.is_empty() || v.chars().any(|c| c.is_control() || c.is_whitespace());
    if unsafe_field(&alias) || unsafe_field(&h.hostname) || unsafe_field(&h.user) {
        return Err(AppError::invalid(format!(
            "{} has fields that cannot be written safely to ssh_config",
            h.label
        )));
    }
    let mut jumps = Vec::new();
    for j in h.jump.clone() {
        jumps.push(export_one(vault, cfg, j, depth + 1)?);
    }
    let mut entry = cfg.host(&alias);
    entry.alias.clone_from(&alias);
    entry.hostname = Some(h.hostname.clone());
    entry.user = Some(h.user.clone());
    entry.port = (h.port != 22).then_some(h.port);
    entry.proxy_jump = jumps;
    entry.forward_agent = h.agent_forward.then_some(true);
    cfg.upsert(&entry);
    if h.ssh_alias.as_deref() != Some(alias.as_str()) {
        h.ssh_alias = Some(alias.clone());
        vault.put(&mut h)?;
    }
    Ok(alias)
}

#[tauri::command]
pub async fn sshconfig_export(state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<String> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let path = config_path()?;
        let mut cfg = SshConfig::load(&path)?;
        for raw in ids {
            export_one(&vault, &mut cfg, parse_id(&raw)?, 0)?;
        }
        cfg.save(&path)?;
        Ok(path.display().to_string())
    })
    .await
}

#[tauri::command]
pub async fn agents_list() -> CmdResult<Vec<AgentSocket>> {
    blocking(|| {
        let cfg = SshConfig::default_path().and_then(|p| SshConfig::load(&p).ok());
        Ok(discover_with(cfg.as_ref()))
    })
    .await
}
