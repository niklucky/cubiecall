#!/bin/sh
# Build Cubie.app for Apple Silicon and for Intel, signed and zipped:
# target/mac/Cubie-apple-silicon.zip and target/mac/Cubie-intel.zip.
# One app per architecture: a universal binary would be ~60 MB, over budget.
#
#   deploy/mac/package.sh            build only
#   deploy/mac/package.sh --upload   build and publish at https://cubiecall.com/download/
#
# CUBIE_BUILD overrides the commit-count build number for an uncommitted release.
# CUBIE_SERVER picks the server the app talks to (default https://cubiecall.com).
# The signing identity lives in CUBIE_SIGNING_KEYCHAIN (made on first use,
# see signing.sh).
set -eu
cd "$(dirname "$0")/../.."

HOST=${CUBIE_HOST:-${QUIPCALL_HOST:-root@testron.dev}}
export CUBIE_SERVER="${CUBIE_SERVER:-${QUIPCALL_SERVER:-https://cubiecall.com}}"
# ScreenCaptureKit arrived in 12.3. Below 10.14.4 the linker would also point
# Swift overlays (libswiftCoreMedia) at @rpath, which doesn't exist in the app.
export MACOSX_DEPLOYMENT_TARGET=12.3

KEYCHAIN=${CUBIE_SIGNING_KEYCHAIN:-${QUIPCALL_SIGNING_KEYCHAIN:-}}
if [ -z "$KEYCHAIN" ]; then
    KEYCHAIN=$HOME/Library/Keychains/cubie-signing.keychain-db
    # Keep the shipping certificate and its designated requirement on upgrades.
    old_keychain=$HOME/Library/Keychains/quipcall-signing.keychain-db
    [ ! -f "$old_keychain" ] || KEYCHAIN=$old_keychain
fi
[ -f "$KEYCHAIN" ] || deploy/mac/signing.sh "$KEYCHAIN"
security unlock-keychain -p "" "$KEYCHAIN"
# Match either our original signing certificate or the one created on a new Mac.
identities=$(security find-identity -p codesigning "$KEYCHAIN")
identity=$(printf '%s\n' "$identities" | awk '/"(Quipcall|Cubie) Signing"/ { print $2; exit }')
[ -n "$identity" ] || { echo "No Cubie signing identity in $KEYCHAIN" >&2; exit 1; }

VERSION=$(cargo pkgid -p cubie-desktop | sed 's/.*@//')
BUILD=${CUBIE_BUILD:-${QUIPCALL_BUILD:-$(git rev-list --count HEAD)}}
case "$BUILD" in
    ''|*[!0-9]*) echo "error: CUBIE_BUILD must be a positive integer" >&2; exit 1 ;;
esac
[ "$BUILD" -gt 0 ] || { echo "error: CUBIE_BUILD must be a positive integer" >&2; exit 1; }
OUT=target/mac
# The app icon in apps/desktop/assets: the cube on transparency. `Quby` is
# the earlier one, the cube on a black tile.
ICON=QubyTransparent
rm -rf "$OUT"

for pair in aarch64-apple-darwin:apple-silicon x86_64-apple-darwin:intel; do
	target=${pair%%:*}
	name=${pair##*:}
	cargo build --release -p cubie-desktop --target "$target"

	app=$OUT/$name/Cubie.app
	mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
	cp "target/$target/release/cubie-desktop" "$app/Contents/MacOS/Cubie"
	cp "apps/desktop/assets/$ICON.icns" "$app/Contents/Resources/$ICON.icns"
	sed -e "s/@VERSION@/$VERSION/" -e "s/@BUILD@/$BUILD/" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/" -e "s/@ICON@/$ICON/" \
		deploy/mac/Info.plist >"$app/Contents/Info.plist"

	# Everything the app loads must ship with macOS: no @rpath / @executable_path.
	if otool -L "$app/Contents/MacOS/Cubie" | tail -n +2 | grep -q '@'; then
		otool -L "$app/Contents/MacOS/Cubie"
		echo "error: the $name binary links a library that isn't part of macOS" >&2
		exit 1
	fi

	# The same certificate every build, so macOS keeps the app's permissions
	# across updates. It isn't a Developer ID: Gatekeeper still needs one
	# manual "Open" for a downloaded zip.
	codesign --force --keychain "$KEYCHAIN" --sign "$identity" "$app"
	codesign -d -r- "$app" 2>&1 | grep -q 'certificate leaf' || {
		echo "error: $app isn't signed with the Cubie certificate" >&2
		exit 1
	}
	ditto -c -k --keepParent "$app" "$OUT/Cubie-$name.zip"
done

echo "$VERSION ($BUILD)" >"$OUT/version.txt"
ls -lh "$OUT"/*.zip

if [ "${1:-}" = "--upload" ]; then
	# Upload next to the live files, then swap them in.
	ssh "$HOST" 'rm -rf /opt/cubie/download/incoming && mkdir -p /opt/cubie/download/incoming'
	scp -q "$OUT"/Cubie-*.zip "$OUT/version.txt" "$HOST:/opt/cubie/download/incoming/"
	ssh "$HOST" 'cd /opt/cubie/download && chmod 644 incoming/* && mv incoming/* . && rmdir incoming'
	echo "published $VERSION (build $BUILD): https://cubiecall.com/download/"
fi
