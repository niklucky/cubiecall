#!/bin/sh
# Archive the iPhone app and upload it to App Store Connect, where TestFlight
# picks it up. Needs the app's record to exist there already (made by hand,
# once, with the same bundle id).
#
#   QUIPCALL_IOS_TEAM=<team id> deploy/ios/release.sh
#   QUIPCALL_IOS_TEAM=<team id> deploy/ios/release.sh --no-upload   # just the .ipa
#
# Signing is automatic: Xcode makes the certificate's profile for the team.
# It signs in either as the account in Xcode › Settings › Accounts, or, set
# apart from anyone's login, with an App Store Connect API key:
#
#   ASC_KEY_ID=<key id> ASC_ISSUER_ID=<issuer id>   (the key itself is
#   ~/.appstoreconnect/private_keys/AuthKey_<key id>.p8)
#
# QUIPCALL_IOS_BUNDLE changes the bundle id (default com.cubiecall.ios);
# QUIPCALL_IOS_BUILD the build number (default: the time, so each upload is
# newer than the last).
set -eu
cd "$(dirname "$0")/../.."

: "${QUIPCALL_IOS_TEAM:?set QUIPCALL_IOS_TEAM to the Apple developer team id}"
bundle=${QUIPCALL_IOS_BUNDLE:-com.cubiecall.ios}
build=${QUIPCALL_IOS_BUILD:-$(date +%Y%m%d%H%M)}
destination=upload
[ "${1:-}" = "--no-upload" ] && destination=export

set -- -allowProvisioningUpdates
if [ -n "${ASC_KEY_ID:-}" ]; then
	key="$HOME/.appstoreconnect/private_keys/AuthKey_$ASC_KEY_ID.p8"
	[ -f "$key" ] || { echo "release: no key at $key" >&2; exit 1; }
	: "${ASC_ISSUER_ID:?set ASC_ISSUER_ID (App Store Connect › Users and Access › Integrations)}"
	set -- "$@" -authenticationKeyPath "$key" -authenticationKeyID "$ASC_KEY_ID" \
		-authenticationKeyIssuerID "$ASC_ISSUER_ID"
fi

out=target/ios
archive=$out/Cubie.xcarchive
rm -rf "$archive" "$out/export"

echo "release: Cubie for iOS, build $build, $bundle, team $QUIPCALL_IOS_TEAM"
# The "Rust core" build phase builds the engine for the phone in release mode.
xcodebuild -project apps/ios/Cubie.xcodeproj -scheme Cubie -configuration Release \
	-destination 'generic/platform=iOS' -derivedDataPath "$out" -archivePath "$archive" \
	DEVELOPMENT_TEAM="$QUIPCALL_IOS_TEAM" PRODUCT_BUNDLE_IDENTIFIER="$bundle" \
	CURRENT_PROJECT_VERSION="$build" "$@" archive |
	grep -E "error:|warning: .*(sign|provision)|\*\* ARCHIVE" || true
[ -d "$archive" ] || { echo "release: the archive failed (run the xcodebuild line by hand to see why)" >&2; exit 1; }

options=$out/ExportOptions.plist
cat >"$options" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key>
	<string>app-store-connect</string>
	<key>destination</key>
	<string>$destination</string>
	<key>teamID</key>
	<string>$QUIPCALL_IOS_TEAM</string>
	<key>signingStyle</key>
	<string>automatic</string>
	<key>uploadSymbols</key>
	<true/>
	<key>manageAppVersionAndBuildNumber</key>
	<false/>
</dict>
</plist>
PLIST
xcodebuild -exportArchive -archivePath "$archive" -exportOptionsPlist "$options" \
	-exportPath "$out/export" "$@" | grep -E "error|Upload|EXPORT|\.ipa" || true

app="$archive/Products/Applications/Cubie.app"
echo "release: the app is $(du -sh "$app" | cut -f1) on the phone ($(du -h "$app/Cubie" | cut -f1) of it the binary)"
if [ "$destination" = upload ]; then
	echo "release: uploaded. It shows in TestFlight once Apple has processed it (a few minutes to an hour)."
else
	ls -lh "$out/export"/*.ipa
fi
