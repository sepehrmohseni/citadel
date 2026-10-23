#![allow(clippy::unwrap_used, clippy::expect_used)]

mod openssh;
mod sftp;

pub use openssh::OpenSsh;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, Disconnect, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub const USER: &str = "ops";
pub const PASSWORD: &str = "correct horse";

#[derive(Clone, Debug, Default)]
pub struct ExecScript {
    pub frames: Vec<Vec<u8>>,
    pub gap: Duration,
    pub status: u32,
    pub stderr: Vec<u8>,
}

impl ExecScript {
    pub fn once(out: impl Into<Vec<u8>>, status: u32) -> Self {
        Self {
            frames: vec![out.into()],
            gap: Duration::ZERO,
            status,
            stderr: Vec::new(),
        }
    }
}

pub type ExecHook = Arc<dyn Fn(&str) -> ExecScript + Send + Sync>;

struct Shared {
    name: String,
    accepted_key: PublicKey,
    exec: Mutex<Option<ExecHook>>,
    handles: Mutex<Vec<server::Handle>>,
    root: PathBuf,
    pty_seen: Mutex<Vec<(String, u32, u32)>>,
    challenge: Mutex<Challenge>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Challenge {
    #[default]
    None,
    KeyThenCode(String),
    PasswordViaKeyboard,
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub host_key: PrivateKey,
    pub client_key: Arc<PrivateKey>,
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
    _root: Option<tempfile::TempDir>,
}

pub fn random_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("keygen")
}

impl TestServer {
    pub async fn start(name: &str) -> Self {
        Self::start_with(
            name,
            "127.0.0.1:0".parse().expect("addr"),
            random_key(),
            Arc::new(random_key()),
        )
        .await
    }

    pub async fn start_with(
        name: &str,
        addr: SocketAddr,
        host_key: PrivateKey,
        client_key: Arc<PrivateKey>,
    ) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let listener = bind_retry(addr).await;
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            name: name.to_owned(),
            accepted_key: client_key.public_key().clone(),
            exec: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            root: root.path().to_path_buf(),
            pty_seen: Mutex::new(Vec::new()),
            challenge: Mutex::new(Challenge::None),
        });
        let mut methods = MethodSet::empty();
        methods.push(MethodKind::Password);
        methods.push(MethodKind::PublicKey);
        methods.push(MethodKind::KeyboardInteractive);
        let config = Arc::new(server::Config {
            keys: vec![host_key.clone()],
            methods,
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            ..server::Config::default()
        });
        let accept = tokio::spawn(accept_loop(listener, config, shared.clone()));
        Self {
            addr,
            host_key,
            client_key,
            shared,
            accept,
            _root: Some(root),
        }
    }

    pub fn set_challenge(&self, c: Challenge) {
        *self.shared.challenge.lock().expect("challenge lock") = c;
    }

    pub fn host_public(&self) -> PublicKey {
        self.host_key.public_key().clone()
    }

    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    pub fn set_exec(&self, hook: ExecHook) {
        *self.shared.exec.lock().unwrap() = Some(hook);
    }

    pub fn ptys(&self) -> Vec<(String, u32, u32)> {
        self.shared.pty_seen.lock().unwrap().clone()
    }

    pub fn session_count(&self) -> usize {
        self.shared.handles.lock().unwrap().len()
    }

    pub async fn kill(self) {
        self.accept.abort();
        let handles: Vec<_> = self.shared.handles.lock().unwrap().drain(..).collect();
        for h in handles {
            let _ = h
                .disconnect(Disconnect::ByApplication, "shutdown".into(), "en".into())
                .await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    pub async fn agent_identities(&self) -> Option<Vec<Vec<u8>>> {
        let handle = self.shared.handles.lock().unwrap().last().cloned()?;
        let ch = handle.channel_open_agent().await.ok()?;
        let mut io = ch.into_stream();
        io.write_all(&[0, 0, 0, 1, 11]).await.ok()?;
        let mut len = [0u8; 4];
        io.read_exact(&mut len).await.ok()?;
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        io.read_exact(&mut body).await.ok()?;
        if body.first() != Some(&12) {
            return None;
        }
        let mut cur = &body[1..];
        let n = take_u32(&mut cur)?;
        let mut out = Vec::new();
        for _ in 0..n {
            let blob = take_string(&mut cur)?;
            take_string(&mut cur)?;
            out.push(blob);
        }
        Some(out)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

fn take_u32(cur: &mut &[u8]) -> Option<u32> {
    let (h, rest) = cur.split_first_chunk::<4>()?;
    *cur = rest;
    Some(u32::from_be_bytes(*h))
}

fn take_string(cur: &mut &[u8]) -> Option<Vec<u8>> {
    let len = take_u32(cur)? as usize;
    if cur.len() < len {
        return None;
    }
    let (s, rest) = cur.split_at(len);
    *cur = rest;
    Some(s.to_vec())
}

async fn bind_retry(addr: SocketAddr) -> TcpListener {
    for _ in 0..100 {
        if let Ok(l) = TcpListener::bind(addr).await {
            return l;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    TcpListener::bind(addr).await.expect("bind")
}

async fn accept_loop(listener: TcpListener, config: Arc<server::Config>, shared: Arc<Shared>) {
    while let Ok((sock, _)) = listener.accept().await {
        let handler = Conn {
            shared: shared.clone(),
            channels: HashMap::new(),
            shells: std::collections::HashSet::new(),
            forwards: HashMap::new(),
            key_ok: false,
        };
        match server::run_stream(config.clone(), sock, handler).await {
            Ok(running) => {
                shared.handles.lock().unwrap().push(running.handle());
                tokio::spawn(async move {
                    let _ = running.await;
                });
            }
            Err(_) => continue,
        }
    }
}

struct Conn {
    shared: Arc<Shared>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    shells: std::collections::HashSet<ChannelId>,
    forwards: HashMap<(String, u32), JoinHandle<()>>,
    key_ok: bool,
}

impl Conn {
    fn challenge(&self) -> Challenge {
        self.shared
            .challenge
            .lock()
            .expect("challenge lock")
            .clone()
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        for (_, t) in self.forwards.drain() {
            t.abort();
        }
    }
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if self.challenge() != Challenge::None {
            return Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: false,
            });
        }
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let (prompt, want) = match self.challenge() {
            Challenge::KeyThenCode(code) if self.key_ok => ("Verification code: ", code),
            Challenge::PasswordViaKeyboard => ("Password: ", PASSWORD.to_owned()),
            _ => return Ok(Auth::reject()),
        };
        let Some(mut answers) = response else {
            return Ok(Auth::Partial {
                name: "challenge".into(),
                instructions: "".into(),
                prompts: vec![(prompt.into(), false)].into(),
            });
        };
        let given = answers.next().map(|b| b.to_vec());
        Ok(
            if user == USER && given.as_deref() == Some(want.as_bytes()) {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(
            if user == USER && key.key_data() == self.shared.accepted_key.key_data() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let res = self.auth_publickey_offered(user, key).await?;
        if matches!(res, Auth::Accept) && matches!(self.challenge(), Challenge::KeyThenCode(_)) {
            self.key_ok = true;
            return Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: true,
            });
        }
        Ok(res)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _orig: &str,
        _orig_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Ok(port) = u16::try_from(port) else {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        match TcpStream::connect((host, port)).await {
            Ok(tcp) => {
                reply.accept().await;
                tokio::spawn(relay(tcp, channel));
            }
            Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shared
            .pty_seen
            .lock()
            .unwrap()
            .push((term.to_owned(), cols, rows));
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shared
            .pty_seen
            .lock()
            .unwrap()
            .push(("resize".into(), cols, rows));
        Ok(())
    }

    async fn agent_request(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        self.shells.insert(channel);
        let banner = format!("{}$ ", self.shared.name);
        session.data(channel, banner.into_bytes())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.shells.contains(&channel) {
            return Ok(());
        }
        if data == b"exit\r" {
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }
        session.data(channel, data.to_vec())?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let cmd = String::from_utf8_lossy(data).into_owned();
        let hook = self.shared.exec.lock().unwrap().clone();
        let script = match hook {
            Some(h) => h(&cmd),
            None => default_exec(&self.shared.name, &cmd),
        };
        let handle = session.handle();
        tokio::spawn(async move {
            for (i, frame) in script.frames.into_iter().enumerate() {
                if i > 0 && !script.gap.is_zero() {
                    tokio::time::sleep(script.gap).await;
                }
                if handle.data(channel, frame).await.is_err() {
                    return;
                }
            }
            if !script.stderr.is_empty() {
                let _ = handle.extended_data(channel, 1, script.stderr).await;
            }
            let _ = handle.exit_status_request(channel, script.status).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&channel)) {
            ("sftp", Some(ch)) => {
                session.channel_success(channel)?;
                let fs = sftp::Fs::new(self.shared.root.clone());
                tokio::spawn(russh_sftp::server::run(ch.into_stream(), fs));
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Ok(want) = u16::try_from(*port) else {
            return Ok(false);
        };
        let Ok(listener) = TcpListener::bind(("127.0.0.1", want)).await else {
            return Ok(false);
        };
        let Ok(local) = listener.local_addr() else {
            return Ok(false);
        };
        *port = u32::from(local.port());
        let handle = session.handle();
        let addr = address.to_owned();
        let bound = *port;
        let task = tokio::spawn(async move {
            while let Ok((sock, peer)) = listener.accept().await {
                let handle = handle.clone();
                let addr = addr.clone();
                tokio::spawn(async move {
                    let Ok(ch) = handle
                        .channel_open_forwarded_tcpip(
                            addr,
                            bound,
                            peer.ip().to_string(),
                            u32::from(peer.port()),
                        )
                        .await
                    else {
                        return;
                    };
                    relay(sock, ch).await;
                });
            }
        });
        self.forwards.insert((address.to_owned(), bound), task);
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(match self.forwards.remove(&(address.to_owned(), port)) {
            Some(t) => {
                t.abort();
                true
            }
            None => false,
        })
    }
}

async fn relay(tcp: TcpStream, ch: Channel<Msg>) {
    let (mut rd, mut wr) = tcp.into_split();
    let (mut ch_rd, ch_wr) = ch.split();
    let upstream = async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ch_wr.data_bytes(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ch_wr.eof().await;
        ch_wr
    };
    let downstream = async move {
        while let Some(msg) = ch_rd.wait().await {
            match msg {
                russh::ChannelMsg::Data { data } => {
                    if wr.write_all(&data).await.is_err() {
                        break;
                    }
                }
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = wr.shutdown().await;
    };
    let (ch_wr, ()) = tokio::join!(upstream, downstream);
    let _ = ch_wr.close().await;
}

fn default_exec(name: &str, cmd: &str) -> ExecScript {
    match cmd {
        "whoami" => ExecScript::once(format!("{name}\n"), 0),
        c if c.starts_with("echo ") => ExecScript::once(format!("{}\n", &c[5..]), 0),
        c if c.starts_with("exit ") => {
            ExecScript::once(Vec::new(), c[5..].trim().parse().unwrap_or(1))
        }
        "fail" => ExecScript {
            frames: Vec::new(),
            gap: Duration::ZERO,
            status: 2,
            stderr: b"boom\n".to_vec(),
        },
        _ => ExecScript::once(Vec::new(), 127),
    }
}

pub async fn echo_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
    let addr = listener.local_addr().expect("echo addr");
    let task = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = sock.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (addr, task)
}
