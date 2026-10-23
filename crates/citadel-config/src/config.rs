use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lex::{self, Line, quote, split_lines};
use crate::{ConfigError, Result};

const DEFAULT_INDENT: &str = "    ";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub proxy_jump: Vec<String>,
    pub identity_agent: Option<String>,
    pub forward_agent: Option<bool>,
    pub identities_only: Option<bool>,
    pub local_forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub dynamic_forwards: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    HostName,
    User,
    Port,
    IdentityFile,
    ProxyJump,
    IdentityAgent,
    ForwardAgent,
    IdentitiesOnly,
    LocalForward,
    RemoteForward,
    DynamicForward,
}

const KEYS: [(Key, &str); 11] = [
    (Key::HostName, "HostName"),
    (Key::User, "User"),
    (Key::Port, "Port"),
    (Key::IdentityFile, "IdentityFile"),
    (Key::ProxyJump, "ProxyJump"),
    (Key::IdentityAgent, "IdentityAgent"),
    (Key::ForwardAgent, "ForwardAgent"),
    (Key::IdentitiesOnly, "IdentitiesOnly"),
    (Key::LocalForward, "LocalForward"),
    (Key::RemoteForward, "RemoteForward"),
    (Key::DynamicForward, "DynamicForward"),
];

impl Key {
    fn of(lower: &str) -> Option<Self> {
        KEYS.iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(lower))
            .map(|(k, _)| *k)
    }

    fn forward(self) -> bool {
        matches!(
            self,
            Self::LocalForward | Self::RemoteForward | Self::DynamicForward
        )
    }

    fn render(self, v: &str) -> String {
        if self.forward() {
            v.split_whitespace()
                .map(quote)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            quote(v)
        }
    }

    fn line_value(self, args: &[String]) -> String {
        if self.forward() {
            args.join(" ")
        } else {
            args.first().cloned().unwrap_or_default()
        }
    }

    fn values(self, h: &SshHost) -> Vec<String> {
        let flag = |b: Option<bool>| b.map(|b| if b { "yes" } else { "no" }.to_owned());
        match self {
            Self::HostName => h.hostname.iter().cloned().collect(),
            Self::User => h.user.iter().cloned().collect(),
            Self::Port => h.port.iter().map(u16::to_string).collect(),
            Self::IdentityFile => h.identity_files.clone(),
            Self::ProxyJump if h.proxy_jump.is_empty() => Vec::new(),
            Self::ProxyJump => vec![h.proxy_jump.join(",")],
            Self::IdentityAgent => h.identity_agent.iter().cloned().collect(),
            Self::ForwardAgent => flag(h.forward_agent).into_iter().collect(),
            Self::IdentitiesOnly => flag(h.identities_only).into_iter().collect(),
            Self::LocalForward => h.local_forwards.clone(),
            Self::RemoteForward => h.remote_forwards.clone(),
            Self::DynamicForward => h.dynamic_forwards.clone(),
        }
    }
}

fn concrete(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.starts_with('!') && !pattern.contains(['*', '?'])
}

fn host_match(alias: &str, patterns: &[String]) -> bool {
    let mut hit = false;
    for p in patterns {
        match p.strip_prefix('!') {
            Some(neg) if lex::glob(neg, alias, true) => return false,
            Some(_) => {}
            None => hit |= lex::glob(p, alias, true),
        }
    }
    hit
}

fn match_all(args: &[String]) -> bool {
    args.len() == 1 && args[0].eq_ignore_ascii_case("all")
}

fn flag(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        "" => None,
        _ => Some(true),
    }
}

fn apply(h: &mut SshHost, seen: &mut Vec<Key>, key: &str, args: &[String]) {
    let Some(k) = Key::of(key) else { return };
    let first = args.first().map(String::as_str).unwrap_or_default();
    let single = !matches!(
        k,
        Key::IdentityFile | Key::LocalForward | Key::RemoteForward | Key::DynamicForward
    );
    if single {
        if seen.contains(&k) {
            return;
        }
        seen.push(k);
    }
    match k {
        Key::HostName => h.hostname = Some(first.to_owned()),
        Key::User => h.user = Some(first.to_owned()),
        Key::Port => h.port = first.parse().ok(),
        Key::IdentityAgent => h.identity_agent = Some(first.to_owned()),
        Key::ForwardAgent => h.forward_agent = flag(first),
        Key::IdentitiesOnly => h.identities_only = flag(first),
        Key::ProxyJump => {
            h.proxy_jump = if first.eq_ignore_ascii_case("none") {
                Vec::new()
            } else {
                first
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
        }
        Key::IdentityFile => {
            if !first.is_empty() && !first.eq_ignore_ascii_case("none") {
                h.identity_files.push(first.to_owned());
            }
        }
        Key::LocalForward => h.local_forwards.push(args.join(" ")),
        Key::RemoteForward => h.remote_forwards.push(args.join(" ")),
        Key::DynamicForward => h.dynamic_forwards.push(args.join(" ")),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshConfig {
    lines: Vec<Line>,
}

impl SshConfig {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: split_lines(text),
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".ssh").join("config"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(raw) => {
                let text = String::from_utf8(raw)
                    .map_err(|_| ConfigError::Encoding(path.to_path_buf()))?;
                Ok(Self::parse(&text))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(self.lines.iter().map(|l| l.text.len() + 2).sum());
        for l in &self.lines {
            out.push_str(&l.text);
            out.push_str(&l.eol);
        }
        out
    }

    pub fn aliases(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in self.lines.iter().filter_map(Line::directive) {
            if d.key != "host" {
                continue;
            }
            for p in d.args.iter().filter(|p| concrete(p)) {
                if !out.iter().any(|a| a.eq_ignore_ascii_case(p)) {
                    out.push(p.clone());
                }
            }
        }
        out
    }

    pub fn host(&self, alias: &str) -> SshHost {
        let mut h = SshHost {
            alias: alias.to_owned(),
            ..SshHost::default()
        };
        let mut seen = Vec::new();
        let mut active = true;
        for d in self.lines.iter().filter_map(Line::directive) {
            match d.key.as_str() {
                "host" => active = host_match(alias, &d.args),
                "match" => active = match_all(&d.args),
                key if active => apply(&mut h, &mut seen, key, &d.args),
                _ => {}
            }
        }
        h
    }

    pub fn hosts(&self) -> Vec<SshHost> {
        self.aliases().iter().map(|a| self.host(a)).collect()
    }

    pub(crate) fn directive_args(&self, key: &str) -> Vec<&[String]> {
        self.lines
            .iter()
            .filter_map(Line::directive)
            .filter(|d| d.key == key)
            .map(|d| d.args.as_slice())
            .collect()
    }

    fn eol(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.eol.as_str())
            .find(|e| !e.is_empty())
            .unwrap_or("\n")
            .to_owned()
    }

    fn seal(&mut self, eol: &str) {
        let n = self.lines.len();
        for l in self.lines.iter_mut().take(n.saturating_sub(1)) {
            if l.eol.is_empty() {
                l.eol = eol.to_owned();
            }
        }
    }

    fn block_end(&self, hdr: usize) -> usize {
        (hdr + 1..self.lines.len())
            .find(|&i| self.lines[i].is_header())
            .unwrap_or(self.lines.len())
    }

    fn find_block(&self, alias: &str) -> Option<usize> {
        self.lines.iter().position(|l| {
            l.directive().is_some_and(|d| {
                d.key == "host" && d.args.len() == 1 && d.args[0].eq_ignore_ascii_case(alias)
            })
        })
    }

    fn insertion_point(&self, alias: &str) -> usize {
        for (i, l) in self.lines.iter().enumerate() {
            let Some(d) = l.directive() else { continue };
            let hit = match d.key.as_str() {
                "host" => host_match(alias, &d.args),
                "match" => true,
                _ => false,
            };
            if hit {
                let mut j = i;
                while j > 0 && self.lines[j - 1].is_comment() {
                    j -= 1;
                }
                return j;
            }
        }
        self.lines.len()
    }

    pub fn upsert(&mut self, host: &SshHost) {
        let eol = self.eol();
        match self.find_block(&host.alias) {
            Some(hdr) => self.patch(hdr, host, &eol),
            None => self.insert_block(host, &eol),
        }
        self.seal(&eol);
    }

    fn patch(&mut self, hdr: usize, host: &SshHost, eol: &str) {
        let end = self.block_end(hdr);
        let body: Vec<Line> = self.lines[hdr + 1..end].to_vec();
        let mut current = SshHost::default();
        let mut seen = Vec::new();
        for d in body.iter().filter_map(Line::directive) {
            apply(&mut current, &mut seen, &d.key, &d.args);
        }
        let mut pending: Vec<(Key, &str, std::collections::VecDeque<String>)> = KEYS
            .iter()
            .filter(|(k, _)| k.values(&current) != k.values(host))
            .map(|(k, name)| (*k, *name, k.values(host).into()))
            .collect();
        if pending.is_empty() {
            return;
        }
        let indent = body
            .iter()
            .find(|l| l.directive().is_some())
            .map_or(DEFAULT_INDENT, Line::indent)
            .to_owned();
        let fresh =
            |name: &str, k: Key, v: &str| Line::new(format!("{indent}{name} {}", k.render(v)), eol);
        let last_of = |k: Key| {
            body.iter()
                .rposition(|l| l.directive().is_some_and(|d| Key::of(&d.key) == Some(k)))
        };
        let lasts: Vec<Option<usize>> = pending.iter().map(|(k, _, _)| last_of(*k)).collect();
        let anchor = body.iter().rposition(|l| l.directive().is_some());
        let mut out: Vec<Line> = Vec::with_capacity(body.len() + 8);
        let orphans = |pending: &mut Vec<(Key, &str, std::collections::VecDeque<String>)>,
                       out: &mut Vec<Line>| {
            for (slot, (k, name, wants)) in pending.iter_mut().enumerate() {
                if lasts[slot].is_none() {
                    for v in wants.drain(..) {
                        out.push(fresh(name, *k, &v));
                    }
                }
            }
        };
        if anchor.is_none() {
            orphans(&mut pending, &mut out);
        }
        for (i, line) in body.into_iter().enumerate() {
            let slot = line
                .directive()
                .and_then(|d| Key::of(&d.key))
                .and_then(|k| pending.iter().position(|(pk, _, _)| *pk == k));
            match slot {
                Some(s) => {
                    let (k, name, wants) = &mut pending[s];
                    if let Some(v) = wants.pop_front() {
                        out.push(rewrite(line, *k, &v));
                    }
                    if lasts[s] == Some(i) {
                        for v in wants.drain(..) {
                            out.push(fresh(name, *k, &v));
                        }
                    }
                }
                None => out.push(line),
            }
            if anchor == Some(i) {
                orphans(&mut pending, &mut out);
            }
        }
        self.lines.splice(hdr + 1..end, out);
    }

    fn insert_block(&mut self, host: &SshHost, eol: &str) {
        let pos = self.insertion_point(&host.alias);
        let mut chunk = Vec::new();
        if pos > 0 && !self.lines[pos - 1].is_blank() {
            chunk.push(Line::new(String::new(), eol));
        }
        chunk.push(Line::new(format!("Host {}", quote(&host.alias)), eol));
        for (k, name) in KEYS {
            for v in k.values(host) {
                chunk.push(Line::new(
                    format!("{DEFAULT_INDENT}{name} {}", k.render(&v)),
                    eol,
                ));
            }
        }
        if pos < self.lines.len() {
            chunk.push(Line::new(String::new(), eol));
        }
        self.lines.splice(pos..pos, chunk);
    }

    pub fn remove(&mut self, alias: &str) -> bool {
        let eol = self.eol();
        let mut removed = false;
        while let Some(hdr) = self.find_block(alias) {
            let end = self.block_end(hdr);
            let last = (hdr..end)
                .rev()
                .find(|&i| self.lines[i].directive().is_some())
                .unwrap_or(hdr);
            let mut start = hdr;
            while start > 0 && self.lines[start - 1].is_comment() {
                start -= 1;
            }
            if start == hdr || start == 0 || !self.lines[start - 1].is_blank() {
                start = hdr;
            }
            self.lines.drain(start..=last);
            let n = self.lines.len();
            if start > 0
                && self.lines[start - 1].is_blank()
                && (start == n || self.lines[start].is_blank())
            {
                self.lines.remove(start - 1);
            }
            removed = true;
        }
        if removed {
            self.seal(&eol);
        }
        removed
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let target = match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)?,
            _ => path.to_path_buf(),
        };
        let dir = target
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
            restrict(&dir, 0o700)?;
        }
        let next = self.render();
        match fs::read(&target) {
            Ok(prev) if prev == next.as_bytes() => return Ok(()),
            Ok(prev) => write_private(&backup_path(&target), &prev)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut tmp = tempfile::Builder::new()
            .prefix(".citadel-config-")
            .tempfile_in(&dir)?;
        tmp.write_all(next.as_bytes())?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        restrict(tmp.path(), 0o600)?;
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(())
    }
}

fn rewrite(line: Line, k: Key, v: &str) -> Line {
    let Some(d) = line.directive() else {
        return line;
    };
    if k.line_value(&d.args) == v {
        return line;
    }
    let sep = if d.key_end == d.val_start { " " } else { "" };
    let text = format!(
        "{}{sep}{}{}",
        &line.text[..d.val_start],
        k.render(v),
        &line.text[d.val_end..]
    );
    Line::new(text, &line.eol)
}

fn backup_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_owned());
    target.with_file_name(format!("{name}.citadel.bak"))
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

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    restrict(path, 0o600)
}

pub fn includes(cfg: &SshConfig, base: &Path) -> Vec<PathBuf> {
    includes_from(cfg, base, dirs::home_dir().as_deref())
}

pub(crate) fn includes_from(cfg: &SshConfig, base: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for args in cfg.directive_args("include") {
        for arg in args {
            for p in resolve(&expand(arg, base, home)) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn expand(arg: &str, base: &Path, home: Option<&Path>) -> PathBuf {
    let tilde = |rest: &str| home.map_or_else(|| base.join(rest), |h| h.join(rest));
    if arg == "~" {
        return home.map_or_else(|| base.to_path_buf(), Path::to_path_buf);
    }
    if let Some(rest) = arg.strip_prefix("~/") {
        return tilde(rest);
    }
    let p = PathBuf::from(arg);
    if p.is_absolute() { p } else { base.join(p) }
}

fn resolve(p: &Path) -> Vec<PathBuf> {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return if p.is_file() {
            vec![p.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let dir = p
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut hits: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| lex::glob(name, n, false))
        })
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    hits.sort();
    hits
}
