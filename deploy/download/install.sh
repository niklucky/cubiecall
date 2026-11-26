#!/bin/sh
# Install or update Cubie:
#   curl -fsSL --proto-redir '=https' https://cubiecall.com/download/install.sh | sh
#   curl -fsSL --proto-redir '=https' https://cubiecall.ru/download/install.sh | sh -s -- --reserve
set -eu
primary=https://cubiecall.com
reserve=https://cubiecall.ru
if [ "${1:-}" = --reserve ]; then
    first=$reserve
    second=$primary
else
    first=$primary
    second=$reserve
fi
# hw.optional.arm64 is 1 on Apple Silicon, even in a Terminal running under Rosetta.
if [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
	archive=/download/Cubie-apple-silicon.zip
else
	archive=/download/Cubie-intel.zip
fi

dest=/Applications
[ -w "$dest" ] || dest=$HOME/Applications
mkdir -p "$dest"

bundle_id() {
    /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true
}
if [ -e "$dest/Cubie.app" ] && [ "$(bundle_id "$dest/Cubie.app")" != com.quipcall.desktop ]; then
    echo "Another app already occupies $dest/Cubie.app. Move it before installing Cubie." >&2
    exit 1
fi

# Stage on the destination volume so macOS can atomically replace the bundle.
tmp=$(mktemp -d "$dest/.cubie-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
requirement='anchor apple generic and identifier "com.quipcall.desktop" and certificate leaf[subject.OU] = "8BSJZG6B89" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
fetch_signed_app() {
    mkdir "$1" &&
        curl -fL --proto '=https' --proto-redir '=https' --connect-timeout 5 \
            --speed-limit 1024 --speed-time 10 --max-time 900 --progress-bar \
            "$2$archive" -o "$1/Cubie.zip" &&
        ditto -x -k "$1/Cubie.zip" "$1" &&
        [ -d "$1/Cubie.app" ] && [ ! -L "$1/Cubie.app" ] &&
        [ -x "$1/Cubie.app/Contents/MacOS/Cubie" ] &&
        [ "$(bundle_id "$1/Cubie.app")" = com.quipcall.desktop ] &&
        /usr/bin/codesign --verify --deep --strict "$1/Cubie.app" &&
        /usr/bin/codesign --verify --strict -R="$requirement" "$1/Cubie.app"
}

echo "Downloading Cubie…"
if fetch_signed_app "$tmp/first" "$first"; then
    app=$tmp/first/Cubie.app
else
    echo "Trying the other CubieCall domain…"
    if ! fetch_signed_app "$tmp/second" "$second"; then
        echo "Could not download an app with a valid Cubie Developer ID signature." >&2
        exit 1
    fi
    app=$tmp/second/Cubie.app
fi

# Only replace the old bundle when it belongs to this app.
legacy="$dest/Quipcall.app"
legacy_id=$(bundle_id "$legacy")
if [ "$legacy_id" = com.quipcall.desktop ] && pgrep -xq Quipcall; then
    pkill -x Quipcall || true
    sleep 1
fi
if [ -d "$dest/Cubie.app" ] && pgrep -xq Cubie; then
	echo "Quitting the running Cubie…"
	pkill -x Cubie || true
	sleep 1
fi
if [ -d "$dest/Cubie.app" ]; then
    if ! /usr/bin/osascript -l JavaScript - "$dest/Cubie.app" "$app" <<'JS'
ObjC.import('Foundation');
function run(paths) {
    const url = path => $.NSURL.fileURLWithPath(path);
    // NSFileManagerItemReplacementUsingNewMetadataOnly preserves the signed bundle.
    const replaced = $.NSFileManager.defaultManager.replaceItemAtURLWithItemAtURLBackupItemNameOptionsResultingItemURLError(
        url(paths[0]), url(paths[1]), $(), 1, Ref(), Ref()
    );
    if (!replaced) throw Error('Bundle replacement failed');
}
JS
    then
        echo "Could not replace $dest/Cubie.app. The previous installation is available at that path." >&2
        exit 1
    fi
else
    mv "$app" "$dest/Cubie.app"
fi
if [ "$legacy_id" = com.quipcall.desktop ]; then
    rm -rf "$legacy"
fi
xattr -dr com.apple.quarantine "$dest/Cubie.app" 2>/dev/null || true

echo "Installed $dest/Cubie.app"
open "$dest/Cubie.app"
