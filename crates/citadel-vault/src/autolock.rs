use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::Vault;

pub struct AutoLock {
    idle: watch::Sender<Option<Duration>>,
    task: JoinHandle<()>,
}

impl AutoLock {
    pub fn spawn(vault: Arc<Vault>, idle: Option<Duration>) -> Self {
        let (tx, rx) = watch::channel(idle);
        let task = tokio::spawn(watchdog(vault, rx));
        Self { idle: tx, task }
    }

    pub fn set_idle(&self, idle: Option<Duration>) {
        self.idle.send_replace(idle);
    }

    pub fn idle(&self) -> Option<Duration> {
        *self.idle.borrow()
    }
}

impl Drop for AutoLock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn watchdog(vault: Arc<Vault>, mut idle_rx: watch::Receiver<Option<Duration>>) {
    let mut state_rx = vault.subscribe();
    loop {
        let limit = *idle_rx.borrow_and_update();
        state_rx.mark_unchanged();
        let wait = match (limit, vault.idle_for()) {
            (Some(limit), Some(elapsed)) if elapsed >= limit => {
                vault.lock();
                continue;
            }
            (Some(limit), Some(elapsed)) => Some(limit - elapsed),
            _ => None,
        };
        tokio::select! {
            changed = idle_rx.changed() => if changed.is_err() { return },
            changed = state_rx.changed() => if changed.is_err() { return },
            () = nap(wait) => {}
        }
    }
}

async fn nap(wait: Option<Duration>) {
    match wait {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
