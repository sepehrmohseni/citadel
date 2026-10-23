#![allow(clippy::print_stdout)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use citadel_ssh::{
    Backoff, Credential, ForwardSpec, Hop, HostKeyVerifier, Link, PtyEvent, PtyRequest, PublicKey,
    Route, Tunnel, TunnelState, connect,
};
use citadel_testkit::{OpenSsh, echo_server};
use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

type Res = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct Pinned(PublicKey);

impl HostKeyVerifier for Pinned {
    fn verify<'a>(
        &'a self,
        _h: &'a str,
        _p: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, citadel_ssh::Result<bool>> {
        let ok = key.key_data() == self.0.key_data();
        Box::pin(async move { Ok(ok) })
    }
}

fn sshd() -> Option<OpenSsh> {
    let srv = OpenSsh::spawn();
    if srv.is_none() {
        println!("skipping: no sshd (set CITADEL_SSHD to an OpenSSH sshd binary)");
    }
    srv
}

fn route(srv: &OpenSsh, depth: usize) -> Route {
    let hop = Hop {
        host: "127.0.0.1".into(),
        port: srv.addr.port(),
        user: srv.user.clone(),
        credential: Credential::Key(srv.client_key.clone()),
        legacy: false,
    };
    let mut r = Route::direct(hop.clone(), Arc::new(Pinned(srv.host_key.clone())));
    r.hops = vec![hop; depth];
    r.connect_timeout = Duration::from_secs(10);
    r
}

fn pattern(n: usize, seed: u32) -> Vec<u8> {
    (0..n)
        .map(|i| ((i as u32).wrapping_mul(2654435761).wrapping_add(seed) >> 11) as u8)
        .collect()
}

async fn echo_through(addr: SocketAddr, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let (mut rd, mut wr) = TcpStream::connect(addr).await?.into_split();
    let len = payload.len();
    let writer = tokio::spawn(async move { wr.write_all(&payload).await.map(|()| wr) });
    let mut back = vec![0u8; len];
    rd.read_exact(&mut back).await?;
    let _ = writer.await;
    Ok(back)
}

async fn shell_line(
    pty: &mut citadel_ssh::Pty,
    needle: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut seen = String::new();
    timeout(Duration::from_secs(10), async {
        while !seen.contains(needle) {
            match pty.events.recv().await {
                Some(PtyEvent::Data(d)) => seen.push_str(&String::from_utf8_lossy(&d)),
                Some(PtyEvent::Closed) | None => break,
                _ => {}
            }
        }
    })
    .await?;
    Ok(seen)
}

#[tokio::test(flavor = "multi_thread")]
async fn real_exec_and_shell() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    let sess = connect(&route(&srv, 1)).await?;
    let out = sess.exec("printf citadel; echo oops >&2; exit 3").await?;
    assert_eq!(out.stdout, b"citadel");
    assert_eq!(out.stderr, b"oops\n");
    assert_eq!(out.status, Some(3));

    let mut pty = sess
        .open_pty(&PtyRequest {
            cols: 120,
            rows: 33,
            command: Some("/bin/sh".into()),
            ..PtyRequest::default()
        })
        .await?;
    pty.writer
        .write(&b"stty size; echo $((6*7))x\n"[..])
        .await?;
    let seen = shell_line(&mut pty, "42x").await?;
    assert!(seen.contains("33 120"), "{seen}");
    pty.writer.resize(90, 20).await?;
    pty.writer.write(&b"stty size; exit 0\n"[..]).await?;
    let seen = shell_line(&mut pty, "20 90").await?;
    assert!(seen.contains("20 90"), "{seen}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn real_bastion_chain_three_hops() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    let sess = connect(&route(&srv, 3)).await?;
    assert_eq!(sess.depth(), 2);
    let out = sess.exec("echo deep").await?;
    assert_eq!(out.stdout, b"deep\n");
    let (echo, _t) = echo_server().await;
    let mut ch = sess.direct_tcpip("127.0.0.1", echo.port()).await?;
    let payload = pattern(2 << 20, 7);
    let (mut rd, mut wr) = tokio::io::split(&mut ch);
    let send = payload.clone();
    let mut back = vec![0u8; payload.len()];
    let (w, r) = tokio::join!(wr.write_all(&send), rd.read_exact(&mut back));
    w?;
    r?;
    assert!(back == payload, "bastion stream corrupted");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn real_tunnels_are_byte_exact() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    let r = route(&srv, 1);
    let link = Link::spawn(
        move || {
            let r = r.clone();
            async move { connect(&r).await }
        },
        Backoff::default(),
    );
    let (echo, _t) = echo_server().await;
    let local = Tunnel::open(
        ForwardSpec::Local {
            bind: "127.0.0.1".into(),
            bind_port: 0,
            dest: "127.0.0.1".into(),
            dest_port: echo.port(),
        },
        link.handle(),
    )
    .await?;
    let laddr = local.local_addr().ok_or("no addr")?;
    let streams: Vec<_> = (0..4u32)
        .map(|seed| tokio::spawn(echo_through(laddr, pattern(4 << 20, seed))))
        .collect();
    for (seed, h) in streams.into_iter().enumerate() {
        let back = timeout(Duration::from_secs(30), h).await???;
        assert!(
            back == pattern(4 << 20, seed as u32),
            "stream {seed} corrupted"
        );
    }

    let socks = Tunnel::open(
        ForwardSpec::Dynamic {
            bind: "127.0.0.1".into(),
            bind_port: 0,
        },
        link.handle(),
    )
    .await?;
    let mut s = TcpStream::connect(socks.local_addr().ok_or("no addr")?).await?;
    s.write_all(&[5, 1, 0]).await?;
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await?;
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend_from_slice(&echo.port().to_be_bytes());
    s.write_all(&req).await?;
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await?;
    assert_eq!(rep[1], 0);
    let payload = pattern(1 << 20, 99);
    let (mut rd, mut wr) = s.into_split();
    let send = payload.clone();
    let writer = tokio::spawn(async move { wr.write_all(&send).await });
    let mut back = vec![0u8; payload.len()];
    rd.read_exact(&mut back).await?;
    writer.await??;
    assert!(back == payload);

    let remote = Tunnel::open(
        ForwardSpec::Remote {
            bind: "127.0.0.1".into(),
            bind_port: 0,
            dest: "127.0.0.1".into(),
            dest_port: echo.port(),
        },
        link.handle(),
    )
    .await?;
    let mut st = remote.state();
    let port = timeout(Duration::from_secs(10), async {
        loop {
            if let TunnelState::Forwarding { remote_port } = &*st.borrow_and_update() {
                return Some(*remote_port);
            }
            st.changed().await.ok()?;
        }
    })
    .await?
    .ok_or("remote forward never came up")?;
    let back = echo_through(
        SocketAddr::from(([127, 0, 0, 1], port)),
        pattern(1 << 20, 5),
    )
    .await?;
    assert!(back == pattern(1 << 20, 5));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn real_sftp_large_and_atomic() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    let sess = connect(&route(&srv, 1)).await?;
    let Ok(sftp) = sess.sftp().await else {
        println!("skipping: sshd has no sftp subsystem");
        return Ok(());
    };
    let work = tempfile::tempdir()?;
    let remote_dir = srv.dir.path().join("files");
    std::fs::create_dir(&remote_dir)?;
    let remote = remote_dir.join("blob.bin");
    let remote = remote.to_str().ok_or("path")?;
    let payload = pattern(16 << 20 | 1234, 3);
    let src = work.path().join("blob.bin");
    std::fs::write(&src, &payload)?;
    let t = std::time::Instant::now();
    assert_eq!(
        sftp.upload(&src, remote, &|_| {}).await?,
        payload.len() as u64
    );
    let up = t.elapsed();
    assert!(std::fs::read(remote)? == payload);
    let dst = work.path().join("back.bin");
    let t = std::time::Instant::now();
    assert_eq!(
        sftp.download(remote, &dst, &|_| {}).await?,
        payload.len() as u64
    );
    let down = t.elapsed();
    assert!(std::fs::read(&dst)? == payload);
    println!("sftp 16MiB up {up:?} down {down:?}");

    let conf = remote_dir.join("app.conf");
    std::fs::write(&conf, b"v1")?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&conf, std::fs::Permissions::from_mode(0o640))?;
    }
    let conf = conf.to_str().ok_or("path")?;
    sftp.write_atomic(conf, b"v2 with more bytes").await?;
    assert_eq!(std::fs::read(conf)?, b"v2 with more bytes");
    assert_eq!(sftp.stat(conf).await?.mode & 0o777, 0o640);
    let names: Vec<_> = sftp
        .list(remote_dir.to_str().ok_or("path")?)
        .await?
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["app.conf".to_owned(), "blob.bin".to_owned()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn real_agent_forwarding_is_isolated() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    if !std::path::Path::new("/usr/bin/ssh-add").exists() {
        println!("skipping: ssh-add not installed");
        return Ok(());
    }
    let mut r = route(&srv, 1);
    r.agent_forward = true;
    let sess = connect(&r).await?;
    let mut pty = sess
        .open_pty(&PtyRequest {
            command: Some("/bin/sh".into()),
            ..PtyRequest::default()
        })
        .await?;
    pty.writer
        .write(&b"ssh-add -L; echo done-$((40+2))\n"[..])
        .await?;
    let seen = shell_line(&mut pty, "done-42").await?;
    let expected = srv.client_key.public_key().to_openssh()?;
    let blob = expected.split_whitespace().nth(1).ok_or("blob")?;
    assert!(seen.contains(blob), "{seen}");
    assert_eq!(seen.matches("ssh-ed25519 ").count(), 1, "{seen}");
    Ok(())
}

const RSA_PEM: &str = include_str!("fixtures/rsa2048.pem");

fn keyed(srv: &OpenSsh, cred: Credential, legacy: bool) -> Route {
    let hop = Hop {
        host: "127.0.0.1".into(),
        port: srv.addr.port(),
        user: srv.user.clone(),
        credential: cred,
        legacy,
    };
    let mut r = Route::direct(hop, Arc::new(Pinned(srv.host_key.clone())));
    r.connect_timeout = Duration::from_secs(10);
    r
}

fn rsa() -> Result<Arc<citadel_ssh::PrivateKey>, citadel_ssh::TransportError> {
    citadel_ssh::decode_key(&secrecy::SecretString::from(RSA_PEM), None)
}

#[tokio::test]
async fn real_rsa_keys_negotiate_the_signature_hash() -> Res {
    for (accepted, label) in [
        ("", "server default (rsa-sha2-512)"),
        ("PubkeyAcceptedAlgorithms rsa-sha2-256\n", "sha2-256 only"),
        ("PubkeyAcceptedAlgorithms ssh-rsa\n", "legacy ssh-rsa only"),
    ] {
        let key = rsa()?;
        let Some(srv) = OpenSsh::spawn_with((*key).clone(), accepted) else {
            println!("skipping: no sshd");
            return Ok(());
        };
        let sess = connect(&keyed(&srv, Credential::Key(key), false))
            .await
            .map_err(|e| format!("{label}: {e}"))?;
        assert_eq!(sess.exec("printf ok").await?.stdout, b"ok", "{label}");
    }
    Ok(())
}

#[tokio::test]
async fn real_mangled_key_paste_still_authenticates() -> Res {
    let key = rsa()?;
    let Some(srv) = OpenSsh::spawn_with((*key).clone(), "") else {
        return Ok(());
    };
    let mangled: String = RSA_PEM.lines().map(|l| format!("  {l} \r\n")).collect();
    let key = citadel_ssh::decode_key(&secrecy::SecretString::from(mangled), None)?;
    let sess = connect(&keyed(&srv, Credential::Key(key), false)).await?;
    assert_eq!(sess.exec("printf ok").await?.stdout, b"ok");
    Ok(())
}

#[tokio::test]
async fn real_legacy_only_server_is_explained_then_reachable() -> Res {
    let legacy = "KexAlgorithms diffie-hellman-group14-sha1\nCiphers aes128-cbc\nMACs hmac-sha1\n";
    let Some(srv) = OpenSsh::spawn_with(citadel_testkit::random_key(), legacy) else {
        return Ok(());
    };
    let err = connect(&keyed(&srv, Credential::Key(srv.client_key.clone()), false))
        .await
        .err();
    match &err {
        Some(citadel_ssh::TransportError::Legacy { kind, offered, .. }) => {
            assert_eq!(kind, "key exchange");
            assert!(offered.contains("diffie-hellman-group14-sha1"), "{offered}");
        }
        other => return Err(format!("expected a legacy error, got {other:?}").into()),
    }
    let sess = connect(&keyed(&srv, Credential::Key(srv.client_key.clone()), true)).await?;
    assert_eq!(sess.exec("printf ok").await?.stdout, b"ok");
    Ok(())
}

#[tokio::test]
async fn real_auth_failure_explains_what_was_tried() -> Res {
    let Some(srv) = sshd() else { return Ok(()) };
    let stranger = Arc::new(citadel_testkit::random_key());
    let err = connect(&keyed(&srv, Credential::Key(stranger), false))
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("ssh-ed25519") && err.contains("rejected"),
        "{err}"
    );
    assert!(err.contains("server accepts publickey"), "{err}");
    Ok(())
}
