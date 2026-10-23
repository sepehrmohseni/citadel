use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use russh_sftp::protocol::{
    Attrs, Data, File as Entry, FileAttributes, Handle, Name, OpenFlags, Packet, Status,
    StatusCode, Version,
};
use russh_sftp::server::{Handler, StatusReply};

enum Slot {
    File(File),
    Dir(Option<Vec<Entry>>),
}

pub struct Fs {
    root: PathBuf,
    slots: HashMap<String, Slot>,
    next: u64,
}

impl Fs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            slots: HashMap::new(),
            next: 0,
        }
    }

    fn local(&self, remote: &str) -> PathBuf {
        let mut out = self.root.clone();
        for c in Path::new(remote).components() {
            if let Component::Normal(p) = c {
                out.push(p);
            }
        }
        out
    }

    fn slot(&mut self, s: Slot) -> String {
        self.next += 1;
        let h = format!("h{}", self.next);
        self.slots.insert(h.clone(), s);
        h
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

fn io_err(e: &std::io::Error) -> StatusReply {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile.into(),
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied.into(),
        _ => StatusCode::Failure.into(),
    }
}

fn attrs_of(meta: &fs::Metadata) -> FileAttributes {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    FileAttributes {
        size: Some(meta.len()),
        permissions: Some(meta.permissions().mode()),
        mtime: u32::try_from(meta.mtime()).ok(),
        atime: u32::try_from(meta.atime()).ok(),
        ..FileAttributes::default()
    }
}

fn take_str(cur: &mut &[u8]) -> Option<String> {
    let (h, rest) = cur.split_first_chunk::<4>()?;
    let len = u32::from_be_bytes(*h) as usize;
    if rest.len() < len {
        return None;
    }
    let (s, rest) = rest.split_at(len);
    *cur = rest;
    String::from_utf8(s.to_vec()).ok()
}

impl Handler for Fs {
    type Error = StatusReply;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported.into()
    }

    async fn init(
        &mut self,
        _version: u32,
        _ext: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        let mut v = Version::new();
        v.extensions
            .insert("posix-rename@openssh.com".into(), "1".into());
        v.extensions.insert("fsync@openssh.com".into(), "1".into());
        Ok(v)
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.local(&filename);
        let mut opts = OpenOptions::new();
        opts.read(flags.contains(OpenFlags::READ))
            .write(flags.contains(OpenFlags::WRITE))
            .append(flags.contains(OpenFlags::APPEND))
            .truncate(flags.contains(OpenFlags::TRUNCATE));
        if flags.contains(OpenFlags::EXCLUDE) {
            opts.create_new(true);
        } else if flags.contains(OpenFlags::CREATE) {
            opts.create(true);
        }
        if let Some(mode) = attrs.permissions {
            opts.mode(mode & 0o7777);
        }
        let file = opts.open(&path).map_err(|e| io_err(&e))?;
        Ok(Handle {
            id,
            handle: self.slot(Slot::File(file)),
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.slots
            .remove(&handle)
            .ok_or(StatusReply::from(StatusCode::Failure))?;
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(Slot::File(f)) = self.slots.get_mut(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        f.seek(SeekFrom::Start(offset)).map_err(|e| io_err(&e))?;
        let mut buf = vec![0u8; len.min(64 * 1024) as usize];
        let n = f.read(&mut buf).map_err(|e| io_err(&e))?;
        if n == 0 {
            return Err(StatusCode::Eof.into());
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(Slot::File(f)) = self.slots.get_mut(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        f.seek(SeekFrom::Start(offset)).map_err(|e| io_err(&e))?;
        f.write_all(&data).map_err(|e| io_err(&e))?;
        Ok(ok(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Slot::File(f)) = self.slots.get(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        let meta = f.metadata().map_err(|e| io_err(&e))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = fs::metadata(self.local(&path)).map_err(|e| io_err(&e))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = fs::symlink_metadata(self.local(&path)).map_err(|e| io_err(&e))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut entries = Vec::new();
        for e in fs::read_dir(self.local(&path)).map_err(|e| io_err(&e))? {
            let e = e.map_err(|e| io_err(&e))?;
            let meta = e.metadata().map_err(|e| io_err(&e))?;
            let name = e.file_name().to_string_lossy().into_owned();
            entries.push(Entry {
                longname: name.clone(),
                filename: name,
                attrs: attrs_of(&meta),
            });
        }
        Ok(Handle {
            id,
            handle: self.slot(Slot::Dir(Some(entries))),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(Slot::Dir(entries)) = self.slots.get_mut(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        match entries.take() {
            Some(files) => Ok(Name { id, files }),
            None => Err(StatusCode::Eof.into()),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        fs::remove_file(self.local(&filename)).map_err(|e| io_err(&e))?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        fs::create_dir(self.local(&path)).map_err(|e| io_err(&e))?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        fs::remove_dir(self.local(&path)).map_err(|e| io_err(&e))?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let mut parts: Vec<String> = Vec::new();
        for c in Path::new(&path).components() {
            match c {
                Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
                Component::ParentDir => {
                    parts.pop();
                }
                _ => {}
            }
        }
        let canon = format!("/{}", parts.join("/"));
        Ok(Name {
            id,
            files: vec![Entry::dummy(canon)],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let to = self.local(&newpath);
        if to.exists() {
            return Err(StatusCode::Failure.into());
        }
        fs::rename(self.local(&oldpath), to).map_err(|e| io_err(&e))?;
        Ok(ok(id))
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        match request.as_str() {
            "posix-rename@openssh.com" => {
                let mut cur = data.as_slice();
                let (Some(from), Some(to)) = (take_str(&mut cur), take_str(&mut cur)) else {
                    return Err(StatusCode::BadMessage.into());
                };
                fs::rename(self.local(&from), self.local(&to)).map_err(|e| io_err(&e))?;
                Ok(Packet::Status(ok(id)))
            }
            "fsync@openssh.com" => Ok(Packet::Status(ok(id))),
            _ => Err(StatusCode::OpUnsupported.into()),
        }
    }
}
