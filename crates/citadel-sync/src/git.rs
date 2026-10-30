use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use secrecy::{ExposeSecret, SecretString};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::{Fetched, Remote, Result, SyncError, Version};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 128 << 20;
const AUTHOR: &str = "Citadel";
const EMAIL: &str = "citadel@localhost";
const HARDENING: [&str; 16] = [
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "credential.interactive=never",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "protocol.allow=never",
    "-c",
    "protocol.https.allow=always",
    "-c",
    "protocol.ssh.allow=always",
    "-c",
    "protocol.file.allow=always",
];
const HELPER: &str = "!f() { test \"$1\" = get && printf 'username=%s\\npassword=%s\\n' \"$CITADEL_GIT_USER\" \"$CITADEL_GIT_PASS\"; }; f";

static LOCKS: LazyLock<std::sync::Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
static INDEXES: AtomicU64 = AtomicU64::new(0);

fn dir_lock(dir: &Path) -> Arc<Mutex<()>> {
    LOCKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(dir.to_owned())
        .or_default()
        .clone()
}

struct Creds {
    user: SecretString,
    pass: SecretString,
}

fn unescape(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn split_remote(remote: &str) -> (String, Option<Creds>) {
    match url::Url::parse(remote) {
        Ok(mut u)
            if matches!(u.scheme(), "http" | "https")
                && (!u.username().is_empty() || u.password().is_some()) =>
        {
            let creds = Creds {
                user: SecretString::from(unescape(u.username())),
                pass: SecretString::from(u.password().map(unescape).unwrap_or_default()),
            };
            let _ = u.set_password(None);
            let _ = u.set_username("");
            (u.to_string(), Some(creds))
        }
        _ => (remote.to_owned(), None),
    }
}

pub(crate) struct Git {
    remote: String,
    creds: Option<Creds>,
    branch: String,
    path: String,
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

struct Run {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

pub(crate) fn fnv1a(s: &str) -> u64 {
    fnv1a_bytes(s.as_bytes())
}

pub(crate) fn fnv1a_bytes(s: &[u8]) -> u64 {
    s.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
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
        let (url, creds) = split_remote(remote);
        let dir = cache.join(format!("git-{:016x}", fnv1a(&url)));
        let legacy = cache.join(format!("git-{:016x}", fnv1a(remote)));
        if legacy != dir && legacy.is_dir() {
            let _ = std::fs::remove_dir_all(&legacy);
        }
        Ok(Self {
            remote: url,
            creds,
            branch: branch.to_owned(),
            path: path.to_owned(),
            lock: dir_lock(&dir),
            dir,
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
            .env("GIT_PROTOCOL_FROM_USER", "0")
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
        if let Some(c) = &self.creds {
            cmd.env("GIT_CONFIG_COUNT", "2")
                .env("GIT_CONFIG_KEY_0", "credential.helper")
                .env("GIT_CONFIG_VALUE_0", "")
                .env("GIT_CONFIG_KEY_1", "credential.helper")
                .env("GIT_CONFIG_VALUE_1", HELPER)
                .env("CITADEL_GIT_USER", c.user.expose_secret())
                .env("CITADEL_GIT_PASS", c.pass.expose_secret());
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| SyncError::Git(format!("cannot run git: {e}")))?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data).await?;
            drop(pipe);
        }
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(SyncError::Git("git pipes unavailable".into()));
        };
        let collect = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut stdout = stdout.take(MAX_OUTPUT as u64 + 1);
            let mut stderr = stderr.take(64 * 1024);
            let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
            a?;
            b?;
            drop(stdout);
            drop(stderr);
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, out, err))
        };
        let (status, out, err) =
            tokio::time::timeout(GIT_TIMEOUT, collect)
                .await
                .map_err(|_| {
                    SyncError::Git(format!("git {} timed out", args.first().unwrap_or(&"")))
                })??;
        if out.len() > MAX_OUTPUT {
            return Err(SyncError::Oversize(MAX_OUTPUT));
        }
        Ok(Run {
            ok: status.success(),
            stdout: out,
            stderr: String::from_utf8_lossy(&err).trim().to_owned(),
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
            self.must(&["remote", "set-url", "origin", &self.remote], None, None)
                .await?;
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.dir).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [Some(self.dir.as_path()), self.dir.parent()]
                .into_iter()
                .flatten()
            {
                let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
            }
        }
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
        let index = self.dir.join(format!(
            "citadel-index-{}-{}",
            std::process::id(),
            INDEXES.fetch_add(1, Ordering::Relaxed)
        ));
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
        format!("git {} ({}:{})", self.remote, self.branch, self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_never_stay_in_the_remote_url() {
        let (url, creds) = split_remote("https://ops:s3cr%40t@git.example/vault.git");
        assert_eq!(url, "https://git.example/vault.git");
        let c = creds.expect("credentials");
        assert_eq!(c.user.expose_secret(), "ops");
        assert_eq!(c.pass.expose_secret(), "s3cr@t");
        let (url, creds) = split_remote("git@github.com:me/vault.git");
        assert_eq!(url, "git@github.com:me/vault.git");
        assert!(creds.is_none());
        let (url, creds) = split_remote("ssh://git@host/vault.git");
        assert_eq!(url, "ssh://git@host/vault.git");
        assert!(creds.is_none());
    }
}
