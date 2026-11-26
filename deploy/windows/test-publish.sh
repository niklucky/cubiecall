#!/bin/sh
# Run on Linux with GNU coreutils and flock, as an unprivileged user.
set -eu
publisher=$(cd "$(dirname "$0")" && pwd)/publish.sh
tmp=$(mktemp -d)
trap 'chmod u+w "$tmp/cubie"; rm -rf "$tmp"' EXIT
download=$tmp/cubie/download
zip=Cubie-windows-x64.zip
mkdir -p "$download" "$tmp/cubie/mac-staging"
# Match the release account: writable download/staging, read-only parent.
chmod 555 "$tmp/cubie"
if [ -w "$tmp/cubie" ]; then
    echo 'Run publish tests as an unprivileged user to check directory permissions.' >&2
    exit 1
fi
# A Mac release is already there and must stay as it is.
mkdir -p "$download/releases/1501"
echo mac >"$download/releases/1501/Cubie-intel.zip"
ln -s releases/1501 "$download/current"
ln -s current/Cubie-intel.zip "$download/Cubie-intel.zip"

staged() {
    stage=$(mktemp -d "$tmp/cubie/mac-staging/.windows-release.XXXXXXXX")
    echo "$1-$zip" >"$stage/$zip"
    echo "0.1.0 ($1)" >"$stage/version.txt"
}

assert_live() {
    test "$(cat "$download/$zip")" = "$1-$zip"
    test "$(cat "$download/windows/current/version.txt")" = "0.1.0 ($1)"
    test "$(cat "$download/Cubie-intel.zip")" = mac
    test "$(readlink "$download/current")" = releases/1501
}

umask 077
staged 104
if sh "$publisher" "$download" "$stage" 105; then
    echo 'Publisher accepted a mismatched build number.' >&2
    exit 1
fi
test ! -e "$download/$zip"

sh "$publisher" "$download" "$stage" 104
assert_live 104
test ! -e "$stage"
# What nginx serves must be world-readable.
test "$(stat -c %a "$download/windows/releases/104")" = 755
test "$(stat -c %a "$download/windows/releases/104/$zip")" = 644

staged 1301
sh "$publisher" "$download" "$stage" 1301
assert_live 1301
# Earlier builds stay where they were.
test "$(cat "$download/windows/releases/104/$zip")" = "104-$zip"

for stale in 1301 1200; do
    staged "$stale"
    if sh "$publisher" "$download" "$stage" "$stale"; then
        echo "Publisher accepted stale build $stale." >&2
        exit 1
    fi
    assert_live 1301
done
test -z "$(find "$download/windows" -maxdepth 1 -name '.links.*')"
echo 'Windows publisher tests passed.'
