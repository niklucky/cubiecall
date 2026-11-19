#!/bin/sh
# Run on Linux with GNU coreutils and flock.
set -eu
publisher=$(cd "$(dirname "$0")" && pwd)/publish.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
download=$tmp/download
stage=$tmp/stage
archives='Cubie-apple-silicon.zip Cubie-intel.zip'
feeds='appcast-apple-silicon.xml appcast-intel.xml'
mkdir "$download"
for file in $archives; do
    echo "legacy-$file" >"$download/$file"
done
echo '0.1.0 (103)' >"$download/version.txt"

staged_release() {
    mkdir -p "$stage"
    for file in $archives $feeds; do
        echo "$1-$file" >"$stage/$file"
    done
    echo "0.1.0 ($1)" >"$stage/version.txt"
}

assert_live() {
    for file in $archives; do
        test "$(cat "$download/$file")" = "$1-$file"
    done
    test "$(cat "$download/version.txt")" = "0.1.0 ($2)"
}

umask 077
staged_release 104
if sh "$publisher" "$download" "$stage" 105; then
    echo 'Publisher accepted a mismatched build number.' >&2
    exit 1
fi
assert_live legacy 103

# Interrupt the first release between the two manual archive link swaps.
mkdir "$tmp/bin"
cat >"$tmp/bin/mv" <<'SH'
#!/bin/sh
for destination do :; done
[ "$destination" != "$FAIL_MOVE_TO" ] || exit 1
exec "$REAL_MV" "$@"
SH
chmod +x "$tmp/bin/mv"
if REAL_MV=$(command -v mv) FAIL_MOVE_TO="$download/Cubie-intel.zip" PATH="$tmp/bin:$PATH" \
    sh "$publisher" "$download" "$stage" 104; then
    echo 'Publish succeeded despite an interrupted link swap.' >&2
    exit 1
fi
assert_live legacy 103
test ! -e "$download/current/appcast-apple-silicon.xml"

sh "$publisher" "$download" "$stage" 104
assert_live 104 104
test "$(stat -c %a "$download/releases")" = 755
test "$(stat -c %a "$download/releases/104")" = 755
staged_release 105
sh "$publisher" "$download" "$stage" 105
assert_live 105 105
for file in $archives $feeds; do
    test "$(cat "$download/current/$file")" = "105-$file"
    test "$(cat "$download/releases/104/$file")" = "104-$file"
done
staged_release 104
if sh "$publisher" "$download" "$stage" 104; then
    echo 'Publisher accepted a stale build.' >&2
    exit 1
fi
assert_live 105 105
echo 'Mac publish tests passed.'
