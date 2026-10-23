#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runs="${RUNS:-5}"

: "${CITADEL_SSHD:=/usr/sbin/sshd}"
: "${CITADEL_SFTP_SERVER:=/usr/lib/openssh/sftp-server}"
export CITADEL_SSHD CITADEL_SFTP_SERVER

if [[ ! -x "$CITADEL_SSHD" ]]; then
    echo "error: no sshd at $CITADEL_SSHD; install openssh-server or set CITADEL_SSHD" >&2
    exit 1
fi
if [[ ! -x "$CITADEL_SFTP_SERVER" ]]; then
    echo "error: no sftp-server at $CITADEL_SFTP_SERVER; set CITADEL_SFTP_SERVER" >&2
    exit 1
fi

cd "$root"
echo "building release test binaries" >&2
cargo test --release -p citadel-ssh --test openssh --no-run >&2

echo "host:   $(uname -srm)"
if [[ -r /proc/cpuinfo ]]; then
    echo "cpu:    $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"
fi
echo "sshd:   $("$CITADEL_SSHD" -V 2>&1 | head -n1 || true)"
echo "runs:   $runs"
echo

for _ in $(seq 1 "$runs"); do
    cargo test --release -q -p citadel-ssh --test openssh real_sftp_large_and_atomic -- --nocapture 2>/dev/null \
        | grep -o 'sftp 16MiB up [^ ]* down [^ ]*' || true
done | awk -v bytes=16778450 '
function ms(v) {
    if (v ~ /ms$/) { sub(/ms$/, "", v); return v + 0 }
    if (v ~ /µs$/) { sub(/µs$/, "", v); return v / 1000 }
    if (v ~ /ns$/) { sub(/ns$/, "", v); return v / 1000000 }
    sub(/s$/, "", v); return v * 1000
}
function rate(t) { return (bytes / 1048576) / (t / 1000) }
{
    up = ms($4); down = ms($6); n++
    upsum += up; downsum += down
    if (n == 1 || up < upmin) upmin = up
    if (n == 1 || up > upmax) upmax = up
    if (n == 1 || down < downmin) downmin = down
    if (n == 1 || down > downmax) downmax = down
}
END {
    if (n == 0) { print "no samples collected" > "/dev/stderr"; exit 1 }
    printf "sftp upload   16 MiB  min %7.1f ms  mean %7.1f ms  max %7.1f ms  (%.0f-%.0f MiB/s)\n", upmin, upsum / n, upmax, rate(upmax), rate(upmin)
    printf "sftp download 16 MiB  min %7.1f ms  mean %7.1f ms  max %7.1f ms  (%.0f-%.0f MiB/s)\n", downmin, downsum / n, downmax, rate(downmax), rate(downmin)
}'

echo
for _ in $(seq 1 "$runs"); do
    cargo test --release -q -p citadel-ssh --test openssh real_tunnels_are_byte_exact -- --nocapture 2>/dev/null \
        | grep -o 'finished in [0-9.]*s' || true
done | awk '
{ t = $3; sub(/s$/, "", t); t = t * 1000; n++; sum += t; if (n == 1 || t < min) min = t; if (n == 1 || t > max) max = t }
END {
    if (n == 0) { print "no samples collected" > "/dev/stderr"; exit 1 }
    printf "tunnel suite  36 MiB  min %7.1f ms  mean %7.1f ms  max %7.1f ms  (wall time incl. sshd start and handshake)\n", min, sum / n, max
}'
