#!/bin/sh
# Run on the download host once the Windows zip has arrived (upload.sh).
# Keeps each build at download/windows/releases/<build>/ and points the stable
# /download/Cubie-windows-x64.zip at the newest one, through windows/current.
# The Mac releases (download/current, download/releases) are left alone.
set -eu

download=$1
stage=$2
build=$3
root=$download/windows
zip=Cubie-windows-x64.zip
mkdir -p "$root/releases"
chmod 755 "$root" "$root/releases"
exec 9>"$root/.release.lock"
flock 9

build_number() {
    sed -n 's/.*(\([0-9][0-9]*\))$/\1/p' "$1"
}

if [ -f "$root/current/version.txt" ]; then
    published=$(build_number "$root/current/version.txt")
    if ! [ "$build" -gt "$published" ]; then
        echo "Build $build must be newer than published Windows build $published." >&2
        exit 1
    fi
fi

release="releases/$build"
if [ -e "$root/$release" ]; then
    echo "Windows build $build already has a zip. Publish a newer build." >&2
    exit 1
fi

for file in $zip version.txt; do
    test -s "$stage/$file"
    chmod 644 "$stage/$file"
done
[ "$(build_number "$stage/version.txt")" = "$build" ] || {
    echo "Staged version.txt does not match build $build." >&2
    exit 1
}
chmod 755 "$stage"

# The release account can write download/, but not its parent /opt/cubie.
links=$(mktemp -d "$root/.links.XXXXXXXX")
trap 'rm -rf "$links"' EXIT
trap 'exit 1' HUP INT TERM

# The zip first, then current, then the stable URL (through current, so it
# only ever needs making once; replacing it again changes nothing).
mv -T "$stage" "$root/$release"
ln -s "$release" "$links/current"
mv -Tf "$links/current" "$root/current"
ln -s "windows/current/$zip" "$links/$zip"
mv -Tf "$links/$zip" "$download/$zip"
