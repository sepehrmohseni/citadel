use std::sync::Arc;

use bytes::Bytes;
use russh::client::Msg;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Pty as Mode};
use tokio::sync::mpsc;

use crate::{Result, Session};

const EVENT_DEPTH: usize = 256;

const MODES: &[(Mode, u32)] = &[
    (Mode::VERASE, 0x7f),
    (Mode::IUTF8, 1),
    (Mode::ECHO, 1),
    (Mode::ICRNL, 1),
    (Mode::TTY_OP_ISPEED, 38400),
    (Mode::TTY_OP_OSPEED, 38400),
];

#[derive(Clone, Debug)]
pub struct PtyRequest {
    pub term: String,
    pub cols: u32,
    pub rows: u32,
    pub env: Vec<(String, String)>,
    pub command: Option<String>,
}

impl Default for PtyRequest {
    fn default() -> Self {
        Self {
            term: "xterm-256color".to_owned(),
            cols: 80,
            rows: 24,
            env: Vec::new(),
            command: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyEvent {
    Data(Bytes),
    Exit(u32),
    Signal(String),
    Closed,
}

#[derive(Clone)]
pub struct PtyWriter(Arc<ChannelWriteHalf<Msg>>);

impl PtyWriter {
    pub async fn write(&self, data: impl Into<Bytes>) -> Result<()> {
        Ok(self.0.data_bytes(data).await?)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        Ok(self.0.window_change(cols, rows, 0, 0).await?)
    }

    pub async fn close(&self) -> Result<()> {
        let _ = self.0.eof().await;
        Ok(self.0.close().await?)
    }
}

pub struct Pty {
    pub writer: PtyWriter,
    pub events: mpsc::Receiver<PtyEvent>,
}

impl Session {
    pub async fn open_pty(&self, req: &PtyRequest) -> Result<Pty> {
        let ch = self.open_session_channel().await?;
        if self.agent_forward {
            ch.agent_forward(false).await?;
        }
        for (k, v) in &req.env {
            ch.set_env(false, k.as_str(), v.as_str()).await?;
        }
        ch.request_pty(true, &req.term, req.cols, req.rows, 0, 0, MODES)
            .await?;
        match &req.command {
            Some(cmd) => ch.exec(true, cmd.as_str()).await?,
            None => ch.request_shell(true).await?,
        }
        let (read, write) = ch.split();
        let (tx, events) = mpsc::channel(EVENT_DEPTH);
        tokio::spawn(pump(read, tx));
        Ok(Pty {
            writer: PtyWriter(Arc::new(write)),
            events,
        })
    }
}

async fn pump(mut read: ChannelReadHalf, tx: mpsc::Sender<PtyEvent>) {
    while let Some(msg) = read.wait().await {
        let ev = match msg {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                PtyEvent::Data(data)
            }
            ChannelMsg::ExitStatus { exit_status } => PtyEvent::Exit(exit_status),
            ChannelMsg::ExitSignal { signal_name, .. } => {
                PtyEvent::Signal(format!("{signal_name:?}"))
            }
            ChannelMsg::Close => break,
            _ => continue,
        };
        if tx.send(ev).await.is_err() {
            return;
        }
    }
    let _ = tx.send(PtyEvent::Closed).await;
}
