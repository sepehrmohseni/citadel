# The checks from .github/workflows/ci.yml (frontend, rustfmt, clippy, unit and SSH tests, and the
# end-to-end scenarios under Xvfb) in one image, so they run the same on any Linux or macOS
# machine with Docker, on x86_64 or arm64.
# Run it through scripts/docker-check.sh rather than by hand.

# The Rust release pinned in rust-toolchain.toml; keep the two in step.
ARG RUST_IMAGE=rust:1.100.0-trixie
FROM ${RUST_IMAGE}

ARG NODE_VERSION=22.23.5
ARG PNPM_VERSION=10.24.0

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      openssh-server libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
      libayatana-appindicator3-dev librsvg2-dev xz-utils procps iproute2 \
      xvfb xauth x11-apps x11-utils x11-xkb-utils xdotool fonts-dejavu-core \
      libgl1-mesa-dri libegl1 libgbm1 \
 && rm -rf /var/lib/apt/lists/*

ENV COREPACK_HOME=/opt/corepack \
    COREPACK_ENABLE_DOWNLOAD_PROMPT=0

# Node is verified against the release's SHASUMS256.txt before it is unpacked.
RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) arch=x64 ;; \
      arm64) arch=arm64 ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    file="node-v${NODE_VERSION}-linux-${arch}.tar.xz"; \
    cd /tmp; \
    curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${file}"; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" | grep " ${file}\$" | sha256sum -c -; \
    tar -xJf "$file" -C /usr/local --strip-components=1 --no-same-owner; \
    rm "$file"; \
    corepack enable; \
    corepack prepare "pnpm@${PNPM_VERSION}" --activate; \
    chmod -R a+rX "$COREPACK_HOME"

RUN rustup component add clippy rustfmt \
 && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME"

# The container runs as the caller's own user id, so caches and the account file must be
# writable by any id. The caches live in named volumes created from these directories.
RUN mkdir -p /cache/cargo /cache/target /cache/pnpm /cache/home /work \
 && chmod 1777 /cache/cargo /cache/target /cache/pnpm /cache/home /work \
 && chmod 666 /etc/passwd

ENV CARGO_HOME=/cache/cargo \
    CARGO_TARGET_DIR=/cache/target \
    CARGO_TERM_COLOR=always \
    HOME=/cache/home \
    CITADEL_SSHD=/usr/sbin/sshd \
    CITADEL_SFTP_SERVER=/usr/lib/openssh/sftp-server \
    PATH=/usr/local/cargo/bin:$PATH

COPY check.sh /usr/local/bin/citadel-check
RUN chmod 755 /usr/local/bin/citadel-check

ENTRYPOINT ["/usr/local/bin/citadel-check"]
CMD ["all"]
