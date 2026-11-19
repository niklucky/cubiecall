#!/bin/bash
# GitHub-hosted runners only. Never print secrets or upload the temporary keychain.
set -euo pipefail
umask 077
: "${RUNNER_TEMP:?}" "${GITHUB_ENV:?}"
: "${MACOS_CERTIFICATE:?}" "${MACOS_CERTIFICATE_PASSWORD:?}"
: "${APPLE_ID:?}" "${APPLE_NOTARY_PASSWORD:?}" "${SPARKLE_PRIVATE_KEY:?}"
keychain="$RUNNER_TEMP/cubie-signing.keychain-db"
certificate="$RUNNER_TEMP/cubie-signing.p12"
sparkle_key="$RUNNER_TEMP/cubie-sparkle-key"
keychain_password=$(openssl rand -hex 32)
echo "::add-mask::$keychain_password"
# Record paths first so always() cleanup also handles import errors.
{
    printf 'CUBIE_SIGNING_KEYCHAIN=%s\n' "$keychain"
    printf 'CUBIE_NOTARY_KEYCHAIN=%s\n' "$keychain"
    printf 'CUBIE_SPARKLE_KEY_FILE=%s\n' "$sparkle_key"
} >> "$GITHUB_ENV"
printf '%s' "$MACOS_CERTIFICATE" | base64 --decode > "$certificate"
printf '%s' "$SPARKLE_PRIVATE_KEY" > "$sparkle_key"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$certificate" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain"
security default-keychain -d user -s "$keychain"
sparkle=$(deploy/mac/sparkle.sh)
"$sparkle/bin/generate_keys" --account com.quipcall.desktop -f "$sparkle_key"
# notarytool's noninteractive CI mode accepts the password as an argument;
# GitHub masks it in logs and the temporary runner is isolated. Never enable tracing.
xcrun notarytool store-credentials cubie-notary --keychain "$keychain" \
    --apple-id "$APPLE_ID" --team-id 8BSJZG6B89 --password "$APPLE_NOTARY_PASSWORD"
rm -f "$certificate"
