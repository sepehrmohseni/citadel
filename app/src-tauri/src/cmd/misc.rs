use tauri::State;

use super::AppState;
use crate::core::{Answer, lock, parse_id};
use crate::dto::LinkView;
use crate::error::CmdResult;
use crate::prefs::Prefs;

#[tauri::command]
pub async fn prefs_get(state: State<'_, AppState>) -> CmdResult<Prefs> {
    Ok(state.0.prefs())
}

#[tauri::command]
pub async fn prefs_set(state: State<'_, AppState>, prefs: Prefs) -> CmdResult<Prefs> {
    let core = &state.0;
    let prefs = prefs.sanitized();
    prefs.save(&core.dirs.config)?;
    core.autolock.set_idle(prefs.autolock());
    *lock(&core.prefs) = prefs.clone();
    Ok(prefs)
}

#[tauri::command]
pub async fn prompt_answer(state: State<'_, AppState>, id: u64, answer: Answer) -> CmdResult<bool> {
    Ok(state.0.prompts.answer(id, answer))
}

#[tauri::command]
pub async fn links_list(state: State<'_, AppState>) -> CmdResult<Vec<LinkView>> {
    Ok(state
        .0
        .link_states()
        .into_iter()
        .map(|(id, s)| LinkView {
            host: id.to_string(),
            state: s,
        })
        .collect())
}

#[tauri::command]
pub async fn link_reconnect(state: State<'_, AppState>, host: String) -> CmdResult<()> {
    let id = parse_id(&host)?;
    if let Some(hub) = lock(&state.0.links).get(&id) {
        hub.link.reconnect_now();
    }
    Ok(())
}
