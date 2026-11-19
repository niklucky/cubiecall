#!/bin/sh
# Build Cubie.app for Apple Silicon and for Intel, signed and zipped:
# target/mac/Cubie-apple-silicon.zip and target/mac/Cubie-intel.zip.
# One app per architecture to keep the downloads small.
#
#   deploy/mac/package.sh            build only
#   deploy/mac/package.sh --appcast  build and sign the update feeds locally
#   deploy/mac/package.sh --upload   build and publish at https://cubiecall.com/download/
#
# CUBIE_BUILD overrides the commit-count build number for an uncommitted release.
# CUBIE_SERVER picks the server the app talks to (default https://cubiecall.com).
# The signing identity lives in CUBIE_SIGNING_KEYCHAIN (made on first use,
# see signing.sh).
set -eu
cd "$(dirname "$0")/../.."

mode=${1:-}
case "$mode" in
    ''|--appcast|--upload) ;;
    *) echo "usage: $0 [--appcast|--upload]" >&2; exit 1 ;;
esac

HOST=${CUBIE_HOST:-${QUIPCALL_HOST:-root@testron.dev}}
PRODUCTION_SERVER=https://cubiecall.com
DOWNLOAD_URL=$PRODUCTION_SERVER/download
export CUBIE_SERVER="${CUBIE_SERVER:-${QUIPCALL_SERVER:-$PRODUCTION_SERVER}}"
# ScreenCaptureKit arrived in 12.3. Below 10.14.4 the linker would also point
# Swift overlays (libswiftCoreMedia) at @rpath, which doesn't exist in the app.
export MACOSX_DEPLOYMENT_TARGET=12.3

# Protect target/mac through the upload. Concurrent calls wait for the same lock.
mkdir -p target
exec 9>target/.mac-package.lock
lockf 9

sparkle=
if [ "${CUBIE_SERVER%/}" = "$PRODUCTION_SERVER" ]; then
    sparkle=$(deploy/mac/sparkle.sh)
elif [ -n "$mode" ]; then
    echo "The Cubie release feeds are for $PRODUCTION_SERVER. Build custom-server archives with make mac-app." >&2
    exit 1
fi
account=com.quipcall.desktop
# SHA-1 fingerprint used by codesign to select the certificate in the shipping
# app's designated requirement.
shipping_identity=7792AB4735B4F5DE6EB9A909C597E169863E4860
if [ -n "$mode" ]; then
    public_key=$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' deploy/mac/Info.plist)
    if ! signing_key=$("$sparkle/bin/generate_keys" --account "$account" -p) ||
        [ "$signing_key" != "$public_key" ]; then
        echo "Restore the Cubie Sparkle signing key to your login Keychain (see docs/DEPLOY.md)." >&2
        exit 1
    fi
fi

KEYCHAIN=${CUBIE_SIGNING_KEYCHAIN:-${QUIPCALL_SIGNING_KEYCHAIN:-}}
if [ -z "$KEYCHAIN" ]; then
    KEYCHAIN=$HOME/Library/Keychains/cubie-signing.keychain-db
    # Keep the shipping certificate and its designated requirement on upgrades.
    old_keychain=$HOME/Library/Keychains/quipcall-signing.keychain-db
    [ ! -f "$old_keychain" ] || KEYCHAIN=$old_keychain
fi
if [ ! -f "$KEYCHAIN" ]; then
    if [ -n "$mode" ]; then
        echo "Restore the shipping Cubie code-signing keychain (see docs/DEPLOY.md)." >&2
        exit 1
    fi
    deploy/mac/signing.sh "$KEYCHAIN"
fi
security unlock-keychain -p "" "$KEYCHAIN"
identities=$(security find-identity -p codesigning "$KEYCHAIN")
if [ -n "$mode" ]; then
    identity=$(printf '%s\n' "$identities" | awk -v shipping="$shipping_identity" '$2 == shipping { print $2; exit }')
    [ -n "$identity" ] || { echo "No shipping Cubie code-signing identity in $KEYCHAIN (see docs/DEPLOY.md)." >&2; exit 1; }
else
    identity=$(printf '%s\n' "$identities" | awk '/"(Quipcall|Cubie) Signing"/ { print $2; exit }')
    [ -n "$identity" ] || { echo "No Cubie signing identity in $KEYCHAIN" >&2; exit 1; }
fi

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
	sed -e "s/@VERSION@/$VERSION/" -e "s/@BUILD@/$BUILD/" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/" -e "s/@ICON@/$ICON/" -e "s/@ARCH@/$name/" \
		deploy/mac/Info.plist >"$app/Contents/Info.plist"
	if [ -n "$sparkle" ]; then
		cp "$sparkle/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE.txt"
		ditto "$sparkle/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
	else
		/usr/libexec/PlistBuddy -c 'Delete :SUFeedURL' "$app/Contents/Info.plist"
	fi

	# Everything the app loads must ship with macOS: no @rpath / @executable_path.
	if otool -L "$app/Contents/MacOS/Cubie" | tail -n +2 | grep -q '@'; then
		otool -L "$app/Contents/MacOS/Cubie"
		echo "error: the $name binary links a library that isn't part of macOS" >&2
		exit 1
	fi

	# Keep Cubie's certificate and Sparkle's upstream helper signatures.
	codesign --force --keychain "$KEYCHAIN" --sign "$identity" "$app"
	codesign --verify --deep --strict "$app"
	codesign -d -r- "$app" 2>&1 | grep -q 'certificate leaf' || {
		echo "error: $app isn't signed with the Cubie certificate" >&2
		exit 1
	}
	zip=Cubie-$name.zip
	ditto -c -k --keepParent "$app" "$OUT/$zip"
	if [ -n "$mode" ]; then
		# Sparkle selects by build number, so generate each feed from only its architecture.
		ln "$OUT/$zip" "$OUT/$name/$zip"
		"$sparkle/bin/generate_appcast" --account "$account" --maximum-deltas 0 \
			--download-url-prefix "$DOWNLOAD_URL/releases/$BUILD/" -o "$OUT/appcast-$name.xml" "$OUT/$name"
		rm "$OUT/$name/$zip"
	fi
done

echo "$VERSION ($BUILD)" >"$OUT/version.txt"
ls -lh "$OUT"/*.zip

if [ "$mode" = "--upload" ]; then
	stage=$(ssh "$HOST" 'install -d -m 755 /opt/cubie /opt/cubie/download && mktemp -d /opt/cubie/.mac-release.XXXXXXXX')
	trap 'ssh "$HOST" "rm -rf -- $stage" || true' EXIT
	trap 'exit 1' HUP INT TERM
	scp -q "$OUT"/Cubie-*.zip "$OUT"/appcast-*.xml "$OUT/version.txt" "$HOST:$stage/"
	# The staging path and numeric build are evaluated here.
	# shellcheck disable=SC2029
	ssh "$HOST" "sh -s -- /opt/cubie/download $stage $BUILD" < deploy/mac/publish.sh
	echo "published $VERSION (build $BUILD): $DOWNLOAD_URL/"
fi
