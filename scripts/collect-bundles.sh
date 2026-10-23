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

if [ "$found" -eq 0 ]; then
    echo "error: no bundles for version $version under $root/target" >&2
    exit 1
fi
