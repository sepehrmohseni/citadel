use tauri::State;

use super::{AppState, blocking};
use crate::error::{AppError, CmdResult};

const MAX_SESSION: usize = 256 * 1024;
const FILE: &str = "session.json";

#[tauri::command]
pub async fn session_load(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let path = state.0.dirs.config.join(FILE);
    blocking(move || {
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        if meta.len() as usize > MAX_SESSION {
            return Ok(None);
        }
        let body = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .map(|_| body))
    })
    .await
}

#[tauri::command]
pub async fn session_save(state: State<'_, AppState>, body: String) -> CmdResult<()> {
    if body.len() > MAX_SESSION {
        return Err(AppError::invalid("session layout is too large"));
    }
    if serde_json::from_str::<serde_json::Value>(&body).is_err() {
        return Err(AppError::invalid("session layout is not JSON"));
    }
    let dir = state.0.dirs.config.clone();
    blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("session.json.part");
        crate::safety::write_private(&tmp, body.as_bytes())?;
        std::fs::rename(tmp, dir.join(FILE))?;
        Ok(())
    })
    .await
}
