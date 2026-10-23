use std::net::{SocketAddr, TcpListener as StdListener, TcpStream as StdStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::keys::ssh_key::LineEnding;
use russh::keys::{PrivateKey, PublicKey};

use crate::random_key;

pub struct OpenSsh {
    pub addr: SocketAddr,
    pub user: String,
    pub client_key: Arc<PrivateKey>,
    pub host_key: PublicKey,
    pub dir: tempfile::TempDir,
    child: Child,
}

fn locate(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(fallback)))
        .filter(|p| p.is_file())
}

fn write_secret(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).expect("write key");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
}

impl OpenSsh {
    pub fn spawn() -> Option<Self> {
        Self::spawn_with(random_key(), "")
    }

    pub fn spawn_with(client: PrivateKey, extra: &str) -> Option<Self> {
        let sshd = locate("CITADEL_SSHD", "/usr/sbin/sshd")?;
        let sftp = locate("CITADEL_SFTP_SERVER", "/usr/lib/openssh/sftp-server");
        let user = std::env::var("USER").ok()?;
        let dir = tempfile::tempdir().ok()?;
        let host = random_key();
        let host_path = dir.path().join("host_ed25519");
        write_secret(&host_path, &host.to_openssh(LineEnding::LF).ok()?);
        let auth_path = dir.path().join("authorized_keys");
        write_secret(&auth_path, &client.public_key().to_openssh().ok()?);
        let port = StdListener::bind("127.0.0.1:0")
            .ok()?
            .local_addr()
            .ok()?
            .port();
        let mut cfg = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\n\
             StrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
             PubkeyAuthentication yes\nAllowTcpForwarding yes\nAllowAgentForwarding yes\n\
             GatewayPorts no\nMaxSessions 64\nMaxStartups 64\nLogLevel ERROR\n",
            host_path.display(),
            auth_path.display(),
            dir.path().join("sshd.pid").display(),
        );
        if let Some(sftp) = sftp {
            cfg.push_str(&format!("Subsystem sftp {}\n", sftp.display()));
        }
        cfg.push_str(extra);
        let cfg_path = dir.path().join("sshd_config");
        std::fs::write(&cfg_path, cfg).ok()?;
        let child = Command::new(&sshd)
            .arg("-D")
            .arg("-e")
            .arg("-f")
            .arg(&cfg_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(10);
        while StdStream::connect(addr).is_err() {
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Some(Self {
            addr,
            user,
            client_key: Arc::new(client),
            host_key: host.public_key().clone(),
            dir,
            child,
        })
    }
}

impl Drop for OpenSsh {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
