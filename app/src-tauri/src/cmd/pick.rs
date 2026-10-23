use std::path::PathBuf;

use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};
use tokio::sync::oneshot;

use super::AppState;
use crate::error::{AppError, CmdResult};
use crate::grants::{Access, Picked};

const MAX_FILTERS: usize = 8;

#[derive(Deserialize)]
pub struct Filter {
    pub name: String,
    pub extensions: Vec<String>,
}

fn builder(
    app: &AppHandle,
    title: &str,
    filters: &[Filter],
) -> CmdResult<FileDialogBuilder<tauri::Wry>> {
    let mut d = app
        .dialog()
        .file()
        .set_title(title.chars().take(120).collect::<String>());
    for f in filters.iter().take(MAX_FILTERS) {
        let exts: Vec<&str> = f
            .extensions
            .iter()
            .map(String::as_str)
            .filter(|e| {
                !e.is_empty() && e.len() <= 16 && e.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .collect();
        if exts.is_empty() {
            return Err(AppError::invalid("file filter without extensions"));
        }
        d = d.add_filter(f.name.chars().take(60).collect::<String>(), &exts);
    }
    Ok(d)
}

fn local(p: FilePath) -> Option<PathBuf> {
    p.into_path().ok()
}

#[tauri::command]
pub async fn pick_open(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    filters: Vec<Filter>,
    multiple: bool,
) -> CmdResult<Vec<Picked>> {
    let d = builder(&app, &title, &filters)?;
    let (tx, rx) = oneshot::channel::<Vec<PathBuf>>();
    if multiple {
        d.pick_files(move |files| {
            let _ = tx.send(
                files
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(local)
                    .collect(),
            );
        });
    } else {
        d.pick_file(move |file| {
            let _ = tx.send(file.and_then(local).into_iter().collect());
        });
    }
    let paths = rx.await.unwrap_or_default();
    paths
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| state.0.grants.issue(p, Access::Read))
        .collect()
}

#[tauri::command]
pub async fn pick_save(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    default_name: String,
    filters: Vec<Filter>,
) -> CmdResult<Option<Picked>> {
    let name: String = default_name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
        .take(200)
        .collect();
    let d = builder(&app, &title, &filters)?.set_file_name(name);
    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    d.save_file(move |file| {
        let _ = tx.send(file.and_then(local));
    });
    match rx.await.ok().flatten() {
        Some(p) if p.is_dir() => Err(AppError::invalid("choose a file, not a folder")),
        Some(p) => state.0.grants.issue(p, Access::Write).map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn grant_dropped(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<Vec<Picked>> {
    if paths.len() > 64 {
        return Err(AppError::invalid("drop at most 64 files at once"));
    }
    state.0.grants.claim_dropped(&paths)
}
