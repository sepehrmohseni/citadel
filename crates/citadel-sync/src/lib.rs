#![forbid(unsafe_code)]

mod dav;
mod error;
mod git;
mod http;
mod s3;

use std::path::Path;
use std::sync::Arc;

use citadel_vault::{Absorbed, MergeReport, SyncBackend, Vault};
use futures::future::BoxFuture;
use secrecy::SecretString;
use serde::Serialize;

pub use error::{Result, SyncError};

pub type Version = String;

pub type Fetched = Option<(Vec<u8>, Version)>;

pub const MAX_ROUNDS: u32 = 5;

pub trait Remote: Send + Sync {
    fn fetch(&self) -> BoxFuture<'_, Result<Fetched>>;
    fn store<'a>(
        &'a self,
        blob: &'a [u8],
        expected: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Version>>;
    fn describe(&self) -> String;
}

pub fn remote_for(target: &SyncBackend, cache_dir: &Path) -> Result<Box<dyn Remote>> {
    Ok(match target {
        SyncBackend::S3 {
            endpoint,
            region,
            bucket,
            object,
            access_key,
            secret_key,
            path_style,
        } => Box::new(s3::S3::new(
            endpoint,
            region,
            bucket,
            object,
            access_key,
            secret_key,
            *path_style,
        )?),
        SyncBackend::WebDav {
            url,
            user,
            password,
        } => Box::new(dav::WebDav::new(url, user, password)?),
        SyncBackend::Git {
            remote,
            branch,
            path,
        } => Box::new(git::Git::new(remote, branch, path, cache_dir)?),
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct SyncReport {
    pub pulled: Option<Absorbed>,
    pub pushed: bool,
    pub rounds: u32,
    pub version: Option<String>,
}

fn fold(acc: Option<Absorbed>, next: Absorbed) -> Absorbed {
    let Some(prev) = acc else {
        return next;
    };
    let (a, b) = (prev.report, next.report);
    Absorbed {
        report: MergeReport {
            added: a.added + b.added,
            updated: a.updated + b.updated,
            deleted: a.deleted + b.deleted,
            unchanged: b.unchanged,
        },
        rekeyed: prev.rekeyed || next.rekeyed,
        header_adopted: prev.header_adopted || next.header_adopted,
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> citadel_vault::Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| SyncError::Transport(format!("vault task failed: {e}")))?
        .map_err(error::vault)
}

pub async fn sync(
    vault: Arc<Vault>,
    remote: &dyn Remote,
    password: Option<&SecretString>,
) -> Result<SyncReport> {
    let password = password.cloned();
    let mut pulled: Option<Absorbed> = None;
    for round in 1..=MAX_ROUNDS {
        let fetched = remote.fetch().await?;
        let expected = match fetched {
            Some((blob, version)) => {
                let v = vault.clone();
                let pw = password.clone();
                let got = blocking(move || v.absorb(&blob, pw.as_ref())).await?;
                pulled = Some(fold(pulled, got));
                Some(version)
            }
            None => None,
        };
        let v = vault.clone();
        let blob = blocking(move || v.sync_blob()).await?;
        match remote.store(&blob, expected.as_deref()).await {
            Ok(version) => {
                tracing::info!(remote = %remote.describe(), rounds = round, "vault replicated");
                return Ok(SyncReport {
                    pulled,
                    pushed: true,
                    rounds: round,
                    version: Some(version),
                });
            }
            Err(SyncError::Conflict) => {
                tracing::debug!(remote = %remote.describe(), round, "remote moved during sync, retrying");
            }
            Err(e) => return Err(e),
        }
    }
    Err(SyncError::Conflict)
}
