use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SshConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Native,
    OnePassword,
    Bitwarden,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSocket {
    pub kind: AgentKind,
    pub path: String,
    pub available: bool,
}

pub const WINDOWS_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

pub fn is_pipe(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(r"\\.\pipe\") || lower.starts_with("//./pipe/")
}

pub fn discover() -> Vec<AgentSocket> {
    discover_with(None)
}

pub fn discover_with(cfg: Option<&SshConfig>) -> Vec<AgentSocket> {
    scan(
        std::env::var("SSH_AUTH_SOCK").ok(),
        dirs::home_dir().as_deref(),
        cfg,
        &|k| std::env::var(k).ok(),
    )
}

pub(crate) fn scan(
    env_sock: Option<String>,
    home: Option<&Path>,
    cfg: Option<&SshConfig>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<AgentSocket> {
    let mut found: Vec<(AgentKind, String)> = Vec::new();
    if let Some(sock) = env_sock.filter(|s| !s.trim().is_empty()) {
        found.push((classify(&sock), sock));
    }
    found.extend(platform(home));
    if let Some(cfg) = cfg {
        for args in cfg.directive_args("identityagent") {
            if let Some(p) = args.first().and_then(|v| expand(v, home, env)) {
                found.push((AgentKind::Custom, p));
            }
        }
    }
    let mut out: Vec<AgentSocket> = Vec::new();
    for (kind, path) in found {
        if out.iter().any(|s| s.path == path) {
            continue;
        }
        out.push(AgentSocket {
            kind,
            available: probe(&path),
            path,
        });
    }
    out
}

fn classify(path: &str) -> AgentKind {
    let lower = path.to_ascii_lowercase();
    if lower.contains("1password") {
        AgentKind::OnePassword
    } else if lower.contains("bitwarden") {
        AgentKind::Bitwarden
    } else {
        AgentKind::Native
    }
}

fn under(home: Option<&Path>, rel: &str) -> Option<String> {
    home.map(|h| h.join(rel).to_string_lossy().into_owned())
}

#[cfg(windows)]
fn platform(_home: Option<&Path>) -> Vec<(AgentKind, String)> {
    vec![(AgentKind::Native, WINDOWS_PIPE.to_owned())]
}

#[cfg(not(windows))]
fn platform(home: Option<&Path>) -> Vec<(AgentKind, String)> {
    let mut rels: Vec<(AgentKind, &str)> = Vec::new();
    if cfg!(target_os = "macos") {
        rels.push((
            AgentKind::OnePassword,
            "Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock",
        ));
    }
    rels.push((AgentKind::OnePassword, ".1password/agent.sock"));
    rels.push((AgentKind::Bitwarden, ".bitwarden-ssh-agent.sock"));
    if !cfg!(target_os = "macos") {
        rels.push((
            AgentKind::Bitwarden,
            ".var/app/com.bitwarden.desktop/data/.bitwarden-ssh-agent.sock",
        ));
        rels.push((
            AgentKind::Bitwarden,
            "snap/bitwarden/current/.bitwarden-ssh-agent.sock",
        ));
    }
    rels.into_iter()
        .filter_map(|(k, rel)| under(home, rel).map(|p| (k, p)))
        .collect()
}

fn expand(raw: &str, home: Option<&Path>, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return None;
    }
    if v == "SSH_AUTH_SOCK" {
        return env("SSH_AUTH_SOCK").filter(|s| !s.is_empty());
    }
    if let Some(name) = v.strip_prefix('$').filter(|n| !n.starts_with('{')) {
        return env(name).filter(|s| !s.is_empty());
    }
    let v = interpolate(v, env)?;
    if v == "~" {
        return home.map(|h| h.to_string_lossy().into_owned());
    }
    if let Some(rest) = v.strip_prefix("~/") {
        return under(home, rest);
    }
    Some(v)
}

fn interpolate(v: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        let close = tail.find('}')?;
        out.push_str(&env(&tail[..close]).filter(|s| !s.is_empty())?);
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn probe(path: &str) -> bool {
    if is_pipe(path) {
        pipe_available(path)
    } else {
        socket_available(&PathBuf::from(path))
    }
}

#[cfg(windows)]
fn pipe_available(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

#[cfg(not(windows))]
fn pipe_available(_path: &str) -> bool {
    false
}

#[cfg(unix)]
fn socket_available(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

#[cfg(not(unix))]
fn socket_available(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn pipes_are_recognized() {
        assert!(is_pipe(r"\\.\pipe\openssh-ssh-agent"));
        assert!(is_pipe(r"\\.\PIPE\custom"));
        assert!(is_pipe("//./pipe/agent"));
        assert!(!is_pipe("/tmp/agent.sock"));
        assert!(!is_pipe(r"C:\agent"));
    }

    #[test]
    fn env_socket_first_and_deduplicated() {
        let home = Path::new("/home/u");
        let one = "/home/u/.1password/agent.sock".to_owned();
        let got = scan(Some(one.clone()), Some(home), None, &no_env);
        assert_eq!(got[0].path, one);
        assert_eq!(got[0].kind, AgentKind::OnePassword);
        assert_eq!(got.iter().filter(|s| s.path == one).count(), 1);
        assert!(got.iter().all(|s| !s.available));
    }

    #[test]
    fn identity_agent_values_become_custom_entries() {
        let cfg = SshConfig::parse(
            "Host a\n  IdentityAgent ~/.custom/agent.sock\nHost b\n  IdentityAgent none\nHost c\n  IdentityAgent ${XDG_RUNTIME_DIR}/gcr/ssh\nHost d\n  IdentityAgent SSH_AUTH_SOCK\n",
        );
        let env = |k: &str| match k {
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_owned()),
            "SSH_AUTH_SOCK" => Some("/tmp/native.sock".to_owned()),
            _ => None,
        };
        let got = scan(
            Some("/tmp/native.sock".into()),
            Some(Path::new("/home/u")),
            Some(&cfg),
            &env,
        );
        let custom: Vec<&str> = got
            .iter()
            .filter(|s| s.kind == AgentKind::Custom)
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(
            custom,
            vec!["/home/u/.custom/agent.sock", "/run/user/1000/gcr/ssh"]
        );
        assert_eq!(got[0].kind, AgentKind::Native);
        assert_eq!(
            got.iter().filter(|s| s.path == "/tmp/native.sock").count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn live_unix_socket_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("agent.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let plain = dir.path().join("file");
        std::fs::write(&plain, b"x").unwrap();
        let got = scan(
            Some(sock.to_string_lossy().into_owned()),
            None,
            None,
            &no_env,
        );
        assert!(got[0].available);
        assert!(!probe(&plain.to_string_lossy()));
    }

    #[test]
    fn classify_by_vendor_path() {
        assert_eq!(
            classify("/x/Bitwarden/.bitwarden-ssh-agent.sock"),
            AgentKind::Bitwarden
        );
        assert_eq!(classify("/tmp/ssh-XYZ/agent.123"), AgentKind::Native);
    }
}
