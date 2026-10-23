use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use citadel_ssh::{PtyEvent, PtyRequest};
use citadel_vault::Id;
use tauri::State;
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request};

use super::AppState;
use crate::core::{Core, Flow, PtyEntry, lock, parse_id};
use crate::dto::PtyStatus;
use crate::error::{AppError, CmdResult};

const FRAME: usize = 64 * 1024;
const HIGH_WATER: u64 = 4 << 20;
const CONNECT_WAIT: Duration = Duration::from_secs(45);
const MAX_DIM: u32 = 2000;

struct Cleanup {
    core: Arc<Core>,
    pid: u32,
    host: Id,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        lock(&self.core.ptys).remove(&self.pid);
        self.core.release(self.host);
    }
}

fn utf8(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    v.contains("utf-8") || v.contains("utf8")
}

pub(crate) fn remote_locale() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .filter(|v| utf8(v))
        .unwrap_or_else(|| "C.UTF-8".to_owned())
}

async fn drained(flow: &Flow, sent: u64) {
    loop {
        let wait = flow.wake.notified();
        if sent.saturating_sub(flow.acked.load(Ordering::Acquire)) <= HIGH_WATER {
            return;
        }
        wait.await;
    }
}

fn dims(cols: u32, rows: u32) -> CmdResult<(u32, u32)> {
    if cols == 0 || rows == 0 || cols > MAX_DIM || rows > MAX_DIM {
        return Err(AppError::invalid("terminal size out of range"));
    }
    Ok((cols, rows))
}

#[tauri::command]
pub async fn pty_open(
    state: State<'_, AppState>,
    host: String,
    cols: u32,
    rows: u32,
    data: Channel<InvokeResponseBody>,
    status: Channel<PtyStatus>,
) -> CmdResult<u32> {
    let core = state.0.clone();
    let host = parse_id(&host)?;
    let (cols, rows) = dims(cols, rows)?;
    let (_, sess) = core.session(host, CONNECT_WAIT).await?;
    let req = PtyRequest {
        cols,
        rows,
        env: vec![
            ("COLORTERM".into(), "truecolor".into()),
            ("LANG".into(), remote_locale()),
        ],
        ..PtyRequest::default()
    };
    let pty = match sess.open_pty(&req).await {
        Ok(p) => p,
        Err(e) => {
            core.release(host);
            return Err(e.into());
        }
    };
    let pid = core.next_id();
    let writer = pty.writer.clone();
    let flow = Arc::new(Flow::default());
    let gate = flow.clone();
    let mut events = pty.events;
    let guard = Cleanup {
        core: core.clone(),
        pid,
        host,
    };
    let task = tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let mut sent: u64 = 0;
        let mut frame: Vec<u8> = Vec::with_capacity(FRAME);
        while let Some(ev) = events.recv().await {
            let mut pending = Some(ev);
            while let Some(ev) = pending.take() {
                match ev {
                    PtyEvent::Data(d) => {
                        frame.extend_from_slice(&d);
                        if frame.len() < FRAME {
                            pending = events.try_recv().ok();
                        }
                    }
                    PtyEvent::Exit(code) => {
                        let _ = status.send(PtyStatus::Exit { code });
                    }
                    PtyEvent::Signal(name) => {
                        let _ = status.send(PtyStatus::Signal { name });
                    }
                    PtyEvent::Closed => {
                        if !frame.is_empty() {
                            let _ = data.send(InvokeResponseBody::Raw(std::mem::take(&mut frame)));
                        }
                        let _ = status.send(PtyStatus::Closed);
                        return;
                    }
                }
            }
            if !frame.is_empty() {
                drained(&gate, sent).await;
                let out = std::mem::replace(&mut frame, Vec::with_capacity(FRAME));
                sent += out.len() as u64;
                if data.send(InvokeResponseBody::Raw(out)).is_err() {
                    return;
                }
            }
        }
        let _ = status.send(PtyStatus::Closed);
    });
    lock(&core.ptys).insert(
        pid,
        PtyEntry {
            writer,
            task: task.inner().abort_handle(),
            flow,
        },
    );
    Ok(pid)
}

fn writer(core: &Core, pid: u32) -> CmdResult<citadel_ssh::PtyWriter> {
    lock(&core.ptys)
        .get(&pid)
        .map(|e| e.writer.clone())
        .ok_or(AppError::NotFound("terminal"))
}

#[tauri::command]
pub async fn pty_write(state: State<'_, AppState>, request: Request<'_>) -> CmdResult<()> {
    let pid = request
        .headers()
        .get("x-pty")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| AppError::invalid("missing x-pty header"))?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::invalid("expected a binary body"));
    };
    if bytes.is_empty() {
        return Ok(());
    }
    state.0.vault.touch();
    let w = writer(&state.0, pid)?;
    w.write(bytes.clone()).await?;
    Ok(())
}

#[tauri::command]
pub async fn pty_ack(state: State<'_, AppState>, pid: u32, bytes: u64) -> CmdResult<()> {
    let flow = lock(&state.0.ptys)
        .get(&pid)
        .map(|e| e.flow.clone())
        .ok_or(AppError::NotFound("terminal"))?;
    flow.acked.fetch_max(bytes, Ordering::AcqRel);
    flow.wake.notify_one();
    Ok(())
}

#[tauri::command]
pub async fn pty_resize(
    state: State<'_, AppState>,
    pid: u32,
    cols: u32,
    rows: u32,
) -> CmdResult<()> {
    let (cols, rows) = dims(cols, rows)?;
    writer(&state.0, pid)?.resize(cols, rows).await?;
    Ok(())
}

#[tauri::command]
pub async fn pty_close(state: State<'_, AppState>, pid: u32) -> CmdResult<()> {
    let core = state.0.clone();
    let Ok(w) = writer(&core, pid) else {
        return Ok(());
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), w.close()).await;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let stale = lock(&core.ptys).get(&pid).map(|e| e.task.clone());
        if let Some(task) = stale {
            task.abort();
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_detection_only_accepts_utf8() {
        assert!(utf8("en_US.UTF-8"));
        assert!(utf8("C.utf8"));
        assert!(!utf8("POSIX"));
        assert!(!utf8("en_US.ISO-8859-1"));
    }

    #[tokio::test]
    async fn output_waits_for_acks_past_the_high_water_mark() {
        let flow = Arc::new(Flow::default());
        drained(&flow, HIGH_WATER).await;
        let gate = flow.clone();
        let waiter = tokio::spawn(async move { drained(&gate, HIGH_WATER * 2).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished());
        flow.acked.store(HIGH_WATER, Ordering::Release);
        flow.wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }
}
