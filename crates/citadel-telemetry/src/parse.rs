use std::collections::HashMap;

use serde_json::Value;

use crate::model::{Container, Disk, Docker, MemStat};

const PSEUDO_FS: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fuse.gvfsd-fuse",
    "fuse.lxcfs",
    "fuse.portal",
    "fuse.snapfuse",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "rootfs",
    "rpc_pipefs",
    "securityfs",
    "shm",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
    "udev",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Jiffies {
    pub busy: u64,
    pub idle: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CpuTimes {
    pub total: Jiffies,
    pub cores: Vec<(u32, Jiffies)>,
}

fn jiffies(fields: &[&str]) -> Jiffies {
    let v = |i: usize| {
        fields
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let sum = |idx: &[usize]| idx.iter().fold(0u64, |acc, &i| acc.saturating_add(v(i)));
    Jiffies {
        busy: sum(&[0, 1, 2, 5, 6, 7]),
        idle: sum(&[3, 4]),
    }
}

pub(crate) fn stat(lines: &[String]) -> Option<CpuTimes> {
    let mut total = None;
    let mut cores = Vec::new();
    for line in lines {
        let mut it = line.split_whitespace();
        let Some(rest) = it.next().and_then(|n| n.strip_prefix("cpu")) else {
            continue;
        };
        let fields: Vec<&str> = it.collect();
        if rest.is_empty() {
            total = Some(jiffies(&fields));
        } else if let Ok(idx) = rest.parse::<u32>() {
            cores.push((idx, jiffies(&fields)));
        }
    }
    cores.sort_by_key(|(i, _)| *i);
    total.map(|total| CpuTimes { total, cores })
}

pub(crate) fn pct(prev: Jiffies, cur: Jiffies) -> f32 {
    let busy = cur.busy.saturating_sub(prev.busy);
    let idle = cur.idle.saturating_sub(prev.idle);
    let all = busy.saturating_add(idle);
    if all == 0 {
        0.0
    } else {
        (busy as f64 * 100.0 / all as f64) as f32
    }
}

pub(crate) fn meminfo(lines: &[String]) -> MemStat {
    let mut kv: HashMap<&str, u64> = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && let Some(n) = v.split_whitespace().next().and_then(|n| n.parse().ok())
        {
            kv.insert(k.trim(), n);
        }
    }
    let get = |k: &str| kv.get(k).copied().unwrap_or(0);
    let total = get("MemTotal");
    let available = kv
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| {
            get("MemFree")
                .saturating_add(get("Buffers"))
                .saturating_add(get("Cached"))
        })
        .min(total);
    let swap_total = get("SwapTotal");
    MemStat {
        total_kib: total,
        available_kib: available,
        used_pct: if total == 0 {
            0.0
        } else {
            ((total - available) as f64 * 100.0 / total as f64) as f32
        },
        swap_total_kib: swap_total,
        swap_used_kib: swap_total.saturating_sub(get("SwapFree")),
    }
}

pub(crate) fn net_dev(lines: &[String]) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = lines
        .iter()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty()
                || name == "lo"
                || name.contains(|c: char| c == '|' || c.is_whitespace())
            {
                return None;
            }
            let f: Vec<u64> = rest
                .split_whitespace()
                .map(|x| x.parse().ok())
                .collect::<Option<_>>()?;
            if f.len() < 9 {
                return None;
            }
            Some((name.to_owned(), f[0], f[8]))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub(crate) fn loadavg(lines: &[String]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    if let Some(line) = lines.first() {
        for (slot, tok) in out.iter_mut().zip(line.split_whitespace()) {
            *slot = tok.parse().unwrap_or(0.0);
        }
    }
    out
}

pub(crate) fn uptime(lines: &[String]) -> Option<f64> {
    lines
        .first()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v >= 0.0)
}

pub(crate) fn date_ns(lines: &[String]) -> Option<u64> {
    let line = lines.first()?.trim();
    if line.len() >= 16 && line.bytes().all(|b| b.is_ascii_digit()) {
        line.parse().ok()
    } else {
        None
    }
}

fn unescape(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = u32::from(b[i + 1] - b'0') * 64
                + u32::from(b[i + 2] - b'0') * 8
                + u32::from(b[i + 3] - b'0');
            match u8::try_from(v) {
                Ok(byte) => out.push(byte),
                Err(_) => out.extend_from_slice(&b[i..i + 4]),
            }
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn mounts(lines: &[String]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let _src = it.next()?;
            let mount = it.next()?;
            let fs = it.next()?;
            Some((unescape(mount), fs.to_owned()))
        })
        .collect()
}

fn split_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut rest = line.trim_start();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let end = rest.find(char::is_whitespace)?;
        out.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }
    let rest = rest.trim_end();
    if rest.is_empty() {
        None
    } else {
        Some((out, rest))
    }
}

pub(crate) fn df(lines: &[String], types: &HashMap<String, String>) -> Vec<Disk> {
    let mut out: Vec<Disk> = Vec::new();
    let mut devices: Vec<&str> = Vec::new();
    for line in lines {
        let Some((f, mount)) = split_fields(line, 5) else {
            continue;
        };
        let (Ok(total), Ok(used), Ok(avail)) = (
            f[1].parse::<u64>(),
            f[2].parse::<u64>(),
            f[3].parse::<u64>(),
        ) else {
            continue;
        };
        let fs = types.get(mount).cloned().unwrap_or_else(|| f[0].to_owned());
        let pseudo =
            PSEUDO_FS.contains(&fs.as_str()) || (types.is_empty() && PSEUDO_FS.contains(&f[0]));
        let device = f[0].starts_with("/dev/").then_some(f[0]);
        if pseudo
            || total == 0
            || out.iter().any(|d| d.mount == mount)
            || device.is_some_and(|dev| devices.contains(&dev))
        {
            continue;
        }
        if let Some(dev) = device {
            devices.push(dev);
        }
        let denom = used.saturating_add(avail);
        out.push(Disk {
            mount: mount.to_owned(),
            fs,
            total_kib: total,
            used_kib: used,
            used_pct: if denom == 0 {
                0.0
            } else {
                (used as f64 * 100.0 / denom as f64) as f32
            },
        });
    }
    out
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn from_api(v: &Value) -> Container {
    let name = v
        .get("Names")
        .and_then(Value::as_array)
        .and_then(|n| n.first())
        .and_then(Value::as_str)
        .unwrap_or_default();
    Container {
        id: short_id(&text(v, "Id")),
        name: name.trim_start_matches('/').to_owned(),
        image: text(v, "Image"),
        state: text(v, "State"),
        status: text(v, "Status"),
    }
}

fn from_cli(v: &Value) -> Container {
    let names = text(v, "Names");
    let name = names.split(',').next().unwrap_or_default();
    Container {
        id: short_id(&text(v, "ID")),
        name: name.trim_start_matches('/').to_owned(),
        image: text(v, "Image"),
        state: text(v, "State"),
        status: text(v, "Status"),
    }
}

fn classify(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("permission denied") {
        "permission denied on docker socket".to_owned()
    } else if lower.contains("cannot connect to the docker daemon")
        || lower.contains("is the docker daemon running")
    {
        "docker daemon not running".to_owned()
    } else {
        err.chars().take(200).collect()
    }
}

fn unavailable(reason: impl Into<String>) -> Docker {
    Docker::Unavailable {
        reason: reason.into(),
    }
}

pub(crate) fn docker(mode: Option<&str>, lines: &[String]) -> Docker {
    match mode {
        None => Docker::Unknown,
        Some("api") => {
            let body = lines.join("\n");
            let body = body.trim();
            match serde_json::from_str::<Value>(body) {
                Ok(Value::Array(items)) => Docker::Available {
                    containers: items.iter().map(from_api).collect(),
                },
                Ok(Value::Object(o)) => unavailable(
                    o.get("message")
                        .and_then(Value::as_str)
                        .map_or_else(|| "unexpected docker api response".to_owned(), classify),
                ),
                _ if body.is_empty() => unavailable("docker daemon did not respond"),
                _ => unavailable("unexpected docker api response"),
            }
        }
        Some("cli") => {
            let mut containers = Vec::new();
            let mut first_err = None;
            for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
                match serde_json::from_str::<Value>(line) {
                    Ok(v @ Value::Object(_)) => containers.push(from_cli(&v)),
                    _ => {
                        first_err.get_or_insert(line);
                    }
                }
            }
            match first_err {
                Some(e) if containers.is_empty() => unavailable(classify(e)),
                _ => Docker::Available { containers },
            }
        }
        Some("denied") => unavailable("permission denied on /var/run/docker.sock"),
        Some("noclient") => {
            unavailable("docker socket present but neither curl nor docker client is installed")
        }
        Some("absent") => unavailable("docker not installed"),
        Some(other) => unavailable(format!("unknown docker probe mode {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }

    const STAT: &str = "cpu  179630 1 54200 2744214 30137 0 12566 0 0 0
cpu0 40496 0 13440 690098 6868 0 6936 0 0 0
cpu1 53938 0 13654 677006 7872 0 2580 0 0 0
cpu3 41874 0 13585 689162 7967 0 1151 0 0 0
cpu2 43322 0 13520 687947 7429 0 1897 0 0 0
intr 9149203 0 0 0
ctxt 22341567
btime 1761170000
procs_running 2";

    const MEMINFO: &str = "MemTotal:        8133872 kB
MemFree:         1118656 kB
MemAvailable:    5891232 kB
Buffers:          164592 kB
Cached:          4499528 kB
SwapCached:         2540 kB
SwapTotal:      12582912 kB
SwapFree:       12485648 kB
HugePages_Total:       0";

    const NETDEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 2073608178  454118    0    0    0     0          0         0 2073608178  454118    0    0    0     0       0          0
  eth0: 276385568  299034    0    0    0     0          0         0 42859633  134767    0    0    0     0       0          0
docker0: 41733440  139833    0    0    0     0          0         0 61216678  143695    0    0    0     0       0          0
  ens3:4294967296 1 0 0 0 0 0 0 7 1 0 0 0 0 0 0
 broken: 1 2 3";

    const DF: &str = "Filesystem     1024-blocks      Used Available Capacity Mounted on
none               4066936         0   4066936       0% /usr/lib/modules/6.6.87.2-microsoft-standard-WSL2
none               4066936         4   4066932       1% /mnt/wsl
drivers          498943996 393909216 105034780      79% /usr/lib/wsl/drivers
/dev/sdd        1055762868 194597528 807461868      20% /
/dev/sdd        1055762868 194597528 807461868      20% /
/dev/sdd        1055762868 194597528 807461868      20% /mnt/wslg/distro
/dev/loop3           65408     65408         0     100% /snap/core20/2922
/dev/sde          10255636   2048000   7667016      22% /srv/My Data
sysfs                    0         0         0       -  /sys
garbage line";

    const MOUNTS: &str =
        "none /usr/lib/modules/6.6.87.2-microsoft-standard-WSL2 overlay rw,nosuid 0 0
none /mnt/wsl tmpfs rw,relatime 0 0
drivers /usr/lib/wsl/drivers 9p ro,nosuid 0 0
/dev/sdd / ext4 rw,relatime 0 0
/dev/sdd /mnt/wslg/distro ext4 ro,relatime 0 0
/dev/loop3 /snap/core20/2922 fuse.snapfuse ro 0 0
/dev/sde /srv/My\\040Data xfs rw 0 0
sysfs /sys sysfs rw 0 0";

    #[test]
    fn stat_aggregate_and_cores_sorted() {
        let t = stat(&lines(STAT)).unwrap();
        assert_eq!(
            t.total,
            Jiffies {
                busy: 179630 + 1 + 54200 + 12566,
                idle: 2744214 + 30137
            }
        );
        let idx: Vec<u32> = t.cores.iter().map(|(i, _)| *i).collect();
        assert_eq!(idx, vec![0, 1, 2, 3]);
        assert_eq!(
            t.cores[0].1,
            Jiffies {
                busy: 40496 + 13440 + 6936,
                idle: 690098 + 6868
            }
        );
        assert!(stat(&lines("intr 1 2 3")).is_none());
    }

    #[test]
    fn busy_percentage_from_deltas() {
        let a = Jiffies {
            busy: 100,
            idle: 900,
        };
        let b = Jiffies {
            busy: 175,
            idle: 925,
        };
        assert!((pct(a, b) - 75.0).abs() < 1e-4);
        assert_eq!(pct(b, b), 0.0);
        assert_eq!(pct(b, a), 0.0);
        let huge = Jiffies {
            busy: u64::MAX,
            idle: u64::MAX,
        };
        assert!(pct(Jiffies::default(), huge).is_finite());
    }

    #[test]
    fn meminfo_with_and_without_available() {
        let m = meminfo(&lines(MEMINFO));
        assert_eq!(m.total_kib, 8133872);
        assert_eq!(m.available_kib, 5891232);
        assert!((m.used_pct - 27.57).abs() < 0.01);
        assert_eq!(m.swap_total_kib, 12582912);
        assert_eq!(m.swap_used_kib, 12582912 - 12485648);
        let legacy = meminfo(&lines(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 250 kB",
        ));
        assert_eq!(legacy.available_kib, 400);
        assert!((legacy.used_pct - 60.0).abs() < 1e-4);
        assert_eq!(meminfo(&[]), MemStat::default());
    }

    #[test]
    fn net_dev_skips_loopback_headers_and_garbage() {
        let n = net_dev(&lines(NETDEV));
        assert_eq!(
            n,
            vec![
                ("docker0".to_owned(), 41733440, 61216678),
                ("ens3".to_owned(), 4294967296, 7),
                ("eth0".to_owned(), 276385568, 42859633),
            ]
        );
    }

    #[test]
    fn loadavg_uptime_and_clock() {
        assert_eq!(
            loadavg(&lines("0.52 0.54 0.45 1/517 71636")),
            [0.52, 0.54, 0.45]
        );
        assert_eq!(loadavg(&[]), [0.0; 3]);
        assert_eq!(uptime(&lines("7564.47 27442.27")), Some(7564.47));
        assert_eq!(uptime(&lines("nan 1")), None);
        assert_eq!(
            date_ns(&lines("1792701486438765979")),
            Some(1792701486438765979)
        );
        assert_eq!(date_ns(&lines("1792701486N")), None);
        assert_eq!(date_ns(&lines("1792701486%N")), None);
    }

    #[test]
    fn mounts_unescape_octal() {
        let m = mounts(&lines(MOUNTS));
        assert_eq!(m.get("/srv/My Data").map(String::as_str), Some("xfs"));
        assert_eq!(m.get("/").map(String::as_str), Some("ext4"));
        assert_eq!(unescape("a\\011b\\134c\\04"), "a\tb\\c\\04");
        assert_eq!(unescape("\\777x"), "\\777x");
    }

    #[test]
    fn df_filters_pseudo_dedupes_and_keeps_spaces() {
        let disks = df(&lines(DF), &mounts(&lines(MOUNTS)));
        let names: Vec<(&str, &str)> = disks
            .iter()
            .map(|d| (d.mount.as_str(), d.fs.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("/usr/lib/wsl/drivers", "9p"),
                ("/", "ext4"),
                ("/srv/My Data", "xfs")
            ]
        );
        let root = &disks[1];
        assert_eq!(root.total_kib, 1055762868);
        assert_eq!(root.used_kib, 194597528);
        assert!((root.used_pct - 19.42).abs() < 0.01);
    }

    #[test]
    fn df_without_mounts_falls_back_to_source() {
        let disks = df(
            &lines(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\ntmpfs 100 1 99 1% /run\n/dev/vda1 1000 500 500 50% /",
            ),
            &HashMap::new(),
        );
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].fs, "/dev/vda1");
    }

    #[test]
    fn docker_api_json() {
        let body = r#"[{"Id":"8dfafdbc3a40aaaa","Names":["/web"],"Image":"nginx:1.27","State":"running","Status":"Up 2 hours"},{"Id":"0123","Names":[],"Image":"redis","State":"exited","Status":"Exited (0) 3 days ago"}]"#;
        let d = docker(Some("api"), &lines(body));
        let Docker::Available { containers } = d else {
            panic!("{d:?}")
        };
        assert_eq!(
            containers[0],
            Container {
                id: "8dfafdbc3a40".into(),
                name: "web".into(),
                image: "nginx:1.27".into(),
                state: "running".into(),
                status: "Up 2 hours".into(),
            }
        );
        assert_eq!(containers[1].name, "");
        assert_eq!(
            docker(Some("api"), &lines("[]")),
            Docker::Available { containers: vec![] }
        );
        assert_eq!(
            docker(
                Some("api"),
                &lines(r#"{"message":"client version 1.52 is too new"}"#)
            ),
            Docker::Unavailable {
                reason: "client version 1.52 is too new".into()
            }
        );
        assert!(matches!(
            docker(Some("api"), &[]),
            Docker::Unavailable { .. }
        ));
        assert!(matches!(
            docker(Some("api"), &lines("<html>")),
            Docker::Unavailable { .. }
        ));
    }

    #[test]
    fn docker_cli_lines_and_errors() {
        let out = r#"{"Command":"\"nginx\"","ID":"8dfafdbc3a40","Image":"nginx","Names":"web,alias","State":"running","Status":"Up 5 minutes"}
{"ID":"aa11","Image":"pg","Names":"db","State":"exited","Status":"Exited (1)"}"#;
        let Docker::Available { containers } = docker(Some("cli"), &lines(out)) else {
            panic!()
        };
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "web");
        assert_eq!(containers[1].state, "exited");
        let denied = "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock: Get \"http://%2Fvar%2Frun%2Fdocker.sock/v1.47/containers/json\": dial unix /var/run/docker.sock: connect: permission denied";
        assert_eq!(
            docker(Some("cli"), &lines(denied)),
            Docker::Unavailable {
                reason: "permission denied on docker socket".into()
            }
        );
        let down = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?";
        assert_eq!(
            docker(Some("cli"), &lines(down)),
            Docker::Unavailable {
                reason: "docker daemon not running".into()
            }
        );
        assert_eq!(
            docker(Some("cli"), &[]),
            Docker::Available { containers: vec![] }
        );
    }

    #[test]
    fn docker_modes() {
        assert_eq!(docker(None, &[]), Docker::Unknown);
        for mode in ["denied", "noclient", "absent", "weird"] {
            assert!(
                matches!(docker(Some(mode), &[]), Docker::Unavailable { .. }),
                "{mode}"
            );
        }
    }
}
