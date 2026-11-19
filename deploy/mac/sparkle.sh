#!/bin/sh
# Fetch the pinned Sparkle framework and its release tools into target/.
set -eu
cd "$(dirname "$0")/../.."

version=2.10.1
sha256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
destination="target/sparkle-$version"
if [ ! -d "$destination" ]; then
    mkdir -p target
    tmp=$(mktemp -d target/.sparkle.XXXXXXXX)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM
    archive="$tmp/Sparkle.tar.xz"
    curl -fLsS --retry 3 "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" -o "$archive"
    printf '%s  %s\n' "$sha256" "$archive" | shasum -a 256 -c - >&2
    mkdir "$tmp/unpacked"
    tar -xf "$archive" -C "$tmp/unpacked"
    [ -d "$destination" ] || mv "$tmp/unpacked" "$destination"
fi
printf '%s\n' "$destination"
