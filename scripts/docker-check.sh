#!/usr/bin/env bash
# Runs the CI checks in Docker, so they work on any Linux or macOS machine with Docker, without
# installing Rust, Node, pnpm, OpenSSH or the WebKitGTK headers.
#
#   scripts/docker-check.sh            frontend, rustfmt, clippy, tests, SSH suite and e2e, like CI
#   scripts/docker-check.sh frontend   typecheck, unit tests and build of app/
#   scripts/docker-check.sh rust       rustfmt, clippy and cargo test
#   scripts/docker-check.sh openssh    the release-mode SSH suite against a real sshd
#   scripts/docker-check.sh e2e        the end-to-end scenarios: the real app under Xvfb
#   scripts/docker-check.sh shell      a shell in the prepared container
#   scripts/docker-check.sh -- CMD     any command, for example: -- cargo test -p citadel-vault
#
# The checkout is mounted read-only and copied inside the container, so nothing is written to it.
# Caches live in the Docker volumes citadel-ci-cargo, citadel-ci-target and citadel-ci-pnpm;
# remove them with: docker volume rm citadel-ci-cargo citadel-ci-target citadel-ci-pnpm
# Cargo never deletes old build outputs, so the target cache starts afresh once it passes
# CITADEL_CI_TARGET_GB (30 GB by default).
# The checks use half of the CPU cores by default, so the machine stays usable while they run;
# set CITADEL_CI_CPUS to change that. On Linux the GPU (/dev/dri: Intel, AMD, or NVIDIA through
# nouveau) is passed in so the end-to-end run renders with it instead of the CPU; set
# CITADEL_CI_GPU=off to render in software, as CI does. Compiling and testing stay on the CPU. Set CITADEL_CI_RUST_IMAGE (for example rust:1.90-trixie)
# to check with a Rust version other than the one pinned in rust-toolchain.toml.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${CITADEL_CI_IMAGE:-citadel-ci}"
cores="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)"
cpus="${CITADEL_CI_CPUS:-$(( cores > 1 ? cores / 2 : 1 ))}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is not installed or not on PATH" >&2
  exit 1
fi

build=(docker build --tag "$image")
if [ -n "${CITADEL_CI_RUST_IMAGE:-}" ]; then
  build+=(--build-arg "RUST_IMAGE=$CITADEL_CI_RUST_IMAGE")
fi
if docker image inspect "$image" >/dev/null 2>&1; then
  # Already built: a quick, quiet rebuild that only redoes layers whose inputs changed.
  "${build[@]}" --quiet "$root/docker/ci" >/dev/null
else
  echo "Building the $image image once (downloads Rust, Node and the WebKitGTK headers)..." >&2
  "${build[@]}" "$root/docker/ci" >&2
fi

tty=()
if [ -t 0 ] && [ -t 1 ]; then
  tty=(--interactive --tty)
fi

gpu=()
if [ "${CITADEL_CI_GPU:-auto}" != off ] && [ -d /dev/dri ]; then
  gpu=(--device /dev/dri)
  # The render nodes belong to these groups; the container user needs them to open the GPU.
  for group in render video; do
    gid="$(getent group "$group" 2>/dev/null | cut -d: -f3)"
    if [ -n "$gid" ]; then
      gpu+=(--group-add "$gid")
    fi
  done
fi

# Running as your own user id keeps the read-only checkout readable even with a private umask.
# The ",z" label lets SELinux hosts (Fedora, RHEL) share the mount; elsewhere it is ignored.
exec docker run --rm ${tty[@]+"${tty[@]}"} ${gpu[@]+"${gpu[@]}"} \
  --cpus "$cpus" \
  --env "CARGO_BUILD_JOBS=$cpus" \
  --env "CITADEL_CI_TARGET_GB=${CITADEL_CI_TARGET_GB:-30}" \
  --user "$(id -u):$(id -g)" \
  --volume "$root:/src:ro,z" \
  --volume citadel-ci-cargo:/cache/cargo \
  --volume citadel-ci-target:/cache/target \
  --volume citadel-ci-pnpm:/cache/pnpm \
  "$image" "$@"
