#!/bin/sh
# Build Cubie.app for Apple Silicon and for Intel, signed, notarized and zipped:
# target/mac/Cubie-apple-silicon.zip and target/mac/Cubie-intel.zip.
# One app per architecture to keep the downloads small.
#
#   deploy/mac/package.sh            build and notarize only
#   deploy/mac/package.sh --appcast  build and sign the update feeds locally
#   deploy/mac/package.sh --upload   build and publish at https://cubiecall.com/download/
#
# CUBIE_BUILD overrides the commit-count build number for an uncommitted release.
# CUBIE_SERVER picks the server the app talks to (default https://cubiecall.com).
# CUBIE_SIGNING_IDENTITY overrides the Developer ID certificate fingerprint.
# CUBIE_SIGNING_KEYCHAIN optionally selects a keychain (default: search list).
# CUBIE_NOTARY_PROFILE selects notarytool credentials (default: cubie-notary).
# CUBIE_VERSION sets the displayed app version (CI uses the release tag).
# --arch apple-silicon|intel builds one architecture for a CI job.
set -eu
cd "$(dirname "$0")/../.."

mode=
architecture=all
while [ "$#" -gt 0 ]; do
    case "$1" in
        --appcast|--upload) [ -z "$mode" ] || exit 1; mode=$1; shift ;;
        --arch) architecture=${2:?missing architecture}; shift 2 ;;
        *) echo "usage: $0 [--appcast|--upload] [--arch apple-silicon|intel]" >&2; exit 1 ;;
    esac
done
case "$architecture" in
    all) pairs='aarch64-apple-darwin:apple-silicon x86_64-apple-darwin:intel' ;;
    apple-silicon) pairs=aarch64-apple-darwin:apple-silicon ;;
    intel) pairs=x86_64-apple-darwin:intel ;;
    *) echo "Unknown Mac architecture: $architecture" >&2; exit 1 ;;
esac
if [ "$mode" = --upload ] && [ "$architecture" != all ]; then
    echo 'Publishing requires both architectures.' >&2
    exit 1
fi

HOST=${CUBIE_HOST:-${QUIPCALL_HOST:-root@testron.dev}}
PRODUCTION_SERVER=https://cubiecall.com
DOWNLOAD_URL=$PRODUCTION_SERVER/download
export CUBIE_SERVER="${CUBIE_SERVER:-${QUIPCALL_SERVER:-$PRODUCTION_SERVER}}"
# ScreenCaptureKit arrived in 12.3. Below 10.14.4 the linker would also point
# Swift overlays (libswiftCoreMedia) at @rpath, which doesn't exist in the app.
export MACOSX_DEPLOYMENT_TARGET=12.3

# Protect target/mac through the upload. Concurrent calls wait for the same lock.
mkdir -p target
if [ -z "${CUBIE_PACKAGE_LOCKED:-}" ]; then
    set --
    [ -z "$mode" ] || set -- "$mode"
    exec perl -MFcntl=:flock -e '
        open(my $lock, ">>", shift) or die "lock: $!";
        flock($lock, LOCK_EX) or die "lock: $!";
        system(@ARGV);
        exit($? == -1 ? 1 : ($? & 127 ? 128 + ($? & 127) : $? >> 8));
    ' target/.mac-package.lock env CUBIE_PACKAGE_LOCKED=1 "$0" "$@" --arch "$architecture"
fi

sparkle=
if [ "${CUBIE_SERVER%/}" = "$PRODUCTION_SERVER" ]; then
    sparkle=$(deploy/mac/sparkle.sh)
elif [ -n "$mode" ]; then
    echo "The Cubie release feeds are for $PRODUCTION_SERVER. Build custom-server archives with make mac-app." >&2
    exit 1
fi
account=com.quipcall.desktop
identity=${CUBIE_SIGNING_IDENTITY:-FB9D41EDF844DF368B68B049D8A9895D5C5F0B96}
team=8BSJZG6B89
profile=${CUBIE_NOTARY_PROFILE:-cubie-notary}
# Refuse a missing, development or self-signed identity before building.
set --
if [ -n "${CUBIE_SIGNING_KEYCHAIN:-}" ]; then
    set -- "$CUBIE_SIGNING_KEYCHAIN"
fi
identities=$(security find-identity -v -p codesigning "$@")
if ! printf '%s\n' "$identities" | awk -v identity="$identity" -v team="$team" '
    $2 == identity && index($0, "\"Developer ID Application: ") && index($0, "(" team ")\"") { found = 1 }
    END { exit !found }
'; then
    echo "No valid Developer ID Application identity $identity for team $team. See docs/DEPLOY.md." >&2
    exit 1
fi
set -- --keychain-profile "$profile"
if [ -n "${CUBIE_NOTARY_KEYCHAIN:-}" ]; then
    set -- "$@" --keychain "$CUBIE_NOTARY_KEYCHAIN"
fi
xcrun notarytool history "$@" --output-format json >/dev/null
if [ -n "$mode" ]; then
    public_key=$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' deploy/mac/Info.plist)
    if ! signing_key=$("$sparkle/bin/generate_keys" --account "$account" -p) ||
        [ "$signing_key" != "$public_key" ]; then
        echo "Restore the Cubie Sparkle signing key to your login Keychain (see docs/DEPLOY.md)." >&2
        exit 1
    fi
fi

sign_code() {
    if [ -n "${CUBIE_SIGNING_KEYCHAIN:-}" ]; then
        set -- --keychain "$CUBIE_SIGNING_KEYCHAIN" "$@"
    fi
    codesign --force --sign "$identity" --options runtime --timestamp "$@"
}

export CUBIE_VERSION="${CUBIE_VERSION:-$(cargo pkgid -p cubie-desktop | sed 's/.*@//')}"
VERSION=$CUBIE_VERSION
printf '%s\n' "$VERSION" | LC_ALL=C grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || {
    echo 'CUBIE_VERSION must be X.Y.Z.' >&2; exit 1;
}
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

for pair in $pairs; do
	target=${pair%%:*}
	name=${pair##*:}
	cargo build --locked --release -p cubie-desktop --target "$target"

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

	# Sign nested code from the inside out, with the same team as the app.
	# Do not use --deep for signing: helpers need their own entitlements.
	if [ -n "$sparkle" ]; then
		framework=$app/Contents/Frameworks/Sparkle.framework
		sign_code "$framework/Versions/B/XPCServices/Installer.xpc"
		sign_code --preserve-metadata=entitlements "$framework/Versions/B/XPCServices/Downloader.xpc"
		sign_code "$framework/Versions/B/Autoupdate"
		sign_code "$framework/Versions/B/Updater.app"
		sign_code "$framework"
	fi
	sign_code --entitlements deploy/mac/entitlements.plist "$app"
	codesign --verify --deep --strict "$app"
	codesign -dv "$app" 2>&1 | grep -qx "TeamIdentifier=$team" || {
		echo "error: $app isn't signed by the Cubie Apple developer team" >&2
		exit 1
	}
	zip=Cubie-$name.zip
	deploy/mac/notarize.sh "$app" "$OUT/$zip" "$profile"
	if [ -n "$mode" ]; then
		# Sparkle selects by build number, so generate each feed from only its architecture.
		ln "$OUT/$zip" "$OUT/$name/$zip"
		set -- --account "$account"
		if [ -n "${CUBIE_SPARKLE_KEY_FILE:-}" ]; then
			# CI uses a protected temporary file to avoid a Keychain access dialog
			# when generate_appcast reads an item created by generate_keys.
			set -- --ed-key-file "$CUBIE_SPARKLE_KEY_FILE"
		fi
		"$sparkle/bin/generate_appcast" "$@" --maximum-deltas 0 \
			--download-url-prefix "$DOWNLOAD_URL/releases/$BUILD/" -o "$OUT/appcast-$name.xml" "$OUT/$name"
		rm "$OUT/$name/$zip"
	fi
done

echo "$VERSION ($BUILD)" >"$OUT/version.txt"
ls -lh "$OUT"/*.zip

if [ "$mode" = "--upload" ]; then
	python3 deploy/mac/verify-release.py "$OUT" --write-checksums
	CUBIE_HOST="$HOST" deploy/mac/upload.sh "$OUT"
	echo "published $VERSION (build $BUILD): $DOWNLOAD_URL/"
fi
