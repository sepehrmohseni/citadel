#!/usr/bin/env bash
# Copies the installers produced by `pnpm tauri build` for the current app
# version into one flat directory, ready for upload. Usage:
#   scripts/collect-bundles.sh <out-dir>
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:?usage: collect-bundles.sh <out-dir>}"
version="$(node -p "require(process.argv[1]).version" "$root/app/src-tauri/tauri.conf.json")"

mkdir -p "$out"
found=0
while IFS= read -r -d '' file; do
    name="$(basename "$file")"
    case "$name" in
        *"_${version}_"* | *"-${version}-"*) ;;
        *) echo "skipping stale bundle $name (app version is $version)" >&2; continue ;;
    esac
    cp -- "$file" "$out/$name"
    echo "collected $name"
    found=$((found + 1))
done < <(find "$root/target" -type f \( \
    -path '*/release/bundle/deb/*.deb' -o \
    -path '*/release/bundle/rpm/*.rpm' -o \
    -path '*/release/bundle/appimage/*.AppImage' -o \
    -path '*/release/bundle/nsis/*-setup.exe' -o \
    -path '*/release/bundle/msi/*.msi' -o \
    -path '*/release/bundle/dmg/*.dmg' \) -print0)

# macOS: the updater installs from a tar.gz of the .app bundle, with the bundle at its top level.
while IFS= read -r -d '' app; do
    built="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist" 2>/dev/null || true)"
    if [ "$built" != "$version" ]; then
        echo "skipping stale app bundle $app (version ${built:-unknown}, app version is $version)" >&2
        continue
    fi
    case "$app" in
        *universal-apple-darwin*) arch=universal ;;
        *aarch64-apple-darwin*) arch=aarch64 ;;
        *x86_64-apple-darwin*) arch=x64 ;;
        *) arch="$(uname -m)" ;;
    esac
    name="Citadel_${version}_${arch}.app.tar.gz"
    # No AppleDouble (._*) entries: the updater unpacks every entry below the bundle directory.
    COPYFILE_DISABLE=1 tar -czf "$out/$name" -C "$(dirname "$app")" "$(basename "$app")"
    echo "collected $name"
    found=$((found + 1))
done < <(find "$root/target" -type d -path '*/release/bundle/macos/*.app' -prune -print0)

if [ "$found" -eq 0 ]; then
    echo "error: no bundles for version $version under $root/target" >&2
    exit 1
fi
