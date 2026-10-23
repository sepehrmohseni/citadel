pub mod audit;
pub mod config;
pub mod files;
pub mod hosts;
pub mod misc;
pub mod sync;
pub mod telemetry;
pub mod term;
pub mod tunnels;
pub mod vault;

use std::sync::Arc;

use citadel_vault::Id;

use crate::core::Core;
use crate::error::CmdResult;

pub struct AppState(pub Arc<Core>);

pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}

pub struct Lease {
    core: Arc<Core>,
    host: Id,
}

impl Lease {
    pub fn new(core: &Arc<Core>, host: Id) -> Self {
        Self {
            core: core.clone(),
            host,
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.core.release(self.host);
    }
}
