use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use citadel_vault::Id;
use serde::Serialize;

use crate::core::lock;
use crate::error::{AppError, CmdResult};

const TTL: Duration = Duration::from_secs(600);
const DROP_TTL: Duration = Duration::from_secs(60);
const MAX_LIVE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

struct Grant {
    path: PathBuf,
    access: Access,
    at: Instant,
}

#[derive(Default)]
pub struct Grants {
    issued: Mutex<HashMap<String, Grant>>,
    dropped: Mutex<Vec<(PathBuf, Instant)>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Picked {
    pub token: String,
    pub name: String,
    pub path: String,
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Grants {
    pub fn issue(&self, path: PathBuf, access: Access) -> CmdResult<Picked> {
        let token = Id::random()?.to_string();
        let picked = Picked {
            token: token.clone(),
            name: display_name(&path),
            path: path.display().to_string(),
        };
        let mut issued = lock(&self.issued);
        issued.retain(|_, g| g.at.elapsed() < TTL);
        if issued.len() >= MAX_LIVE {
            return Err(AppError::invalid("too many pending file selections"));
        }
        issued.insert(
            token,
            Grant {
                path,
                access,
                at: Instant::now(),
            },
        );
        Ok(picked)
    }

    pub fn redeem(&self, token: &str, access: Access) -> CmdResult<PathBuf> {
        let grant = lock(&self.issued)
            .remove(token)
            .filter(|g| g.access == access && g.at.elapsed() < TTL)
            .ok_or_else(|| AppError::invalid("file selection expired; choose the file again"))?;
        Ok(grant.path)
    }

    pub fn note_dropped(&self, paths: &[PathBuf]) {
        let mut dropped = lock(&self.dropped);
        dropped.retain(|(_, at)| at.elapsed() < DROP_TTL);
        let now = Instant::now();
        dropped.extend(paths.iter().map(|p| (p.clone(), now)));
        let excess = dropped.len().saturating_sub(MAX_LIVE);
        dropped.drain(..excess);
    }

    pub fn claim_dropped(&self, paths: &[String]) -> CmdResult<Vec<Picked>> {
        let mut claimed = Vec::with_capacity(paths.len());
        for raw in paths {
            let path = PathBuf::from(raw);
            let fresh = {
                let mut dropped = lock(&self.dropped);
                let pos = dropped
                    .iter()
                    .position(|(p, at)| *p == path && at.elapsed() < DROP_TTL);
                pos.map(|i| dropped.swap_remove(i)).is_some()
            };
            if !fresh {
                return Err(AppError::invalid(format!(
                    "{} was not dropped onto Citadel",
                    display_name(&path)
                )));
            }
            claimed.push(self.issue(path, Access::Read)?);
        }
        Ok(claimed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_single_use_and_typed() {
        let g = Grants::default();
        let p = g.issue(PathBuf::from("/tmp/a.ctdl"), Access::Read).unwrap();
        assert_eq!(p.name, "a.ctdl");
        assert!(g.redeem(&p.token, Access::Write).is_err());
        let p = g.issue(PathBuf::from("/tmp/a.ctdl"), Access::Read).unwrap();
        assert_eq!(
            g.redeem(&p.token, Access::Read).unwrap(),
            PathBuf::from("/tmp/a.ctdl")
        );
        assert!(g.redeem(&p.token, Access::Read).is_err());
        assert!(g.redeem("not-a-token", Access::Read).is_err());
    }

    #[test]
    fn only_paths_the_os_reported_as_dropped_can_be_claimed() {
        let g = Grants::default();
        g.note_dropped(&[PathBuf::from("/home/u/report.pdf")]);
        assert!(
            g.claim_dropped(&["/home/u/.ssh/id_ed25519".into()])
                .is_err()
        );
        let ok = g.claim_dropped(&["/home/u/report.pdf".into()]).unwrap();
        assert_eq!(ok.len(), 1);
        assert!(g.claim_dropped(&["/home/u/report.pdf".into()]).is_err());
    }
}
