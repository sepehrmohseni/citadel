use citadel_sync::SyncReport;
use citadel_vault::{Id, SyncBackend, SyncTarget, secret_from};
use tauri::State;

use super::{AppState, blocking};
use crate::core::parse_id;
use crate::dto::{SyncBackendInput, SyncTargetInput, SyncTargetView};
use crate::error::{AppError, CmdResult};

#[tauri::command]
pub async fn sync_targets_list(state: State<'_, AppState>) -> CmdResult<Vec<SyncTargetView>> {
    let vault = state.0.vault.clone();
    blocking(move || {
        Ok(vault
            .list::<SyncTarget>()?
            .iter()
            .map(SyncTargetView::from)
            .collect())
    })
    .await
}

fn required(field: &str, v: &str) -> CmdResult<String> {
    let v = v.trim().to_owned();
    if v.is_empty() {
        return Err(AppError::invalid(format!("{field} is required")));
    }
    Ok(v)
}

#[tauri::command]
pub async fn sync_target_save(
    state: State<'_, AppState>,
    input: SyncTargetInput,
) -> CmdResult<SyncTargetView> {
    let vault = state.0.vault.clone();
    blocking(move || {
        let existing = match &input.id {
            Some(raw) => Some(
                vault
                    .get::<SyncTarget>(parse_id(raw)?)?
                    .ok_or(AppError::NotFound("sync target"))?,
            ),
            None => None,
        };
        let backend = match input.backend {
            SyncBackendInput::S3 {
                endpoint,
                region,
                bucket,
                object,
                access_key,
                secret_key,
                path_style,
            } => {
                let secret = match (
                    secret_key.filter(|s| !s.is_empty()),
                    existing.as_ref().map(|e| &e.backend),
                ) {
                    (Some(s), _) => secret_from(s),
                    (None, Some(SyncBackend::S3 { secret_key, .. })) => secret_key.clone(),
                    (None, _) => return Err(AppError::invalid("secret key is required")),
                };
                SyncBackend::S3 {
                    endpoint: required("endpoint", &endpoint)?,
                    region: required("region", &region)?,
                    bucket: required("bucket", &bucket)?,
                    object: required("object key", &object)?,
                    access_key: required("access key", &access_key)?,
                    secret_key: secret,
                    path_style,
                }
            }
            SyncBackendInput::WebDav {
                url,
                user,
                password,
            } => {
                let pw = match (
                    password.filter(|s| !s.is_empty()),
                    existing.as_ref().map(|e| &e.backend),
                ) {
                    (Some(s), _) => secret_from(s),
                    (None, Some(SyncBackend::WebDav { password, .. })) => password.clone(),
                    (None, _) => return Err(AppError::invalid("password is required")),
                };
                SyncBackend::WebDav {
                    url: required("url", &url)?,
                    user: required("user", &user)?,
                    password: pw,
                }
            }
            SyncBackendInput::Git {
                remote,
                branch,
                path,
            } => SyncBackend::Git {
                remote: required("remote", &remote)?,
                branch: required("branch", &branch)?,
                path: required("path", &path)?,
            },
        };
        let mut target = match existing {
            Some(mut t) => {
                t.backend = backend;
                t
            }
            None => SyncTarget {
                id: Id::random()?,
                label: String::new(),
                backend,
                updated_ms: 0,
            },
        };
        target.label = required("label", &input.label)?;
        vault.put(&mut target)?;
        Ok(SyncTargetView::from(&target))
    })
    .await
}

#[tauri::command]
pub async fn sync_target_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let id = parse_id(&id)?;
    let vault = state.0.vault.clone();
    blocking(move || {
        vault
            .get::<SyncTarget>(id)?
            .ok_or(AppError::NotFound("sync target"))?;
        vault.remove(id)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn sync_run(
    state: State<'_, AppState>,
    id: String,
    password: Option<String>,
) -> CmdResult<SyncReport> {
    let core = state.0.clone();
    let id = parse_id(&id)?;
    let vault = core.vault.clone();
    let target = blocking(move || {
        vault
            .get::<SyncTarget>(id)?
            .ok_or(AppError::NotFound("sync target"))
    })
    .await?;
    let cache = core.dirs.cache.join("sync");
    std::fs::create_dir_all(&cache)?;
    let remote = citadel_sync::remote_for(&target.backend, &cache)?;
    let pw = password.filter(|p| !p.is_empty()).map(secret_from);
    Ok(citadel_sync::sync(core.vault.clone(), remote.as_ref(), pw.as_ref()).await?)
}
