use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use citadel_ssh::{Ended, ExecEvent};
use citadel_vault::Id;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use super::AppState;
use crate::core::{Core, lock, parse_id};
use crate::error::{AppError, CmdResult};

const MAX_HOSTS: usize = 500;
const PARALLEL: usize = 16;
const MAX_COMMAND: usize = 16 * 1024;
const PER_HOST_OUTPUT: usize = 256 * 1024;
const CONNECT_WAIT: Duration = Duration::from_secs(30);
const STOP_GRACE: Duration = Duration::from_secs(2);

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

#[derive(Clone, Copy)]
enum Halt {
    Cancelled,
    TimedOut,
}

enum Outcome {
    Done(Option<u32>),
    Halted(Halt, Option<Ended>),
}

enum Step {
    Event(Option<ExecEvent>),
    Halt(Halt),
}

async fn halted(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn halt_message(why: Halt, limit: Duration, ended: Option<Ended>) -> String {
    let what = match why {
        Halt::Cancelled => "cancelled".to_owned(),
        Halt::TimedOut => format!("timed out after {} s", limit.as_secs()),
    };
    match ended {
        None => format!("{what} before the command started"),
        Some(Ended::Status(code)) => format!("{what}; the command stopped with exit status {code}"),
        Some(Ended::Signal(sig)) => format!("{what}; the command was stopped by SIG{sig}"),
        Some(Ended::Unknown) => {
            format!(
                "{what}; the server did not confirm that the command stopped, so it may still be running"
            )
        }
    }
}

async fn one(
    core: Arc<Core>,
    host: Id,
    command: Arc<str>,
    limit: Duration,
    feed: Channel<FleetEvent>,
    mut stop: watch::Receiver<bool>,
) {
    let label = host.to_string();
    let started = Instant::now();
    let _ = feed.send(FleetEvent::Started {
        host: label.clone(),
    });
    let deadline = tokio::time::Instant::now() + limit;
    let run = async {
        // `biased` polls the cancel branch first, so a run cancelled before this host's turn never
        // connects or starts the command, even when a session is already up.
        let (_lease, sess) = tokio::select! {
            biased;
            () = halted(&mut stop) => return Ok(Outcome::Halted(Halt::Cancelled, None)),
            () = tokio::time::sleep_until(deadline) => return Ok(Outcome::Halted(Halt::TimedOut, None)),
            r = core.session(host, CONNECT_WAIT) => r?,
        };
        let mut stream = sess.exec_stream(&command).await?;
        let mut sent = 0usize;
        let mut status = None;
        let mut cut = false;
        loop {
            let step = tokio::select! {
                biased;
                () = halted(&mut stop) => Step::Halt(Halt::Cancelled),
                () = tokio::time::sleep_until(deadline) => Step::Halt(Halt::TimedOut),
                ev = stream.next() => Step::Event(ev),
            };
            let ev = match step {
                Step::Halt(why) => {
                    return Ok(Outcome::Halted(why, Some(stream.stop(STOP_GRACE).await)));
                }
                Step::Event(None) => break,
                Step::Event(Some(ev)) => ev,
            };
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
        Ok::<_, AppError>(Outcome::Done(status))
    };
    let event = match run.await {
        Ok(Outcome::Done(status)) => FleetEvent::Done {
            host: label,
            status,
            ms: elapsed_ms(started),
        },
        Ok(Outcome::Halted(why, ended)) => FleetEvent::Failed {
            host: label,
            error: halt_message(why, limit, ended),
            ms: elapsed_ms(started),
        },
        Err(e) => FleetEvent::Failed {
            host: label,
            error: e.to_string(),
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
    let (halt, stop) = watch::channel(false);
    lock(&core.fleets).insert(id, halt);
    tauri::async_runtime::spawn(async move {
        let gate = Arc::new(Semaphore::new(PARALLEL));
        let mut set = JoinSet::new();
        for host in ids {
            let Ok(permit) = gate.clone().acquire_owned().await else {
                break;
            };
            let (core, command, feed, stop) =
                (core.clone(), command.clone(), feed.clone(), stop.clone());
            set.spawn(async move {
                let _permit = permit;
                one(core, host, command, limit, feed, stop).await;
            });
        }
        while set.join_next().await.is_some() {}
        lock(&reg.fleets).remove(&id);
        let _ = feed.send(FleetEvent::Finished);
    });
    Ok(id)
}

#[tauri::command]
pub async fn fleet_cancel(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    if let Some(halt) = lock(&state.0.fleets).get(&id) {
        let _ = halt.send(true);
    }
    Ok(())
}
