# Local development tasks. `make dev` runs LiveKit + the server; run `make web` / `make desktop` in other terminals.
ifneq ($(filter dev livekit server web desktop,$(MAKECMDGOALS)),)
include dev/dev.env
endif
export

.PHONY: dev livekit server web desktop check fmt deploy mac-app release-mac vendor ios-core ios-sim release-ios

# Cargo needs vendor/webrtc-sys (crates.io source + our patch) before any build.
vendor:
	@dev/vendor-webrtc-sys.sh

dev:
	$(MAKE) -j2 livekit server

livekit:
	livekit-server --config dev/livekit.yaml

server: vendor
	cargo run -p cubie-server

web:
	cd apps/web && pnpm dev

# Opens home. `make desktop ROOM=spike` joins a room straight away.
desktop: vendor
	cargo run -p cubie-desktop -- $(if $(ROOM),--room $(ROOM))

# The iOS app's Rust core and its Swift bindings, for the Simulator
# (`dev/ios-core.sh device release` for an iPhone). Xcode runs this itself on
# every build; run it once by hand before opening apps/ios/Cubie.xcodeproj
# in a fresh checkout, so the generated Swift is there when Xcode looks.
ios-core: vendor
	dev/ios-core.sh sim

# The iOS app for the Simulator → target/ios/Build/Products/Debug-iphonesimulator/Cubie.app
ios-sim: ios-core
	cd apps/ios && xcodebuild -project Cubie.xcodeproj -scheme Cubie -configuration Debug \
		-sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
		-derivedDataPath ../../target/ios ARCHS=arm64 build | grep -E "error:|warning: |\*\* BUILD" || true

# Archive the iPhone app and upload it for TestFlight (deploy/ios/release.sh):
# `make release-ios CUBIE_IOS_TEAM=<team id>`.
release-ios: vendor
	deploy/ios/release.sh

check: vendor
	cargo clippy --workspace --all-targets -- -D warnings
	cd apps/web && pnpm check

fmt: vendor
	cargo fmt --all

# The former unauthenticated GET /api/join is intentionally gone.

# Server, web client and VPS config for the configured deployment.
deploy: vendor
	deploy/deploy.sh

# The Mac app (Apple Silicon + Intel, Developer ID signed and notarized) → target/mac/Cubie-*.zip.
mac-app: vendor
	deploy/mac/package.sh

# Build the Mac app and publish it on the configured site.
release-mac: vendor
	deploy/mac/package.sh --upload
