use std::sync::Arc;
use std::time::Duration;

use citadel_vault::{
    Auth, AutoLock, Bundle, Endpoint, Entry, ExposeSecret, Forward, HostProfile, Id, IdentityKey,
    ImportMode, KdfParams, KeyMaterial, LockState, SecretString, TunnelSpec, Vault, VaultError,
};
use tempfile::TempDir;

type Res = Result<(), Box<dyn std::error::Error>>;

const FAST: KdfParams = KdfParams {
    m_kib: 256,
    t: 1,
    p: 1,
};

fn pw(s: &str) -> SecretString {
    SecretString::from(s)
}

fn fresh(dir: &TempDir, name: &str, pass: &str) -> Result<Vault, VaultError> {
    let v = Vault::open(dir.path().join(name))?;
    v.initialize(&pw(pass), FAST)?;
    Ok(v)
}

fn host(label: &str) -> Result<HostProfile, VaultError> {
    HostProfile::new(
        label,
        &format!("{label}.example.net"),
        "ops",
        Auth::Interactive,
    )
}

#[test]
fn lifecycle_and_wrong_password() -> Res {
    let dir = TempDir::new()?;
    let path = dir.path().join("v.redb");
    {
        let v = Vault::open(&path)?;
        assert_eq!(v.state(), LockState::Uninitialized);
        v.initialize(&pw("correct horse"), FAST)?;
        assert!(v.is_unlocked());
        let mut h = host("alpha")?;
        v.put(&mut h)?;
        v.lock();
        assert_eq!(v.state(), LockState::Locked);
        assert!(matches!(v.list::<HostProfile>(), Err(VaultError::Locked)));
    }
    let v = Vault::open(&path)?;
    assert_eq!(v.state(), LockState::Locked);
    assert!(matches!(
        v.unlock(&pw("wrong")),
        Err(VaultError::BadPassword)
    ));
    v.unlock(&pw("correct horse"))?;
    let hosts = v.list::<HostProfile>()?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].label, "alpha");
    assert!(matches!(
        v.initialize(&pw("x"), FAST),
        Err(VaultError::AlreadyInitialized)
    ));
    Ok(())
}

#[test]
fn crud_and_tombstones() -> Res {
    let dir = TempDir::new()?;
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut a = host("a")?;
    let mut b = host("b")?;
    v.put(&mut a)?;
    v.put(&mut b)?;
    let first = a.updated_ms;
    a.port = 2222;
    v.put(&mut a)?;
    assert!(a.updated_ms > first);
    let got = v.get::<HostProfile>(a.id)?.ok_or("missing")?;
    assert_eq!(got.port, 2222);
    assert!(v.get::<IdentityKey>(a.id)?.is_none());
    assert!(v.remove(b.id)?);
    assert!(!v.remove(b.id)?);
    assert_eq!(v.list::<HostProfile>()?.len(), 1);
    let tombs: Vec<_> = v
        .entries()?
        .into_iter()
        .filter(Entry::is_tombstone)
        .collect();
    assert_eq!(tombs.len(), 1);
    assert_eq!(tombs[0].id(), b.id);
    Ok(())
}

#[test]
fn secrets_roundtrip() -> Res {
    let dir = TempDir::new()?;
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut key = IdentityKey {
        id: Id::random()?,
        label: "deploy".into(),
        algorithm: "ssh-ed25519".into(),
        public_key: "ssh-ed25519 AAAA deploy".into(),
        fingerprint: "SHA256:abc".into(),
        material: KeyMaterial::Embedded {
            private_key: pw("-----BEGIN OPENSSH PRIVATE KEY-----"),
            passphrase: Some(pw("hunter2")),
        },
        updated_ms: 0,
    };
    v.put(&mut key)?;
    let mut h = HostProfile::new("db", "db.internal", "root", Auth::Password(pw("s3cret")))?;
    v.put(&mut h)?;
    let back = v.get::<IdentityKey>(key.id)?.ok_or("missing")?;
    match &back.material {
        KeyMaterial::Embedded {
            private_key,
            passphrase,
        } => {
            assert_eq!(
                private_key.expose_secret(),
                "-----BEGIN OPENSSH PRIVATE KEY-----"
            );
            assert_eq!(
                passphrase.as_ref().map(|p| p.expose_secret()),
                Some("hunter2")
            );
        }
        KeyMaterial::Agent => return Err("wrong material".into()),
    }
    let h = v.get::<HostProfile>(h.id)?.ok_or("missing")?;
    assert!(matches!(&h.auth, Auth::Password(p) if p.expose_secret() == "s3cret"));
    let path = v.path().to_path_buf();
    drop(v);
    let raw = std::fs::read(&path)?;
    for needle in [&b"hunter2"[..], b"s3cret", b"db.internal", b"OPENSSH"] {
        assert!(!raw.windows(needle.len()).any(|w| w == needle));
    }
    Ok(())
}

#[test]
fn export_import_roundtrip() -> Res {
    let dir = TempDir::new()?;
    let src = fresh(&dir, "src.redb", "pw")?;
    let mut h = host("edge")?;
    src.put(&mut h)?;
    let mut t = TunnelSpec::new(
        "pg",
        h.id,
        Forward::Local {
            bind: Endpoint::new("127.0.0.1", 15432),
            dest: Endpoint::new("10.0.0.5", 5432),
        },
    )?;
    src.put(&mut t)?;
    let blob = src.export(&pw("export-pass"), FAST)?;
    assert_eq!(&blob[..4], b"CTDL");
    assert_eq!(blob[4], 1);

    let dst = fresh(&dir, "dst.redb", "other")?;
    assert!(matches!(
        dst.import(&blob, &pw("nope"), ImportMode::Merge),
        Err(VaultError::BadPassword)
    ));
    let report = dst.import(&blob, &pw("export-pass"), ImportMode::Merge)?;
    assert_eq!(report.added, 2);
    let again = dst.import(&blob, &pw("export-pass"), ImportMode::Merge)?;
    assert_eq!(again.added, 0);
    assert_eq!(again.unchanged, 2);
    let tunnels = dst.list::<TunnelSpec>()?;
    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].forward, t.forward);

    let mut tampered = blob.clone();
    let last = tampered.len() - 20;
    tampered[last] ^= 0x01;
    assert!(matches!(
        Bundle::open(&tampered, &pw("export-pass")),
        Err(VaultError::Tampered)
    ));
    let mut bad_hdr = blob.clone();
    bad_hdr[20] ^= 0x01;
    assert!(matches!(
        Bundle::open(&bad_hdr, &pw("export-pass")),
        Err(VaultError::BadPassword)
    ));
    assert!(matches!(
        Bundle::open(&blob[..100], &pw("export-pass")),
        Err(VaultError::Truncated)
    ));
    assert!(matches!(
        Bundle::open(b"NOPE-not-a-bundle", &pw("export-pass")),
        Err(VaultError::BadMagic)
    ));
    let mut future = blob;
    future[4] = 9;
    assert!(matches!(
        Bundle::open(&future, &pw("export-pass")),
        Err(VaultError::UnsupportedVersion(9))
    ));
    Ok(())
}

#[test]
fn replace_import_wipes_local() -> Res {
    let dir = TempDir::new()?;
    let src = fresh(&dir, "src.redb", "pw")?;
    src.put(&mut host("only")?)?;
    let blob = src.export(&pw("x"), FAST)?;
    let dst = fresh(&dir, "dst.redb", "pw")?;
    dst.put(&mut host("stale-1")?)?;
    dst.put(&mut host("stale-2")?)?;
    dst.import(&blob, &pw("x"), ImportMode::Replace)?;
    let hosts = dst.list::<HostProfile>()?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].label, "only");
    Ok(())
}

#[test]
fn last_writer_wins_merge() -> Res {
    let dir = TempDir::new()?;
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut h = host("web")?;
    v.put(&mut h)?;
    let mut newer = h.clone();
    newer.label = "web-renamed".into();
    newer.updated_ms = h.updated_ms + 10;
    let mut older = h.clone();
    older.label = "web-ancient".into();
    older.updated_ms = h.updated_ms - 1;
    let r = v.merge(vec![Entry::Host(older)])?;
    assert_eq!(r.unchanged, 1);
    assert_eq!(v.get::<HostProfile>(h.id)?.ok_or("missing")?.label, "web");
    let r = v.merge(vec![Entry::Host(newer.clone())])?;
    assert_eq!(r.updated, 1);
    assert_eq!(
        v.get::<HostProfile>(h.id)?.ok_or("missing")?.label,
        "web-renamed"
    );
    let grave = citadel_vault::Tombstone {
        id: h.id,
        updated_ms: newer.updated_ms + 1,
    };
    let r = v.merge(vec![Entry::Tombstone(grave)])?;
    assert_eq!(r.deleted, 1);
    assert!(v.get::<HostProfile>(h.id)?.is_none());
    let mut revived = newer;
    revived.updated_ms = grave.updated_ms + 1;
    let r = v.merge(vec![Entry::Host(revived)])?;
    assert_eq!(r.added, 1);
    assert!(v.get::<HostProfile>(h.id)?.is_some());
    Ok(())
}

#[test]
fn sync_blob_same_key_merges_without_password() -> Res {
    let dir = TempDir::new()?;
    let a = fresh(&dir, "a.redb", "shared")?;
    a.put(&mut host("one")?)?;
    let seed = a.sync_blob()?;

    let b = Vault::open(dir.path().join("b.redb"))?;
    b.initialize(&pw("shared"), FAST)?;
    let joined = b.absorb(&seed, Some(&pw("shared")))?;
    assert!(joined.rekeyed);
    assert_eq!(joined.report.added, 1);

    b.put(&mut host("two")?)?;
    let back = a.absorb(&b.sync_blob()?, None)?;
    assert!(!back.rekeyed);
    assert_eq!(back.report.added, 1);
    assert_eq!(a.list::<HostProfile>()?.len(), 2);
    Ok(())
}

#[test]
fn sync_blob_foreign_key_requires_password() -> Res {
    let dir = TempDir::new()?;
    let a = fresh(&dir, "a.redb", "alpha")?;
    a.put(&mut host("x")?)?;
    let blob = a.sync_blob()?;
    let b = fresh(&dir, "b.redb", "beta")?;
    b.put(&mut host("y")?)?;
    assert!(matches!(b.absorb(&blob, None), Err(VaultError::ForeignKey)));
    assert!(matches!(
        b.absorb(&blob, Some(&pw("beta"))),
        Err(VaultError::BadPassword)
    ));
    let done = b.absorb(&blob, Some(&pw("alpha")))?;
    assert!(done.rekeyed && done.header_adopted);
    assert_eq!(b.list::<HostProfile>()?.len(), 2);
    b.lock();
    assert!(matches!(
        b.unlock(&pw("beta")),
        Err(VaultError::BadPassword)
    ));
    b.unlock(&pw("alpha"))?;
    assert_eq!(b.list::<HostProfile>()?.len(), 2);
    let back = a.absorb(&b.sync_blob()?, None)?;
    assert!(!back.rekeyed);
    assert_eq!(a.list::<HostProfile>()?.len(), 2);
    Ok(())
}

#[test]
fn password_change_propagates_through_sync() -> Res {
    let dir = TempDir::new()?;
    let a = fresh(&dir, "a.redb", "old")?;
    let b = Vault::open(dir.path().join("b.redb"))?;
    b.initialize(&pw("tmp"), FAST)?;
    b.absorb(&a.sync_blob()?, Some(&pw("old")))?;
    a.change_password(&pw("old"), &pw("new"), FAST)?;
    a.lock();
    assert!(matches!(a.unlock(&pw("old")), Err(VaultError::BadPassword)));
    a.unlock(&pw("new"))?;
    let got = b.absorb(&a.sync_blob()?, None)?;
    assert!(got.header_adopted && !got.rekeyed);
    b.lock();
    b.unlock(&pw("new"))?;
    Ok(())
}

#[test]
fn kdf_bounds_rejected() -> Res {
    let dir = TempDir::new()?;
    let v = Vault::open(dir.path().join("v.redb"))?;
    let absurd = KdfParams {
        m_kib: u32::MAX,
        t: 1,
        p: 1,
    };
    assert!(matches!(
        v.initialize(&pw("x"), absurd),
        Err(VaultError::KdfBounds { .. })
    ));
    assert!(matches!(
        v.initialize(
            &pw("x"),
            KdfParams {
                m_kib: 64,
                t: 0,
                p: 1
            }
        ),
        Err(VaultError::KdfBounds { .. })
    ));
    assert_eq!(
        KdfParams::default(),
        KdfParams {
            m_kib: 65536,
            t: 3,
            p: 4
        }
    );
    Ok(())
}

#[test]
fn id_text_roundtrip() -> Res {
    let id = Id::random()?;
    let text = id.to_string();
    assert_eq!(text.len(), 32);
    assert_eq!(text.parse::<Id>()?, id);
    assert!("zz".parse::<Id>().is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn autolock_fires_after_idle() -> Res {
    let dir = TempDir::new()?;
    let v = Arc::new(fresh(&dir, "v.redb", "pw")?);
    let mut state = v.subscribe();
    let guard = AutoLock::spawn(v.clone(), Some(Duration::from_secs(60)));
    tokio::time::sleep(Duration::from_secs(45)).await;
    assert!(v.is_unlocked());
    v.touch();
    tokio::time::sleep(Duration::from_secs(45)).await;
    assert!(v.is_unlocked());
    tokio::time::sleep(Duration::from_secs(16)).await;
    tokio::task::yield_now().await;
    assert!(!v.is_unlocked());
    assert_eq!(*state.borrow_and_update(), LockState::Locked);

    v.unlock(&pw("pw"))?;
    guard.set_idle(None);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert!(v.is_unlocked());
    guard.set_idle(Some(Duration::from_secs(1)));
    tokio::time::sleep(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(!v.is_unlocked());
    Ok(())
}

type Raw = redb::TableDefinition<'static, [u8; 16], &'static [u8]>;
const META: redb::TableDefinition<'static, &'static str, &'static [u8]> =
    redb::TableDefinition::new("meta");

fn hosts_table() -> Raw {
    redb::TableDefinition::new("hosts")
}

#[test]
fn unreadable_records_are_skipped_not_fatal() -> Res {
    let dir = TempDir::new()?;
    let path = dir.path().join("v.redb");
    let (keep, bad) = {
        let v = fresh(&dir, "v.redb", "pw")?;
        let mut a = host("keep")?;
        let mut b = host("broken")?;
        v.put(&mut a)?;
        v.put(&mut b)?;
        (a.id, b.id)
    };
    {
        let db = redb::Database::open(&path)?;
        let txn = db.begin_write()?;
        txn.open_table(hosts_table())?
            .insert(bad.as_bytes(), b"written by some future release".as_slice())?;
        txn.commit()?;
    }
    let v = Vault::open(&path)?;
    v.unlock(&pw("pw"))?;
    let listed: Vec<Id> = v.list::<HostProfile>()?.into_iter().map(|h| h.id).collect();
    assert_eq!(listed, vec![keep]);
    let health = v.health()?;
    assert_eq!((health.records, health.unreadable), (1, 1));
    assert_eq!(v.entries()?.len(), 1);
    Ok(())
}

#[test]
fn newer_store_format_refuses_to_open_and_old_vaults_get_marked() -> Res {
    let dir = TempDir::new()?;
    let path = dir.path().join("v.redb");
    drop(fresh(&dir, "v.redb", "pw")?);
    {
        let db = redb::Database::open(&path)?;
        let txn = db.begin_write()?;
        txn.open_table(META)?.remove("format")?;
        txn.commit()?;
    }
    let v = Vault::open(&path)?;
    assert_eq!(v.state(), LockState::Locked);
    drop(v);
    {
        let db = redb::Database::open(&path)?;
        let txn = redb::ReadableDatabase::begin_read(&db)?;
        let marker = txn
            .open_table(META)?
            .get("format")?
            .map(|v| v.value().to_vec());
        assert_eq!(
            marker,
            Some(citadel_vault::STORE_FORMAT.to_le_bytes().to_vec())
        );
    }
    {
        let db = redb::Database::open(&path)?;
        let txn = db.begin_write()?;
        txn.open_table(META)?
            .insert("format", 99u32.to_le_bytes().as_slice())?;
        txn.commit()?;
    }
    assert!(matches!(
        Vault::open(&path),
        Err(VaultError::NewerFormat(99))
    ));
    Ok(())
}

#[test]
fn backups_rotate_and_restore() -> Res {
    let dir = TempDir::new()?;
    let backups = dir.path().join("backups");
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut a = host("alpha")?;
    let mut b = host("bravo")?;
    v.put(&mut a)?;
    v.put(&mut b)?;
    let mut made = Vec::new();
    for _ in 0..5 {
        made.push(v.backup_to(&backups, 3)?);
        std::thread::sleep(Duration::from_millis(3));
    }
    let listed = citadel_vault::list_backups(&backups);
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].path, made[4].path);
    assert!(
        listed
            .windows(2)
            .all(|w| w[0].created_ms >= w[1].created_ms)
    );
    assert_eq!(listed[0].generation, v.health()?.generation);

    let blob = std::fs::read(&listed[0].path)?;
    v.remove(a.id)?;
    let merged = v.restore(&blob, &pw("pw"), false)?;
    assert_eq!(merged.added, 0);
    assert!(v.get::<HostProfile>(a.id)?.is_none());
    v.restore(&blob, &pw("pw"), true)?;
    assert!(v.get::<HostProfile>(a.id)?.is_some());

    v.change_password(&pw("pw"), &pw("newer"), FAST)?;
    let other = Vault::open(dir.path().join("fresh.redb"))?;
    assert_eq!(other.state(), LockState::Uninitialized);
    assert!(matches!(
        other.restore(&blob, &pw("wrong"), false),
        Err(VaultError::BadPassword)
    ));
    let report = other.restore(&blob, &pw("pw"), false)?;
    assert_eq!(report.added, 2);
    other.lock();
    other.unlock(&pw("pw"))?;
    assert_eq!(other.list::<HostProfile>()?.len(), 2);
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[test]
fn clock_is_seeded_from_stored_stamps_after_reopen() -> Res {
    let dir = TempDir::new()?;
    let path = dir.path().join("v.redb");
    let far = now_ms() + 10 * 86_400_000;
    let id = {
        let v = fresh(&dir, "v.redb", "pw")?;
        let mut h = host("skewed")?;
        h.updated_ms = far;
        v.merge(vec![Entry::Host(h.clone())])?;
        h.id
    };
    let v = Vault::open(&path)?;
    v.unlock(&pw("pw"))?;
    let mut h = v.get::<HostProfile>(id)?.ok_or("missing")?;
    h.port = 2201;
    v.put(&mut h)?;
    let stamped = v.get::<HostProfile>(id)?.ok_or("missing")?.updated_ms;
    assert!(stamped > far, "edit stamped {stamped} not after {far}");
    Ok(())
}

#[test]
fn replace_supersedes_stale_copies_and_buries_dropped_records() -> Res {
    let dir = TempDir::new()?;
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut a = host("alpha")?;
    let mut b = host("bravo")?;
    v.put(&mut a)?;
    v.put(&mut b)?;
    let stale_a = a.clone();
    let stale_b = b.clone();
    let mut old_a = a.clone();
    old_a.port = 2022;
    old_a.updated_ms = 5;
    let report = v.replace(&[Entry::Host(old_a)])?;
    assert_eq!((report.added, report.deleted), (1, 1));
    let merged = v.merge(vec![Entry::Host(stale_a), Entry::Host(stale_b)])?;
    assert_eq!(merged.added + merged.updated, 0);
    assert_eq!(v.get::<HostProfile>(a.id)?.ok_or("missing")?.port, 2022);
    assert!(v.get::<HostProfile>(b.id)?.is_none());
    Ok(())
}

#[test]
fn rotation_never_deletes_the_backup_it_just_wrote() -> Res {
    let dir = TempDir::new()?;
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups)?;
    for i in 0..4 {
        std::fs::write(
            backups.join(format!("citadel-999999999999{i}-1.ctdl")),
            b"future",
        )?;
    }
    let v = fresh(&dir, "v.redb", "pw")?;
    let mut a = host("alpha")?;
    v.put(&mut a)?;
    let made = v.backup_to(&backups, 3)?;
    assert!(made.path.exists());
    assert_eq!(citadel_vault::list_backups(&backups).len(), 3);
    Ok(())
}
