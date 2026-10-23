#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d)"
trap 'kill "$(cat "$work/sshd.pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$work"' EXIT

port="${CITADEL_E2E_SSH_PORT:-2299}"
sshd="${CITADEL_SSHD:-/usr/sbin/sshd}"
ssh-keygen -q -t ed25519 -N "" -f "$work/host"
ssh-keygen -q -t ed25519 -N "" -f "$work/client"
cp "$work/client.pub" "$work/authorized_keys"
chmod 600 "$work/authorized_keys"
cat > "$work/sshd_config" <<CFG
Port $port
ListenAddress 127.0.0.1
HostKey $work/host
AuthorizedKeysFile $work/authorized_keys
AcceptEnv LANG LC_*
PidFile $work/sshd.pid
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
AllowTcpForwarding yes
Subsystem sftp ${CITADEL_SFTP_SERVER:-/usr/lib/openssh/sftp-server}
CFG
"$sshd" -f "$work/sshd_config"

(cd "$root/app" && pnpm exec vite build --mode e2e)
cargo build --manifest-path "$root/Cargo.toml" -p citadel-app --features tauri/custom-protocol --target-dir "$root/target/e2e"

export CITADEL_BIN="$root/target/e2e/debug/citadel-app"
export CITADEL_E2E_KEY="$work/client"
export CITADEL_E2E_SSH_PORT="$port"
export CITADEL_E2E_SSHD_PIDFILE="$work/sshd.pid"
export CITADEL_E2E_SSHD_START="$sshd -f $work/sshd_config"
export CITADEL_E2E_OUT="${CITADEL_E2E_OUT:-$root/app/e2e-artifacts}"
cd "$root/app"
xvfb-run -a -s "-screen 0 1440x900x24" node e2e/run.mjs
xvfb-run -a -s "-screen 0 1440x900x24" node e2e/intl.mjs
exec xvfb-run -a -s "-screen 0 1440x900x24" node e2e/features.mjs
