use std::time::Duration;

use citadel_telemetry::{LISTENERS_COMMAND, Listener, parse_listeners};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::{AppState, Lease};
use crate::core::parse_id;
use crate::error::{AppError, CmdResult};

const WAIT: Duration = Duration::from_secs(30);
const PROBE: Duration = Duration::from_secs(10);

#[tauri::command]
pub async fn ports_list(state: State<'_, AppState>, host: String) -> CmdResult<Vec<Listener>> {
    let core = state.0.clone();
    let host = parse_id(&host)?;
    let (_, sess) = core.session(host, WAIT).await?;
    let _lease = Lease::new(&core, host);
    let out = tokio::time::timeout(PROBE, sess.exec(LISTENERS_COMMAND))
        .await
        .map_err(|_| AppError::invalid("listing ports timed out"))??;
    Ok(parse_listeners(&String::from_utf8_lossy(&out.stdout)))
}

#[tauri::command]
pub async fn port_free(preferred: u16) -> CmdResult<u16> {
    if preferred >= 1024
        && let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", preferred))
    {
        drop(l);
        return Ok(preferred);
    }
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

#[tauri::command]
pub async fn open_local(app: AppHandle, port: u16) -> CmdResult<()> {
    if port == 0 {
        return Err(AppError::invalid("invalid port"));
    }
    app.opener()
        .open_url(format!("http://127.0.0.1:{port}/"), None::<&str>)
        .map_err(|e| AppError::invalid(e.to_string()))
}

#[tauri::command]
pub async fn open_link(app: AppHandle, which: String) -> CmdResult<()> {
    let url = match which.as_str() {
        "repo" => "https://github.com/sepehrmohseni/citadel",
        "issues" => "https://github.com/sepehrmohseni/citadel/issues",
        "releases" => "https://github.com/sepehrmohseni/citadel/releases",
        "site" => "https://sepehrmohseni.github.io/citadel/",
        "author" => "https://sepehrmohseni.com",
        _ => return Err(AppError::invalid("unknown link")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::invalid(e.to_string()))
}
