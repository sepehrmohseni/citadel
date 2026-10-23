# Security Policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub Security Advisories:
<https://github.com/sepehrmohseni/citadel/security/advisories/new>.

Do not open public issues for security problems. Include the affected
version or commit, the platform, a description of the impact, and a
reproduction if one exists. Reports are acknowledged within seven days.
Fixes are released before details are published; reporters are credited
unless they ask not to be.

## Supported versions

Citadel is pre-1.0. Only the latest release and the `main` branch receive
security fixes.

## Scope

In scope:

- The vault storage engine and the `.ctdl` container format
  (`crates/citadel-vault`).
- SSH transport, authentication, host key verification, agent forwarding,
  port forwarding and SFTP (`crates/citadel-ssh`).
- Remote telemetry collection (`crates/citadel-telemetry`).
- `~/.ssh/config` handling and agent socket discovery (`crates/citadel-config`).
- Vault replication backends (`crates/citadel-sync`).
- The desktop application and its IPC surface (`app/`).

Out of scope: vulnerabilities in the operating system, the system webview
(WebKitGTK, WebView2, WKWebView), the SSH server being connected to, or the
storage provider chosen for sync. Vulnerabilities in dependencies (russh,
ring, RustCrypto, redb, Tauri) should be reported upstream; reports that
show how Citadel's use of them is exploitable are in scope.

## Design summary

| Property | Implementation |
| --- | --- |
| Key derivation | Argon2id v1.3, m = 64 MiB, t = 3, p = 4, 32-byte random salt |
| Record encryption | XChaCha20-Poly1305, 24-byte random nonce per write |
| Key hierarchy | Password-derived key wraps a random 256-bit data key; password changes rewrap without re-encrypting records |
| Record binding | AAD = `"citadel.record.v1" ‖ 0x00 ‖ table ‖ 0x00 ‖ id`; the decrypted id must match the storage key |
| Plaintext metadata at rest | KDF parameters, salt, header epoch, record ids (random 128-bit), per-kind record counts and ciphertext sizes |
| Container | `.ctdl`: every header byte is authenticated as AAD |
| Memory hygiene | `secrecy` and `zeroize` on keys, passwords and decrypted buffers; Argon2 working memory is zeroized; serialization buffers are pre-sized to avoid reallocation copies |
| Lock | Dropping the cipher state zeroizes the data key; an idle watchdog locks automatically |
| SSH | russh 0.64 with the ring backend; no OpenSSL or libssh2 |
| Host keys | Verified before authentication on every hop of a jump chain |
| Agent forwarding | Citadel answers the forwarded agent protocol itself and exposes only the key assigned to the host |
| Snapshots | Written with the vault's own data key and wrapped key header, mode 0600 in a 0700 directory next to the database; opening one requires the master password that was current when it was taken |
| Pre-upgrade copies | A byte-for-byte copy of the encrypted database, kept for the five most recent version changes |
| Legacy algorithms | Off by default and scoped to one host; never negotiated silently |
| Local file access | Paths come only from native dialogs opened by the core or from drops reported by the OS; the webview receives single-use tokens and holds no file permission |
| Host key prompts | Shown only while the vault is unlocked; a pin of any algorithm for the host makes a different key "changed", with the old fingerprint displayed; identical concurrent prompts are merged |
| Agent forwarding | For the system key source, only the key that authenticated is exposed; forwarded agents refuse to sign while the vault is locked |
| System key source | `~/.ssh/id_ed25519`, `id_ecdsa` and `id_rsa` are read at connect time when a host uses the system source and the agent did not authenticate; they are never copied into the vault |

## Known limitations

- A process that can read Citadel's memory while the vault is unlocked can
  recover the data key and decrypted records. Locking limits the window; it
  does not defend against a compromised host.
- Zeroization is best-effort. Moves of key-holding structs may leave copies
  in stack frames that are overwritten later rather than immediately, and
  third-party code (zstd compression during export, the SSH stack, the
  webview) maintains internal buffers Citadel cannot wipe.
- Process memory is not locked against swapping. Use encrypted swap or
  disable swap on hosts where that matters.
- Record counts per kind and approximate record sizes are visible to anyone
  with read access to the vault file.
- Last-writer-wins sync uses wall-clock timestamps. A device with a clock far
  in the future wins conflicts until the others catch up.
- When an SFTP server does not offer `posix-rename@openssh.com`, atomic
  writes fall back to remove-then-rename, which leaves a short window in
  which the destination does not exist.
- `ring` contains C and assembly. Citadel has no other native cryptographic
  dependencies.
- Snapshots and pre-upgrade copies inherit the protection of the vault file.
  An old snapshot still opens with the password that was current when it was
  written, so after changing a compromised password, delete older snapshots
  from the `backups` directory.
- Records written by a newer release that this release cannot parse are kept
  on disk but hidden. Re-keying the vault through a foreign sync blob is
  refused while such records exist, so they are never silently dropped.
