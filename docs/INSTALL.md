# Installing Citadel

Citadel is distributed as native installers attached to each
[GitHub release](https://github.com/sepehrmohseni/citadel/releases). Every
release also carries a `SHA256SUMS` file covering all of its assets.

| Platform | Requirement |
| --- | --- |
| Windows | Windows 10 or 11, x64. Uses the WebView2 runtime, which the installer fetches if it is missing. |
| macOS | macOS 11 or later. One universal build for Apple Silicon and Intel. |
| Linux | x86_64. The `.deb` and `.rpm` need WebKitGTK 4.1 and GTK 3 from the distribution (Ubuntu 22.04+, Debian 12+, current Fedora and openSUSE). The AppImage bundles its own WebKitGTK. |
| Android | Android 7.0 (API 24) or later. One APK covers arm64, armv7 and x86_64. |

## One-line install

Linux and macOS:

```sh
curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh
```

Windows (PowerShell 5.1 or later, no administrator rights needed):

```powershell
irm https://sepehrmohseni.github.io/citadel/install.ps1 | iex
```

Both scripts:

1. resolve the latest published release through the GitHub API (drafts and
   pre-releases are skipped), or use the version you pin;
2. download `SHA256SUMS`, pick the asset for your OS and CPU from it, download
   that asset over HTTPS and check its SHA-256. A release without
   `SHA256SUMS`, or an asset whose hash does not match, aborts the install;
3. install it as described below and print every path they touched.

The script bodies are wrapped in a function that is called on the last line, so
a truncated download fails to parse instead of running partway. To read a
script before running it, download it first:

```sh
curl -fsSLO https://sepehrmohseni.github.io/citadel/install.sh
less install.sh
sh install.sh
```

### Options

| install.sh | install.ps1 | Effect |
| --- | --- | --- |
| `--version 0.1.0` | `-Version 0.1.0` | install that release instead of the latest |
| `--method deb\|rpm\|appimage` | | Linux only; override the automatic choice |
| `--appimage` | | same as `--method appimage` |
| `--uninstall` | `-Uninstall` | remove what the installer put on the system |
| `CITADEL_VERSION` | `CITADEL_VERSION` | environment form of the version option |
| `CITADEL_METHOD` | | environment form of `--method` |
| `CITADEL_ASSETS_DIR` | `CITADEL_ASSETS_DIR` | install from a local directory, see [Testing builds](#testing-builds-before-a-release) |

Passing options through a pipe:

```sh
curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh -s -- --version 0.1.0
```

```powershell
& ([scriptblock]::Create((irm https://sepehrmohseni.github.io/citadel/install.ps1))) -Version 0.1.0
```

## What each installer integrates

### Windows

`Citadel_<version>_x64-setup.exe` (NSIS, used by `install.ps1`):

- installs per user into `%LOCALAPPDATA%\Citadel\citadel.exe`; no UAC prompt;
- Start Menu shortcut `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Citadel.lnk`;
- desktop shortcut: a checkbox on the last page of the interactive installer
  (checked by default); always created by a silent (`/S`) or passive (`/P`)
  install;
- both shortcuts carry the AppUserModelID `io.github.sepehrmohseni.citadel`, so
  the running window groups under the pinned or Start Menu icon on the taskbar;
- an entry under Settings > Apps (registry `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Citadel`);
- the WebView2 bootstrapper is embedded and only runs when WebView2 is not
  already present (it needs network access).

Installer switches: `/S` silent, `/P` passive (progress only), `/NS` no
shortcuts, `/D=C:\path` install directory (must be the last argument).

`Citadel_<version>_x64_en-US.msi` (WiX) installs per machine into
`C:\Program Files\Citadel`, needs administrator rights, and creates Start Menu
and desktop shortcuts. Use it for managed deployment, e.g.
`msiexec /i Citadel_0.1.0_x64_en-US.msi /qn`. Install either the MSI or the
setup.exe, not both.

### macOS

`Citadel_<version>_universal.dmg` contains `Citadel.app`. `install.sh` mounts
it, copies the app to `/Applications` (or `~/Applications` when `/Applications`
is not writable for your user), detaches the image and removes the
`com.apple.quarantine` attribute. Running the script yourself is the consent
that Gatekeeper's first-launch dialog would otherwise ask for; the script says
so when it does this. The app then appears in Launchpad and Spotlight and
keeps its own Dock icon.

### Linux

`install.sh` chooses, in order:

1. Debian, Ubuntu and derivatives (`apt-get` present, root or sudo available):
   `Citadel_<version>_amd64.deb` through `apt-get install`;
2. Fedora, RHEL, openSUSE (`dnf`, `zypper` or `yum` present, root or sudo
   available): `Citadel-<version>-1.x86_64.rpm` through the package manager
   (`zypper` gets `--allow-unsigned-rpm`);
3. anything else, no sudo, no terminal for a sudo password, or a package
   manager failure (for example a distribution without WebKitGTK 4.1): the
   AppImage, installed per user without root.

The `.deb` and `.rpm` install:

| Path | Purpose |
| --- | --- |
| `/usr/bin/citadel` | the application |
| `/usr/share/applications/Citadel.desktop` | menu entry, `StartupWMClass=citadel` |
| `/usr/share/icons/hicolor/{32x32,64x64,128x128,256x256@2,512x512}/apps/citadel.png` | icons |

Package metadata: name `citadel`, depends on `libwebkit2gtk-4.1-0` and
`libgtk-3-0` (deb) or `libwebkit2gtk-4.1.so.0` and `libgtk-3.so.0` (rpm). The
deb declares `Conflicts: citadel-client` because Ubuntu's unrelated Citadel
groupware client also installs `/usr/bin/citadel`.

The per-user AppImage install writes:

| Path | Purpose |
| --- | --- |
| `~/.local/bin/citadel` | the AppImage itself |
| `~/.local/share/applications/citadel.desktop` | menu entry, `StartupWMClass=citadel` |
| `~/.local/share/icons/hicolor/<size>/apps/citadel.png` | icons extracted from the AppImage |

On X11 the window reports `WM_CLASS` `"citadel", "Citadel"`, which matches
`StartupWMClass=citadel`, so docks and task bars show the Citadel icon and
group the window with its launcher. On Wayland, GTK uses the same program name
(`citadel`) as the app ID. The
AppImage uses FUSE to mount itself; if FUSE is unavailable, install your
distribution's `fuse3` package or run it with `APPIMAGE_EXTRACT_AND_RUN=1`.
`install.sh` warns when it detects this.

## Android

Download `Citadel_<version>_android.apk` from the
[latest release](https://github.com/sepehrmohseni/citadel/releases/latest) on
the phone and open it. Android asks once to allow installs from the browser or
file manager you opened it with ("Install unknown apps").

The APK is signed with the project's release key. Newer releases install over
older ones and keep the vault; an APK signed with a different key is refused by
Android, which protects the installed app from a tampered build. To check the
certificate before installing:

```sh
apksigner verify --print-certs Citadel_0.1.0_android.apk
```

The `SHA-256 digest` must be
`73d60e8e634f8139367d2a93c43979735391c675a0c3c3f6fc646869b3c8967a`.

On phones the navigation moves to a bottom bar and terminals get a key row
with Esc, Tab, Ctrl, Alt, arrows and common symbols. Ctrl and Alt apply to the
next key typed on the on-screen keyboard. The global show/hide shortcut is a
desktop feature and is not available on Android.

## Manual downloads

Download from the [latest release](https://github.com/sepehrmohseni/citadel/releases/latest)
together with `SHA256SUMS`, then check the hash:

```sh
sha256sum --ignore-missing -c SHA256SUMS          # Linux
shasum -a 256 Citadel_0.1.0_universal.dmg         # macOS, compare with SHA256SUMS
```

```powershell
(Get-FileHash .\Citadel_0.1.0_x64-setup.exe -Algorithm SHA256).Hash   # compare with SHA256SUMS
```

| Platform | Command |
| --- | --- |
| Windows | run `Citadel_<version>_x64-setup.exe` |
| macOS | open the `.dmg`, drag `Citadel.app` to `Applications` |
| Debian, Ubuntu | `sudo apt install ./Citadel_<version>_amd64.deb` |
| Fedora, RHEL | `sudo dnf install ./Citadel-<version>-1.x86_64.rpm` |
| openSUSE | `sudo zypper install --allow-unsigned-rpm ./Citadel-<version>-1.x86_64.rpm` |
| Other Linux | `chmod +x Citadel_<version>_amd64.AppImage && ./Citadel_<version>_amd64.AppImage` |

A bare AppImage started this way has no menu entry; use `install.sh --appimage`
for that.

## Warnings on unsigned builds

Releases are not code-signed yet. The SHA-256 check protects against corrupted
or altered downloads, but `SHA256SUMS` is published from the same GitHub
release, so it does not protect against a compromised release itself.

Windows SmartScreen shows "Windows protected your PC" for a setup.exe
downloaded with a browser. Select **More info**, then **Run anyway**.
Alternatively unblock the file first: `Unblock-File .\Citadel_0.1.0_x64-setup.exe`.
`install.ps1` downloads with PowerShell, which does not mark the file as coming
from the internet, so this prompt does not appear there.

macOS Gatekeeper blocks the first launch of an app from a browser-downloaded
`.dmg` ("Apple could not verify Citadel is free of malware"). The app is
ad-hoc signed but not notarized. Either:

- open **System Settings > Privacy & Security**, find the message about
  Citadel and choose **Open Anyway** (macOS 15 and later require this route);
  on macOS 14 and earlier, Control-click the app in Finder and choose **Open**;
- or clear the quarantine flag: `xattr -dr com.apple.quarantine /Applications/Citadel.app`.

## Uninstall

One-liners:

```sh
curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh -s -- --uninstall
```

```powershell
& ([scriptblock]::Create((irm https://sepehrmohseni.github.io/citadel/install.ps1))) -Uninstall
```

Manually:

| Install | Remove with |
| --- | --- |
| Windows setup.exe | Settings > Apps > Citadel > Uninstall, or `%LOCALAPPDATA%\Citadel\uninstall.exe` |
| Windows MSI | Settings > Apps > Citadel > Uninstall, or `msiexec /x Citadel_0.1.0_x64_en-US.msi` |
| macOS | delete `/Applications/Citadel.app` (or `~/Applications/Citadel.app`) |
| deb | `sudo apt remove citadel` |
| rpm | `sudo dnf remove citadel` or `sudo zypper remove citadel` |
| AppImage via install.sh | delete `~/.local/bin/citadel`, `~/.local/share/applications/citadel.desktop` and `~/.local/share/icons/hicolor/*/apps/citadel.png` |

Uninstalling never deletes the vault or settings. The uninstall scripts say
where they are. To erase all Citadel data, delete these directories after
uninstalling; this cannot be undone and the vault cannot be recovered:

| Platform | Data |
| --- | --- |
| Linux | `~/.local/share/io.github.sepehrmohseni.citadel`, `~/.config/io.github.sepehrmohseni.citadel`, `~/.cache/io.github.sepehrmohseni.citadel` |
| macOS | `~/Library/Application Support/io.github.sepehrmohseni.citadel`, `~/Library/Caches/io.github.sepehrmohseni.citadel` |
| Windows | `%APPDATA%\io.github.sepehrmohseni.citadel`, `%LOCALAPPDATA%\io.github.sepehrmohseni.citadel` |

The interactive Windows uninstaller offers a "Delete the application data"
checkbox; it is off by default and a silent uninstall never deletes data.

## Release pipeline

`.github/workflows/release.yml` has two triggers.

**Tag push `v*`.** The tag must equal `v` plus the `version` in
`app/src-tauri/tauri.conf.json`, otherwise the run fails before building.
The workflow creates a draft release (or reuses an existing draft for the same
tag, and refuses to touch a published one), builds on three runners, attaches
every installer, then downloads all release assets and attaches `SHA256SUMS`.
Review the draft and publish it by hand; until then the install scripts report
"no published release found".

| Runner | Assets |
| --- | --- |
| ubuntu-22.04 | `.deb`, `.rpm`, `.AppImage` |
| windows-latest | `-setup.exe` (NSIS), `.msi` |
| macos-latest | `_universal.dmg` |
| ubuntu-24.04 | `_android.apk`, signed |

Ubuntu 22.04 is used so the Linux binaries link against an older glibc and run
on more distributions.

**Manual run (`workflow_dispatch`).** Actions > Release > Run workflow builds
the same matrix and uploads the results as workflow artifacts only:
`bundle-linux-x64`, `bundle-windows-x64`, `bundle-macos-universal`,
`bundle-android`, and
`citadel-dist`, which holds all installers plus `SHA256SUMS`. No release or
tag is created.

### Testing builds before a release

Download and unzip the `citadel-dist` artifact, then point the installers at
it. Nothing is fetched from the network in this mode; the hash check against
the local `SHA256SUMS` still runs.

```sh
CITADEL_ASSETS_DIR=./citadel-dist sh docs/install.sh --appimage
CITADEL_ASSETS_DIR=./citadel-dist sh docs/install.sh --uninstall
```

```powershell
$env:CITADEL_ASSETS_DIR = "$PWD\citadel-dist"; .\docs\install.ps1
.\docs\install.ps1 -Uninstall
```

Local builds work the same way: `pnpm tauri build` in `app/`, then
`scripts/collect-bundles.sh dist` and `(cd dist && sha256sum -- Citadel* > SHA256SUMS)`.

### Android signing key

The Android job signs the APK with a keystore kept in two repository secrets
(Settings > Secrets and variables > Actions):

| Secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | the PKCS12 keystore, base64 on one line |
| `ANDROID_KEYSTORE_PASSWORD` | its password (store and key share it) |

The key alias must be `citadel`. Without both secrets the Android job fails
with a message and the desktop installers are still released. To create a
keystore:

```sh
keytool -genkeypair -keystore citadel-release.jks -storetype PKCS12 -alias citadel \
  -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Citadel"
base64 -w0 citadel-release.jks > citadel-release.jks.base64
```

Keep the keystore and password backed up offline. Android only installs an
update over an existing install when it is signed with the same key, so losing
it means users must uninstall (and lose local data) to move to a new key.

## Adding code signing later

None of the following is configured. Each item is a repository secret plus a
small workflow or config change. Do not pass empty secrets: the bundler treats
a defined but empty `APPLE_CERTIFICATE` as set and fails.

### macOS (Developer ID and notarization)

Secrets: `APPLE_CERTIFICATE` (base64 of the Developer ID Application `.p12`),
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`
(`Developer ID Application: Name (TEAMID)`), and for notarization either
`APPLE_ID`, `APPLE_PASSWORD` (app-specific password) and `APPLE_TEAM_ID`, or an
App Store Connect API key via `APPLE_API_ISSUER`, `APPLE_API_KEY` and
`APPLE_API_KEY_PATH`.

Pass them as `env:` on the "Build installers" step for the macOS runner only,
e.g. by splitting that step into a macOS variant with
`if: runner.os == 'macOS'`. `APPLE_SIGNING_IDENTITY` must be set: it overrides
the ad-hoc identity `"signingIdentity": "-"` in `tauri.conf.json`, which would
otherwise not match the imported certificate. The bundler then signs with the
hardened runtime, notarizes and staples the app.

### Windows (Authenticode)

Either:

- a `.pfx` certificate: store it base64-encoded as `WINDOWS_CERTIFICATE` with
  `WINDOWS_CERTIFICATE_PASSWORD`, add a step on the Windows runner that imports
  it into `Cert:\CurrentUser\My`, and set `bundle.windows.certificateThumbprint`,
  `digestAlgorithm: "sha256"` and `timestampUrl` in `tauri.conf.json`;
- or a cloud signing service (Azure Trusted Signing, a hardware-backed EV
  provider) through `bundle.windows.signCommand`, with its credentials as
  secrets.

The bundler signs `citadel.exe`, the NSIS installer and uninstaller, and the
MSI. Signing reduces but does not immediately remove SmartScreen prompts; an
EV certificate or accumulated reputation does.

### Linux

- RPM: set `TAURI_SIGNING_RPM_KEY` (ASCII-armored private GPG key) and
  `TAURI_SIGNING_RPM_KEY_PASSPHRASE`; the bundler signs the package. `zypper`
  then no longer needs `--allow-unsigned-rpm` once users import the public key.
- deb and AppImage have no built-in signing in the bundler. Publish an apt
  repository with a signed `Release` file, or sign `SHA256SUMS` (below).

### Signing SHA256SUMS

Signing `SHA256SUMS` once covers every asset on every platform. Options:
a detached GPG or minisign signature uploaded next to it, or GitHub artifact
attestations (`actions/attest-build-provenance`), which users verify with
`gh attestation verify <file> --repo sepehrmohseni/citadel`. The install
scripts can then verify the signature before trusting the hashes.
