use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chacha20poly1305::XChaCha20Poly1305;
use redb::{
    Database, ReadOnlyTable, ReadTransaction, ReadableDatabase, ReadableTable, TableDefinition,
    TableError, WriteTransaction,
};
use secrecy::SecretString;
use serde::Serialize;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::envelope::{self, Bundle, KeyHeader, Sealed};
use crate::kdf::KdfParams;
use crate::model::sealed::Sealed as _;
use crate::model::{
    Entry, HostProfile, IdentityKey, KnownHost, LIVE_TABLES, Record, Snippet, SyncTarget,
    Tombstone, TunnelSpec,
};
use crate::{Id, Result, VaultError, codec, crypto};

type Table = TableDefinition<'static, [u8; 16], &'static [u8]>;

const META: TableDefinition<'static, &'static str, &'static [u8]> = TableDefinition::new("meta");
const HEADER_KEY: &str = "header";
const FORMAT_KEY: &str = "format";
pub const STORE_FORMAT: u32 = 1;
const BACKUP_PREFIX: &str = "citadel-";
const BACKUP_EXT: &str = ".ctdl";
const CACHE_BYTES: usize = 4 << 20;
const AAD_DOMAIN: &[u8] = b"citadel.record.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    Uninitialized,
    Locked,
    Unlocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    Merge,
    Replace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Health {
    pub records: usize,
    pub unreadable: usize,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Backup {
    pub path: PathBuf,
    pub created_ms: u64,
    pub generation: u64,
    pub bytes: u64,
}

impl Backup {
    fn parse(path: PathBuf, bytes: u64) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        let core = name.strip_prefix(BACKUP_PREFIX)?.strip_suffix(BACKUP_EXT)?;
        let (ms, gen_hex) = core.split_once('-')?;
        Some(Self {
            created_ms: ms.parse().ok()?,
            generation: u64::from_str_radix(gen_hex, 16).ok()?,
            path,
            bytes,
        })
    }
}

pub fn list_backups(dir: &Path) -> Vec<Backup> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Backup> = rd
        .filter_map(std::result::Result::ok)
        .filter_map(|e| {
            let bytes = e.metadata().ok().filter(fs::Metadata::is_file)?.len();
            Backup::parse(e.path(), bytes)
        })
        .collect();
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Absorbed {
    pub report: MergeReport,
    pub rekeyed: bool,
    pub header_adopted: bool,
}

struct Session {
    cipher: XChaCha20Poly1305,
    last_use: Instant,
}

pub struct Vault {
    db: Database,
    path: PathBuf,
    session: Mutex<Option<Session>>,
    state: watch::Sender<LockState>,
    clock: AtomicU64,
}

fn table(name: &'static str) -> Table {
    TableDefinition::new(name)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn record_aad(kind: &str, id: Id) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + kind.len() + 18);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.push(0);
    aad.extend_from_slice(kind.as_bytes());
    aad.push(0);
    aad.extend_from_slice(id.as_bytes());
    aad
}

fn open_ro(
    txn: &ReadTransaction,
    def: Table,
) -> Result<Option<ReadOnlyTable<[u8; 16], &'static [u8]>>> {
    match txn.open_table(def) {
        Ok(t) => Ok(Some(t)),
        Err(TableError::TableDoesNotExist(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn seal_record<R: Record>(cipher: &XChaCha20Poly1305, rec: &R) -> Result<Vec<u8>> {
    let plain = codec::encode(rec)?;
    crypto::seal(cipher, &record_aad(R::TABLE, rec.id()), &plain)
}

fn open_record<R: Record>(cipher: &XChaCha20Poly1305, key: [u8; 16], blob: &[u8]) -> Result<R> {
    let id = Id::from_bytes(key);
    let plain = crypto::open(cipher, &record_aad(R::TABLE, id), blob)?;
    let rec: R = codec::decode(&plain)?;
    if rec.id() != id {
        return Err(VaultError::Tampered);
    }
    Ok(rec)
}

fn scan<R: Record>(
    txn: &ReadTransaction,
    cipher: &XChaCha20Poly1305,
    skipped: &mut usize,
) -> Result<Vec<R>> {
    let Some(t) = open_ro(txn, table(R::TABLE))? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for row in t.iter()? {
        let (k, v) = row?;
        match open_record::<R>(cipher, k.value(), v.value()) {
            Ok(rec) => out.push(rec),
            Err(
                VaultError::Decode(_)
                | VaultError::Tampered
                | VaultError::Truncated
                | VaultError::BadId,
            ) => {
                *skipped += 1;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn gather<R: Record>(
    txn: &ReadTransaction,
    cipher: &XChaCha20Poly1305,
    out: &mut BTreeMap<Id, Entry>,
    skipped: &mut usize,
) -> Result<()> {
    for rec in scan::<R>(txn, cipher, skipped)? {
        out.insert(rec.id(), rec.into_entry());
    }
    Ok(())
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("part");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(body)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

fn insert<R: Record>(txn: &WriteTransaction, cipher: &XChaCha20Poly1305, rec: &R) -> Result<()> {
    let blob = seal_record(cipher, rec)?;
    txn.open_table(table(R::TABLE))?
        .insert(rec.id().as_bytes(), blob.as_slice())?;
    Ok(())
}

fn evict(txn: &WriteTransaction, id: Id) -> Result<bool> {
    let mut live = false;
    for name in LIVE_TABLES {
        live |= txn
            .open_table(table(name))?
            .remove(id.as_bytes())?
            .is_some();
    }
    Ok(live)
}

fn purge(txn: &WriteTransaction, id: Id) -> Result<()> {
    evict(txn, id)?;
    txn.open_table(table(Tombstone::TABLE))?
        .remove(id.as_bytes())?;
    Ok(())
}

fn store(txn: &WriteTransaction, cipher: &XChaCha20Poly1305, entry: &Entry) -> Result<()> {
    purge(txn, entry.id())?;
    match entry {
        Entry::Host(r) => insert(txn, cipher, r),
        Entry::Identity(r) => insert(txn, cipher, r),
        Entry::Tunnel(r) => insert(txn, cipher, r),
        Entry::Snippet(r) => insert(txn, cipher, r),
        Entry::KnownHost(r) => insert(txn, cipher, r),
        Entry::SyncTarget(r) => insert(txn, cipher, r),
        Entry::Tombstone(r) => insert(txn, cipher, r),
    }
}

fn wipe(txn: &WriteTransaction) -> Result<()> {
    for name in LIVE_TABLES.into_iter().chain([Tombstone::TABLE]) {
        txn.delete_table(table(name))?;
        txn.open_table(table(name))?;
    }
    Ok(())
}

fn write_header(txn: &WriteTransaction, header: &KeyHeader) -> Result<()> {
    let mut meta = txn.open_table(META)?;
    meta.insert(HEADER_KEY, header.encode().as_slice())?;
    meta.insert(FORMAT_KEY, STORE_FORMAT.to_le_bytes().as_slice())?;
    Ok(())
}

fn classify(local: Option<&Entry>, incoming: &Entry, report: &mut MergeReport) -> bool {
    match local {
        None if incoming.is_tombstone() => true,
        None => {
            report.added += 1;
            true
        }
        Some(l) if incoming.stamp() > l.stamp() => {
            match (l.is_tombstone(), incoming.is_tombstone()) {
                (false, true) => report.deleted += 1,
                (true, false) => report.added += 1,
                (true, true) => {}
                (false, false) => report.updated += 1,
            }
            true
        }
        Some(_) => {
            report.unchanged += 1;
            false
        }
    }
}

impl Vault {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty() && !d.exists())
        {
            fs::create_dir_all(dir)?;
            restrict(dir, 0o700)?;
        }
        let db = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create(&path)?;
        restrict(&path, 0o600)?;
        let vault = Self {
            db,
            path,
            session: Mutex::new(None),
            state: watch::Sender::new(LockState::Uninitialized),
            clock: AtomicU64::new(0),
        };
        match vault.read_format()? {
            Some(f) if f > STORE_FORMAT => return Err(VaultError::NewerFormat(f)),
            None if vault.read_header()?.is_some() => {
                let txn = vault.db.begin_write()?;
                txn.open_table(META)?
                    .insert(FORMAT_KEY, STORE_FORMAT.to_le_bytes().as_slice())?;
                txn.commit()?;
            }
            _ => {}
        }
        if vault.read_header()?.is_some() {
            vault.state.send_replace(LockState::Locked);
        }
        Ok(vault)
    }

    pub fn health(&self) -> Result<Health> {
        self.with_cipher(|cipher| {
            let mut skipped = 0;
            let snap = self.snapshot_counting(cipher, &mut skipped)?;
            Ok(Health {
                records: snap.values().filter(|e| !e.is_tombstone()).count(),
                unreadable: skipped,
                generation: snap.values().map(Entry::stamp).max().unwrap_or(0),
            })
        })
    }

    pub fn backup_to(&self, dir: &Path, keep: usize) -> Result<Backup> {
        let generation = self.health()?.generation;
        let blob = self.sync_blob()?;
        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }
        restrict(dir, 0o700)?;
        let created_ms = now_ms();
        let path = dir.join(format!(
            "{BACKUP_PREFIX}{created_ms:013}-{generation:x}{BACKUP_EXT}"
        ));
        write_atomic(&path, &blob)?;
        for old in list_backups(dir).into_iter().skip(keep.max(1)) {
            let _ = fs::remove_file(&old.path);
        }
        Ok(Backup {
            path,
            created_ms,
            generation,
            bytes: blob.len() as u64,
        })
    }

    pub fn restore(
        &self,
        blob: &[u8],
        password: &SecretString,
        replace: bool,
    ) -> Result<MergeReport> {
        let sealed = Sealed::parse(blob)?;
        if self.state() == LockState::Uninitialized {
            let dek = sealed.header.unwrap_key(password)?;
            let cipher = crypto::cipher(&dek);
            let bundle = sealed.open(&cipher)?;
            let txn = self.db.begin_write()?;
            write_header(&txn, &sealed.header)?;
            for name in LIVE_TABLES.into_iter().chain([Tombstone::TABLE]) {
                txn.open_table(table(name))?;
            }
            let mut report = MergeReport::default();
            for e in &bundle.entries {
                self.observe(e.stamp());
                if !e.is_tombstone() {
                    report.added += 1;
                }
                store(&txn, &cipher, e)?;
            }
            txn.commit()?;
            self.install(cipher);
            return Ok(report);
        }
        let local = self.with_cipher(|cipher| match sealed.open(cipher) {
            Ok(bundle) => Ok(Some(bundle)),
            Err(VaultError::Tampered) => Ok(None),
            Err(e) => Err(e),
        })?;
        let bundle = match local {
            Some(b) => b,
            None => sealed.open(&crypto::cipher(&sealed.header.unwrap_key(password)?))?,
        };
        if replace {
            self.replace(&bundle.entries)
        } else {
            self.merge(bundle.entries)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state(&self) -> LockState {
        *self.state.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<LockState> {
        self.state.subscribe()
    }

    pub fn is_unlocked(&self) -> bool {
        self.guard().is_some()
    }

    pub fn is_initialized(&self) -> bool {
        self.state() != LockState::Uninitialized
    }

    pub fn initialize(&self, password: &SecretString, kdf: KdfParams) -> Result<()> {
        if self.read_header()?.is_some() {
            return Err(VaultError::AlreadyInitialized);
        }
        let dek = crypto::random_key()?;
        let header = KeyHeader::wrap(password, &dek, kdf, self.tick())?;
        let txn = self.db.begin_write()?;
        write_header(&txn, &header)?;
        for name in LIVE_TABLES.into_iter().chain([Tombstone::TABLE]) {
            txn.open_table(table(name))?;
        }
        txn.commit()?;
        self.install(crypto::cipher(&dek));
        Ok(())
    }

    pub fn unlock(&self, password: &SecretString) -> Result<()> {
        let header = self.header()?;
        let dek = header.unwrap_key(password)?;
        self.install(crypto::cipher(&dek));
        Ok(())
    }

    pub fn lock(&self) {
        let prev = self.guard().take();
        if prev.is_some() {
            drop(prev);
            self.state.send_replace(LockState::Locked);
        }
    }

    pub fn touch(&self) {
        if let Some(s) = self.guard().as_mut() {
            s.last_use = Instant::now();
        }
    }

    pub fn idle_for(&self) -> Option<Duration> {
        self.guard().as_ref().map(|s| s.last_use.elapsed())
    }

    pub fn kdf_params(&self) -> Result<KdfParams> {
        Ok(self.header()?.kdf)
    }

    pub fn change_password(
        &self,
        current: &SecretString,
        next: &SecretString,
        kdf: KdfParams,
    ) -> Result<()> {
        let mut guard = self.guard();
        let sess = guard.as_mut().ok_or(VaultError::Locked)?;
        let dek = self.header()?.unwrap_key(current)?;
        let fresh = KeyHeader::wrap(next, &dek, kdf, self.tick())?;
        let txn = self.db.begin_write()?;
        write_header(&txn, &fresh)?;
        txn.commit()?;
        sess.last_use = Instant::now();
        Ok(())
    }

    pub fn put<R: Record>(&self, rec: &mut R) -> Result<()> {
        self.with_cipher(|cipher| {
            rec.set_stamp(self.tick());
            let txn = self.db.begin_write()?;
            purge(&txn, rec.id())?;
            insert(&txn, cipher, rec)?;
            txn.commit()?;
            Ok(())
        })
    }

    pub fn get<R: Record>(&self, id: Id) -> Result<Option<R>> {
        self.with_cipher(|cipher| {
            let txn = self.db.begin_read()?;
            let Some(t) = open_ro(&txn, table(R::TABLE))? else {
                return Ok(None);
            };
            let Some(blob) = t.get(id.as_bytes())? else {
                return Ok(None);
            };
            open_record(cipher, *id.as_bytes(), blob.value()).map(Some)
        })
    }

    pub fn list<R: Record>(&self) -> Result<Vec<R>> {
        self.with_cipher(|cipher| scan(&self.db.begin_read()?, cipher, &mut 0))
    }

    pub fn remove(&self, id: Id) -> Result<bool> {
        self.with_cipher(|cipher| {
            let txn = self.db.begin_write()?;
            let live = evict(&txn, id)?;
            if live {
                insert(
                    &txn,
                    cipher,
                    &Tombstone {
                        id,
                        updated_ms: self.tick(),
                    },
                )?;
            }
            txn.commit()?;
            Ok(live)
        })
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        self.with_cipher(|cipher| Ok(self.snapshot(cipher)?.into_values().collect()))
    }

    pub fn export(&self, password: &SecretString, kdf: KdfParams) -> Result<Vec<u8>> {
        let bundle = Bundle {
            created_ms: self.tick(),
            entries: self.entries()?,
        };
        bundle.seal(password, kdf)
    }

    pub fn import(
        &self,
        blob: &[u8],
        password: &SecretString,
        mode: ImportMode,
    ) -> Result<MergeReport> {
        let bundle = Bundle::open(blob, password)?;
        match mode {
            ImportMode::Merge => self.merge(bundle.entries),
            ImportMode::Replace => self.replace(&bundle.entries),
        }
    }

    pub fn merge(&self, incoming: Vec<Entry>) -> Result<MergeReport> {
        self.with_cipher(|cipher| self.merge_with(cipher, incoming))
    }

    pub fn replace(&self, entries: &[Entry]) -> Result<MergeReport> {
        self.with_cipher(|cipher| {
            let txn = self.db.begin_write()?;
            wipe(&txn)?;
            let mut report = MergeReport::default();
            for e in entries {
                if !e.is_tombstone() {
                    report.added += 1;
                }
                self.observe(e.stamp());
                store(&txn, cipher, e)?;
            }
            txn.commit()?;
            Ok(report)
        })
    }

    pub fn sync_blob(&self) -> Result<Vec<u8>> {
        self.with_cipher(|cipher| {
            let header = self.header()?;
            let bundle = Bundle {
                created_ms: self.tick(),
                entries: self.snapshot(cipher)?.into_values().collect(),
            };
            envelope::seal(cipher, &header, &bundle)
        })
    }

    pub fn absorb(&self, blob: &[u8], password: Option<&SecretString>) -> Result<Absorbed> {
        let sealed = Sealed::parse(blob)?;
        let fast = self.with_cipher(|cipher| match sealed.open(cipher) {
            Ok(bundle) => {
                let local = self.header()?;
                let adopt = sealed.header != local && sealed.header.epoch > local.epoch;
                let report = self.merge_with(cipher, bundle.entries)?;
                if adopt {
                    let txn = self.db.begin_write()?;
                    write_header(&txn, &sealed.header)?;
                    txn.commit()?;
                }
                Ok(Some(Absorbed {
                    report,
                    rekeyed: false,
                    header_adopted: adopt,
                }))
            }
            Err(VaultError::Tampered) => Ok(None),
            Err(e) => Err(e),
        })?;
        if let Some(done) = fast {
            return Ok(done);
        }
        let password = password.ok_or(VaultError::ForeignKey)?;
        let dek = sealed.header.unwrap_key(password)?;
        let remote = crypto::cipher(&dek);
        let bundle = sealed.open(&remote)?;
        self.rekey(remote, &sealed.header, bundle.entries)
    }

    fn rekey(
        &self,
        remote: XChaCha20Poly1305,
        header: &KeyHeader,
        incoming: Vec<Entry>,
    ) -> Result<Absorbed> {
        let mut guard = self.guard();
        let sess = guard.as_mut().ok_or(VaultError::Locked)?;
        let mut skipped = 0;
        let mut merged = self.snapshot_counting(&sess.cipher, &mut skipped)?;
        if skipped > 0 {
            return Err(VaultError::Unreadable(skipped));
        }
        let mut report = MergeReport::default();
        for e in incoming {
            self.observe(e.stamp());
            if classify(merged.get(&e.id()), &e, &mut report) {
                merged.insert(e.id(), e);
            }
        }
        let txn = self.db.begin_write()?;
        wipe(&txn)?;
        for e in merged.values() {
            store(&txn, &remote, e)?;
        }
        write_header(&txn, header)?;
        txn.commit()?;
        sess.cipher = remote;
        sess.last_use = Instant::now();
        Ok(Absorbed {
            report,
            rekeyed: true,
            header_adopted: true,
        })
    }

    fn merge_with(&self, cipher: &XChaCha20Poly1305, incoming: Vec<Entry>) -> Result<MergeReport> {
        let local = self.snapshot(cipher)?;
        let mut report = MergeReport::default();
        let txn = self.db.begin_write()?;
        for e in incoming {
            self.observe(e.stamp());
            if classify(local.get(&e.id()), &e, &mut report) {
                store(&txn, cipher, &e)?;
            }
        }
        txn.commit()?;
        Ok(report)
    }

    fn snapshot(&self, cipher: &XChaCha20Poly1305) -> Result<BTreeMap<Id, Entry>> {
        self.snapshot_counting(cipher, &mut 0)
    }

    fn snapshot_counting(
        &self,
        cipher: &XChaCha20Poly1305,
        skipped: &mut usize,
    ) -> Result<BTreeMap<Id, Entry>> {
        let txn = self.db.begin_read()?;
        let mut out = BTreeMap::new();
        gather::<HostProfile>(&txn, cipher, &mut out, skipped)?;
        gather::<IdentityKey>(&txn, cipher, &mut out, skipped)?;
        gather::<TunnelSpec>(&txn, cipher, &mut out, skipped)?;
        gather::<Snippet>(&txn, cipher, &mut out, skipped)?;
        gather::<KnownHost>(&txn, cipher, &mut out, skipped)?;
        gather::<SyncTarget>(&txn, cipher, &mut out, skipped)?;
        gather::<Tombstone>(&txn, cipher, &mut out, skipped)?;
        Ok(out)
    }

    fn with_cipher<T>(&self, f: impl FnOnce(&XChaCha20Poly1305) -> Result<T>) -> Result<T> {
        let mut guard = self.guard();
        let sess = guard.as_mut().ok_or(VaultError::Locked)?;
        sess.last_use = Instant::now();
        f(&sess.cipher)
    }

    fn install(&self, cipher: XChaCha20Poly1305) {
        *self.guard() = Some(Session {
            cipher,
            last_use: Instant::now(),
        });
        self.state.send_replace(LockState::Unlocked);
    }

    fn guard(&self) -> MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_header(&self) -> Result<Option<KeyHeader>> {
        let txn = self.db.begin_read()?;
        let t = match txn.open_table(META) {
            Ok(t) => t,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let raw = t.get(HEADER_KEY)?;
        raw.map(|v| KeyHeader::decode(v.value())).transpose()
    }

    fn read_format(&self) -> Result<Option<u32>> {
        let txn = self.db.begin_read()?;
        let t = match txn.open_table(META) {
            Ok(t) => t,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let raw = t.get(FORMAT_KEY)?;
        Ok(raw.and_then(|v| v.value().try_into().ok().map(u32::from_le_bytes)))
    }

    fn header(&self) -> Result<KeyHeader> {
        self.read_header()?.ok_or(VaultError::Uninitialized)
    }

    fn tick(&self) -> u64 {
        let now = now_ms();
        let prev = self
            .clock
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
                Some(now.max(last.saturating_add(1)))
            })
            .unwrap_or_else(|v| v);
        now.max(prev.saturating_add(1))
    }

    fn observe(&self, stamp: u64) {
        self.clock.fetch_max(stamp, Ordering::SeqCst);
    }
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
