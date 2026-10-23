use std::time::Duration;

pub(crate) const MARK: &str = "@@citadel:";

pub(crate) const MIN_INTERVAL: Duration = Duration::from_millis(200);
pub(crate) const MAX_INTERVAL: Duration = Duration::from_secs(3600);

const PRELUDE: &str = concat!(
    "export LC_ALL=C;",
    "M=@@citadel;",
    "s(){ printf \"%s:%s\\n\" \"$M\" \"$1\"; };",
    "d=/var/run/docker.sock;",
    "dk(){ if [ -S $d ];then ",
    "if [ -r $d ]&&[ -w $d ];then ",
    "if command -v curl >/dev/null 2>&1;then s \"docker api\";",
    "curl -s --max-time 2 --unix-socket $d \"http://localhost/containers/json?all=1\";echo;",
    "elif command -v docker >/dev/null 2>&1;then s \"docker cli\";",
    "docker ps -a --format \"{{json .}}\" 2>&1;",
    "else s \"docker noclient\";fi;",
    "else s \"docker denied\";fi;",
    "elif command -v docker >/dev/null 2>&1;then s \"docker cli\";",
    "docker ps -a --format \"{{json .}}\" 2>&1;",
    "else s \"docker absent\";fi; };",
    "emit(){ s \"frame $1\";",
    "s date;date +%s%N 2>/dev/null;",
    "s uptime;cat /proc/uptime 2>/dev/null;",
    "s stat;cat /proc/stat 2>/dev/null;",
    "s net;cat /proc/net/dev 2>/dev/null;",
    "if [ \"$1\" = full ];then ",
    "s hostname;hostname 2>/dev/null||cat /proc/sys/kernel/hostname 2>/dev/null;",
    "s kernel;uname -sr 2>/dev/null;",
    "s nproc;nproc 2>/dev/null;",
    "s load;cat /proc/loadavg 2>/dev/null;",
    "s mem;cat /proc/meminfo 2>/dev/null;",
    "s mounts;cat /proc/mounts 2>/dev/null;",
    "s df;if command -v timeout >/dev/null 2>&1;then timeout 3 df -P -k 2>/dev/null;",
    "else df -P -k 2>/dev/null;fi;",
    "dk;fi;",
    "s end; };",
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Once,
    Every(Duration),
}

fn nap(d: Duration) -> String {
    let secs = d.as_secs_f64();
    format!(
        "sleep {secs:.3} 2>/dev/null||sleep {}",
        secs.ceil().max(1.0)
    )
}

pub(crate) fn clamp(ivl: Duration) -> Duration {
    ivl.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

pub(crate) fn command(mode: Mode) -> String {
    let prime = nap(Duration::from_millis(250));
    let body = match mode {
        Mode::Once => format!("{PRELUDE}emit prime;{prime};emit full"),
        Mode::Every(ivl) => format!(
            "{PRELUDE}emit prime;{prime};while :;do emit full;{};done",
            nap(clamp(ivl))
        ),
    };
    format!("exec sh -c '{}'", body.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_single_line_and_quote_safe() {
        for mode in [Mode::Once, Mode::Every(Duration::from_secs(2))] {
            let cmd = command(mode);
            assert!(cmd.starts_with("exec sh -c '"));
            assert!(cmd.ends_with('\''));
            assert!(!cmd.contains('\n'));
            assert!(!cmd.contains('!'));
            assert_eq!(cmd.matches('\'').count(), 2);
        }
    }

    #[test]
    fn interval_is_clamped() {
        assert!(command(Mode::Every(Duration::ZERO)).contains("sleep 0.200 2>/dev/null||sleep 1"));
        assert!(
            command(Mode::Every(Duration::from_millis(2500)))
                .contains("sleep 2.500 2>/dev/null||sleep 3")
        );
        assert_eq!(clamp(Duration::from_secs(10_000)), MAX_INTERVAL);
    }
}
