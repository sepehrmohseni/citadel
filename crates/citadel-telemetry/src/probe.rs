use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use citadel_ssh::{ExecEvent, ExecStream, Session};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

use crate::decode::Decoder;
use crate::model::Snapshot;
use crate::sample::Sampler;
use crate::script::{self, Mode};
use crate::{Result, TelemetryError};

const SNAPSHOT_DEADLINE: Duration = Duration::from_secs(20);
const STDERR_TAIL: usize = 4096;
const QUEUE: usize = 8;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[derive(Default)]
struct Feed {
    dec: Decoder,
    sampler: Sampler,
    stderr: Vec<u8>,
    status: Option<u32>,
}

impl Feed {
    fn absorb(&mut self, ev: ExecEvent) -> Vec<Result<Snapshot>> {
        match ev {
            ExecEvent::Stdout(d) => {
                let frames = self.dec.push(&d);
                frames
                    .iter()
                    .filter_map(|f| self.sampler.feed(f, now_ms()))
                    .collect()
            }
            ExecEvent::Stderr(d) => {
                self.stderr.extend_from_slice(&d);
                if self.stderr.len() > STDERR_TAIL {
                    let cut = self.stderr.len() - STDERR_TAIL;
                    self.stderr.drain(..cut);
                }
                Vec::new()
            }
            ExecEvent::Exit(code) => {
                self.status = Some(code);
                Vec::new()
            }
        }
    }

    fn ended(&self) -> TelemetryError {
        TelemetryError::Ended {
            status: self.status,
            stderr: String::from_utf8_lossy(&self.stderr).trim().to_owned(),
        }
    }
}

async fn first(stream: &mut ExecStream, feed: &mut Feed) -> Option<Result<Snapshot>> {
    while let Some(ev) = stream.next().await {
        if let Some(r) = feed.absorb(ev).into_iter().next() {
            return Some(r);
        }
    }
    None
}

pub async fn snapshot(sess: &Session) -> Result<Snapshot> {
    let mut stream = sess.exec_stream(&script::command(Mode::Once)).await?;
    let mut feed = Feed::default();
    let got = timeout(SNAPSHOT_DEADLINE, first(&mut stream, &mut feed)).await;
    let _ = stream.close().await;
    match got {
        Ok(Some(r)) => r,
        Ok(None) => Err(feed.ended()),
        Err(_) => Err(TelemetryError::Timeout),
    }
}

pub struct Monitor {
    rx: mpsc::Receiver<Result<Snapshot>>,
    stop: watch::Sender<bool>,
}

pub fn monitor(sess: Arc<Session>, interval: Duration) -> Monitor {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (stop, stop_rx) = watch::channel(false);
    tokio::spawn(pump(sess, script::clamp(interval), tx, stop_rx));
    Monitor { rx, stop }
}

impl Monitor {
    pub async fn next(&mut self) -> Option<Result<Snapshot>> {
        self.rx.recv().await
    }

    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    pub fn is_stopped(&self) -> bool {
        *self.stop.borrow()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

async fn halted(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn pump(
    sess: Arc<Session>,
    interval: Duration,
    tx: mpsc::Sender<Result<Snapshot>>,
    mut stop: watch::Receiver<bool>,
) {
    let cmd = script::command(Mode::Every(interval));
    let mut stream = tokio::select! {
        opened = sess.exec_stream(&cmd) => match opened {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.try_send(Err(e.into()));
                return;
            }
        },
        () = halted(&mut stop) => return,
    };
    let mut feed = Feed::default();
    loop {
        let ev = tokio::select! {
            () = halted(&mut stop) => break,
            ev = stream.next() => ev,
        };
        let Some(ev) = ev else {
            let _ = tx.try_send(Err(feed.ended()));
            return;
        };
        for out in feed.absorb(ev) {
            let fatal = matches!(out, Err(TelemetryError::Unsupported { .. }));
            match tx.try_send(out) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::debug!(
                        host = sess.host(),
                        "telemetry consumer lagging, frame dropped"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    let _ = stream.close().await;
                    return;
                }
            }
            if fatal {
                let _ = stream.close().await;
                return;
            }
        }
    }
    let _ = stream.close().await;
}
