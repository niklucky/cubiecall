#!/bin/sh
# Notarize a signed app and create the distributable ZIP only after verification.
# Logs and Apple's submission ID remain beside the ZIP for diagnosis.
set -eu
app=$1
zip=$2
profile=$3
submission=${zip%.zip}-notary.json
log=${zip%.zip}-notary-log.json
upload=${zip%.zip}-submission.zip
set -- --keychain-profile "$profile"
if [ -n "${CUBIE_NOTARY_KEYCHAIN:-}" ]; then
    set -- "$@" --keychain "$CUBIE_NOTARY_KEYCHAIN"
fi

ditto -c -k --keepParent "$app" "$upload"
submitted=0
if xcrun notarytool submit "$upload" "$@" \
    --wait --timeout 30m --output-format json >"$submission"; then
    submitted=1
fi
cat "$submission"
# Fetch the full log on both Accepted and Invalid results.
if ! id=$(plutil -extract id raw -o - "$submission"); then
    echo "No notarization submission ID. See $submission." >&2
    exit 1
fi
xcrun notarytool log "$id" "$@" "$log"
if [ "$submitted" != 1 ] ||
    [ "$(plutil -extract status raw -o - "$submission")" != Accepted ]; then
    echo "Notarization failed or is still pending. See $submission and $log." >&2
    exit 1
fi
xcrun stapler staple "$app"
xcrun stapler validate "$app"
# Require notarization explicitly even on a build Mac with Gatekeeper disabled.
codesign --verify --deep --strict --check-notarization -R=notarized "$app"
spctl --assess --type execute --verbose=2 "$app"
# ZIPs cannot be stapled. Repack the app with its ticket before Sparkle signs it.
ditto -c -k --keepParent "$app" "$zip"
rm "$upload"
