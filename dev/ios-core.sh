#!/bin/sh
# Build the Rust core for the iOS app and write its Swift bindings.
#
#   dev/ios-core.sh [sim|device] [debug|release]
#
# Xcode's "Rust core" build phase runs it with no arguments: the platform and
# configuration then come from the build (PLATFORM_NAME, CONFIGURATION).
# Out: apps/ios/Core/libcubie_mobile.a and its C header, and
# apps/ios/Cubie/Generated/cubie_mobile.swift.
set -eu
cd "$(dirname "$0")/.."

platform=${1:-${PLATFORM_NAME:-iphonesimulator}}
config=${2:-${CONFIGURATION:-Debug}}

# Xcode's environment points every compiler at the iOS SDK, which breaks the
# parts of the build that run on this Mac (build scripts, proc macros). Start
# again with a clean one.
if [ -n "${XCODE_VERSION_ACTUAL:-}" ] && [ -z "${CUBIE_CLEAN_ENV:-}" ]; then
	exec env -i HOME="$HOME" USER="${USER:-}" CUBIE_CLEAN_ENV=1 \
		PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
		"$0" "$platform" "$config"
fi
PATH="$HOME/.cargo/bin:$PATH"

case "$platform" in
sim | iphonesimulator) target=aarch64-apple-ios-sim ;;
device | iphoneos) target=aarch64-apple-ios ;;
*) echo "ios-core: unknown platform '$platform' (sim or device)" >&2; exit 2 ;;
esac
case "$config" in
[Rr]elease) profile=release; flag=--release ;;
*) profile=debug; flag= ;;
esac

if ! rustup target list --installed | grep -qx "$target"; then
	echo "ios-core: run 'rustup target add aarch64-apple-ios aarch64-apple-ios-sim' first" >&2
	exit 1
fi

dev/vendor-webrtc-sys.sh

# The same floor as the app (apps/ios/Cubie.xcodeproj), for the C++ in the tree.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
# shellcheck disable=SC2086
cargo build -p cubie-mobile --target "$target" $flag

lib=target/$target/$profile/libcubie_mobile.a
out=apps/ios/Core
generated=apps/ios/Cubie/Generated
tmp=$out/.bindings
mkdir -p "$out/include" "$generated"
rm -rf "$tmp"
cargo run -q -p cubie-bindgen -- generate --library "$lib" --language swift --out-dir "$tmp"

# Only touch what changed, so Xcode recompiles no more than it must.
put() {
	cmp -s "$1" "$2" || cp "$1" "$2"
}
put "$tmp/cubie_mobile.swift" "$generated/cubie_mobile.swift"
put "$tmp/cubie_mobileFFI.h" "$out/include/cubie_mobileFFI.h"
put "$tmp/cubie_mobileFFI.modulemap" "$out/include/module.modulemap"
put "$lib" "$out/libcubie_mobile.a"
# Xcode compiles every Swift file in Generated, including pre-rename bindings.
rm -f "$generated/quipcall_mobile.swift"
rm -rf "$tmp"
echo "ios-core: $lib ($(du -h "$lib" | cut -f1)), bindings in $generated"
