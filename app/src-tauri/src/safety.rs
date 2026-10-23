use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use citadel_vault::{Backup, Vault, list_backups};

pub const KEEP: usize = 20;
const KEEP_RAW: usize = 5;
const VAULT_FILE: &str = "vault.redb";
const VERSION_FILE: &str = "version";
const RAW_PREFIX: &str = "pre-upgrade-";

pub fn backups_dir(data: &Path) -> PathBuf {
    data.join("backups")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[cfg(unix)]
fn private(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn private(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

pub fn write_private(path: &Path, body: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(body)?;
    f.sync_all()
}

fn copy_private(from: &Path, to: &Path) -> io::Result<()> {
    let tmp = to.with_extension("part");
    fs::copy(from, &tmp)?;
    private(&tmp, 0o600)?;
    fs::rename(tmp, to)
}

pub fn preserve_before_upgrade(data: &Path, running: &str) -> io::Result<Option<PathBuf>> {
    let vault = data.join(VAULT_FILE);
    let marker = data.join(VERSION_FILE);
    let previous = fs::read_to_string(&marker)
        .ok()
        .map(|v| v.trim().to_owned());
    let mut saved = None;
    if vault.is_file() && previous.as_deref() != Some(running) {
        let dir = backups_dir(data);
        fs::create_dir_all(&dir)?;
        private(&dir, 0o700)?;
        let from = previous
            .as_deref()
            .filter(|v| {
                !v.is_empty()
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
            })
            .unwrap_or("unknown");
        let target = dir.join(format!("{RAW_PREFIX}{from}-{}.redb", now_ms()));
        copy_private(&vault, &target)?;
        let mut raws: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(RAW_PREFIX) && n.ends_with(".redb"))
            })
            .collect();
        raws.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        let excess = raws.len().saturating_sub(KEEP_RAW);
        for old in raws.into_iter().take(excess) {
            let _ = fs::remove_file(old);
        }
        saved = Some(target);
    }
    if previous.as_deref() != Some(running) {
        fs::write(&marker, running)?;
    }
    Ok(saved)
}

pub fn adopt_orphaned_vault(data: &Path) -> io::Result<Option<PathBuf>> {
    let target = data.join(VAULT_FILE);
    if target.exists() {
        return Ok(None);
    }
    let Some(parent) = data.parent() else {
        return Ok(None);
    };
    let Ok(siblings) = fs::read_dir(parent) else {
        return Ok(None);
    };
    let best = siblings
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p != data)
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().contains("citadel"))
        })
        .map(|p| p.join(VAULT_FILE))
        .filter(|v| v.is_file())
        .filter_map(|v| {
            fs::metadata(&v)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| (t, v))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, v)| v);
    let Some(source) = best else {
        return Ok(None);
    };
    fs::create_dir_all(data)?;
    copy_private(&source, &target)?;
    if let Some(old_backups) = source.parent().map(backups_dir) {
        for b in list_backups(&old_backups) {
            let dir = backups_dir(data);
            fs::create_dir_all(&dir)?;
            if let Some(name) = b.path.file_name() {
                let _ = copy_private(&b.path, &dir.join(name));
            }
        }
    }
    Ok(Some(source))
}

pub fn snapshot_if_changed(vault: &Vault, dir: &Path) -> citadel_vault::Result<Option<Backup>> {
    if !vault.is_unlocked() {
        return Ok(None);
    }
    let generation = vault.health()?.generation;
    if generation == 0 {
        return Ok(None);
    }
    if list_backups(dir)
        .first()
        .is_some_and(|b| b.generation == generation)
    {
        return Ok(None);
    }
    vault.backup_to(dir, KEEP).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_vault::{Auth, HostProfile, KdfParams, SecretString};

    const FAST: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    #[test]
    fn upgrades_keep_a_raw_copy_and_only_once_per_version() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("io.example.citadel");
        fs::create_dir_all(&data).unwrap();
        assert!(preserve_before_upgrade(&data, "0.1.0").unwrap().is_none());
        fs::write(data.join(VAULT_FILE), b"vault bytes").unwrap();
        assert!(preserve_before_upgrade(&data, "0.1.0").unwrap().is_none());
        let saved = preserve_before_upgrade(&data, "0.2.0").unwrap().unwrap();
        assert!(
            saved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("pre-upgrade-0.1.0-")
        );
        assert_eq!(fs::read(&saved).unwrap(), b"vault bytes");
        assert!(preserve_before_upgrade(&data, "0.2.0").unwrap().is_none());
        for v in 3..12 {
            preserve_before_upgrade(&data, &format!("0.{v}.0")).unwrap();
        }
        let raws = fs::read_dir(backups_dir(&data)).unwrap().count();
        assert_eq!(raws, KEEP_RAW);
    }

    #[test]
    fn a_renamed_app_directory_finds_the_old_vault() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("dev.someone.citadel");
        fs::create_dir_all(backups_dir(&old)).unwrap();
        fs::write(old.join(VAULT_FILE), b"old vault").unwrap();
        fs::write(
            backups_dir(&old).join("citadel-0000000000001-a.ctdl"),
            b"snap",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("unrelated")).unwrap();
        fs::write(dir.path().join("unrelated").join(VAULT_FILE), b"nope").unwrap();
        let new = dir.path().join("io.github.someone.citadel");
        let from = adopt_orphaned_vault(&new).unwrap().unwrap();
        assert_eq!(from, old.join(VAULT_FILE));
        assert_eq!(fs::read(new.join(VAULT_FILE)).unwrap(), b"old vault");
        assert_eq!(list_backups(&backups_dir(&new)).len(), 1);
        assert!(old.join(VAULT_FILE).exists());
        assert!(adopt_orphaned_vault(&new).unwrap().is_none());
    }

    #[test]
    fn snapshots_are_taken_only_when_the_vault_changed() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("backups");
        let v = Vault::open(dir.path().join("v.redb")).unwrap();
        v.initialize(&SecretString::from("pw"), FAST).unwrap();
        assert!(snapshot_if_changed(&v, &backups).unwrap().is_none());
        let mut h = HostProfile::new("a", "a.example", "u", Auth::Interactive).unwrap();
        v.put(&mut h).unwrap();
        assert!(snapshot_if_changed(&v, &backups).unwrap().is_some());
        assert!(snapshot_if_changed(&v, &backups).unwrap().is_none());
        h.port = 2200;
        v.put(&mut h).unwrap();
        assert!(snapshot_if_changed(&v, &backups).unwrap().is_some());
        assert_eq!(list_backups(&backups).len(), 2);
        v.lock();
        assert!(snapshot_if_changed(&v, &backups).unwrap().is_none());
    }
}
