use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use citadel_ssh::{
    Backoff, Credential, ForwardSpec, Hop, HostKeyStatus, HostKeyVerifier, KnownHostsFile, Link,
    LinkState, PtyEvent, PtyRequest, PublicKey, Route, TransportError, Tunnel, TunnelState,
    UnknownHost, connect,
};
use citadel_testkit::{PASSWORD, TestServer, USER, echo_server, random_key};
use futures::future::BoxFuture;
use secrecy::SecretString;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

type Res = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct Pinned(Vec<PublicKey>);

impl HostKeyVerifier for Pinned {
    fn verify<'a>(
        &'a self,
        _h: &'a str,
        _p: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, citadel_ssh::Result<bool>> {
        let ok = self.0.iter().any(|k| k.key_data() == key.key_data());
        Box::pin(async move { Ok(ok) })
    }
}

fn hop(addr: SocketAddr, credential: Credential) -> Hop {
    Hop {
        host: addr.ip().to_string(),
        port: addr.port(),
        user: USER.to_owned(),
        credential,
        legacy: false,
    }
}

fn password() -> Credential {
    Credential::Password(SecretString::from(PASSWORD))
}

fn route(hops: Vec<Hop>, pins: Vec<PublicKey>) -> Route {
    let mut r = Route::direct(hops[0].clone(), Arc::new(Pinned(pins)));
    r.hops = hops;
    r.connect_timeout = Duration::from_secs(5);
    r.keepalive = Duration::from_secs(1);
    r
}

async fn roundtrip(addr: SocketAddr, payload: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut s = TcpStream::connect(addr).await?;
    s.write_all(payload).await?;
    let mut buf = vec![0u8; payload.len()];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

#[tokio::test]
async fn password_and_key_auth_exec() -> Res {
    let srv = TestServer::start("alpha").await;
    let pins = vec![srv.host_public()];
    let sess = connect(&route(vec![hop(srv.addr, password())], pins.clone())).await?;
    let out = sess.exec("echo citadel").await?;
    assert_eq!(out.stdout, b"citadel\n");
    assert_eq!(out.status, Some(0));
    let fail = sess.exec("fail").await?;
    assert_eq!(fail.stderr, b"boom\n");
    assert_eq!(fail.status, Some(2));
    assert_eq!(sess.exec("exit 7").await?.status, Some(7));
    assert!(sess.host_key().is_some());

    let keyed = connect(&route(
        vec![hop(srv.addr, Credential::Key(srv.client_key.clone()))],
        pins.clone(),
    ))
    .await?;
    assert_eq!(keyed.exec("whoami").await?.stdout, b"alpha\n");

    let wrong = Credential::Password(SecretString::from("nope"));
    let err = connect(&route(vec![hop(srv.addr, wrong)], pins.clone()))
        .await
        .err();
    assert!(
        matches!(err, Some(TransportError::AuthFailed { .. })),
        "{err:?}"
    );

    let stranger = Credential::Key(Arc::new(random_key()));
    let err = connect(&route(vec![hop(srv.addr, stranger)], pins))
        .await
        .err();
    assert!(
        matches!(err, Some(TransportError::AuthFailed { .. })),
        "{err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn host_key_pinning_and_known_hosts() -> Res {
    let srv = TestServer::start("beta").await;
    let impostor = random_key().public_key().clone();
    let err = connect(&route(vec![hop(srv.addr, password())], vec![impostor]))
        .await
        .err();
    assert!(
        matches!(err, Some(TransportError::HostKeyRejected { .. })),
        "{err:?}"
    );

    let dir = tempfile::tempdir()?;
    let file = KnownHostsFile {
        path: dir.path().join("known_hosts"),
        unknown: UnknownHost::Reject,
    };
    let mut r = route(vec![hop(srv.addr, password())], Vec::new());
    r.verifier = Arc::new(file.clone());
    assert!(matches!(
        connect(&r).await.err(),
        Some(TransportError::HostKeyRejected { .. })
    ));

    let learn = KnownHostsFile {
        unknown: UnknownHost::Learn,
        ..file.clone()
    };
    r.verifier = Arc::new(learn);
    connect(&r).await?;
    let host = srv.addr.ip().to_string();
    assert_eq!(
        file.status(&host, srv.addr.port(), &srv.host_public())?,
        HostKeyStatus::Known
    );
    r.verifier = Arc::new(file.clone());
    connect(&r).await?;

    let other = random_key().public_key().clone();
    assert!(matches!(
        file.status(&host, srv.addr.port(), &other)?,
        HostKeyStatus::Changed { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn pty_streams_and_resizes() -> Res {
    let srv = TestServer::start("gamma").await;
    let sess = connect(&route(
        vec![hop(srv.addr, password())],
        vec![srv.host_public()],
    ))
    .await?;
    let req = PtyRequest {
        cols: 132,
        rows: 40,
        ..PtyRequest::default()
    };
    let mut pty = sess.open_pty(&req).await?;
    let mut seen = Vec::new();
    while !seen.ends_with(b"gamma$ ") {
        match timeout(Duration::from_secs(5), pty.events.recv()).await? {
            Some(PtyEvent::Data(d)) => seen.extend_from_slice(&d),
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    pty.writer.write(&b"ls -la"[..]).await?;
    let mut echo = Vec::new();
    while echo.len() < 6 {
        if let Some(PtyEvent::Data(d)) = timeout(Duration::from_secs(5), pty.events.recv()).await? {
            echo.extend_from_slice(&d);
        }
    }
    assert_eq!(echo, b"ls -la");
    pty.writer.resize(200, 60).await?;
    pty.writer.write(&b"exit\r"[..]).await?;
    let mut exit = None;
    while let Some(ev) = timeout(Duration::from_secs(5), pty.events.recv()).await? {
        match ev {
            PtyEvent::Exit(code) => exit = Some(code),
            PtyEvent::Closed => break,
            _ => {}
        }
    }
    assert_eq!(exit, Some(0));
    let ptys = srv.ptys();
    assert_eq!(ptys[0], ("xterm-256color".to_owned(), 132, 40));
    assert!(ptys.contains(&("resize".to_owned(), 200, 60)));
    Ok(())
}

#[tokio::test]
async fn local_and_socks_forwarding() -> Res {
    let srv = TestServer::start("fwd").await;
    let (echo, _task) = echo_server().await;
    let r = route(vec![hop(srv.addr, password())], vec![srv.host_public()]);
    let link = Link::spawn(
        move || {
            let r = r.clone();
            async move { connect(&r).await }
        },
        Backoff::default(),
    );

    let local = Tunnel::open(
        ForwardSpec::Local {
            bind: "127.0.0.1".into(),
            bind_port: 0,
            dest: echo.ip().to_string(),
            dest_port: echo.port(),
        },
        link.handle(),
    )
    .await?;
    let laddr = local.local_addr().ok_or("no addr")?;
    assert_eq!(
        roundtrip(laddr, b"through the tunnel").await?,
        b"through the tunnel"
    );
    let big: Vec<u8> = (0..16 * 1024).map(|i| (i % 251) as u8).collect();
    assert_eq!(roundtrip(laddr, &big).await?, big);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let stats = local.stats();
    assert_eq!(stats.accepted, 2);
    assert!(stats.tx >= big.len() as u64 && stats.rx >= big.len() as u64);

    let socks = Tunnel::open(
        ForwardSpec::Dynamic {
            bind: "127.0.0.1".into(),
            bind_port: 0,
        },
        link.handle(),
    )
    .await?;
    let saddr = socks.local_addr().ok_or("no addr")?;
    let mut s = TcpStream::connect(saddr).await?;
    s.write_all(&[5, 1, 0]).await?;
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await?;
    assert_eq!(hello, [5, 0]);
    let host = b"127.0.0.1";
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host);
    req.extend_from_slice(&echo.port().to_be_bytes());
    s.write_all(&req).await?;
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await?;
    assert_eq!(rep[1], 0);
    s.write_all(b"socks5").await?;
    let mut buf = [0u8; 6];
    s.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"socks5");

    let mut bad = TcpStream::connect(saddr).await?;
    bad.write_all(&[5, 1, 0, 5, 2, 0, 1, 127, 0, 0, 1, 0, 1])
        .await?;
    let mut rep = [0u8; 12];
    bad.read_exact(&mut rep).await?;
    assert_eq!(&rep[..2], &[5, 0]);
    assert_eq!(rep[3], 7);
    Ok(())
}

#[tokio::test]
async fn remote_forwarding() -> Res {
    let srv = TestServer::start("rfwd").await;
    let (echo, _task) = echo_server().await;
    let r = route(vec![hop(srv.addr, password())], vec![srv.host_public()]);
    let link = Link::spawn(
        move || {
            let r = r.clone();
            async move { connect(&r).await }
        },
        Backoff::default(),
    );
    let tunnel = Tunnel::open(
        ForwardSpec::Remote {
            bind: "127.0.0.1".into(),
            bind_port: 0,
            dest: echo.ip().to_string(),
            dest_port: echo.port(),
        },
        link.handle(),
    )
    .await?;
    let mut st = tunnel.state();
    let port = timeout(Duration::from_secs(5), async {
        loop {
            if let TunnelState::Forwarding { remote_port } = &*st.borrow_and_update() {
                return Ok(*remote_port);
            }
            if st.changed().await.is_err() {
                return Err("closed");
            }
        }
    })
    .await??;
    let back = roundtrip(SocketAddr::from(([127, 0, 0, 1], port)), b"reverse").await?;
    assert_eq!(back, b"reverse");
    Ok(())
}

#[tokio::test]
async fn link_heals_without_rebinding() -> Res {
    let srv = TestServer::start("phoenix").await;
    let addr = srv.addr;
    let host_key = srv.host_key.clone();
    let client_key = srv.client_key.clone();
    let (echo, _task) = echo_server().await;
    let r = route(vec![hop(addr, password())], vec![srv.host_public()]);
    let dials = Arc::new(AtomicU64::new(0));
    let counter = dials.clone();
    let backoff = Backoff {
        base: Duration::from_millis(50),
        cap: Duration::from_millis(200),
        jitter: 0.0,
        stable_after: Duration::from_secs(60),
    };
    let link = Link::spawn(
        move || {
            let r = r.clone();
            counter.fetch_add(1, Ordering::SeqCst);
            async move { connect(&r).await }
        },
        backoff,
    );
    let tunnel = Tunnel::open(
        ForwardSpec::Local {
            bind: "127.0.0.1".into(),
            bind_port: 0,
            dest: echo.ip().to_string(),
            dest_port: echo.port(),
        },
        link.handle(),
    )
    .await?;
    let laddr = tunnel.local_addr().ok_or("no addr")?;
    assert_eq!(roundtrip(laddr, b"before").await?, b"before");

    srv.kill().await;
    let mut state = link.state();
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(&*state.borrow_and_update(), LinkState::Backoff { .. }) {
                return;
            }
            if state.changed().await.is_err() {
                return;
            }
        }
    })
    .await?;

    let pending = tokio::spawn(async move { roundtrip(laddr, b"during outage").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let revived = TestServer::start_with("phoenix", addr, host_key, client_key).await;
    let healed = timeout(Duration::from_secs(10), pending).await???;
    assert_eq!(healed, b"during outage");
    assert_eq!(roundtrip(laddr, b"after").await?, b"after");
    assert_eq!(tunnel.local_addr(), Some(laddr));
    assert!(dials.load(Ordering::SeqCst) >= 2);
    assert!(matches!(&*link.state().borrow(), LinkState::Up { .. }));
    let mut st = link.state();
    let rtt = timeout(Duration::from_secs(8), async {
        loop {
            if let LinkState::Up {
                rtt_ms: Some(ms), ..
            } = &*st.borrow_and_update()
            {
                return *ms;
            }
            if st.changed().await.is_err() {
                return u64::MAX;
            }
        }
    })
    .await?;
    assert!(rtt < 1000, "loopback rtt {rtt}ms");
    drop(revived);
    Ok(())
}

#[tokio::test]
async fn agent_forwarding_exposes_only_assigned_key() -> Res {
    let srv = TestServer::start("agent").await;
    let mut r = route(
        vec![hop(srv.addr, Credential::Key(srv.client_key.clone()))],
        vec![srv.host_public()],
    );
    r.agent_forward = true;
    let sess = connect(&r).await?;
    let _pty = sess.open_pty(&PtyRequest::default()).await?;
    let ids = timeout(Duration::from_secs(5), srv.agent_identities())
        .await?
        .ok_or("agent probe failed")?;
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0], srv.client_key.public_key().to_bytes()?);

    let mut plain = route(vec![hop(srv.addr, password())], vec![srv.host_public()]);
    plain.agent_forward = true;
    let sess = connect(&plain).await?;
    let _pty = sess.open_pty(&PtyRequest::default()).await?;
    let ids = timeout(Duration::from_secs(5), srv.agent_identities())
        .await?
        .ok_or("agent probe failed")?;
    assert!(ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn sftp_transfers_and_atomic_edit() -> Res {
    let srv = TestServer::start("files").await;
    let sess = connect(&route(
        vec![hop(srv.addr, password())],
        vec![srv.host_public()],
    ))
    .await?;
    let sftp = sess.sftp().await?;
    assert_eq!(sftp.realpath("/a/../b/.").await?, "/b");

    let work = tempfile::tempdir()?;
    let src = work.path().join("payload.bin");
    let payload: Vec<u8> = (0..(24 * 1024 + 4321))
        .map(|i: u32| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    std::fs::write(&src, &payload)?;
    let last = Arc::new(AtomicU64::new(0));
    let seen = last.clone();
    let sent = sftp
        .upload(&src, "/payload.bin", &move |p| {
            seen.store(p.done, Ordering::SeqCst)
        })
        .await?;
    assert_eq!(sent, payload.len() as u64);
    assert_eq!(last.load(Ordering::SeqCst), payload.len() as u64);
    assert_eq!(std::fs::read(srv.root().join("payload.bin"))?, payload);

    let dst = work.path().join("back.bin");
    let got = sftp.download("/payload.bin", &dst, &|_| {}).await?;
    assert_eq!(got, payload.len() as u64);
    assert_eq!(std::fs::read(&dst)?, payload);
    assert!(!work.path().join("back.bin.citadel-part").exists());

    sftp.mkdir("/etc").await?;
    std::fs::write(srv.root().join("etc/app.conf"), b"old")?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            srv.root().join("etc/app.conf"),
            std::fs::Permissions::from_mode(0o640),
        )?;
    }
    sftp.write_atomic("/etc/app.conf", b"new contents").await?;
    assert_eq!(
        sftp.read_file("/etc/app.conf", 1024).await?,
        b"new contents"
    );
    assert_eq!(sftp.stat("/etc/app.conf").await?.mode & 0o777, 0o640);
    let names: Vec<String> = sftp
        .list("/etc")
        .await?
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["app.conf".to_owned()]);
    assert!(sftp.read_file("/payload.bin", 10).await.is_err());

    sftp.rename("/etc/app.conf", "/etc/app.bak").await?;
    assert!(!sftp.exists("/etc/app.conf").await?);
    sftp.remove("/etc/app.bak").await?;
    sftp.rmdir("/etc").await?;
    let listing: Vec<String> = sftp.list("/").await?.into_iter().map(|e| e.name).collect();
    assert_eq!(listing, vec!["payload.bin".to_owned()]);
    Ok(())
}

#[test]
fn backoff_is_capped_and_monotonic_without_jitter() {
    let b = Backoff {
        base: Duration::from_millis(100),
        cap: Duration::from_secs(5),
        jitter: 0.0,
        stable_after: Duration::from_secs(60),
    };
    let delays: Vec<_> = (1..=10).map(|a| b.delay(a)).collect();
    assert_eq!(delays[0], Duration::from_millis(100));
    assert_eq!(delays[1], Duration::from_millis(200));
    assert!(delays.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(*delays.last().unwrap(), Duration::from_secs(5));
    let j = Backoff { jitter: 0.2, ..b };
    for a in 1..20 {
        assert!(j.delay(a) <= Duration::from_secs(5));
    }
    assert_eq!(b.delay(u32::MAX), Duration::from_secs(5));
}

#[test]
fn keygen_roundtrip() -> Res {
    let (pem, info) = citadel_ssh::generate(citadel_ssh::KeyKind::Ed25519, "ops@citadel")?;
    let key = citadel_ssh::decode_key(&pem, None)?;
    assert_eq!(citadel_ssh::inspect(&key)?, info);
    assert!(info.public_key.starts_with("ssh-ed25519 "));
    assert!(info.fingerprint.starts_with("SHA256:"));
    Ok(())
}

#[test]
fn key_classification() -> Res {
    let (_, ed) = citadel_ssh::generate(citadel_ssh::KeyKind::Ed25519, "x")?;
    assert_eq!(
        citadel_ssh::classify(&ed.public_key)?,
        (citadel_ssh::KeyFamily::Ed25519, 256)
    );
    let (_, ec) = citadel_ssh::generate(citadel_ssh::KeyKind::EcdsaP256, "x")?;
    assert_eq!(
        citadel_ssh::classify(&ec.public_key)?,
        (citadel_ssh::KeyFamily::Ecdsa, 256)
    );
    let rsa = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQDL1u1lTx2l4HXWTHxdmQZD3qTUOS1G3WLxKVZaBfXYcbH7mrgFp4hKQ7z1gqDfeO1ZQ4NnVMKvDXmrG8bQf8mOM9w9P4qNqBf3XZJpWwF0E1ytvP1EtvHkCgGq5BS1AwZ5W9e4TkTz0jE8ABwnSAP1kRiOlOtXsoaVv4vBc8+lWQ== weak";
    assert_eq!(
        citadel_ssh::classify(rsa)?,
        (citadel_ssh::KeyFamily::Rsa, 1024)
    );
    assert!(citadel_ssh::classify("not a key").is_err());
    Ok(())
}

struct Answer(&'static str);

impl citadel_ssh::Prompter for Answer {
    fn prompt(
        &self,
        req: citadel_ssh::PromptRequest,
    ) -> BoxFuture<'static, Option<Vec<SecretString>>> {
        let n = req.prompts.len();
        let a = self.0;
        Box::pin(async move { Some(vec![SecretString::from(a); n]) })
    }
}

#[tokio::test]
async fn key_then_code_second_factor() -> Res {
    let srv = TestServer::start("mfa").await;
    srv.set_challenge(citadel_testkit::Challenge::KeyThenCode("424242".into()));
    let cred = Credential::Key(srv.client_key.clone());
    let mut r = route(vec![hop(srv.addr, cred.clone())], vec![srv.host_public()]);
    let err = connect(&r)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("accepted") && err.contains("keyboard-interactive"),
        "{err}"
    );
    r.prompter = Some(Arc::new(Answer("424242")));
    let sess = connect(&r).await?;
    assert_eq!(sess.exec("whoami").await?.stdout, b"mfa\n");
    r.prompter = Some(Arc::new(Answer("000000")));
    assert!(matches!(
        connect(&r).await.err(),
        Some(TransportError::AuthFailed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn password_is_retried_over_keyboard_interactive() -> Res {
    let srv = TestServer::start("pam").await;
    srv.set_challenge(citadel_testkit::Challenge::PasswordViaKeyboard);
    let sess = connect(&route(
        vec![hop(srv.addr, password())],
        vec![srv.host_public()],
    ))
    .await?;
    assert_eq!(sess.exec("whoami").await?.stdout, b"pam\n");
    Ok(())
}

#[tokio::test]
async fn auto_credential_falls_back_to_identity_files() -> Res {
    use russh::keys::ssh_key::LineEnding;
    let srv = TestServer::start("files").await;
    let dir = tempfile::tempdir()?;
    let plain = dir.path().join("id_ed25519");
    std::fs::write(
        &plain,
        srv.client_key.to_openssh(LineEnding::LF)?.as_bytes(),
    )?;
    let missing = dir.path().join("id_rsa");
    let cred = Credential::Auto {
        endpoint: citadel_ssh::AgentEndpoint::Unix(dir.path().join("no-agent.sock")),
        files: vec![missing.clone(), plain],
    };
    let sess = connect(&route(vec![hop(srv.addr, cred)], vec![srv.host_public()])).await?;
    assert_eq!(sess.exec("whoami").await?.stdout, b"files\n");

    let locked = dir.path().join("id_ecdsa");
    let enc = srv
        .client_key
        .encrypt(&mut rand::rng(), "hunter22")?
        .to_openssh(LineEnding::LF)?;
    std::fs::write(&locked, enc.as_bytes())?;
    let cred = Credential::Auto {
        endpoint: citadel_ssh::AgentEndpoint::Unix(dir.path().join("no-agent.sock")),
        files: vec![missing, locked],
    };
    let mut r = route(vec![hop(srv.addr, cred)], vec![srv.host_public()]);
    let err = connect(&r)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("needs a passphrase"), "{err}");
    r.prompter = Some(Arc::new(Answer("hunter22")));
    let sess = connect(&r).await?;
    assert_eq!(sess.exec("whoami").await?.stdout, b"files\n");
    Ok(())
}
