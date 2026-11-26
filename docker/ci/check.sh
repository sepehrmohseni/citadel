#!/bin/sh
# Entry point of the citadel-ci image: copies the read-only checkout from /src into the container
# and runs the same steps as .github/workflows/ci.yml. See scripts/docker-check.sh.
set -eu

usage() {
  cat <<'EOF'
usage: citadel-check [all|frontend|rust|openssh|e2e|shell|-- command...]
  all        frontend, rust, openssh, then e2e (the default; what CI runs)
  frontend   pnpm install, typecheck, unit tests and production build of app/
  rust       cargo fmt --check, clippy -D warnings and cargo test for the workspace
  openssh    the release-mode SSH suite against a real sshd
  e2e        the end-to-end scenarios: the real app under Xvfb against a throwaway sshd
  shell      an interactive shell in the prepared copy
  -- cmd     run any command in the prepared copy
EOF
}

# OpenSSH refuses to run for a user id without an account, and the SSH tests log in as $USER.
if ! getent passwd "$(id -u)" >/dev/null 2>&1; then
  echo "ci:x:$(id -u):$(id -g):citadel ci:${HOME}:/bin/sh" >>/etc/passwd
fi
USER="$(id -un)"
export USER
mkdir -p "$HOME"

# Cargo never deletes old build outputs, so the target cache only grows: every change to the code,
# the dependencies or the toolchain leaves the previous outputs behind. Start it afresh once it
# passes CITADEL_CI_TARGET_GB (30 GB by default), so it cannot fill the disk.
limit_gb="${CITADEL_CI_TARGET_GB:-30}"
case "$limit_gb" in '' | *[!0-9]*) limit_gb=30 ;; esac
used_kb="$(du -sk /cache/target 2>/dev/null | cut -f1)"
if [ "${used_kb:-0}" -gt $((limit_gb * 1024 * 1024)) ]; then
  echo "The build cache has grown past ${limit_gb} GB; starting it afresh." >&2
  find /cache/target -mindepth 1 -maxdepth 1 -exec rm -rf {} +
fi

# Work on a private copy. The checkout stays untouched, and host build outputs (target/,
# node_modules/, dist/) belong to the host platform. tar keeps modification times, so cargo's
# cache in the target volume stays valid between runs.
W=/work/citadel
mkdir -p "$W"
tar -C /src --exclude=./.git --exclude=./target --exclude=./app/node_modules --exclude=./app/dist \
  --exclude=./app/e2e-artifacts -cf - . | tar -C "$W" -xf -
cd "$W"

step() {
  printf '\n\033[1m==> %s\033[0m\n' "$*"
  "$@"
}

frontend() {
  cd "$W/app"
  step pnpm install --frozen-lockfile --store-dir /cache/pnpm
  step pnpm exec tsc --noEmit
  step pnpm test
  step pnpm run build
  cd "$W"
}

# The Tauri crate embeds app/dist when it compiles, so the Rust steps need a build of it.
dist() {
  if [ ! -f "$W/app/dist/index.html" ]; then
    (cd "$W/app" && pnpm install --frozen-lockfile --store-dir /cache/pnpm --silent && pnpm run build >/dev/null)
  fi
}

rust() {
  dist
  step cargo fmt --all --check
  step cargo clippy --workspace --all-targets --locked -- -D warnings
  step cargo test --workspace --locked
}

openssh() {
  dist
  step cargo test --release -p citadel-ssh --test openssh --locked -- --nocapture
}

e2e() {
  dist
  # app/e2e/ci.sh builds into target/e2e; keep that build in the cache volume.
  mkdir -p "$W/target" /cache/target/e2e
  ln -sfn /cache/target/e2e "$W/target/e2e"
  # Docker does not let WebKit's sandbox create user namespaces; this container only ever loads
  # the app's own pages. Without a GPU passed in (see scripts/docker-check.sh), WebKit renders in
  # software instead of failing to set up hardware buffers.
  if [ -e /dev/dri ]; then
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 NO_AT_BRIDGE=1 step app/e2e/ci.sh
  else
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 NO_AT_BRIDGE=1 \
      step app/e2e/ci.sh
  fi
}

task="${1:-all}"
[ "$#" -gt 0 ] && shift
case "$task" in
  all)
    frontend
    rust
    openssh
    e2e
    printf '\n\033[1;32mAll checks passed.\033[0m\n'
    ;;
  frontend) frontend ;;
  rust) rust ;;
  openssh) openssh ;;
  e2e) e2e ;;
  shell) dist; exec bash ;;
  --) exec "$@" ;;
  -h | --help | help) usage ;;
  *)
    usage >&2
    exit 2
    ;;
esac
