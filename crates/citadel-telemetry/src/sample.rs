use std::collections::HashMap;

use crate::decode::RawFrame;
use crate::model::{CpuStat, HostInfo, NetIf, Snapshot};
use crate::parse::{self, CpuTimes};
use crate::{Result, TelemetryError};

#[derive(Clone, Debug, Default)]
struct Sample {
    date_ns: Option<u64>,
    uptime: Option<f64>,
    cpu: Option<CpuTimes>,
    net: Vec<(String, u64, u64)>,
}

impl Sample {
    fn of(f: &RawFrame) -> Self {
        Self {
            date_ns: parse::date_ns(f.section("date")),
            uptime: parse::uptime(f.section("uptime")),
            cpu: parse::stat(f.section("stat")),
            net: parse::net_dev(f.section("net")),
        }
    }

    fn elapsed_since(&self, prev: &Self) -> Option<f64> {
        let by_date = match (prev.date_ns, self.date_ns) {
            (Some(a), Some(b)) if b > a => Some((b - a) as f64 / 1e9),
            _ => None,
        };
        by_date.or(match (prev.uptime, self.uptime) {
            (Some(a), Some(b)) if b > a => Some(b - a),
            _ => None,
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct Sampler {
    prev: Option<Sample>,
}

fn rate(prev: Option<u64>, cur: u64, secs: Option<f64>) -> f64 {
    match (prev, secs) {
        (Some(p), Some(s)) if s > 0.0 => cur.saturating_sub(p) as f64 / s,
        _ => 0.0,
    }
}

impl Sampler {
    pub fn feed(&mut self, f: &RawFrame, at_ms: u64) -> Option<Result<Snapshot>> {
        let cur = Sample::of(f);
        if !f.full {
            self.prev = Some(cur);
            return None;
        }
        let out = build(f, self.prev.as_ref(), &cur, at_ms);
        self.prev = Some(cur);
        Some(out)
    }
}

fn build(f: &RawFrame, prev: Option<&Sample>, cur: &Sample, at_ms: u64) -> Result<Snapshot> {
    let kernel = f.first("kernel").unwrap_or_default().to_owned();
    let mem = parse::meminfo(f.section("mem"));
    if cur.cpu.is_none() && mem.total_kib == 0 {
        return Err(TelemetryError::Unsupported {
            kernel: if kernel.is_empty() {
                "unknown".into()
            } else {
                kernel
            },
        });
    }
    let secs = prev.and_then(|p| cur.elapsed_since(p));
    let prev_cpu = prev.and_then(|p| p.cpu.as_ref());
    let (busy_pct, per_core) = match &cur.cpu {
        Some(c) => {
            let busy = prev_cpu.map_or(0.0, |p| parse::pct(p.total, c.total));
            let cores = c
                .cores
                .iter()
                .map(|(idx, j)| {
                    prev_cpu
                        .and_then(|p| p.cores.iter().find(|(i, _)| i == idx))
                        .map_or(0.0, |(_, pj)| parse::pct(*pj, *j))
                })
                .collect();
            (busy, cores)
        }
        None => (0.0, Vec::new()),
    };
    let cores = f
        .first("nproc")
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| u32::try_from(per_core.len()).unwrap_or(u32::MAX));
    let prev_net: HashMap<&str, (u64, u64)> = prev
        .map(|p| {
            p.net
                .iter()
                .map(|(n, rx, tx)| (n.as_str(), (*rx, *tx)))
                .collect()
        })
        .unwrap_or_default();
    let net = cur
        .net
        .iter()
        .map(|(name, rx, tx)| {
            let before = prev_net.get(name.as_str());
            NetIf {
                name: name.clone(),
                rx_bytes: *rx,
                tx_bytes: *tx,
                rx_bps: rate(before.map(|b| b.0), *rx, secs),
                tx_bps: rate(before.map(|b| b.1), *tx, secs),
            }
        })
        .collect();
    let types = parse::mounts(f.section("mounts"));
    Ok(Snapshot {
        at_ms,
        host: HostInfo {
            hostname: f.first("hostname").unwrap_or_default().to_owned(),
            kernel,
            uptime_secs: cur.uptime.map_or(0, |u| u as u64),
            cores,
        },
        cpu: CpuStat {
            busy_pct,
            per_core,
            load: parse::loadavg(f.section("load")),
        },
        mem,
        net,
        disks: parse::df(f.section("df"), &types),
        docker: parse::docker(f.docker_mode.as_deref(), f.section("docker")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Decoder;
    use crate::model::Docker;

    fn frame(
        kind: &str,
        ns: u64,
        up: f64,
        cpu: (u64, u64),
        core1: (u64, u64),
        eth: (u64, u64),
    ) -> String {
        let mut s = format!(
            "@@citadel:frame {kind}\n@@citadel:date\n{ns}\n@@citadel:uptime\n{up:.2} 1.00\n"
        );
        s += &format!(
            "@@citadel:stat\ncpu  {} 0 0 {} 0 0 0 0 0 0\ncpu0 {} 0 0 {} 0 0 0 0 0 0\ncpu1 {} 0 0 {} 0 0 0 0 0 0\n",
            cpu.0,
            cpu.1,
            cpu.0 - core1.0,
            cpu.1 - core1.1,
            core1.0,
            core1.1
        );
        s += &format!(
            "@@citadel:net\n  eth0: {} 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0\n",
            eth.0, eth.1
        );
        if kind == "full" {
            s += "@@citadel:hostname\nweb-01\n@@citadel:kernel\nLinux 6.8.0\n@@citadel:nproc\n2\n";
            s += "@@citadel:load\n1.50 0.75 0.25 2/300 99\n@@citadel:mem\nMemTotal: 1000 kB\nMemAvailable: 250 kB\n";
            s += "@@citadel:mounts\n/dev/vda1 / ext4 rw 0 0\n@@citadel:df\nFilesystem 1024-blocks Used Available Capacity Mounted on\n/dev/vda1 1000 400 600 40% /\n";
            s += "@@citadel:docker absent\n";
        }
        s + "@@citadel:end\n"
    }

    fn decode(text: &str) -> Vec<RawFrame> {
        Decoder::default().push(text.as_bytes())
    }

    #[test]
    fn prime_then_full_yields_real_rates() {
        let text = frame(
            "prime",
            1_000_000_000_000_000_000,
            100.0,
            (100, 900),
            (40, 460),
            (1_000, 2_000),
        ) + &frame(
            "full",
            1_000_000_000_250_000_000,
            100.25,
            (175, 925),
            (90, 470),
            (251_000, 52_000),
        );
        let frames = decode(&text);
        let mut s = Sampler::default();
        assert!(s.feed(&frames[0], 1).is_none());
        let snap = s.feed(&frames[1], 2).unwrap().unwrap();
        assert_eq!(snap.at_ms, 2);
        assert!((snap.cpu.busy_pct - 75.0).abs() < 1e-3);
        assert!((snap.cpu.per_core[1] - 50.0 * 100.0 / 60.0).abs() < 1e-3);
        assert!((snap.cpu.per_core[0] - 25.0 * 100.0 / 40.0).abs() < 1e-3);
        assert_eq!(snap.net[0].rx_bps, 1_000_000.0);
        assert_eq!(snap.net[0].tx_bps, 200_000.0);
        assert_eq!(snap.host.cores, 2);
        assert_eq!(snap.host.uptime_secs, 100);
        assert_eq!(snap.host.hostname, "web-01");
        assert_eq!(snap.cpu.load, [1.5, 0.75, 0.25]);
        assert!((snap.mem.used_pct - 75.0).abs() < 1e-4);
        assert_eq!(snap.disks.len(), 1);
        assert!((snap.disks[0].used_pct - 40.0).abs() < 1e-4);
        assert_eq!(
            snap.docker,
            Docker::Unavailable {
                reason: "docker not installed".into()
            }
        );
    }

    #[test]
    fn full_without_prime_reports_zero_rates() {
        let frames = decode(&frame(
            "full",
            5_000_000_000_000_000,
            10.0,
            (10, 90),
            (5, 45),
            (500, 500),
        ));
        let snap = Sampler::default().feed(&frames[0], 0).unwrap().unwrap();
        assert_eq!(snap.cpu.busy_pct, 0.0);
        assert_eq!(snap.cpu.per_core, vec![0.0, 0.0]);
        assert_eq!(snap.net[0].rx_bytes, 500);
        assert_eq!(snap.net[0].rx_bps, 0.0);
    }

    #[test]
    fn counter_reset_clamps_to_zero() {
        let text = frame("prime", 0, 50.0, (500, 500), (250, 250), (9_000, 9_000))
            + &frame("full", 0, 51.0, (10, 20), (5, 10), (10, 10));
        let frames = decode(&text);
        let mut s = Sampler::default();
        s.feed(&frames[0], 0);
        let snap = s.feed(&frames[1], 0).unwrap().unwrap();
        assert_eq!(snap.cpu.busy_pct, 0.0);
        assert_eq!(snap.net[0].rx_bps, 0.0);
        assert_eq!(snap.net[0].tx_bps, 0.0);
    }

    #[test]
    fn uptime_used_when_date_lacks_nanoseconds() {
        let text = frame("prime", 0, 200.0, (0, 0), (0, 0), (0, 0))
            .replace("@@citadel:date\n0\n", "@@citadel:date\n1700000000N\n")
            + &frame("full", 0, 202.0, (0, 0), (0, 0), (4_000, 0))
                .replace("@@citadel:date\n0\n", "@@citadel:date\n1700000002N\n");
        let frames = decode(&text);
        let mut s = Sampler::default();
        s.feed(&frames[0], 0);
        let snap = s.feed(&frames[1], 0).unwrap().unwrap();
        assert_eq!(snap.net[0].rx_bps, 2_000.0);
    }

    #[test]
    fn missing_procfs_is_unsupported() {
        let frames = decode(
            "@@citadel:frame full\n@@citadel:kernel\nDarwin 24.1.0\n@@citadel:stat\n@@citadel:mem\n@@citadel:end\n",
        );
        let err = Sampler::default().feed(&frames[0], 0).unwrap().unwrap_err();
        assert!(
            matches!(err, TelemetryError::Unsupported { ref kernel } if kernel == "Darwin 24.1.0")
        );
    }

    #[test]
    fn nproc_absent_counts_cores() {
        let text = frame("full", 0, 1.0, (2, 2), (1, 1), (0, 0))
            .replace("@@citadel:nproc\n2\n", "@@citadel:nproc\n");
        let snap = Sampler::default()
            .feed(&decode(&text)[0], 0)
            .unwrap()
            .unwrap();
        assert_eq!(snap.host.cores, 2);
    }
}
