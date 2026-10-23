use std::time::Duration;

use citadel_telemetry::Snapshot;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use super::{AppState, Lease};
use crate::core::{lock, parse_id};
use crate::error::CmdResult;

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryMsg {
    Sample(Box<Snapshot>),
    Error { message: String },
    Ended,
}

#[tauri::command]
pub async fn telemetry_start(
    state: State<'_, AppState>,
    host: String,
    feed: Channel<TelemetryMsg>,
) -> CmdResult<u32> {
    let core = state.0.clone();
    let host = parse_id(&host)?;
    let (_, sess) = core.session(host, Duration::from_secs(45)).await?;
    let lease = Lease::new(&core, host);
    let every = Duration::from_millis(u64::from(core.prefs().telemetry_ms));
    let mut mon = citadel_telemetry::monitor(sess, every);
    let id = core.next_id();
    let reg = core.clone();
    let task = tauri::async_runtime::spawn(async move {
        let _lease = lease;
        while let Some(item) = mon.next().await {
            let msg = match item {
                Ok(s) => TelemetryMsg::Sample(Box::new(s)),
                Err(e) => TelemetryMsg::Error {
                    message: e.to_string(),
                },
            };
            if feed.send(msg).is_err() {
                mon.stop();
                break;
            }
        }
        let _ = feed.send(TelemetryMsg::Ended);
        lock(&reg.monitors).remove(&id);
    });
    lock(&core.monitors).insert(id, task.inner().abort_handle());
    Ok(id)
}

#[tauri::command]
pub async fn telemetry_stop(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    if let Some(task) = lock(&state.0.monitors).remove(&id) {
        task.abort();
    }
    Ok(())
}
