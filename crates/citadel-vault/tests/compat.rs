use std::path::{Path, PathBuf};

use citadel_vault::{
    Auth, Bundle, Endpoint, Entry, ExposeSecret, Forward, HostProfile, Id, IdentityKey, KdfParams,
    KeyMaterial, KnownHost, Point, SecretString, Snippet, TunnelSpec, Vault,
};
use tempfile::TempDir;

type Res = Result<(), Box<dyn std::error::Error>>;

const PASS: &str = "fixture-passphrase";
const KDF: KdfParams = KdfParams {
    m_kib: 1024,
    t: 1,
    p: 1,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn populate(v: &Vault) -> Result<(), citadel_vault::VaultError> {
    let mut id = IdentityKey {
        id: Id::random()?,
        label: "deploy".into(),
        algorithm: "ssh-ed25519".into(),
        public_key:
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM8NsbakFPOYFbRZgaLeaxze8ciZynJsvM6DU2Qlb2Cn"
                .into(),
        fingerprint: "SHA256:fixture".into(),
        material: KeyMaterial::Embedded {
            private_key: SecretString::from(
                "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n-----END OPENSSH PRIVATE KEY-----\n",
            ),
            passphrase: Some(SecretString::from("key-pass")),
        },
        updated_ms: 0,
    };
    v.put(&mut id)?;
    let mut bastion = HostProfile::new(
        "edge",
        "edge.example.net",
        "jump",
        Auth::Agent { fingerprint: None },
    )?;
    v.put(&mut bastion)?;
    let mut h = HostProfile::new("prod-db", "10.0.4.12", "ops", Auth::Identity(id.id))?;
    h.port = 2222;
    h.jump = vec![bastion.id];
    h.workspace = Some("production".into());
    h.tags = vec!["postgres".into(), "سلام".into()];
    h.keepalive_secs = 30;
    h.agent_forward = true;
    h.ssh_alias = Some("db".into());
    h.canvas = Some(Point { x: 12.5, y: -3.0 });
    v.put(&mut h)?;
    let mut pw = HostProfile::new(
        "legacy",
        "router.lan",
        "admin",
        Auth::Password(SecretString::from("router-pass")),
    )?;
    v.put(&mut pw)?;
    let mut t = TunnelSpec::new(
        "pg",
        h.id,
        Forward::Local {
            bind: Endpoint::new("127.0.0.1", 15432),
            dest: Endpoint::new("127.0.0.1", 5432),
        },
    )?;
    t.autostart = true;
    v.put(&mut t)?;
    let mut s = Snippet {
        id: Id::random()?,
        label: "disk".into(),
        body: "df -h".into(),
        updated_ms: 0,
    };
    v.put(&mut s)?;
    let mut k = KnownHost {
        id: Id::random()?,
        host: "10.0.4.12".into(),
        port: 2222,
        algorithm: "ssh-ed25519".into(),
        key: "AAAAC3NzaC1lZDI1NTE5AAAAIM8NsbakFPOYFbRZgaLeaxze8ciZynJsvM6DU2Qlb2Cn".into(),
        fingerprint: "SHA256:fixture".into(),
        updated_ms: 0,
    };
    v.put(&mut k)?;
    let gone = HostProfile::new("deleted", "gone.example.net", "x", Auth::Interactive)?;
    let mut gone = gone;
    v.put(&mut gone)?;
    v.remove(gone.id)?;
    Ok(())
}

fn verify(entries: &[Entry]) -> Res {
    let hosts: Vec<&HostProfile> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Host(h) => Some(h),
            _ => None,
        })
        .collect();
    assert_eq!(hosts.len(), 3);
    let db = hosts
        .iter()
        .find(|h| h.label == "prod-db")
        .ok_or("prod-db missing")?;
    assert_eq!(
        (db.hostname.as_str(), db.port, db.user.as_str()),
        ("10.0.4.12", 2222, "ops")
    );
    assert_eq!(db.tags, ["postgres", "سلام"]);
    assert_eq!(db.workspace.as_deref(), Some("production"));
    assert_eq!(db.ssh_alias.as_deref(), Some("db"));
    assert!(db.agent_forward && !db.legacy_crypto);
    assert_eq!(db.keepalive_secs, 30);
    assert_eq!(db.jump.len(), 1);
    assert!(matches!(db.auth, Auth::Identity(_)));
    let legacy = hosts
        .iter()
        .find(|h| h.label == "legacy")
        .ok_or("legacy missing")?;
    match &legacy.auth {
        Auth::Password(p) => assert_eq!(p.expose_secret(), "router-pass"),
        other => return Err(format!("unexpected auth {other:?}").into()),
    }
    let key = entries
        .iter()
        .find_map(|e| match e {
            Entry::Identity(k) => Some(k),
            _ => None,
        })
        .ok_or("identity missing")?;
    match &key.material {
        KeyMaterial::Embedded { passphrase, .. } => {
            assert_eq!(
                passphrase
                    .as_ref()
                    .map(|p| p.expose_secret().to_owned())
                    .as_deref(),
                Some("key-pass")
            );
        }
        KeyMaterial::Agent => return Err("expected embedded key".into()),
    }
    let tunnel = entries
        .iter()
        .find_map(|e| match e {
            Entry::Tunnel(t) => Some(t),
            _ => None,
        })
        .ok_or("tunnel missing")?;
    assert!(tunnel.autostart);
    assert!(matches!(tunnel.forward, Forward::Local { .. }));
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Snippet(s) if s.body == "df -h"))
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::KnownHost(k) if k.port == 2222))
    );
    assert!(entries.iter().any(Entry::is_tombstone));
    Ok(())
}

#[test]
fn write_fixtures_when_requested() -> Res {
    if std::env::var_os("CITADEL_WRITE_FIXTURES").is_none() {
        return Ok(());
    }
    let dir = fixtures();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("v1.redb");
    let _ = std::fs::remove_file(&path);
    let v = Vault::open(&path)?;
    v.initialize(&SecretString::from(PASS), KDF)?;
    populate(&v)?;
    std::fs::write(
        dir.join("v1-export.ctdl"),
        v.export(&SecretString::from(PASS), KDF)?,
    )?;
    std::fs::write(dir.join("v1-backup.ctdl"), v.sync_blob()?)?;
    Ok(())
}

#[test]
fn version_one_files_still_open() -> Res {
    let src = fixtures();
    let tmp = TempDir::new()?;
    let copy = tmp.path().join("v1.redb");
    std::fs::copy(src.join("v1.redb"), &copy)?;
    let pass = SecretString::from(PASS);

    let v = Vault::open(&copy)?;
    v.unlock(&pass)?;
    verify(&v.entries()?)?;
    assert_eq!(v.health()?.unreadable, 0);

    let bundle = Bundle::open(&std::fs::read(src.join("v1-export.ctdl"))?, &pass)?;
    verify(&bundle.entries)?;

    let fresh = Vault::open(tmp.path().join("restored.redb"))?;
    fresh.restore(&std::fs::read(src.join("v1-backup.ctdl"))?, &pass, false)?;
    verify(&fresh.entries()?)?;
    Ok(())
}
