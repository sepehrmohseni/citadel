use std::sync::Arc;
use std::time::{Duration, Instant};

use citadel_ssh::{DirEntry, Progress, Sftp};
use tauri::State;
use tauri::ipc::Channel;

use super::{AppState, Lease};
use crate::core::{Core, parse_id};
use crate::error::{AppError, CmdResult};
use crate::grants::Access;

const WAIT: Duration = Duration::from_secs(30);
const EDIT_CAP: u64 = 8 << 20;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

async fn open(core: &Arc<Core>, host: &str) -> CmdResult<(Lease, Sftp)> {
    let host = parse_id(host)?;
    let (_, sess) = core.session(host, WAIT).await?;
    let lease = Lease::new(core, host);
    let sftp = sess.sftp().await?;
    Ok((lease, sftp))
}

fn throttled(ch: Channel<Progress>) -> impl Fn(Progress) + Send + Sync {
    let last = std::sync::Mutex::new(Instant::now() - PROGRESS_EVERY);
    move |p: Progress| {
        let mut at = crate::core::lock(&last);
        if p.done == p.total || at.elapsed() >= PROGRESS_EVERY {
            *at = Instant::now();
            let _ = ch.send(p);
        }
    }
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>, host: String) -> CmdResult<String> {
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.realpath(".").await?)
}

#[tauri::command]
pub async fn sftp_realpath(
    state: State<'_, AppState>,
    host: String,
    path: String,
) -> CmdResult<String> {
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.realpath(&path).await?)
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    host: String,
    path: String,
) -> CmdResult<Vec<DirEntry>> {
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.list(&path).await?)
}

#[tauri::command]
pub async fn sftp_mkdir(state: State<'_, AppState>, host: String, path: String) -> CmdResult<()> {
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.mkdir(&path).await?)
}

#[tauri::command]
pub async fn sftp_remove(
    state: State<'_, AppState>,
    host: String,
    path: String,
    dir: bool,
) -> CmdResult<()> {
    let (_l, sftp) = open(&state.0, &host).await?;
    if dir {
        Ok(sftp.rmdir(&path).await?)
    } else {
        Ok(sftp.remove(&path).await?)
    }
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    host: String,
    from: String,
    to: String,
) -> CmdResult<()> {
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.rename(&from, &to).await?)
}

#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    host: String,
    remote: String,
    local: String,
    progress: Channel<Progress>,
) -> CmdResult<u64> {
    let local = state.0.grants.redeem(&local, Access::Write)?;
    if local.is_dir() {
        return Err(AppError::invalid("destination is a directory"));
    }
    let (_l, sftp) = open(&state.0, &host).await?;
    let report = throttled(progress);
    Ok(sftp.download(&remote, &local, &report).await?)
}

#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, AppState>,
    host: String,
    local: String,
    remote: String,
    progress: Channel<Progress>,
) -> CmdResult<u64> {
    let local = state.0.grants.redeem(&local, Access::Read)?;
    let (_l, sftp) = open(&state.0, &host).await?;
    let report = throttled(progress);
    Ok(sftp.upload(&local, &remote, &report).await?)
}

#[tauri::command]
pub async fn sftp_read(
    state: State<'_, AppState>,
    host: String,
    path: String,
) -> CmdResult<String> {
    let (_l, sftp) = open(&state.0, &host).await?;
    let raw = sftp.read_file(&path, EDIT_CAP).await?;
    String::from_utf8(raw).map_err(|_| AppError::invalid("file is not UTF-8 text"))
}

#[tauri::command]
pub async fn sftp_write(
    state: State<'_, AppState>,
    host: String,
    path: String,
    content: String,
) -> CmdResult<()> {
    if content.len() as u64 > EDIT_CAP {
        return Err(AppError::invalid("file exceeds the 8 MiB editor limit"));
    }
    let (_l, sftp) = open(&state.0, &host).await?;
    Ok(sftp.write_atomic(&path, content.as_bytes()).await?)
}
