#!/bin/sh
# Run on the download host after both signed archives and feeds have arrived.
set -eu

download=$1
stage=$2
build=$3
mkdir -p "$download/releases"
chmod 755 "$download/releases"
exec 9>"$download/../.mac-release.lock"
flock 9

build_number() {
    sed -n 's/.*(\([0-9][0-9]*\))$/\1/p' "$1"
}

# Before the first Sparkle release, version.txt is still a regular file.
version="$download/current/version.txt"
[ -f "$version" ] || version="$download/version.txt"
if [ -f "$version" ]; then
    published=$(build_number "$version")
    if ! [ "$build" -gt "$published" ]; then
        echo "Build $build must be newer than published build $published." >&2
        exit 1
    fi
fi

release="releases/$build"
if [ -e "$download/$release" ]; then
    echo "Build $build already has archives. Publish with a higher CUBIE_BUILD." >&2
    exit 1
fi

manual_files='Cubie-apple-silicon.zip Cubie-intel.zip version.txt'
for file in $manual_files appcast-apple-silicon.xml appcast-intel.xml; do
    test -s "$stage/$file"
    chmod 644 "$stage/$file"
done
[ "$(build_number "$stage/version.txt")" = "$build" ] || {
    echo "Staged version.txt does not match build $build." >&2
    exit 1
}
chmod 755 "$stage"

links=$(mktemp -d "$download/../.mac-links.XXXXXXXX")
trap 'rm -rf "$links"' EXIT
trap 'exit 1' HUP INT TERM

# On the first release, point current at the legacy downloads while the
# stable links are migrated. An interruption still serves the old downloads.
if [ ! -L "$download/current" ]; then
    legacy="$download/releases/legacy"
    mkdir -p "$legacy"
    chmod 755 "$legacy"
    for file in $manual_files; do
        if [ -f "$download/$file" ]; then
            ln -f "$download/$file" "$legacy/$file"
        fi
    done
    ln -s releases/legacy "$links/current"
    mv -Tf "$links/current" "$download/current"
fi
for file in $manual_files; do
    ln -s "current/$file" "$links/$file"
    mv -Tf "$links/$file" "$download/$file"
done

# Publish the archives first, then switch every stable URL together.
ln -s "$release" "$links/current"
mv -T "$stage" "$download/$release"
mv -Tf "$links/current" "$download/current"
