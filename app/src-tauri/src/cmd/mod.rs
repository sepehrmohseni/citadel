pub mod audit;
pub mod config;
pub mod device;
pub mod files;
pub mod fleet;
pub mod fonts;
pub mod hosts;
pub mod misc;
pub mod pick;
pub mod ports;
pub mod report;
pub mod session;
pub mod sync;
pub mod telemetry;
pub mod term;
pub mod tunnels;
pub mod vault;

use std::sync::Arc;

use crate::core::Core;
pub use crate::core::Lease;
use crate::error::CmdResult;

pub struct AppState(pub Arc<Core>);

pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}
