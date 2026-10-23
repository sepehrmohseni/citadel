#!/bin/sh
# Citadel installer for Linux and macOS.
#
#   curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh
#   curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh -s -- --version 0.1.0
#   curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh -s -- --uninstall
#
# Run with --help for all options. Documentation:
# https://github.com/sepehrmohseni/citadel/blob/master/docs/INSTALL.md
#
# Everything below is function definitions; the only top-level command is the
# last line, so a truncated download fails to parse instead of running halfway.

set -eu

CITADEL_REPO="sepehrmohseni/citadel"
CITADEL_APP_ID="io.github.sepehrmohseni.citadel"
CITADEL_HOMEPAGE="https://sepehrmohseni.github.io/citadel/"
CITADEL_PKG="citadel"
CITADEL_BIN="citadel"
CITADEL_MARKER="X-Citadel-Installer=install.sh"

say() {
    printf 'citadel: %s\n' "$*"
}

warn() {
    printf 'citadel: warning: %s\n' "$*" >&2
}

die() {
    printf 'citadel: error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

usage() {
    cat <<'EOF'
Citadel installer for Linux and macOS

usage: install.sh [--version X.Y.Z] [--method auto|deb|rpm|appimage] [--uninstall]

  --version X.Y.Z   install this release instead of the latest one
  --method M        Linux only. auto (default) picks deb on apt systems,
                    rpm on dnf/zypper/yum systems and falls back to a
                    per-user AppImage when root is not available
  --appimage        same as --method appimage
  --uninstall       remove what this installer put on the system
  -h, --help        show this text

environment:
  CITADEL_VERSION      same as --version
  CITADEL_METHOD       same as --method
  CITADEL_ASSETS_DIR   install from a local directory holding release assets
                       and their SHA256SUMS instead of downloading
EOF
}

cleanup() {
    if [ -n "${CITADEL_MNT:-}" ]; then
        hdiutil detach "$CITADEL_MNT" -quiet >/dev/null 2>&1 ||
            hdiutil detach "$CITADEL_MNT" -force -quiet >/dev/null 2>&1 || true
        CITADEL_MNT=""
    fi
    if [ -n "${CITADEL_TMP:-}" ] && [ -d "$CITADEL_TMP" ]; then
        rm -rf "$CITADEL_TMP"
    fi
}

fetch() {
    if [ -t 2 ] && [ "${3:-}" != quiet ]; then
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL --retry 3 --progress-bar -o "$2" "$1"
    else
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
    fi
}

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{ print tolower($1) }'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{ print tolower($1) }'
    elif have openssl; then
        openssl dgst -sha256 "$1" | awk '{ print tolower($NF) }'
    else
        die "need sha256sum, shasum or openssl to verify downloads"
    fi
}

check_name() {
    case "$1" in
        "" | .* | *[!A-Za-z0-9._+-]*) die "refusing unexpected name '$1'" ;;
    esac
}

resolve_tag() {
    if [ -n "$CITADEL_ASSETS_DIR" ]; then
        CITADEL_TAG="local"
        return
    fi
    if [ -n "$CITADEL_WANT" ]; then
        case "$CITADEL_WANT" in
            v*) CITADEL_TAG="$CITADEL_WANT" ;;
            *) CITADEL_TAG="v$CITADEL_WANT" ;;
        esac
        check_name "$CITADEL_TAG"
        return
    fi
    api="https://api.github.com/repos/$CITADEL_REPO/releases/latest"
    code=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -sSL --retry 2 \
        -H 'Accept: application/vnd.github+json' \
        -o "$CITADEL_TMP/latest.json" -w '%{http_code}' "$api" 2>/dev/null) || code="000"
    case "$code" in
        200)
            CITADEL_TAG=$(sed -n 's/^[[:space:]]*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
                "$CITADEL_TMP/latest.json" | head -n 1)
            ;;
        404)
            die "no published release found for $CITADEL_REPO (drafts and pre-releases are skipped; use --version to pick one)"
            ;;
        *)
            warn "GitHub API answered HTTP $code; resolving the latest release through github.com instead"
            url=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSLI -o /dev/null \
                -w '%{url_effective}' "https://github.com/$CITADEL_REPO/releases/latest") ||
                die "cannot reach github.com"
            case "$url" in
                */releases/tag/*) CITADEL_TAG="${url##*/releases/tag/}" ;;
                *) die "no published release found for $CITADEL_REPO" ;;
            esac
            ;;
    esac
    [ -n "$CITADEL_TAG" ] || die "could not read the latest release tag from $api"
    check_name "$CITADEL_TAG"
}

load_sums() {
    if [ -n "$CITADEL_ASSETS_DIR" ]; then
        [ -f "$CITADEL_ASSETS_DIR/SHA256SUMS" ] || die "no SHA256SUMS in CITADEL_ASSETS_DIR=$CITADEL_ASSETS_DIR"
        cp "$CITADEL_ASSETS_DIR/SHA256SUMS" "$CITADEL_TMP/SHA256SUMS"
        say "using local assets from $CITADEL_ASSETS_DIR"
        return
    fi
    CITADEL_BASE="https://github.com/$CITADEL_REPO/releases/download/$CITADEL_TAG"
    fetch "$CITADEL_BASE/SHA256SUMS" "$CITADEL_TMP/SHA256SUMS" quiet ||
        die "release $CITADEL_TAG not found or it has no SHA256SUMS; refusing to install unverified files"
    say "release $CITADEL_TAG"
}

pick_asset() {
    awk -v suf="$1" '{
        f = $2
        sub(/^\*/, "", f)
        n = length(f)
        m = length(suf)
        if (substr(f, 1, 8) == "Citadel_" || substr(f, 1, 8) == "Citadel-")
            if (n > m && substr(f, n - m + 1) == suf) { print f; exit }
    }' "$CITADEL_TMP/SHA256SUMS"
}

get_asset() {
    name="$1"
    check_name "$name"
    CITADEL_FILE="$CITADEL_TMP/$name"
    if [ -n "$CITADEL_ASSETS_DIR" ]; then
        [ -f "$CITADEL_ASSETS_DIR/$name" ] || die "$name is listed in SHA256SUMS but missing from $CITADEL_ASSETS_DIR"
        cp "$CITADEL_ASSETS_DIR/$name" "$CITADEL_FILE"
    else
        say "downloading $name"
        fetch "$CITADEL_BASE/$name" "$CITADEL_FILE" || die "download failed: $CITADEL_BASE/$name"
    fi
    want=$(awk -v n="$name" '{ f = $2; sub(/^\*/, "", f); if (f == n) { print tolower($1); exit } }' \
        "$CITADEL_TMP/SHA256SUMS")
    case "$want" in
        *[!0-9a-f]* | "") die "no valid SHA-256 for $name in SHA256SUMS" ;;
    esac
    [ "${#want}" -eq 64 ] || die "no valid SHA-256 for $name in SHA256SUMS"
    got=$(sha256_of "$CITADEL_FILE")
    [ "$got" = "$want" ] || die "checksum mismatch for $name: expected $want, got $got"
    say "sha256 ok  $got  $name"
}

detect_platform() {
    case "$(uname -s)" in
        Linux) CITADEL_OS=linux ;;
        Darwin) CITADEL_OS=macos ;;
        *) die "unsupported OS $(uname -s); on Windows use install.ps1" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) CITADEL_ARCH=x86_64 ;;
        aarch64 | arm64) CITADEL_ARCH=aarch64 ;;
        *) die "unsupported CPU architecture $(uname -m)" ;;
    esac
}

can_elevate() {
    if [ "$(id -u)" -eq 0 ]; then
        CITADEL_SUDO=""
        return 0
    fi
    have sudo || return 1
    CITADEL_SUDO="sudo"
    sudo -n true >/dev/null 2>&1 && return 0
    (: </dev/tty) >/dev/null 2>&1 && return 0
    return 1
}

as_root() {
    if [ -n "$CITADEL_SUDO" ]; then
        sudo "$@"
    else
        "$@"
    fi
}

user_paths() {
    CITADEL_BINDIR="$HOME/.local/bin"
    CITADEL_DATADIR="${XDG_DATA_HOME:-$HOME/.local/share}"
    CITADEL_DESKTOP="$CITADEL_DATADIR/applications/$CITADEL_BIN.desktop"
}

linux_install() {
    case "$CITADEL_ARCH" in
        x86_64) deb_suf="_amd64.deb" rpm_suf=".x86_64.rpm" ai_suf="_amd64.AppImage" ;;
        *) deb_suf="_arm64.deb" rpm_suf=".aarch64.rpm" ai_suf="_aarch64.AppImage" ;;
    esac
    method="$CITADEL_METHOD"
    if [ "$method" = auto ]; then
        if have apt-get && have dpkg && can_elevate; then
            method=deb
        elif { have dnf || have zypper || have yum; } && have rpm && can_elevate; then
            method=rpm
        else
            method=appimage
        fi
        say "install method: $method (override with --method)"
    fi
    case "$method" in
        deb | rpm)
            if [ "$method" = deb ]; then suf="$deb_suf"; else suf="$rpm_suf"; fi
            asset=$(pick_asset "$suf")
            if [ -z "$asset" ]; then
                [ "$CITADEL_METHOD" = auto ] || die "release $CITADEL_TAG has no *$suf asset"
                warn "release $CITADEL_TAG has no *$suf asset; using the AppImage"
                method=appimage
            elif ! can_elevate; then
                die "--method $method needs root or sudo; use --method appimage for a per-user install"
            fi
            ;;
        appimage) ;;
        *) die "unknown method '$method' (auto, deb, rpm, appimage)" ;;
    esac
    if [ "$method" != appimage ]; then
        get_asset "$asset"
        if system_install "$method"; then
            remove_user_install quiet
            say "installed system package $CITADEL_PKG from $asset"
            say "command: /usr/bin/$CITADEL_BIN"
            if [ -f /usr/share/applications/Citadel.desktop ]; then
                say "menu entry: /usr/share/applications/Citadel.desktop"
            else
                warn "the package did not install /usr/share/applications/Citadel.desktop"
            fi
            say "uninstall: rerun this script with --uninstall"
            return
        fi
        [ "$CITADEL_METHOD" = auto ] || die "package installation failed"
        warn "package installation failed; installing the per-user AppImage instead"
    fi
    asset=$(pick_asset "$ai_suf")
    [ -n "$asset" ] || die "release $CITADEL_TAG has no Linux $CITADEL_ARCH build (*$ai_suf)"
    get_asset "$asset"
    appimage_install
}

system_install() {
    chmod 755 "$CITADEL_TMP"
    chmod 644 "$CITADEL_FILE"
    if [ -n "$CITADEL_SUDO" ]; then
        say "running the package manager through sudo (it may ask for your password)"
    fi
    if [ "$1" = deb ]; then
        as_root apt-get install -y "$CITADEL_FILE" </dev/null
    elif have dnf; then
        as_root dnf install -y "$CITADEL_FILE" </dev/null
    elif have zypper; then
        as_root zypper --non-interactive install --allow-unsigned-rpm "$CITADEL_FILE" </dev/null
    else
        as_root yum install -y "$CITADEL_FILE" </dev/null
    fi
}

desktop_safe() {
    case "$1" in
        *'"'* | *'`'* | *'$'* | *\\* | *"
"*) die "path '$1' contains characters a .desktop Exec line cannot hold; set HOME or XDG_DATA_HOME to a plain path" ;;
    esac
}

appimage_install() {
    user_paths
    desktop_safe "$CITADEL_BINDIR"
    target="$CITADEL_BINDIR/$CITADEL_BIN"
    if [ -e "$target" ] && ! grep -qx "$CITADEL_MARKER" "$CITADEL_DESKTOP" 2>/dev/null; then
        die "$target exists and was not installed by this script; move it away and rerun"
    fi
    mkdir -p "$CITADEL_BINDIR" "$CITADEL_DATADIR/applications"
    cp "$CITADEL_FILE" "$target.new.$$"
    chmod 755 "$target.new.$$"
    mv -f "$target.new.$$" "$target"
    say "installed $target"

    icons=0
    if (cd "$CITADEL_TMP" && "$target" --appimage-extract 'usr/share/icons/hicolor/*' >/dev/null 2>&1); then
        for png in "$CITADEL_TMP"/squashfs-root/usr/share/icons/hicolor/*/apps/"$CITADEL_BIN".png; do
            [ -f "$png" ] || continue
            size=$(basename "$(dirname "$(dirname "$png")")")
            size="${size%@2}"
            mkdir -p "$CITADEL_DATADIR/icons/hicolor/$size/apps"
            cp "$png" "$CITADEL_DATADIR/icons/hicolor/$size/apps/$CITADEL_BIN.png"
            icons=$((icons + 1))
        done
    fi
    if [ "$icons" -gt 0 ]; then
        touch "$CITADEL_DATADIR/icons/hicolor"
        if [ -f "$CITADEL_DATADIR/icons/hicolor/icon-theme.cache" ] && have gtk-update-icon-cache; then
            gtk-update-icon-cache -q -t -f "$CITADEL_DATADIR/icons/hicolor" >/dev/null 2>&1 || true
        fi
        say "installed $icons icon sizes under $CITADEL_DATADIR/icons/hicolor"
    else
        warn "could not extract icons from the AppImage; the menu entry will use a generic icon"
    fi

    cat >"$CITADEL_DESKTOP.new.$$" <<EOF
[Desktop Entry]
Type=Application
Name=Citadel
GenericName=SSH Client
Comment=Local-first SSH, tunnel and infrastructure workstation
Exec="$target"
TryExec=$target
Icon=$CITADEL_BIN
Terminal=false
Categories=Development;
Keywords=ssh;sftp;tunnel;terminal;
StartupWMClass=$CITADEL_BIN
$CITADEL_MARKER
EOF
    mv -f "$CITADEL_DESKTOP.new.$$" "$CITADEL_DESKTOP"
    if have update-desktop-database; then
        update-desktop-database -q "$CITADEL_DATADIR/applications" >/dev/null 2>&1 || true
    fi
    say "menu entry: $CITADEL_DESKTOP"

    if [ ! -e /dev/fuse ] || { ! have fusermount && ! have fusermount3; }; then
        warn "FUSE is not available, so the AppImage cannot mount itself."
        warn "install your distribution's fuse3 (or libfuse2) package, or run it as: APPIMAGE_EXTRACT_AND_RUN=1 $target"
    fi
    case ":$PATH:" in
        *":$CITADEL_BINDIR:"*) ;;
        *) warn "$CITADEL_BINDIR is not in PATH; the menu entry works, but to run 'citadel' from a shell add it to PATH" ;;
    esac
    if have dpkg-query && dpkg-query -W -f='${Status}' "$CITADEL_PKG" 2>/dev/null | grep -q 'ok installed'; then
        warn "the system package $CITADEL_PKG is installed as well; remove one of them to avoid two menu entries"
    fi
    say "uninstall: rerun this script with --uninstall"
}

remove_user_install() {
    user_paths
    if ! grep -qx "$CITADEL_MARKER" "$CITADEL_DESKTOP" 2>/dev/null; then
        [ "${1:-}" = quiet ] || say "no per-user AppImage install found"
        return 0
    fi
    rm -f "$CITADEL_BINDIR/$CITADEL_BIN" "$CITADEL_DESKTOP"
    for png in "$CITADEL_DATADIR"/icons/hicolor/*/apps/"$CITADEL_BIN".png; do
        [ -f "$png" ] && rm -f "$png"
    done
    touch "$CITADEL_DATADIR/icons/hicolor" 2>/dev/null || true
    if have update-desktop-database; then
        update-desktop-database -q "$CITADEL_DATADIR/applications" >/dev/null 2>&1 || true
    fi
    say "removed per-user AppImage install: $CITADEL_BINDIR/$CITADEL_BIN, $CITADEL_DESKTOP and icons"
}

linux_uninstall() {
    remove_user_install
    if have dpkg-query && dpkg-query -W -f='${Status}' "$CITADEL_PKG" 2>/dev/null | grep -q 'ok installed'; then
        home=$(dpkg-query -W -f='${Homepage}' "$CITADEL_PKG" 2>/dev/null || true)
        if [ "$home" != "$CITADEL_HOMEPAGE" ]; then
            warn "an unrelated package named $CITADEL_PKG is installed (homepage '$home'); leaving it"
        elif can_elevate; then
            as_root apt-get remove -y "$CITADEL_PKG" </dev/null
            say "removed system package $CITADEL_PKG (apt)"
        else
            die "removing the system package needs root: sudo apt-get remove $CITADEL_PKG"
        fi
    elif have rpm && rpm -q "$CITADEL_PKG" >/dev/null 2>&1; then
        home=$(rpm -q --qf '%{URL}' "$CITADEL_PKG" 2>/dev/null || true)
        if [ "$home" != "$CITADEL_HOMEPAGE" ]; then
            warn "an unrelated package named $CITADEL_PKG is installed (URL '$home'); leaving it"
        elif can_elevate; then
            if have dnf; then
                as_root dnf remove -y "$CITADEL_PKG" </dev/null
            elif have zypper; then
                as_root zypper --non-interactive remove "$CITADEL_PKG" </dev/null
            elif have yum; then
                as_root yum remove -y "$CITADEL_PKG" </dev/null
            else
                as_root rpm -e "$CITADEL_PKG"
            fi
            say "removed system package $CITADEL_PKG (rpm)"
        else
            die "removing the system package needs root: sudo rpm -e $CITADEL_PKG"
        fi
    fi
    say "kept your vault and settings in ${XDG_DATA_HOME:-$HOME/.local/share}/$CITADEL_APP_ID and ${XDG_CONFIG_HOME:-$HOME/.config}/$CITADEL_APP_ID; delete them by hand to erase all data"
}

app_bundle_id() {
    defaults read "$1/Contents/Info" CFBundleIdentifier 2>/dev/null || true
}

macos_install() {
    asset=$(pick_asset "_universal.dmg")
    if [ -z "$asset" ]; then
        case "$CITADEL_ARCH" in
            aarch64) asset=$(pick_asset "_aarch64.dmg") ;;
            *) asset=$(pick_asset "_x64.dmg") ;;
        esac
    fi
    [ -n "$asset" ] || die "release $CITADEL_TAG has no macOS disk image"
    get_asset "$asset"

    if [ -w /Applications ]; then
        dest="/Applications"
    else
        dest="$HOME/Applications"
        mkdir -p "$dest"
        say "/Applications is not writable for $(id -un); installing into $dest"
    fi
    app="$dest/Citadel.app"
    if [ -e "$app" ]; then
        [ "$(app_bundle_id "$app")" = "$CITADEL_APP_ID" ] || die "$app exists and is not Citadel; move it away and rerun"
        if pgrep -f "$app/Contents/MacOS/" >/dev/null 2>&1; then
            die "Citadel is running; quit it and rerun the installer"
        fi
    fi

    CITADEL_MNT="$CITADEL_TMP/mnt"
    mkdir -p "$CITADEL_MNT"
    hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$CITADEL_MNT" "$CITADEL_FILE" >/dev/null ||
        die "could not mount $asset"
    [ -d "$CITADEL_MNT/Citadel.app" ] || die "$asset does not contain Citadel.app"
    rm -rf "$app.new.$$"
    ditto "$CITADEL_MNT/Citadel.app" "$app.new.$$"
    hdiutil detach "$CITADEL_MNT" -quiet >/dev/null 2>&1 || hdiutil detach "$CITADEL_MNT" -force -quiet >/dev/null 2>&1 || true
    CITADEL_MNT=""
    rm -rf "$app"
    mv "$app.new.$$" "$app"
    say "installed $app"

    xattr -dr com.apple.quarantine "$app" 2>/dev/null || true
    say "cleared com.apple.quarantine on $app: you ran this installer yourself, which is the consent Gatekeeper's first-launch prompt would ask for"
    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    if [ -x "$lsregister" ]; then
        "$lsregister" -f "$app" >/dev/null 2>&1 || true
    fi
    say "open it from Launchpad, Spotlight or $dest; uninstall: rerun this script with --uninstall"
}

macos_uninstall() {
    found=0
    for app in "/Applications/Citadel.app" "$HOME/Applications/Citadel.app"; do
        [ -e "$app" ] || continue
        if [ "$(app_bundle_id "$app")" != "$CITADEL_APP_ID" ]; then
            warn "$app is not Citadel ($CITADEL_APP_ID); leaving it"
            continue
        fi
        rm -rf "$app" || die "could not remove $app; try: sudo rm -rf '$app'"
        say "removed $app"
        found=1
    done
    [ "$found" -eq 1 ] || say "no Citadel.app found in /Applications or ~/Applications"
    say "kept your vault and settings in $HOME/Library/Application Support/$CITADEL_APP_ID; delete it by hand to erase all data"
}

main() {
    CITADEL_WANT="${CITADEL_VERSION:-}"
    CITADEL_METHOD="${CITADEL_METHOD:-auto}"
    CITADEL_ASSETS_DIR="${CITADEL_ASSETS_DIR:-}"
    CITADEL_SUDO=""
    CITADEL_MNT=""
    CITADEL_TMP=""
    action=install
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || die "--version needs a value"
                CITADEL_WANT="$2"
                shift 2
                ;;
            --version=*) CITADEL_WANT="${1#*=}"; shift ;;
            --method)
                [ $# -ge 2 ] || die "--method needs a value"
                CITADEL_METHOD="$2"
                shift 2
                ;;
            --method=*) CITADEL_METHOD="${1#*=}"; shift ;;
            --appimage) CITADEL_METHOD=appimage; shift ;;
            --uninstall) action=uninstall; shift ;;
            -h | --help) usage; return 0 ;;
            *) die "unknown option '$1' (see --help)" ;;
        esac
    done
    [ -n "${HOME:-}" ] || die "HOME is not set"
    detect_platform

    if [ "$action" = uninstall ]; then
        if [ "$CITADEL_OS" = linux ]; then linux_uninstall; else macos_uninstall; fi
        return 0
    fi

    have curl || [ -n "$CITADEL_ASSETS_DIR" ] || die "curl is required"
    if [ -n "$CITADEL_ASSETS_DIR" ]; then
        [ -d "$CITADEL_ASSETS_DIR" ] || die "CITADEL_ASSETS_DIR=$CITADEL_ASSETS_DIR is not a directory"
        CITADEL_ASSETS_DIR=$(cd "$CITADEL_ASSETS_DIR" && pwd)
    fi
    CITADEL_TMP=$(mktemp -d 2>/dev/null || mktemp -d -t citadel) || die "cannot create a temporary directory"
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM HUP

    resolve_tag
    load_sums
    if [ "$CITADEL_OS" = linux ]; then linux_install; else macos_install; fi
}

main "$@"
