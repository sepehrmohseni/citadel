use std::path::PathBuf;

use futures::future::BoxFuture;
use russh::keys::{Error as KeyError, PublicKey};

use crate::Result;

pub trait HostKeyVerifier: Send + Sync + 'static {
    fn verify<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, Result<bool>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownHost {
    Reject,
    Learn,
}

#[derive(Clone, Debug)]
pub struct KnownHostsFile {
    pub path: PathBuf,
    pub unknown: UnknownHost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    Known,
    Unknown,
    Changed { line: usize },
}

impl KnownHostsFile {
    pub fn system(unknown: UnknownHost) -> Option<Self> {
        std::env::home_dir().map(|home| Self {
            path: home.join(".ssh").join("known_hosts"),
            unknown,
        })
    }

    pub fn status(&self, host: &str, port: u16, key: &PublicKey) -> Result<HostKeyStatus> {
        if !self.path.exists() {
            return Ok(HostKeyStatus::Unknown);
        }
        match russh::keys::check_known_hosts_path(host, port, key, &self.path) {
            Ok(true) => Ok(HostKeyStatus::Known),
            Ok(false) => {
                let pinned =
                    russh::keys::known_hosts::known_host_keys_path(host, port, &self.path)?;
                Ok(match pinned.first() {
                    Some((line, _)) => HostKeyStatus::Changed { line: *line },
                    None => HostKeyStatus::Unknown,
                })
            }
            Err(KeyError::KeyChanged { line }) => Ok(HostKeyStatus::Changed { line }),
            Err(e) => Err(e.into()),
        }
    }
}

impl HostKeyVerifier for KnownHostsFile {
    fn verify<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            match self.status(host, port, key)? {
                HostKeyStatus::Known => Ok(true),
                HostKeyStatus::Changed { .. } => Ok(false),
                HostKeyStatus::Unknown => match self.unknown {
                    UnknownHost::Reject => Ok(false),
                    UnknownHost::Learn => {
                        if let Some(dir) = self.path.parent() {
                            std::fs::create_dir_all(dir)?;
                        }
                        russh::keys::known_hosts::learn_known_hosts_path(
                            host, port, key, &self.path,
                        )?;
                        Ok(true)
                    }
                },
            }
        })
    }
}
