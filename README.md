# Citadel

Citadel is a desktop SSH client, tunnel manager and SFTP browser built around
a local encrypted vault. Hosts, keys, tunnels and trusted host keys are stored
in a single file encrypted with a key derived from your password. Nothing is
sent to a Citadel service; there is no Citadel service. Optional replication
uses storage you control: an S3-compatible bucket, a WebDAV share or a Git
repository.

The core is written in Rust. The interface is a Tauri v2 application using the
system webview, xterm.js with the WebGL renderer, and vanilla TypeScript.

Documentation: <https://sepehrmohseni.github.io/citadel/>

Status: pre-release (0.1.0). File formats are versioned but may change before
1.0.

## Features

- Encrypted local vault: Argon2id key derivation, XChaCha20-Poly1305 per
  record, automatic locking, password changes without re-encryption.
- Password-protected export and import in a documented binary format
  (`.ctdl`).
- Multi-hop jump chains built in memory over `direct-tcpip` channels. Each hop
  verifies its host key and authenticates independently.
- Local (`-L`), remote (`-R`) and dynamic SOCKS5 (`-D`) forwarding. Local
  listeners stay bound while the underlying session reconnects; remote
  forwards are re-registered after every reconnect.
- Connection supervision with keepalive heartbeats and exponential backoff
  with jitter.
- Per-host identities. Agent forwarding exposes only the key assigned to that
  host, never the rest of your agent.
- Pipelined SFTP transfers and atomic remote edits.
- Agentless host telemetry from `/proc`, `df` and the Docker socket over the
  existing SSH session.
- Lossless round-tripping of `~/.ssh/config`, and discovery of the native,
  1Password and Bitwarden SSH agents.
- Live round-trip latency per connected host, measured with SSH-level
  `keepalive@openssh.com` probes every five seconds.
- A local security audit that grades the vault A to F: weak or DSA keys,
  password logins, agent forwarding, tunnels exposed beyond loopback,
  plaintext sync endpoints, disabled auto-lock and below-target KDF cost.
- Broadcast input across selected terminals, and session recording to
  asciicast v2 (`.cast`). Recordings capture output only, never keystrokes.
- Keyboard-first interface: `g` leader navigation, `j`/`k` host selection,
  a command palette with `user@host:port` quick connect, and six terminal
  themes. Pasting a full `ssh -p 2222 user@host` command into the palette or
  the host form fills in the connection.
- Works on every keyboard layout. Shortcuts, `Ctrl` control characters and
  `Alt` meta keys resolve by physical key when a non-Latin layout such as
  Persian, Arabic, Hebrew, Russian or Greek is active. Copy and paste use the
  native clipboard (`Ctrl+Shift+C`/`V`, `Ctrl+Insert`/`Shift+Insert`, or the
  right-click menu). Remote shells receive a UTF-8 locale so readline edits
  multi-byte characters correctly, and runs of Persian, Arabic and Hebrew
  text are rendered shaped and right to left, in the terminal and in every
  input field. Vazirmatn is bundled as the fallback font for Arabic script.
- Saved data survives updates. Every record is stored as named fields with
  defaults, so newer versions read older records and older versions skip,
  rather than lose, records they cannot parse. The first launch of a new
  version keeps a raw copy of the previous database. Encrypted snapshots are
  written on unlock and every 20 minutes when something changed (newest 20
  kept), and before every import or restore. A vault written by a newer
  release refuses to open instead of being misread. If the database is
  missing but snapshots exist, the lock screen offers to restore them.
- SSH keys that other clients reject: OpenSSH, PuTTY `.ppk`, PKCS#8 and PEM
  keys; indentation, CRLF line endings, byte-order marks and collapsed
  newlines from copy-paste are repaired before parsing. RSA signatures are
  negotiated (`rsa-sha2-512`, `rsa-sha2-256`, then `ssh-rsa` for servers that
  advertise nothing). Key-plus-OTP servers, password logins that the server
  only accepts over keyboard-interactive, OpenSSH certificates held by the
  agent, and the `~/.ssh/id_*` fallback used by the `ssh` command all work.
  Failures state what was offered and which methods the server accepts.
- Legacy appliances: a per-host switch enables SHA-1 key exchange, CBC
  ciphers and `hmac-sha1`. It is off by default; when a server offers only
  legacy algorithms, the error names them and offers to enable the switch.
- Bounded resource use. Terminal output is acknowledged after it is
  rendered and at most 4 MiB is in flight per terminal, so a runaway command
  cannot flood the interface and `Ctrl+C` takes effect immediately.
  Background tabs release their GPU context after 30 seconds, the cursor
  stops blinking after 10 idle seconds, hidden pages skip re-rendering, and
  all animation pauses while the window is hidden.
- Replication over S3-compatible storage, WebDAV or Git, with ETag/If-Match
  optimistic concurrency and last-writer-wins merging.

## Specifications

| Component | Detail |
| --- | --- |
| KDF | Argon2id v1.3, m = 64 MiB, t = 3, p = 4, 32-byte salt |
| AEAD | XChaCha20-Poly1305, 24-byte random nonces |
| Key hierarchy | Random 256-bit data key wrapped by the password-derived key |
| Storage | redb (embedded, ACID, pure Rust), one table per record kind |
| Container | `.ctdl`: authenticated header, zstd-compressed MessagePack payload |
| SSH | russh 0.64, ring crypto backend; no OpenSSL or libssh2 |
| Channel window | 8 MiB per channel, 32 KiB maximum packet |
| SFTP | 32 KiB requests, 64 in flight; `posix-rename@openssh.com` and `fsync@openssh.com` when offered |
| Reconnect | 500 ms base delay, 30 s cap, ±20 % jitter; attempt counter resets after 60 s of uptime |
| Heartbeat | Per-host keepalive interval (default 15 s); three unanswered probes end the session |
| Memory target | Under 25 MB resident for the Rust core process. The system webview runs separately and adds its own memory. |

`ring` contains C and assembly. Apart from it, the SSH and cryptographic stack
is Rust.

## Building

Requirements:

- Rust 1.90 or newer (`rustup`).
- Node.js 20 or newer and pnpm 10.
- Linux only: WebKitGTK 4.1 and related development packages.

```sh
sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev
```

Build the application:

```sh
git clone https://github.com/sepehrmohseni/citadel
cd citadel/app
pnpm install
pnpm tauri build
```

Bundles are written to `release/bundle/` under the Cargo target directory. The
Rust crates build without any of the webview packages:

```sh
cargo build --release -p citadel-vault -p citadel-ssh -p citadel-telemetry \
    -p citadel-config -p citadel-sync
```

Release builds use fat LTO, a single codegen unit, `panic = "abort"` and
stripped symbols.

## Testing

```sh
cargo test --workspace
```

The SSH crate has two suites. `tests/ssh.rs` runs against an in-process russh
server from `crates/citadel-testkit` and covers authentication, host key
handling, PTY, exec, forwarding, reconnection after a server restart, agent
isolation and SFTP. `tests/openssh.rs` runs the same paths against a real
OpenSSH `sshd`, started unprivileged on a random loopback port with generated
keys. It is skipped when no `sshd` is found.

```sh
CITADEL_SSHD=/usr/sbin/sshd \
CITADEL_SFTP_SERVER=/usr/lib/openssh/sftp-server \
cargo test -p citadel-ssh --test openssh
```

The mock suite keeps payloads small: russh 0.64's server-side send path
reorders data under sustained bidirectional load, which affects only the mock
server. Byte-exactness of bastion chains, tunnels and large SFTP transfers is
verified against OpenSSH.

`scripts/bench.sh` runs the OpenSSH suite in release mode and reports SFTP and
tunnel timings for the current machine.

Frontend logic and component tests run with Vitest:

```sh
cd app && pnpm test
```

`app/e2e/` drives the real application end to end through WebKitGTK's remote
inspector: it creates a vault, imports a key, connects to a real `sshd`,
verifies the host key prompt, runs a command, opens telemetry and SFTP, pushes
bytes through a tunnel, checks the audit, palette, themes and lock/unlock,
confirms an encrypted snapshot was written, then starts a second instance with
only the snapshots left and restores the vault from its lock screen. It fails
on any console error. Every screen is captured as a PNG.

`app/e2e/intl.mjs` switches the X server to the Persian (`ir`) layout and
sends real key events with `xdotool`: typing Persian (including ZWNJ and Farsi
yeh) into the shell and into forms, Backspace over multi-byte letters,
`Ctrl+C`, `Ctrl+Shift+C`/`V`, `Shift+Insert`, `Ctrl+K` and `g` navigation, and
right-to-left rendering with both renderers. `app/e2e/perf.mjs` reports idle
CPU, memory of the whole process tree, a 10 MB output flood and the time from
`Ctrl+C` to a prompt during `yes`.

On Linux with Xvfb, `app/e2e/ci.sh` sets up a throwaway `sshd` and runs the
scenarios; CI uploads the screenshots as an artifact.

`crates/citadel-vault/tests/fixtures` holds a vault, an export and a snapshot
written by format version 1. Every build must still open them, so a schema or
storage change that would orphan saved data fails CI instead of reaching users.

## Security

Imported `.ctdl` bundles are quarantined: every record is validated, agent
forwarding and tunnel autostart are switched off, and trusted host keys are
dropped so each server is verified again on first connect. Sync blobs are not
quarantined because they are authenticated by the vault's own data key.
Consecutive failed unlocks back off exponentially up to 30 seconds.

The threat model, the plaintext metadata that remains visible at rest, and
known limitations are documented in [SECURITY.md](SECURITY.md). Report
vulnerabilities through GitHub Security Advisories, not public issues.

## Repository layout

```
crates/citadel-vault       encrypted store, key derivation, .ctdl export/import, merge
crates/citadel-ssh         transport, jump chains, PTY, forwarding, agent, SFTP
crates/citadel-telemetry   remote metrics over an existing session
crates/citadel-config      ~/.ssh/config round-trip, agent socket discovery
crates/citadel-sync        S3-compatible, WebDAV and Git replication
crates/citadel-testkit     in-process SSH/SFTP server and OpenSSH harness for tests
app/                       Tauri v2 application (TypeScript frontend, app/src-tauri)
docs/                      project site, deployed to GitHub Pages
scripts/                   benchmarks and tooling
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
