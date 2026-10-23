use citadel_ssh::{KeyFamily, classify};
use citadel_vault::{
    Auth, Forward, HostProfile, IdentityKey, KdfParams, SyncBackend, SyncTarget, TunnelSpec,
};
use serde::Serialize;
use tauri::State;

use super::{AppState, blocking};
use crate::error::{AppError, CmdResult};
use crate::grants::Access;

const MAX_RECORDING: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    fn weight(self) -> u32 {
        match self {
            Self::Info => 0,
            Self::Low => 3,
            Self::Medium => 8,
            Self::High => 18,
            Self::Critical => 35,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub area: &'static str,
    pub title: String,
    pub detail: String,
    pub subject: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub score: u32,
    pub grade: char,
    pub checked: usize,
    pub findings: Vec<Finding>,
}

fn finding(
    severity: Severity,
    area: &'static str,
    title: String,
    detail: &str,
    subject: Option<String>,
) -> Finding {
    Finding {
        severity,
        area,
        title,
        detail: detail.to_owned(),
        subject,
    }
}

fn exposed(host: &str) -> bool {
    matches!(host.trim(), "0.0.0.0" | "::" | "*" | "[::]" | "")
}

pub fn audit(
    hosts: &[HostProfile],
    keys: &[IdentityKey],
    tunnels: &[TunnelSpec],
    targets: &[SyncTarget],
    kdf: KdfParams,
    autolock_minutes: u32,
) -> Report {
    let mut out = Vec::new();
    for k in keys {
        let used = hosts
            .iter()
            .any(|h| matches!(h.auth, Auth::Identity(id) if id == k.id));
        match classify(&k.public_key) {
            Ok((KeyFamily::Dsa, _)) => out.push(finding(
                Severity::Critical,
                "keys",
                format!("{} is a DSA key", k.label),
                "DSA is limited to 1024 bits and disabled by default in OpenSSH. Replace it with Ed25519.",
                Some(k.id.to_string()),
            )),
            Ok((KeyFamily::Rsa, bits)) if bits < 2048 => out.push(finding(
                Severity::Critical,
                "keys",
                format!("{} is RSA-{bits}", k.label),
                "RSA below 2048 bits is factorable with public resources. Rotate to Ed25519.",
                Some(k.id.to_string()),
            )),
            Ok((KeyFamily::Rsa, bits)) if bits < 3072 => out.push(finding(
                Severity::Medium,
                "keys",
                format!("{} is RSA-{bits}", k.label),
                "NIST retires 2048-bit RSA after 2030. Ed25519 is smaller and faster.",
                Some(k.id.to_string()),
            )),
            Err(_) => out.push(finding(
                Severity::Low,
                "keys",
                format!("{} has an unreadable public key", k.label),
                "The stored public key could not be parsed; re-import the identity.",
                Some(k.id.to_string()),
            )),
            _ => {}
        }
        if !used {
            out.push(finding(
                Severity::Info,
                "keys",
                format!("{} is not used by any host", k.label),
                "Unused keys widen the blast radius of a vault compromise. Delete keys you no longer need.",
                Some(k.id.to_string()),
            ));
        }
    }
    for h in hosts {
        match &h.auth {
            Auth::Password(_) => out.push(finding(
                Severity::Medium,
                "hosts",
                format!("{} authenticates with a password", h.label),
                "Passwords can be phished or brute-forced and are sent to the server. Switch to a key.",
                Some(h.id.to_string()),
            )),
            Auth::Agent { fingerprint: None } => out.push(finding(
                Severity::Low,
                "hosts",
                format!("{} offers every agent key", h.label),
                "The server learns every public key in your agent. Pin one fingerprint.",
                Some(h.id.to_string()),
            )),
            _ => {}
        }
        if h.agent_forward {
            out.push(finding(
                Severity::Medium,
                "hosts",
                format!("{} forwards the agent", h.label),
                "A root user on that host can sign with the forwarded key while you are connected. Citadel exposes only the host's own key; prefer a bastion chain over forwarding.",
                Some(h.id.to_string()),
            ));
        }
        if h.keepalive_secs == 0 {
            out.push(finding(
                Severity::Low,
                "hosts",
                format!("{} has heartbeats disabled", h.label),
                "Dead links are only noticed when the TCP stack gives up, and tunnels will not heal promptly.",
                Some(h.id.to_string()),
            ));
        }
    }
    for t in tunnels {
        let bind = match &t.forward {
            Forward::Local { bind, .. } | Forward::Dynamic { bind } => Some(bind),
            Forward::Remote { .. } => None,
        };
        if let Some(b) = bind.filter(|b| exposed(&b.host)) {
            let what = if matches!(t.forward, Forward::Dynamic { .. }) {
                "an open SOCKS proxy into your network"
            } else {
                "a forward any machine on your network can use"
            };
            out.push(finding(
                if matches!(t.forward, Forward::Dynamic { .. }) {
                    Severity::High
                } else {
                    Severity::Medium
                },
                "tunnels",
                format!("{} listens on {}:{}", t.label, b.host, b.port),
                &format!("This is {what}. Bind to 127.0.0.1 unless you mean to share it."),
                Some(t.id.to_string()),
            ));
        }
    }
    for s in targets {
        let insecure = match &s.backend {
            SyncBackend::S3 { endpoint, .. } => endpoint.starts_with("http://"),
            SyncBackend::WebDav { url, .. } => url.starts_with("http://"),
            SyncBackend::Git { remote, .. } => {
                remote.starts_with("http://") || remote.starts_with("git://")
            }
        };
        if insecure {
            out.push(finding(
                Severity::High,
                "sync",
                format!("{} syncs over plaintext", s.label),
                "The vault blob stays encrypted, but credentials and access patterns are exposed in transit. Use https.",
                Some(s.id.to_string()),
            ));
        }
    }
    if autolock_minutes == 0 {
        out.push(finding(
            Severity::Medium,
            "vault",
            "Auto-lock is disabled".into(),
            "An unattended unlocked session keeps the data key in memory indefinitely.",
            None,
        ));
    } else if autolock_minutes > 60 {
        out.push(finding(
            Severity::Low,
            "vault",
            format!("Auto-lock waits {autolock_minutes} minutes"),
            "Shorter windows reduce exposure if the machine is left unlocked.",
            None,
        ));
    }
    let rec = KdfParams::RECOMMENDED;
    if kdf.m_kib < rec.m_kib || kdf.t < rec.t {
        out.push(finding(
            Severity::Medium,
            "vault",
            format!(
                "Key derivation below recommended cost (m={}KiB t={})",
                kdf.m_kib, kdf.t
            ),
            "Change the master password to re-wrap the data key with Argon2id m=64MiB t=3 p=4.",
            None,
        ));
    }
    out.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.title.cmp(&b.title))
    });
    let penalty: u32 = out.iter().map(|f| f.severity.weight()).sum();
    let score = 100u32.saturating_sub(penalty);
    let grade = match score {
        95.. => 'A',
        85..=94 => 'B',
        70..=84 => 'C',
        50..=69 => 'D',
        _ => 'F',
    };
    Report {
        score,
        grade,
        checked: hosts.len() + keys.len() + tunnels.len() + targets.len() + 2,
        findings: out,
    }
}

#[tauri::command]
pub async fn vault_audit(state: State<'_, AppState>) -> CmdResult<Report> {
    let core = state.0.clone();
    let minutes = core.prefs().autolock_minutes;
    blocking(move || {
        let v = &core.vault;
        Ok(audit(
            &v.list::<HostProfile>()?,
            &v.list::<IdentityKey>()?,
            &v.list::<TunnelSpec>()?,
            &v.list::<SyncTarget>()?,
            v.kdf_params()?,
            minutes,
        ))
    })
    .await
}

#[tauri::command]
pub async fn recording_save(
    state: State<'_, AppState>,
    target: String,
    content: String,
) -> CmdResult<u64> {
    let mut path = state.0.grants.redeem(&target, Access::Write)?;
    if path.extension().and_then(|e| e.to_str()) != Some("cast") {
        path.as_mut_os_string().push(".cast");
    }
    if content.len() > MAX_RECORDING {
        return Err(AppError::invalid("recording exceeds 64 MiB"));
    }
    if !content.starts_with("{\"version\":2") {
        return Err(AppError::invalid("not an asciicast v2 document"));
    }
    blocking(move || {
        let parent = path
            .parent()
            .filter(|p| p.is_dir())
            .ok_or(AppError::NotFound("directory"))?;
        let tmp = parent.join(format!(
            ".{}.part",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("recording")
        ));
        crate::safety::write_private(&tmp, content.as_bytes())?;
        std::fs::rename(&tmp, &path)?;
        Ok(content.len() as u64)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_vault::{Endpoint, Id, KeyMaterial, SecretString};

    fn key(label: &str, public: &str) -> IdentityKey {
        IdentityKey {
            id: Id::random().unwrap(),
            label: label.into(),
            algorithm: String::new(),
            public_key: public.into(),
            fingerprint: String::new(),
            material: KeyMaterial::Agent,
            updated_ms: 0,
        }
    }

    #[test]
    fn clean_vault_scores_a() {
        let (_, info) = citadel_ssh::generate(citadel_ssh::KeyKind::Ed25519, "x").unwrap();
        let k = key("deploy", &info.public_key);
        let h = HostProfile::new("web", "web", "ops", Auth::Identity(k.id)).unwrap();
        let r = audit(&[h], &[k], &[], &[], KdfParams::RECOMMENDED, 10);
        assert!(r.findings.is_empty(), "{:?}", r.findings);
        assert_eq!((r.score, r.grade), (100, 'A'));
    }

    #[test]
    fn risky_vault_is_flagged_and_ranked() {
        let mut h =
            HostProfile::new("db", "db", "root", Auth::Password(SecretString::from("x"))).unwrap();
        h.agent_forward = true;
        h.keepalive_secs = 0;
        let mut socks = TunnelSpec::new(
            "proxy",
            h.id,
            Forward::Dynamic {
                bind: Endpoint::new("0.0.0.0", 1080),
            },
        )
        .unwrap();
        socks.autostart = true;
        let weak = KdfParams {
            m_kib: 8192,
            t: 1,
            p: 1,
        };
        let r = audit(&[h], &[], &[socks], &[], weak, 0);
        let sev: Vec<Severity> = r.findings.iter().map(|f| f.severity).collect();
        assert_eq!(sev.first(), Some(&Severity::High));
        assert!(sev.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(r.findings.len(), 6);
        assert!(r.score < 50 && r.grade == 'F', "{} {}", r.score, r.grade);
    }
}
