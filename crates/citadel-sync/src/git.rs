use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::{Fetched, Remote, Result, SyncError, Version};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const AUTHOR: &str = "Citadel";
const EMAIL: &str = "citadel@localhost";
const HARDENING: [&str; 8] = [
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "credential.interactive=never",
    "-c",
    "core.fsmonitor=false",
];

pub(crate) struct Git {
    remote: String,
    branch: String,
    path: String,
    dir: PathBuf,
    lock: Mutex<()>,
}

struct Run {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn valid_branch(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("@{")
        && !b.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c))
}

fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.ends_with('/')
        && p.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != ".." && seg != ".git")
        && !p.chars().any(char::is_control)
}

impl Git {
    pub fn new(remote: &str, branch: &str, path: &str, cache: &Path) -> Result<Self> {
        if remote.is_empty() || remote.starts_with('-') || remote.chars().any(char::is_control) {
            return Err(SyncError::Config("invalid git remote".into()));
        }
        let branch = if branch.is_empty() { "main" } else { branch };
        if !valid_branch(branch) {
            return Err(SyncError::Config(format!("invalid git branch {branch:?}")));
        }
        let path = if path.is_empty() {
            "citadel.ctdl"
        } else {
            path
        };
        if !valid_path(path) {
            return Err(SyncError::Config(format!(
                "invalid path in repository {path:?}"
            )));
        }
        Ok(Self {
            remote: remote.to_owned(),
            branch: branch.to_owned(),
            path: path.to_owned(),
            dir: cache.join(format!("git-{:016x}", fnv1a(remote))),
            lock: Mutex::new(()),
        })
    }

    fn tracking(&self) -> String {
        format!("refs/remotes/origin/{}", self.branch)
    }

    async fn run(&self, args: &[&str], stdin: Option<&[u8]>, index: Option<&Path>) -> Result<Run> {
        let mut cmd = Command::new("git");
        cmd.args(HARDENING)
            .arg("--git-dir")
            .arg(&self.dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", AUTHOR)
            .env("GIT_AUTHOR_EMAIL", EMAIL)
            .env("GIT_COMMITTER_NAME", AUTHOR)
            .env("GIT_COMMITTER_EMAIL", EMAIL)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match index {
            Some(i) => cmd.env("GIT_INDEX_FILE", i),
            None => cmd.env_remove("GIT_INDEX_FILE"),
        };
        let mut child = cmd
            .spawn()
            .map_err(|e| SyncError::Git(format!("cannot run git: {e}")))?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data).await?;
            drop(pipe);
        }
        let out = tokio::time::timeout(GIT_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                SyncError::Git(format!("git {} timed out", args.first().unwrap_or(&"")))
            })??;
        Ok(Run {
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }

    async fn must(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        index: Option<&Path>,
    ) -> Result<Vec<u8>> {
        let r = self.run(args, stdin, index).await?;
        if r.ok {
            Ok(r.stdout)
        } else {
            Err(SyncError::Git(format!(
                "git {}: {}",
                args.first().unwrap_or(&""),
                r.stderr
            )))
        }
    }

    async fn text(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        index: Option<&Path>,
    ) -> Result<String> {
        let out = self.must(args, stdin, index).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    async fn ensure(&self) -> Result<()> {
        if self.dir.join("HEAD").is_file() {
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.dir).await?;
        let init = Command::new("git")
            .args(HARDENING)
            .args(["init", "--bare", "-q"])
            .arg(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| SyncError::Git(format!("cannot run git: {e}")))?;
        if !init.status.success() {
            return Err(SyncError::Git(
                String::from_utf8_lossy(&init.stderr).trim().to_owned(),
            ));
        }
        self.must(&["remote", "add", "origin", &self.remote], None, None)
            .await?;
        Ok(())
    }

    async fn refresh(&self) -> Result<Option<String>> {
        self.ensure().await?;
        let spec = format!("+refs/heads/{}:{}", self.branch, self.tracking());
        let r = self
            .run(
                &[
                    "fetch",
                    "-q",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "origin",
                    &spec,
                ],
                None,
                None,
            )
            .await?;
        if !r.ok {
            let missing = r.stderr.contains("couldn't find remote ref")
                || r.stderr.contains("could not find remote ref");
            if !missing {
                return Err(SyncError::Git(format!("fetch: {}", r.stderr)));
            }
            let _ = self
                .run(&["update-ref", "-d", &self.tracking()], None, None)
                .await;
            return Ok(None);
        }
        let tip = self
            .text(
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("{}^{{commit}}", self.tracking()),
                ],
                None,
                None,
            )
            .await?;
        Ok(Some(tip))
    }

    async fn blob_at(&self, commit: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{commit}:{}", self.path);
        let probe = self.run(&["cat-file", "-e", &spec], None, None).await?;
        if !probe.ok {
            return Ok(None);
        }
        Ok(Some(
            self.must(&["cat-file", "blob", &spec], None, None).await?,
        ))
    }

    async fn pull(&self) -> Result<Fetched> {
        let _guard = self.lock.lock().await;
        let Some(tip) = self.refresh().await? else {
            return Ok(None);
        };
        Ok(self.blob_at(&tip).await?.map(|b| (b, tip)))
    }

    async fn push(&self, blob: &[u8], expected: Option<&str>) -> Result<Version> {
        let _guard = self.lock.lock().await;
        let base = match expected {
            Some(v) => Some(v.to_owned()),
            None => match self.refresh().await? {
                Some(tip) if self.blob_at(&tip).await?.is_some() => {
                    return Err(SyncError::Conflict);
                }
                other => other,
            },
        };
        self.ensure().await?;
        let index = self.dir.join("citadel-index");
        let _ = tokio::fs::remove_file(&index).await;
        match &base {
            Some(b) => self.must(&["read-tree", b], None, Some(&index)).await?,
            None => {
                self.must(&["read-tree", "--empty"], None, Some(&index))
                    .await?
            }
        };
        let sha = self
            .text(&["hash-object", "-w", "--stdin"], Some(blob), None)
            .await?;
        let info = format!("100644,{sha},{}", self.path);
        self.must(
            &["update-index", "--add", "--cacheinfo", &info],
            None,
            Some(&index),
        )
        .await?;
        let tree = self.text(&["write-tree"], None, Some(&index)).await?;
        let _ = tokio::fs::remove_file(&index).await;
        let mut args = vec![
            "commit-tree",
            tree.as_str(),
            "-m",
            "citadel: replicate vault",
        ];
        if let Some(b) = &base {
            args.extend(["-p", b.as_str()]);
        }
        let commit = self.text(&args, None, None).await?;
        let refspec = format!("{commit}:refs/heads/{}", self.branch);
        let r = self
            .run(
                &["push", "-q", "--porcelain", "origin", &refspec],
                None,
                None,
            )
            .await?;
        if !r.ok {
            let out = String::from_utf8_lossy(&r.stdout);
            let rejected = ["rejected", "non-fast-forward", "fetch first", "stale info"]
                .iter()
                .any(|m| r.stderr.contains(m) || out.contains(m));
            return Err(if rejected {
                SyncError::Conflict
            } else {
                SyncError::Git(format!("push: {}", r.stderr))
            });
        }
        let _ = self
            .run(&["update-ref", &self.tracking(), &commit], None, None)
            .await;
        Ok(commit)
    }
}

impl Remote for Git {
    fn fetch(&self) -> BoxFuture<'_, Result<Fetched>> {
        Box::pin(self.pull())
    }

    fn store<'a>(
        &'a self,
        blob: &'a [u8],
        expected: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Version>> {
        Box::pin(self.push(blob, expected))
    }

    fn describe(&self) -> String {
        let shown = match url::Url::parse(&self.remote) {
            Ok(mut u) if u.password().is_some() || !u.username().is_empty() => {
                let _ = u.set_password(None);
                let _ = u.set_username("");
                u.to_string()
            }
            _ => self.remote.clone(),
        };
        format!("git {shown} ({}:{})", self.branch, self.path)
    }
}
