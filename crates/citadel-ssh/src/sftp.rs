use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt, stream};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use serde::Serialize;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{Result, Session, TransportError};

const CHUNK: u32 = 32 * 1024;
const MAX_ENTRIES: usize = 200_000;
const DEPTH: usize = 64;
const POSIX_RENAME: &str = "posix-rename@openssh.com";
const FSYNC: &str = "fsync@openssh.com";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub mtime: u32,
    pub is_dir: bool,
    pub is_link: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FileStat {
    pub size: u64,
    pub mode: u32,
    pub mtime: u32,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

pub struct Sftp {
    raw: Arc<RawSftpSession>,
    posix_rename: bool,
    fsync: bool,
    depth: usize,
}

fn is_eof(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

fn is_missing(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::NoSuchFile)
}

fn stat_of(a: &FileAttributes) -> FileStat {
    let mode = a.permissions.unwrap_or(0);
    FileStat {
        size: a.size.unwrap_or(0),
        mode,
        mtime: a.mtime.unwrap_or(0),
        is_dir: mode & 0o170000 == 0o040000,
    }
}

fn ssh_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&u32::try_from(s.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn sibling_tmp(path: &str) -> Result<String> {
    let mut tag = [0u8; 6];
    getrandom::fill(&mut tag).map_err(|e| TransportError::Io(std::io::Error::other(e)))?;
    let suffix: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    Ok(format!("{dir}.{name}.citadel-{suffix}"))
}

fn part_path(local: &Path) -> PathBuf {
    let mut name = local
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".citadel-part");
    local.with_file_name(name)
}

impl Session {
    pub async fn sftp(&self) -> Result<Sftp> {
        let ch = self.open_session_channel().await?;
        ch.request_subsystem(true, "sftp").await?;
        let raw = RawSftpSession::new(ch.into_stream());
        let version = raw.init().await?;
        Ok(Sftp {
            raw: Arc::new(raw),
            posix_rename: version.extensions.contains_key(POSIX_RENAME),
            fsync: version.extensions.contains_key(FSYNC),
            depth: DEPTH,
        })
    }
}

impl Sftp {
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth.max(1);
        self
    }

    pub async fn realpath(&self, path: &str) -> Result<String> {
        let name = self.raw.realpath(path).await?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| TransportError::BadPath(path.to_owned()))
    }

    pub async fn stat(&self, path: &str) -> Result<FileStat> {
        Ok(stat_of(&self.raw.stat(path).await?.attrs))
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        match self.raw.stat(path).await {
            Ok(_) => Ok(true),
            Err(e) if is_missing(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn list(&self, path: &str) -> Result<Vec<DirEntry>> {
        let handle = self.raw.opendir(path).await?.handle;
        let mut out = Vec::new();
        let listing = loop {
            match self.raw.readdir(handle.as_str()).await {
                Ok(name) => {
                    for f in name.files {
                        if f.filename == "." || f.filename == ".." {
                            continue;
                        }
                        if out.len() >= MAX_ENTRIES {
                            let _ = self.raw.close(handle.clone()).await;
                            return Err(TransportError::Io(std::io::Error::other(format!(
                                "directory lists more than {MAX_ENTRIES} entries"
                            ))));
                        }
                        let st = stat_of(&f.attrs);
                        out.push(DirEntry {
                            name: f.filename,
                            size: st.size,
                            mode: st.mode,
                            mtime: st.mtime,
                            is_dir: st.is_dir,
                            is_link: st.mode & 0o170000 == 0o120000,
                        });
                    }
                }
                Err(e) if is_eof(&e) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        let _ = self.raw.close(handle).await;
        listing?;
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.raw.mkdir(path, FileAttributes::default()).await?;
        Ok(())
    }

    pub async fn remove(&self, path: &str) -> Result<()> {
        self.raw.remove(path).await?;
        Ok(())
    }

    pub async fn rmdir(&self, path: &str) -> Result<()> {
        self.raw.rmdir(path).await?;
        Ok(())
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        if self.posix_rename {
            let mut data = Vec::with_capacity(from.len() + to.len() + 8);
            ssh_string(&mut data, from);
            ssh_string(&mut data, to);
            return match self.raw.extended(POSIX_RENAME, data).await? {
                Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(()),
                Packet::Status(s) => Err(SftpError::Status(s).into()),
                _ => Err(SftpError::UnexpectedPacket.into()),
            };
        }
        self.raw.rename(from, to).await?;
        Ok(())
    }

    async fn read_span(&self, handle: &str, offset: u64, len: u32) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(len as usize);
        while buf.len() < len as usize {
            let got = buf.len() as u64;
            let want = len - u32::try_from(got).unwrap_or(len);
            match self.raw.read(handle, offset + got, want).await {
                Ok(d) if d.data.is_empty() => break,
                Ok(d) => buf.extend_from_slice(&d.data),
                Err(e) if is_eof(&e) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(buf)
    }

    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64> {
        let handle = self
            .raw
            .open(remote, OpenFlags::READ, FileAttributes::default())
            .await?
            .handle;
        let total = self
            .raw
            .fstat(handle.as_str())
            .await
            .map(|a| a.attrs.size.unwrap_or(0))
            .unwrap_or(0);
        let part = part_path(local);
        let result = self.pull(&handle, &part, total, progress).await;
        let _ = self.raw.close(handle).await;
        match result {
            Ok(n) => {
                fs::rename(&part, local).await?;
                Ok(n)
            }
            Err(e) => {
                let _ = fs::remove_file(&part).await;
                Err(e)
            }
        }
    }

    async fn pull(
        &self,
        handle: &str,
        part: &Path,
        total: u64,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64> {
        let mut file = fs::File::create(part).await?;
        let mut done = 0u64;
        let chunks = total.div_ceil(u64::from(CHUNK));
        let mut spans = stream::iter(0..chunks)
            .map(|i| self.read_span(handle, i * u64::from(CHUNK), CHUNK))
            .buffered(self.depth);
        let mut short = false;
        while let Some(span) = spans.next().await {
            let span = span?;
            if short && !span.is_empty() {
                return Err(TransportError::Io(std::io::Error::other(
                    "remote file shrank during transfer",
                )));
            }
            short |= span.len() < CHUNK as usize;
            file.write_all(&span).await?;
            done += span.len() as u64;
            progress(Progress {
                done,
                total: total.max(done),
            });
            if short {
                break;
            }
        }
        drop(spans);
        if !short {
            loop {
                let span = self.read_span(handle, done, CHUNK).await?;
                if span.is_empty() {
                    break;
                }
                file.write_all(&span).await?;
                done += span.len() as u64;
                progress(Progress {
                    done,
                    total: total.max(done),
                });
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(done)
    }

    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64> {
        let src = fs::File::open(local).await?;
        let meta = src.metadata().await?;
        let mode = local_mode(&meta);
        self.commit(remote, mode, Source::File(src), meta.len(), progress)
            .await
    }

    pub async fn read_file(&self, remote: &str, cap: u64) -> Result<Vec<u8>> {
        let handle = self
            .raw
            .open(remote, OpenFlags::READ, FileAttributes::default())
            .await?
            .handle;
        let result = async {
            let size = self
                .raw
                .fstat(handle.as_str())
                .await?
                .attrs
                .size
                .unwrap_or(0);
            if size > cap {
                return Err(TransportError::Io(std::io::Error::other(format!(
                    "{remote} is {size} bytes, limit is {cap}"
                ))));
            }
            let chunks = size.div_ceil(u64::from(CHUNK));
            let spans: Vec<Vec<u8>> = stream::iter(0..chunks)
                .map(|i| self.read_span(&handle, i * u64::from(CHUNK), CHUNK))
                .buffered(self.depth)
                .try_collect()
                .await?;
            Ok(spans.concat())
        }
        .await;
        let _ = self.raw.close(handle).await;
        result
    }

    pub async fn write_atomic(&self, remote: &str, data: &[u8]) -> Result<()> {
        let mode = match self.raw.stat(remote).await {
            Ok(a) => a.attrs.permissions.map(|p| p & 0o7777),
            Err(e) if is_missing(&e) => None,
            Err(e) => return Err(e.into()),
        };
        let noop = |_: Progress| {};
        self.commit(remote, mode, Source::Bytes(data), data.len() as u64, &noop)
            .await?;
        Ok(())
    }

    async fn commit(
        &self,
        remote: &str,
        mode: Option<u32>,
        src: Source<'_>,
        total: u64,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64> {
        let tmp = sibling_tmp(remote)?;
        let attrs = FileAttributes {
            permissions: mode,
            ..FileAttributes::default()
        };
        let handle = self
            .raw
            .open(
                tmp.as_str(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                attrs,
            )
            .await?
            .handle;
        let pushed = self.push(&handle, src, total, progress).await;
        let synced = match &pushed {
            Ok(_) if self.fsync => self
                .raw
                .fsync(handle.as_str())
                .await
                .map(|_| ())
                .map_err(Into::into),
            _ => Ok(()),
        };
        let closed = self.raw.close(handle).await;
        let outcome = pushed
            .and_then(|n| synced.map(|()| n))
            .and_then(|n| closed.map(|_| n).map_err(Into::into));
        let n = match outcome {
            Ok(n) => n,
            Err(e) => {
                let _ = self.raw.remove(tmp.as_str()).await;
                return Err(e);
            }
        };
        if let Err(e) = self.place(&tmp, remote).await {
            let _ = self.raw.remove(tmp.as_str()).await;
            return Err(e);
        }
        Ok(n)
    }

    async fn place(&self, tmp: &str, remote: &str) -> Result<()> {
        if self.posix_rename {
            return self.rename(tmp, remote).await;
        }
        match self.raw.rename(tmp, remote).await {
            Ok(_) => Ok(()),
            Err(_) => {
                match self.raw.remove(remote).await {
                    Ok(_) => {}
                    Err(e) if is_missing(&e) => {}
                    Err(e) => return Err(e.into()),
                }
                self.raw.rename(tmp, remote).await?;
                Ok(())
            }
        }
    }

    async fn push(
        &self,
        handle: &str,
        mut src: Source<'_>,
        total: u64,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64> {
        let mut inflight = FuturesUnordered::new();
        let (mut offset, mut done, mut eof) = (0u64, 0u64, false);
        loop {
            while !eof && inflight.len() < self.depth {
                let block = src.next_block().await?;
                if block.is_empty() {
                    eof = true;
                    break;
                }
                let n = block.len() as u64;
                let (raw, h, at) = (self.raw.clone(), handle.to_owned(), offset);
                inflight.push(async move { raw.write(h, at, block).await.map(|_| n) });
                offset += n;
            }
            match inflight.next().await {
                Some(written) => {
                    done += written?;
                    progress(Progress {
                        done,
                        total: total.max(done),
                    });
                }
                None => return Ok(done),
            }
        }
    }
}

enum Source<'a> {
    File(fs::File),
    Bytes(&'a [u8]),
}

impl Source<'_> {
    async fn next_block(&mut self) -> Result<Vec<u8>> {
        match self {
            Self::File(f) => {
                let mut buf = vec![0u8; CHUNK as usize];
                let mut filled = 0;
                while filled < buf.len() {
                    let n = f.read(&mut buf[filled..]).await?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                buf.truncate(filled);
                Ok(buf)
            }
            Self::Bytes(b) => {
                let take = b.len().min(CHUNK as usize);
                let (head, rest) = b.split_at(take);
                *b = rest;
                Ok(head.to_vec())
            }
        }
    }
}

#[cfg(unix)]
fn local_mode(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn local_mode(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}
