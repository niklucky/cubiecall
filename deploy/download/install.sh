#!/bin/sh
# Install or update Cubie:
#   curl -fsSL https://cubiecall.com/download/install.sh | sh
# Downloaded with curl, the app carries no quarantine flag, so macOS opens it
# without the "unidentified developer" detour.
set -eu
# hw.optional.arm64 is 1 on Apple Silicon, even in a Terminal running under Rosetta.
if [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
	URL=https://cubiecall.com/download/Cubie-apple-silicon.zip
else
	URL=https://cubiecall.com/download/Cubie-intel.zip
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
echo "Downloading Cubie…"
curl -fL --progress-bar "$URL" -o "$tmp/Cubie.zip"
ditto -x -k "$tmp/Cubie.zip" "$tmp"
if [ ! -x "$tmp/Cubie.app/Contents/MacOS/Cubie" ] || [ "$(bundle_id "$tmp/Cubie.app")" != com.quipcall.desktop ]; then
    echo "The download is missing the Cubie app." >&2
    exit 1
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
    /usr/bin/osascript -l JavaScript - "$dest/Cubie.app" "$tmp/Cubie.app" <<'JS'
ObjC.import('Foundation');
function run(paths) {
    const url = path => $.NSURL.fileURLWithPath(path);
    const error = Ref();
    // NSFileManagerItemReplacementUsingNewMetadataOnly preserves the signed bundle.
    const replaced = $.NSFileManager.defaultManager.replaceItemAtURLWithItemAtURLBackupItemNameOptionsResultingItemURLError(
        url(paths[0]), url(paths[1]), $(), 1, Ref(), error
    );
    if (!replaced) throw Error(ObjC.unwrap(error[0].localizedDescription));
}
JS
else
    mv "$tmp/Cubie.app" "$dest/Cubie.app"
fi
if [ "$legacy_id" = com.quipcall.desktop ]; then
    rm -rf "$legacy"
fi
xattr -dr com.apple.quarantine "$dest/Cubie.app" 2>/dev/null || true

echo "Installed $dest/Cubie.app"
open "$dest/Cubie.app"
