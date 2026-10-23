use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub at_ms: u64,
    pub host: HostInfo,
    pub cpu: CpuStat,
    pub mem: MemStat,
    pub net: Vec<NetIf>,
    pub disks: Vec<Disk>,
    pub docker: Docker,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HostInfo {
    pub hostname: String,
    pub kernel: String,
    pub uptime_secs: u64,
    pub cores: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CpuStat {
    pub busy_pct: f32,
    pub per_core: Vec<f32>,
    pub load: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemStat {
    pub total_kib: u64,
    pub available_kib: u64,
    pub used_pct: f32,
    pub swap_total_kib: u64,
    pub swap_used_kib: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NetIf {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Disk {
    pub mount: String,
    pub fs: String,
    pub total_kib: u64,
    pub used_kib: u64,
    pub used_pct: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Docker {
    #[default]
    Unknown,
    Available {
        containers: Vec<Container>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}
