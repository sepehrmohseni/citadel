use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug, Default)]
pub struct Meter {
    pub active: AtomicU64,
    pub accepted: AtomicU64,
    pub failed: AtomicU64,
    pub tx: AtomicU64,
    pub rx: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MeterSnapshot {
    pub active: u64,
    pub accepted: u64,
    pub failed: u64,
    pub tx: u64,
    pub rx: u64,
}

impl Meter {
    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            active: self.active.load(Ordering::Relaxed),
            accepted: self.accepted.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            tx: self.tx.load(Ordering::Relaxed),
            rx: self.rx.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn open(self: &Arc<Self>) -> Lease {
        self.accepted.fetch_add(1, Ordering::Relaxed);
        self.active.fetch_add(1, Ordering::Relaxed);
        Lease(self.clone())
    }

    pub(crate) fn fail(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct Lease(Arc<Meter>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) struct Metered<S> {
    inner: S,
    meter: Arc<Meter>,
}

impl<S> Metered<S> {
    pub fn new(inner: S, meter: Arc<Meter>) -> Self {
        Self { inner, meter }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Metered<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len().saturating_sub(before);
        self.meter.tx.fetch_add(n as u64, Ordering::Relaxed);
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Metered<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &res {
            self.meter.rx.fetch_add(*n as u64, Ordering::Relaxed);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
