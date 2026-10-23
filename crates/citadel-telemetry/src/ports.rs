use std::collections::BTreeMap;

use serde::Serialize;

pub const LISTENERS_COMMAND: &str = "ss -Hltnp 2>/dev/null || netstat -ltnp 2>/dev/null";
const MAX_LINES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Address,
    Loopback,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Listener {
    pub port: u16,
    pub scope: Scope,
    pub address: String,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

fn scope_of(host: &str) -> Scope {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    let h = h.split('%').next().unwrap_or(h);
    match h {
        "0.0.0.0" | "*" | "::" | "" => Scope::Any,
        "::1" | "localhost" => Scope::Loopback,
        _ if h.starts_with("127.") || h.starts_with("::ffff:127.") => Scope::Loopback,
        _ => Scope::Address,
    }
}

fn split_endpoint(raw: &str) -> Option<(&str, u16)> {
    let (host, port) = raw.rsplit_once(':')?;
    Some((host, port.parse().ok()?))
}

fn process_of(line: &str) -> (Option<String>, Option<u32>) {
    if let Some(users) = line.split_once("users:((").map(|(_, r)| r) {
        let name = users
            .split('"')
            .nth(1)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(64).collect());
        let pid = users
            .split_once("pid=")
            .and_then(|(_, r)| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|p| p.parse().ok());
        return (name, pid);
    }
    let last = line.split_whitespace().last().unwrap_or("");
    match last.split_once('/') {
        Some((pid, name)) if pid.chars().all(|c| c.is_ascii_digit()) => {
            (Some(name.chars().take(64).collect()), pid.parse().ok())
        }
        _ => (None, None),
    }
}

fn local_token(line: &str) -> Option<&str> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let first = *words.first()?;
    let idx = if first.starts_with("tcp") || first == "LISTEN" {
        3
    } else {
        2
    };
    words.get(idx).copied()
}

pub fn parse_listeners(output: &str) -> Vec<Listener> {
    let mut by_port: BTreeMap<u16, Listener> = BTreeMap::new();
    for line in output.lines().take(MAX_LINES) {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Proto")
            || line.starts_with("Active")
            || line.starts_with("State")
        {
            continue;
        }
        if line.starts_with("tcp") && !line.contains("LISTEN") {
            continue;
        }
        let Some((host, port)) = local_token(line).and_then(split_endpoint) else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let scope = scope_of(host);
        let address = match scope {
            Scope::Any | Scope::Loopback => "127.0.0.1".to_owned(),
            Scope::Address => host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split('%')
                .next()
                .unwrap_or(host)
                .to_owned(),
        };
        let (process, pid) = process_of(line);
        let entry = by_port.entry(port).or_insert_with(|| Listener {
            port,
            scope,
            address: address.clone(),
            process: None,
            pid: None,
        });
        if scope > entry.scope {
            entry.scope = scope;
            entry.address = address;
        }
        if entry.process.is_none() {
            entry.process = process;
            entry.pid = pid;
        }
    }
    by_port.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SS: &str = "LISTEN 0      4096   127.0.0.53%lo:53        0.0.0.0:*\nLISTEN 0      128          0.0.0.0:22        0.0.0.0:*    users:((\"sshd\",pid=812,fd=3))\nLISTEN 0      511          127.0.0.1:5173    0.0.0.0:*    users:((\"node\",pid=4410,fd=21))\nLISTEN 0      4096               *:8080            *:*    users:((\"caddy\",pid=90,fd=7),(\"caddy\",pid=90,fd=8))\nLISTEN 0      128             [::]:22           [::]:*\nLISTEN 0      100      10.0.4.12:5432      0.0.0.0:*\n";

    const NETSTAT: &str = "Active Internet connections (only servers)\nProto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\ntcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      -\ntcp        0      0 127.0.0.1:6379          0.0.0.0:*               LISTEN      731/redis-server\ntcp6       0      0 :::3000                 :::*                    LISTEN      9001/node\n";

    #[test]
    fn ss_output_is_merged_per_port() {
        let l = parse_listeners(SS);
        let ports: Vec<u16> = l.iter().map(|x| x.port).collect();
        assert_eq!(ports, [22, 53, 5173, 5432, 8080]);
        let ssh = &l[0];
        assert_eq!(
            (ssh.scope, ssh.process.as_deref(), ssh.pid),
            (Scope::Any, Some("sshd"), Some(812))
        );
        assert_eq!(l[1].scope, Scope::Loopback);
        assert_eq!(
            (l[2].process.as_deref(), l[2].address.as_str()),
            (Some("node"), "127.0.0.1")
        );
        assert_eq!(
            (l[3].scope, l[3].address.as_str()),
            (Scope::Address, "10.0.4.12")
        );
        assert_eq!(
            (l[4].scope, l[4].process.as_deref()),
            (Scope::Any, Some("caddy"))
        );
    }

    #[test]
    fn netstat_output_is_understood() {
        let l = parse_listeners(NETSTAT);
        let got: Vec<(u16, Option<&str>)> =
            l.iter().map(|x| (x.port, x.process.as_deref())).collect();
        assert_eq!(
            got,
            [
                (22, None),
                (3000, Some("node")),
                (6379, Some("redis-server"))
            ]
        );
        assert_eq!(l[1].scope, Scope::Any);
    }

    #[test]
    fn garbage_is_ignored() {
        assert!(
            parse_listeners("bash: ss: command not found\n\0\0\nLISTEN x y z:notaport").is_empty()
        );
    }
}
