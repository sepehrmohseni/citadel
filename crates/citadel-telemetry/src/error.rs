use thiserror::Error;

pub type Result<T, E = TelemetryError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error(transparent)]
    Ssh(#[from] citadel_ssh::TransportError),
    #[error("remote probe ended (status {status:?}): {stderr}")]
    Ended { status: Option<u32>, stderr: String },
    #[error("host does not expose Linux procfs ({kernel})")]
    Unsupported { kernel: String },
    #[error("remote probe timed out")]
    Timeout,
}
