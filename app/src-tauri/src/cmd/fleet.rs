use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use citadel_ssh::ExecEvent;
use citadel_vault::Id;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::{AppState, Lease};
use crate::core::{Core, lock, parse_id};
use crate::error::{AppError, CmdResult};

const MAX_HOSTS: usize = 500;
const PARALLEL: usize = 16;
const MAX_COMMAND: usize = 16 * 1024;
const PER_HOST_OUTPUT: usize = 256 * 1024;
const CONNECT_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetEvent {
    Started {
        host: String,
    },
    Output {
        host: String,
        stderr: bool,
        text: String,
    },
    Truncated {
        host: String,
    },
    Done {
        host: String,
        status: Option<u32>,
        ms: u64,
    },
    Failed {
        host: String,
        error: String,
        ms: u64,
    },
    Finished,
}

fn elapsed_ms(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn one(
    core: Arc<Core>,
    host: Id,
    command: Arc<str>,
    limit: Duration,
    feed: Channel<FleetEvent>,
) {
    let label = host.to_string();
    let started = Instant::now();
    let _ = feed.send(FleetEvent::Started {
        host: label.clone(),
    });
    let run = async {
        let (_, sess) = core.session(host, CONNECT_WAIT).await?;
        let _lease = Lease::new(&core, host);
        let mut stream = sess.exec_stream(&command).await?;
        let mut sent = 0usize;
        let mut status = None;
        let mut cut = false;
        while let Some(ev) = stream.next().await {
            let (bytes, stderr) = match ev {
                ExecEvent::Stdout(b) => (b, false),
                ExecEvent::Stderr(b) => (b, true),
                ExecEvent::Exit(code) => {
                    status = Some(code);
                    continue;
                }
            };
            if sent >= PER_HOST_OUTPUT {
                if !cut {
                    cut = true;
                    let _ = feed.send(FleetEvent::Truncated {
                        host: label.clone(),
                    });
                }
                continue;
            }
            let take = bytes.len().min(PER_HOST_OUTPUT - sent);
            sent += take;
            let text = String::from_utf8_lossy(&bytes[..take]).into_owned();
            let _ = feed.send(FleetEvent::Output {
                host: label.clone(),
                stderr,
                text,
            });
        }
        Ok::<_, AppError>(status)
    };
    let event = match tokio::time::timeout(limit, run).await {
        Ok(Ok(status)) => FleetEvent::Done {
            host: label,
            status,
            ms: elapsed_ms(started),
        },
        Ok(Err(e)) => FleetEvent::Failed {
            host: label,
            error: e.to_string(),
            ms: elapsed_ms(started),
        },
        Err(_) => FleetEvent::Failed {
            host: label,
            error: format!("timed out after {} s", limit.as_secs()),
            ms: elapsed_ms(started),
        },
    };
    let _ = feed.send(event);
}

#[tauri::command]
pub async fn fleet_run(
    state: State<'_, AppState>,
    hosts: Vec<String>,
    command: String,
    timeout_secs: u32,
    feed: Channel<FleetEvent>,
) -> CmdResult<u32> {
    let command = command.trim().to_owned();
    if command.is_empty() || command.len() > MAX_COMMAND {
        return Err(AppError::invalid("command must be 1 to 16384 bytes"));
    }
    if hosts.is_empty() || hosts.len() > MAX_HOSTS {
        return Err(AppError::invalid("choose between 1 and 500 hosts"));
    }
    let ids: BTreeSet<Id> = hosts
        .iter()
        .map(|h| parse_id(h))
        .collect::<CmdResult<_>>()?;
    let core = state.0.clone();
    let id = core.next_id();
    let limit = Duration::from_secs(u64::from(timeout_secs.clamp(1, 3600)));
    let command: Arc<str> = Arc::from(command);
    let reg = core.clone();
    let task = tauri::async_runtime::spawn(async move {
        let gate = Arc::new(Semaphore::new(PARALLEL));
        let mut set = JoinSet::new();
        for host in ids {
            let Ok(permit) = gate.clone().acquire_owned().await else {
                break;
            };
            let (core, command, feed) = (core.clone(), command.clone(), feed.clone());
            set.spawn(async move {
                let _permit = permit;
                one(core, host, command, limit, feed).await;
            });
        }
        while set.join_next().await.is_some() {}
        let _ = feed.send(FleetEvent::Finished);
        lock(&reg.monitors).remove(&id);
    });
    lock(&state.0.monitors).insert(id, task.inner().abort_handle());
    Ok(id)
}

#[tauri::command]
pub async fn fleet_cancel(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    if let Some(task) = lock(&state.0.monitors).remove(&id) {
        task.abort();
    }
    Ok(())
}
