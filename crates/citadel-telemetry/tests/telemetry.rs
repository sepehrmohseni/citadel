#![allow(clippy::print_stdout)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use citadel_ssh::{Credential, Hop, HostKeyVerifier, PublicKey, Route, Session, connect};
use citadel_telemetry::{Docker, TelemetryError, monitor, snapshot};
use citadel_testkit::{ExecScript, OpenSsh, TestServer};
use futures::future::BoxFuture;
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

async fn dial(
    addr: std::net::SocketAddr,
    user: &str,
    key: Arc<citadel_ssh::PrivateKey>,
    host: PublicKey,
) -> citadel_ssh::Result<Arc<Session>> {
    let hop = Hop {
        host: "127.0.0.1".into(),
        port: addr.port(),
        user: user.to_owned(),
        credential: Credential::Key(key),
        legacy: false,
    };
    Box::pin(connect(&Route::direct(hop, Arc::new(Pinned(host))))).await
}

async fn mock(
    script: ExecScript,
) -> Result<(TestServer, Arc<Session>, Arc<Mutex<Vec<String>>>), citadel_ssh::TransportError> {
    let srv = TestServer::start("probe").await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    srv.set_exec(Arc::new(move |cmd: &str| {
        log.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(cmd.to_owned());
        script.clone()
    }));
    let sess = dial(
        srv.addr,
        citadel_testkit::USER,
        srv.client_key.clone(),
        srv.host_public(),
    )
    .await?;
    Ok((srv, sess, seen))
}

fn frame(kind: &str, up: f64, busy: u64, idle: u64, rx: u64) -> String {
    let mut s = format!(
        "@@citadel:frame {kind}\n@@citadel:date\n\n@@citadel:uptime\n{up:.2} 0.00\n@@citadel:stat\ncpu  {busy} 0 0 {idle} 0 0 0 0 0 0\ncpu0 {busy} 0 0 {idle} 0 0 0 0 0 0\n@@citadel:net\n  eth0: {rx} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
    );
    if kind == "full" {
        s += "@@citadel:hostname\nmock-host\n@@citadel:kernel\nLinux 6.1.0\n@@citadel:nproc\n1\n@@citadel:mem\nMemTotal: 2048 kB\nMemAvailable: 1024 kB\n";
        s += "@@citadel:docker api\n[{\"Id\":\"abcdef0123456789\",\"Names\":[\"/api\"],\"Image\":\"app:1\",\"State\":\"running\",\"Status\":\"Up 1 minute\"}]\n";
    }
    s + "@@citadel:end\n"
}

fn chunked(text: &str, size: usize) -> Vec<Vec<u8>> {
    text.as_bytes().chunks(size).map(<[u8]>::to_vec).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn monitor_streams_split_frames_then_reports_exit() -> Res {
    let text = format!(
        "garbage before frames\n{}{}{}{}",
        frame("prime", 10.0, 100, 900, 0),
        frame("full", 11.0, 150, 950, 1000),
        frame("full", 12.0, 250, 950, 3000),
        frame("full", 13.0, 250, 1050, 3000),
    );
    let script = ExecScript {
        frames: chunked(&text, 37),
        gap: Duration::from_millis(2),
        status: 0,
        stderr: Vec::new(),
    };
    let (_srv, sess, seen) = mock(script).await?;
    let mut mon = monitor(sess, Duration::from_millis(500));
    let mut busy = Vec::new();
    let mut rx = Vec::new();
    for _ in 0..3 {
        let snap = timeout(Duration::from_secs(5), mon.next())
            .await?
            .ok_or("closed early")??;
        assert_eq!(snap.host.hostname, "mock-host");
        assert_eq!(snap.host.cores, 1);
        assert_eq!(snap.mem.used_pct, 50.0);
        let Docker::Available { containers } = &snap.docker else {
            return Err(format!("{:?}", snap.docker).into());
        };
        assert_eq!(containers[0].name, "api");
        assert_eq!(containers[0].id, "abcdef012345");
        busy.push(snap.cpu.busy_pct);
        rx.push(snap.net[0].rx_bps);
    }
    assert_eq!(busy, vec![50.0, 100.0, 0.0]);
    assert_eq!(rx, vec![1000.0, 2000.0, 0.0]);
    let tail = timeout(Duration::from_secs(5), mon.next()).await?;
    assert!(
        matches!(
            tail,
            Some(Err(TelemetryError::Ended {
                status: Some(0),
                ..
            }))
        ),
        "{tail:?}"
    );
    assert!(timeout(Duration::from_secs(5), mon.next()).await?.is_none());
    let cmds = seen.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].starts_with("exec sh -c '"));
    assert!(cmds[0].contains("while :;do emit full;sleep 0.500"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_closes_the_channel_promptly() -> Res {
    let mut frames = vec![frame("prime", 1.0, 0, 0, 0).into_bytes()];
    frames.extend(
        (0..200u32).map(|i| frame("full", 2.0 + f64::from(i), u64::from(i), 100, 0).into_bytes()),
    );
    let script = ExecScript {
        frames,
        gap: Duration::from_millis(20),
        status: 0,
        stderr: Vec::new(),
    };
    let (_srv, sess, _) = mock(script).await?;
    let mut mon = monitor(sess, Duration::from_secs(1));
    timeout(Duration::from_secs(5), mon.next())
        .await?
        .ok_or("no frame")??;
    mon.stop();
    assert!(mon.is_stopped());
    let mut extra = 0;
    let drained = timeout(Duration::from_secs(2), async {
        while mon.next().await.is_some() {
            extra += 1;
        }
    })
    .await;
    assert!(drained.is_ok(), "monitor kept running after stop");
    assert!(extra < 20, "{extra} frames after stop");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_uses_in_exec_prime_sample() -> Res {
    let text = frame("prime", 5.0, 0, 0, 0) + &frame("full", 5.25, 30, 70, 2500);
    let script = ExecScript {
        frames: chunked(&text, 11),
        gap: Duration::ZERO,
        status: 0,
        stderr: Vec::new(),
    };
    let (_srv, sess, seen) = mock(script).await?;
    let snap = snapshot(&sess).await?;
    assert_eq!(snap.cpu.busy_pct, 30.0);
    assert_eq!(snap.net[0].rx_bps, 10_000.0);
    assert!(snap.at_ms > 0);
    let cmds = seen.lock().map_err(|_| "poisoned")?.clone();
    assert!(cmds[0].contains("emit prime;sleep 0.250"));
    assert!(!cmds[0].contains("while"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_without_frames_reports_stderr() -> Res {
    let script = ExecScript {
        frames: vec![b"sh: not found\n".to_vec()],
        gap: Duration::ZERO,
        status: 127,
        stderr: b"exec: sh: not found\n".to_vec(),
    };
    let (_srv, sess, _) = mock(script).await?;
    match snapshot(&sess).await {
        Err(TelemetryError::Ended { status, stderr }) => {
            assert_eq!(status, Some(127));
            assert_eq!(stderr, "exec: sh: not found");
        }
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn non_linux_host_is_reported_once() -> Res {
    let bsd = "@@citadel:frame prime\n@@citadel:end\n@@citadel:frame full\n@@citadel:kernel\nFreeBSD 14.1-RELEASE\n@@citadel:end\n".repeat(3);
    let script = ExecScript {
        frames: vec![bsd.into_bytes()],
        gap: Duration::ZERO,
        status: 0,
        stderr: Vec::new(),
    };
    let (_srv, sess, _) = mock(script).await?;
    let mut mon = monitor(sess, Duration::from_secs(1));
    let first = timeout(Duration::from_secs(5), mon.next()).await?;
    assert!(
        matches!(first, Some(Err(TelemetryError::Unsupported { ref kernel })) if kernel == "FreeBSD 14.1-RELEASE"),
        "{first:?}"
    );
    assert!(timeout(Duration::from_secs(5), mon.next()).await?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn real_procfs_snapshot_and_monitor() -> Res {
    let Some(srv) = OpenSsh::spawn() else {
        println!("skipping: no sshd (set CITADEL_SSHD to an OpenSSH sshd binary)");
        return Ok(());
    };
    let sess = dial(
        srv.addr,
        &srv.user,
        srv.client_key.clone(),
        srv.host_key.clone(),
    )
    .await?;
    let snap = snapshot(&sess).await?;
    println!("{}", serde_json::to_string(&snap)?);
    assert!(
        snap.host.kernel.starts_with("Linux "),
        "{}",
        snap.host.kernel
    );
    assert!(!snap.host.hostname.is_empty());
    assert!(snap.host.cores >= 1);
    assert!(snap.host.uptime_secs > 0);
    assert!(!snap.cpu.per_core.is_empty());
    assert!((0.0..=100.0).contains(&snap.cpu.busy_pct));
    assert!(snap.mem.total_kib > 0 && snap.mem.available_kib <= snap.mem.total_kib);
    assert!(
        snap.disks
            .iter()
            .all(|d| d.total_kib > 0 && d.fs != "tmpfs")
    );
    assert!(snap.net.iter().all(|n| n.name != "lo"));
    assert!(!matches!(snap.docker, Docker::Unknown));

    let mut mon = monitor(sess, Duration::from_millis(300));
    let mut last = 0;
    for _ in 0..3 {
        let s = timeout(Duration::from_secs(10), mon.next())
            .await?
            .ok_or("monitor closed")??;
        assert!(s.at_ms >= last);
        last = s.at_ms;
        assert!((0.0..=100.0).contains(&s.cpu.busy_pct));
        assert!(s.cpu.per_core.iter().all(|p| (0.0..=100.0).contains(p)));
        assert!(
            s.net
                .iter()
                .all(|n| n.rx_bps.is_finite() && n.rx_bps >= 0.0)
        );
    }
    mon.stop();
    Ok(())
}
