use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine;
use citadel_sync::{Fetched, Remote, Result as SyncResult, SyncError, Version, remote_for, sync};
use citadel_vault::{Auth, HostProfile, KdfParams, SecretString, SyncBackend, Vault, VaultError};
use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Res = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const FAST: KdfParams = KdfParams {
    m_kib: 256,
    t: 1,
    p: 1,
};
const S3_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

#[derive(Clone, Debug)]
struct Logged {
    method: String,
    target: String,
    headers: HashMap<String, String>,
}

#[derive(Default)]
struct State {
    objects: HashMap<String, (Vec<u8>, u64)>,
    collections: HashSet<String>,
    seq: u64,
    log: Vec<Logged>,
    auth: Option<String>,
    need_parent: bool,
}

struct Mock {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl Mock {
    async fn start(auth: Option<String>, need_parent: bool) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State {
            auth,
            need_parent,
            ..State::default()
        }));
        let shared = state.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(serve(sock, shared.clone()));
            }
        });
        Ok(Self { addr, state })
    }

    fn log(&self) -> Vec<Logged> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .log
            .clone()
    }
}

async fn serve(sock: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let (rd, mut wr) = sock.into_split();
    let mut rd = BufReader::new(rd);
    loop {
        let mut line = String::new();
        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let target = parts.next().unwrap_or_default().to_owned();
        let mut headers = HashMap::new();
        loop {
            let mut h = String::new();
            if rd.read_line(&mut h).await.unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let len: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        if rd.read_exact(&mut body).await.is_err() {
            return;
        }
        let (code, etag, payload) = respond(&state, &method, &target, &headers, body);
        let mut head = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\n", payload.len());
        if let Some(tag) = etag {
            head.push_str(&format!("ETag: {tag}\r\n"));
        }
        head.push_str("\r\n");
        if wr.write_all(head.as_bytes()).await.is_err() || wr.write_all(&payload).await.is_err() {
            return;
        }
    }
}

fn respond(
    state: &Mutex<State>,
    method: &str,
    target: &str,
    headers: &HashMap<String, String>,
    body: Vec<u8>,
) -> (u16, Option<String>, Vec<u8>) {
    let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
    st.log.push(Logged {
        method: method.to_owned(),
        target: target.to_owned(),
        headers: headers.clone(),
    });
    if let Some(expected) = &st.auth
        && headers.get("authorization") != Some(expected)
    {
        return (401, None, b"unauthorized".to_vec());
    }
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let tag_of = |seq: u64| format!("\"v{seq}\"");
    match method {
        "GET" => match st.objects.get(&path) {
            Some((data, seq)) => (200, Some(tag_of(*seq)), data.clone()),
            None => (404, None, b"missing".to_vec()),
        },
        "MKCOL" => {
            st.collections.insert(path.trim_end_matches('/').to_owned());
            (201, None, Vec::new())
        }
        "PUT" => {
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| p.to_owned())
                .unwrap_or_default();
            if st.need_parent && !parent.is_empty() && !st.collections.contains(&parent) {
                return (409, None, b"no parent".to_vec());
            }
            let current = st.objects.get(&path).map(|(_, s)| tag_of(*s));
            if headers.get("if-none-match").map(String::as_str) == Some("*") && current.is_some() {
                return (412, None, b"exists".to_vec());
            }
            if let Some(want) = headers.get("if-match")
                && current.as_ref() != Some(want)
            {
                return (412, None, b"stale".to_vec());
            }
            st.seq += 1;
            let seq = st.seq;
            st.objects.insert(path, (body, seq));
            (201, Some(tag_of(seq)), Vec::new())
        }
        _ => (405, None, Vec::new()),
    }
}

fn pw(s: &str) -> SecretString {
    SecretString::from(s)
}

fn device(dir: &Path, name: &str, pass: &str) -> Result<Arc<Vault>, VaultError> {
    let v = Vault::open(dir.join(format!("{name}.redb")))?;
    v.initialize(&pw(pass), FAST)?;
    Ok(Arc::new(v))
}

fn add_host(v: &Vault, label: &str) -> Result<(), VaultError> {
    let mut h = HostProfile::new(
        label,
        &format!("{label}.internal"),
        "ops",
        Auth::Interactive,
    )?;
    v.put(&mut h)
}

fn labels(v: &Vault) -> Result<Vec<String>, VaultError> {
    let mut out: Vec<String> = v
        .list::<HostProfile>()?
        .iter()
        .map(|h| h.label.clone())
        .collect();
    out.sort();
    Ok(out)
}

fn s3_target(addr: SocketAddr) -> SyncBackend {
    SyncBackend::S3 {
        endpoint: format!("http://{addr}"),
        region: "eu-central-1".into(),
        bucket: "vaults".into(),
        object: "team/citadel.ctdl".into(),
        access_key: "AKIDEXAMPLE".into(),
        secret_key: pw(S3_SECRET),
        path_style: true,
    }
}

async fn two_device_scenario(a_remote: &dyn Remote, b_remote: &dyn Remote, dir: &Path) -> Res {
    let a = device(dir, "a", "alpha")?;
    add_host(&a, "a1")?;
    let first = sync(a.clone(), a_remote, None).await?;
    assert!(first.pushed && first.pulled.is_none());
    assert_eq!(first.rounds, 1);

    let b = device(dir, "b", "beta")?;
    add_host(&b, "b-local")?;
    let err = sync(b.clone(), b_remote, None).await.err();
    assert!(matches!(err, Some(SyncError::NeedsPassword)), "{err:?}");
    let err = sync(b.clone(), b_remote, Some(&pw("wrong"))).await.err();
    assert!(
        matches!(err, Some(SyncError::Vault(VaultError::BadPassword))),
        "{err:?}"
    );
    let joined = sync(b.clone(), b_remote, Some(&pw("alpha"))).await?;
    let pulled = joined.pulled.ok_or("nothing pulled")?;
    assert!(pulled.rekeyed && pulled.header_adopted);
    assert_eq!(pulled.report.added, 1);
    b.lock();
    b.unlock(&pw("alpha"))?;

    add_host(&a, "a2")?;
    add_host(&b, "b2")?;
    sync(a.clone(), a_remote, None).await?;
    let mid = sync(b.clone(), b_remote, None).await?;
    assert!(!mid.pulled.ok_or("b pulled nothing")?.rekeyed);
    sync(a.clone(), a_remote, None).await?;
    let want = vec!["a1", "a2", "b-local", "b2"];
    assert_eq!(labels(&a)?, want);
    assert_eq!(labels(&b)?, want);
    Ok(())
}

#[tokio::test]
async fn s3_two_devices_converge() -> Res {
    let mock = Mock::start(None, false).await?;
    let dir = tempfile::tempdir()?;
    let ra = remote_for(&s3_target(mock.addr), dir.path())?;
    let rb = remote_for(&s3_target(mock.addr), dir.path())?;
    assert!(ra.describe().contains("s3://vaults/team/citadel.ctdl"));
    two_device_scenario(ra.as_ref(), rb.as_ref(), dir.path()).await?;

    let log = mock.log();
    let first_put = log.iter().find(|r| r.method == "PUT").ok_or("no PUT")?;
    assert!(first_put.target.starts_with("/vaults/team/citadel.ctdl?"));
    assert_eq!(
        first_put.headers.get("if-none-match").map(String::as_str),
        Some("*")
    );
    for r in &log {
        assert!(r.target.contains("X-Amz-Signature="), "{}", r.target);
        assert!(
            r.target.contains("X-Amz-Credential=AKIDEXAMPLE"),
            "{}",
            r.target
        );
        assert!(r.target.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(!r.target.contains(S3_SECRET) && !r.target.contains("wJalrXUtnFEMI"));
        assert!(r.headers.values().all(|v| !v.contains("wJalrXUtnFEMI")));
    }
    assert!(
        log.iter()
            .filter(|r| r.method == "PUT")
            .skip(1)
            .all(|r| r.headers.contains_key("if-match"))
    );
    Ok(())
}

#[tokio::test]
async fn webdav_basic_auth_and_collection_bootstrap() -> Res {
    let token = base64::engine::general_purpose::STANDARD.encode("ops:hunter2");
    let mock = Mock::start(Some(format!("Basic {token}")), true).await?;
    let dir = tempfile::tempdir()?;
    let target = |pass: &str| SyncBackend::WebDav {
        url: format!("http://{}/dav/citadel/vault.ctdl", mock.addr),
        user: "ops".into(),
        password: pw(pass),
    };
    let ra = remote_for(&target("hunter2"), dir.path())?;
    let rb = remote_for(&target("hunter2"), dir.path())?;
    two_device_scenario(ra.as_ref(), rb.as_ref(), dir.path()).await?;
    let log = mock.log();
    assert!(
        log.iter()
            .any(|r| r.method == "MKCOL" && r.target == "/dav/citadel/")
    );
    assert!(log.iter().all(|r| r.headers.contains_key("authorization")));

    let bad = remote_for(&target("nope"), dir.path())?;
    let lone = device(dir.path(), "lone", "x")?;
    let err = sync(lone, bad.as_ref(), None).await.err();
    assert!(
        matches!(err, Some(SyncError::Http { status: 401, .. })),
        "{err:?}"
    );

    let creds = SyncBackend::WebDav {
        url: format!("http://ops:leak@{}/dav/x.ctdl", mock.addr),
        user: "ops".into(),
        password: pw("leak"),
    };
    assert!(!remote_for(&creds, dir.path())?.describe().contains("leak"));
    Ok(())
}

struct Racer {
    inner: Box<dyn Remote>,
    rival: Arc<Vault>,
    fired: AtomicBool,
}

impl Remote for Racer {
    fn fetch(&self) -> BoxFuture<'_, SyncResult<Fetched>> {
        self.inner.fetch()
    }

    fn store<'a>(
        &'a self,
        blob: &'a [u8],
        expected: Option<&'a str>,
    ) -> BoxFuture<'a, SyncResult<Version>> {
        Box::pin(async move {
            if !self.fired.swap(true, Ordering::SeqCst) {
                let current = self.inner.fetch().await?.map(|(_, v)| v);
                let rival_blob = self.rival.sync_blob()?;
                self.inner.store(&rival_blob, current.as_deref()).await?;
            }
            self.inner.store(blob, expected).await
        })
    }

    fn describe(&self) -> String {
        format!("racing {}", self.inner.describe())
    }
}

async fn conflict_scenario(make: impl Fn() -> SyncResult<Box<dyn Remote>>, dir: &Path) -> Res {
    let a = device(dir, "ca", "pw")?;
    add_host(&a, "seed")?;
    let plain = make()?;
    sync(a.clone(), plain.as_ref(), None).await?;
    let c = device(dir, "cc", "tmp")?;
    sync(c.clone(), plain.as_ref(), Some(&pw("pw"))).await?;
    add_host(&c, "rival")?;
    add_host(&a, "mine")?;
    let racer = Racer {
        inner: make()?,
        rival: c.clone(),
        fired: AtomicBool::new(false),
    };
    let report = sync(a.clone(), &racer, None).await?;
    assert_eq!(report.rounds, 2);
    assert_eq!(labels(&a)?, vec!["mine", "rival", "seed"]);
    sync(c.clone(), plain.as_ref(), None).await?;
    assert_eq!(labels(&c)?, vec!["mine", "rival", "seed"]);
    Ok(())
}

#[tokio::test]
async fn s3_conflict_is_retried() -> Res {
    let mock = Mock::start(None, false).await?;
    let dir = tempfile::tempdir()?;
    let target = s3_target(mock.addr);
    let cache = dir.path().to_path_buf();
    conflict_scenario(|| remote_for(&target, &cache), dir.path()).await?;
    assert!(mock.log().iter().any(|r| r.method == "PUT"));
    Ok(())
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn bare_repo(dir: &Path) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let repo = dir.join("remote.git");
    let ok = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&repo)
        .status()?
        .success();
    if !ok {
        return Err("git init failed".into());
    }
    Ok(repo.to_string_lossy().into_owned())
}

fn git_target(remote: &str) -> SyncBackend {
    SyncBackend::Git {
        remote: remote.to_owned(),
        branch: "main".into(),
        path: "vault/citadel.ctdl".into(),
    }
}

#[tokio::test]
async fn git_two_devices_converge() -> Res {
    if !git_available() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let remote = bare_repo(dir.path())?;
    let cache_a = dir.path().join("cache-a");
    let cache_b = dir.path().join("cache-b");
    let ra = remote_for(&git_target(&remote), &cache_a)?;
    let rb = remote_for(&git_target(&remote), &cache_b)?;
    two_device_scenario(ra.as_ref(), rb.as_ref(), dir.path()).await?;
    let log = std::process::Command::new("git")
        .args(["--git-dir", &remote, "log", "--format=%an <%ae>|%s", "main"])
        .output()?;
    let log = String::from_utf8(log.stdout)?;
    assert!(log.lines().count() >= 4, "{log}");
    assert!(
        log.lines()
            .all(|l| l == "Citadel <citadel@localhost>|citadel: replicate vault"),
        "{log}"
    );
    let tree = std::process::Command::new("git")
        .args(["--git-dir", &remote, "ls-tree", "-r", "--name-only", "main"])
        .output()?;
    assert_eq!(String::from_utf8(tree.stdout)?.trim(), "vault/citadel.ctdl");
    Ok(())
}

#[tokio::test]
async fn git_conflict_is_retried_and_foreign_files_survive() -> Res {
    if !git_available() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let remote = bare_repo(dir.path())?;
    let seed = dir.path().join("seed");
    let run = |args: &[&str]| -> Res {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&seed)
            .status()?
            .success();
        if ok {
            Ok(())
        } else {
            Err(format!("git {args:?} failed").into())
        }
    };
    std::fs::create_dir(&seed)?;
    run(&["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "ops notes\n")?;
    run(&["add", "README.md"])?;
    run(&["commit", "-q", "-m", "notes"])?;
    run(&["push", "-q", &remote, "main"])?;

    let counter = std::sync::atomic::AtomicU32::new(0);
    let base = dir.path().to_path_buf();
    let remote_ref = remote.clone();
    conflict_scenario(
        || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            remote_for(&git_target(&remote_ref), &base.join(format!("cache-{n}")))
        },
        dir.path(),
    )
    .await?;
    let tree = std::process::Command::new("git")
        .args(["--git-dir", &remote, "ls-tree", "-r", "--name-only", "main"])
        .output()?;
    let names = String::from_utf8(tree.stdout)?;
    assert!(names.lines().any(|l| l == "README.md"), "{names}");
    assert!(names.lines().any(|l| l == "vault/citadel.ctdl"), "{names}");
    Ok(())
}

#[test]
fn rejects_unsafe_targets() {
    let dir = Path::new("/tmp");
    let bad_git = [
        ("-oProxyCommand=evil", "main", "v.ctdl"),
        ("origin", "../main", "v.ctdl"),
        ("origin", "main", "../escape"),
        ("origin", "main", ".git/config"),
        ("origin", "ma in", "v.ctdl"),
    ];
    for (remote, branch, path) in bad_git {
        let t = SyncBackend::Git {
            remote: remote.into(),
            branch: branch.into(),
            path: path.into(),
        };
        assert!(
            matches!(remote_for(&t, dir), Err(SyncError::Config(_))),
            "{remote} {branch} {path}"
        );
    }
    let ftp = SyncBackend::S3 {
        endpoint: "ftp://example.com".into(),
        region: String::new(),
        bucket: "b".into(),
        object: "o".into(),
        access_key: "k".into(),
        secret_key: pw("s"),
        path_style: true,
    };
    assert!(matches!(remote_for(&ftp, dir), Err(SyncError::Config(_))));
    let collection = SyncBackend::WebDav {
        url: "https://dav.example.com/folder/".into(),
        user: String::new(),
        password: pw(""),
    };
    assert!(matches!(
        remote_for(&collection, dir),
        Err(SyncError::Config(_))
    ));
}
