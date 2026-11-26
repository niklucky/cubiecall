# Cubie

Small, fast video and audio calls: a native Rust desktop app (macOS first, built with GPUI; unsigned Windows test builds too), an iPhone app on the same engine, and a Svelte web client, on a self-hosted LiveKit server.

The invite-only alpha uses https://cubiecall.com. The domains `cubycall.com` and `quipcall.com` are aliases. Get the Mac app at https://cubiecall.com/download/.

## Layout

| Path | What |
|---|---|
| `apps/desktop` | Mac and Windows app (GPUI). Home, a floating capsule of faces, a call window and a share picker |
| `apps/ios` | iPhone app (Swift, UIKit): sign-in, rooms, calls. Early: see the mobile stages in PLAN.md |
| `apps/server` | Axum + SQLite: accounts, invitations, room access and LiveKit tokens |
| `apps/web` | Svelte 5 web client |
| `crates/core` | Media and call logic, no UI code (LiveKit SDK, capture, video, recording) |
| `crates/mobile` | The engine for phones: accounts, rooms and calls behind UniFFI (Swift now, Kotlin later) |
| `crates/stt` | Speech to text on this Mac: Silero VAD and Parakeet or Qwen3-ASR through transcribe.cpp |
| `crates/protocol` | Types shared by the server and the clients |
| `deploy/` | VPS config (nginx, systemd, LiveKit) and Mac packaging |
| `vendor/gpui-pre-macos` | GPUI's macOS backend, patched (see `vendor/README.md`) |
| `deploy/windows` | Packaging and publishing the unsigned Windows zip |

## Run locally

You need Rust, Xcode, Node 22+ with pnpm, and `brew install livekit livekit-cli cmake` (cmake builds transcribe.cpp, the speech-to-text engine).

```sh
make dev               # LiveKit + server on :8080
make web               # http://127.0.0.1:5173 (run `pnpm install` in apps/web first)
make desktop           # sign in with your email and a code; credentials stay in Keychain
make check             # clippy + svelte-check
```

The iPhone app needs `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`. `make ios-sim` builds it for the Simulator; or run `make ios-core` once and open `apps/ios/Cubie.xcodeproj` (set your team under Signing to run on a phone). It talks to cubiecall.com unless launched with `-server <api> -web <site>`.

For local sign-in use `admin@example.test`; type the code from `dev/data/mail` (no email is sent). Create rooms while signed in. Auth/deployment details: [DEPLOY.md](docs/DEPLOY.md).

To add a low-level LiveKit test participant (bypasses application auth): `lk room join --url ws://127.0.0.1:7880 --api-key cubie-dev --api-secret cubie-dev-secret-0123456789abcdef --identity bot --publish-demo spike`

## Ship

- A push to `main` deploys the server and the web client (GitHub Actions); `make deploy` does the same from this Mac, plus the VPS config.
- Push a tag such as `v0.1.1` to build, sign, notarize and publish both Mac apps through GitHub Actions. A manual **Release Mac app** run builds signed downloads without publishing.
- `make release-mac` remains available for a local recovery release; set `CUBIE_BUILD` higher than the published build number.
- The same tag builds and publishes the unsigned Windows zip (**Release Windows app**). A pull request that touches the app builds it too, as a workflow artifact to try first.
- Details are in [docs/DEPLOY.md](docs/DEPLOY.md).

## Docs

- [PLAN.md](docs/PLAN.md): stages and progress
- [ARCHITECTURE.md](docs/ARCHITECTURE.md): components and decisions
- [UI.md](docs/UI.md): the design of the call UI
- [FOOTPRINT.md](docs/FOOTPRINT.md): CPU and memory measurements

The app writes its log to `~/Library/Logs/Cubie/cubie.log` when launched from Finder.
