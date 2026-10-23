use std::path::PathBuf;

use thiserror::Error;

pub type Result<T, E = ConfigError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{} is not valid UTF-8", .0.display())]
    Encoding(PathBuf),
}
