# Cubie: notes for agents

Read README.md first. Plan: docs/PLAN.md. Decisions: docs/ARCHITECTURE.md.

## Priorities
- **Footprint first.** Keep binaries small and CPU and memory low. Don't add a dependency without a reason.
- **UI is minimal and custom.** It's built on GPUI with no widget library. Less on screen is better (docs/UI.md).
- `crates/core` contains no UI code. Media logic goes there.
- Phones: `apps/ios` (Swift, UIKit, no packages) uses the system's components and colours (no custom controls or fixed colours outside the call's video tiles, faces and the room chat's bubbles) and is views only; what it shows comes from `crates/mobile`, which wraps `crates/core` without its `denoise` and `transcribe` features. The mobile budget in PLAN.md is a floor: raise a limit only when a real call hits it.

## Conventions
- In PLAN.md, finished tasks are marked `[x] ~~task~~` and finished stages `## ~~Stage N~~ ✅`. Update it as work lands.
- Run `make check` (clippy with `-D warnings`, plus svelte-check) and `cargo fmt --all` before committing.
- Cargo needs `vendor/webrtc-sys` (crates.io source + `vendor/webrtc-sys.patch`, gitignored). `make` targets create it; in a fresh checkout run `dev/vendor-webrtc-sys.sh` before plain `cargo`.
- Commit locally only. Push only when asked.

## Never
- Kill the user's `make dev` or `make web` (ports 7880, 8080 and 5173). Run your own stack with `source dev/isolated-env.sh` from the repository root. It allocates fresh database/outbox/native state, matches the browser Origin to Vite 5273, and configures LiveKit 7980 and server 8180. Do not source `dev/dev.env` or use the ordinary `make dev` for isolated tests. Launch commands are in docs/DEPLOY.md. Point the app at it with both `--server "$CUBIE_API"` and `--web "$CUBIE_PUBLIC_URL"`.
- Capture the user's screen. T3 Code has Screen Recording permission, so ScreenCaptureKit sees real windows. Don't open the share picker and don't share a display or another app's window. Use the `share-own` script action and read only numbers and logs.
- Turn on the real camera without asking. Use `CUBIE_FAKE_CAMERA=1 --camera` instead.
- Touch other projects on the VPS (`root@testron.dev`), which is shared. Run `nginx -t` before every reload.

## Testing the desktop app
- `CUBIE_DATA_DIR=/tmp/cubie-data` keeps the user's rooms and history out of tests.
- `CUBIE_NO_MIC=1` starts the app without opening an audio device (and mutes Home's player). Put test variables on the command itself or `export` them in a bash script: zsh doesn't word-split `$VARS`, so `env $COMMON app` silently drops them, and an instance without `CUBIE_NO_MIC` opens the real microphone. Check with `ps eww -p <pid>`.
- `--features snapshot` with `CUBIE_SNAPSHOT_DIR` saves a PNG of every window. The AppKit capsule clip and blur don't show in it, and a window hidden behind others stops drawing, so its PNG keeps an old frame. `CUBIE_SCRIPT` actions `hover`, `unhover`, `stack`, `top`, `panel` and `people` (and `CUBIE_PANEL=transcript|people`) reach the new UI states. The widget's states: `column-left`, `column-right`, `lying`, `lying-top`, `tuck-left`, `tuck-bottom`, `untuck`, `ghost` / `unghost` (the drop outline), `hide` / `unhide` (the first camera's bubble), `big` and `float` (your bubble), `hold` / `release`, `nudge`, `bubble-each`, `no-caption`, `context` / `uncontext` (the right-click menu, which opens on the real screen), `settings-call`. Bubbles are windows of their own, so each gets its own PNG; where they are is in the log (`place window …`). The display must be awake: a sleeping one draws nothing, and the script's steps then change nothing you can see.
- `CUBIE_SCRIPT="4:regular 8:leave"` presses buttons on a timer (`src/snapshot.rs`), in whole seconds. Building it in zsh, write `"${t}:leave"`: `$t:leave` applies zsh's `:l` modifier and the step silently never runs.
- `CUBIE_FAKE_SCREEN=1` (or `=1600x900`) with the `share-own` script action shares a test pattern instead of anything on screen. Two app instances can share at once this way.
- Without a terminal the app logs to `~/Library/Logs/Cubie/`, which is the user's log. Run test instances under `script -q -t 0 <logfile> <app> … < /dev/null` to keep them out of it: without `-t 0` the last 30 s of the log are lost when the instance is killed, which looks like a hang.
- Test guests in any room that lets guests in, production included: `node dev/guests.mjs join <room link> [Name ...]` starts a headless Chrome per guest with a made-up camera and a recorded voice (they take turns speaking, ten seconds each; their own sound output is muted). `mic <Name>`, `camera <Name>`, `status`, `leave`. It presses the join page's own buttons, so it needs no dev-only autojoin.
- Headless web participant: Chrome with `--headless=new --use-fake-device-for-media-stream --use-fake-ui-for-media-stream`, opening `?room=<created-room-id>&autojoin=1&mic=0`. Autojoin exists only in Vite development mode on localhost, never in production. First authenticate against the isolated API using its local mail outbox and create a public test room; unknown room IDs are rejected. The headless browser uses the ordinary guest/session admission flow (private rooms require an authenticated invited browser). `mic=0` keeps audio off; omit it only with fake media configured. The room is `window.cubieRoom`, and its stats can be read over the DevTools protocol. To make it speak, add `--use-file-for-fake-audio-capture=<16-bit WAV>` (`say -o x.wav --file-format=WAVE --data-format=LEI16@48000 "…"`) and `--no-sandbox`, or Chrome can't read the file and sends silence. It sends silence too for a path with `..` in it. Check with the track's `getRTCStatsReport()`: `media-source` should show `totalAudioEnergy` above 0. Editing the web app while it runs hot-reloads the page, which joins again without leaving: restart Chrome after.
- Settings › Try it in a test instance speaks `CUBIE_FAKE_MIC` (a 48 kHz float WAV) instead of the microphone.
- Transcription: `CUBIE_MODELS_DIR=<dir>` points at models already downloaded (named as in `crates/stt/src/catalog.rs`), so tests don't fetch 740 MB. `CUBIE_STT_MODELS=<dir> cargo test -p cubie-stt -- --ignored` runs two `say` voices through the real model.
- Noise suppression: `CUBIE_DENOISE=webrtc` (WebRTC's filter) or `=off` instead of DeepFilterNet, and `CUBIE_DENOISE_LIMIT=15/30` (dB while talking / in pauses, or `none`) for a stronger model, for A/B. `python3 dev/denoise-eval.py` runs real voices and noise through a call's audio processing and scores every chain; listen in `target/denoise-eval/out/`.
- Measure memory with `dev/measure.sh <pid>`, which reports the physical footprint, not RSS. RSS once hid a 3 GB IOSurface leak.

## Testing the iOS app
- `make ios-sim` builds for the Simulator (Rust core, Swift bindings, then Xcode). Don't pass `CODE_SIGNING_ALLOWED=NO`: an unsigned build has no Keychain, and signing in fails with "A required entitlement isn't present".
- Launch arguments: `-server "$CUBIE_API" -web "$CUBIE_PUBLIC_URL"` point it at the isolated stack (the Simulator shares the Mac's localhost); `-room <id>` joins at launch. Debug builds only: `-session <token>` (mint one with `curl`: `/api/auth/desktop/start`, approve it with a signed-in browser cookie, then `/poll`) and `-log <file>` for the engine's log.
- In the Simulator the app sets `CUBIE_NO_MIC` and `CUBIE_FAKE_CAMERA` itself: its microphone is the Mac's real one, and it has no camera. Never pass `-mic YES` without asking.
- Signing in for real: the app opens Safari and shows a code (also copied) to type on the page. Leave the Simulator's Safari signed in to the account the user should get (open the magic link from the isolated outbox in it), or the page offers a guest identity instead. Without the page: `curl` `/api/auth/desktop/approve` with the id from the isolated database's `desktop_logins` and the code from the screen.
- In a Svelte attribute `{8}` is an expression: write a regex quantifier as `pattern={'[a-f0-9]{8}'}`, or the rule silently becomes `[a-f0-9]8` and the field rejects everything.
- The app left in the user's Device panel keeps talking to the server it was launched with. If that's your isolated stack, leave the stack running when you hand back (and say so), or the user's next tap fails with "Can't reach Cubie". To bring a stopped stack back with its rooms and sessions, start the three processes again on the same `CUBIE_TEST_DIR` rather than sourcing `dev/isolated-env.sh` afresh (it makes a new, empty one).
- Its process is a Mac process: `dev/measure.sh $(pgrep -f "CoreSimulator/Devices/.*/Cubie.app/Cubie")`. Measure release builds.
- The Simulator has six sample contacts (Kate Bell, kate-bell@mac.com, and others). Give one an account on the isolated server (an admin's `/api/auth/invites`, then the magic link from the outbox) to see "On Cubie". It can't send texts, so Invite falls back to the share sheet. The system's contacts prompt has a second page: press "Share All 6 Contacts" by its coordinates.
- People to talk to: `node dev/guests.mjs join "$CUBIE_PUBLIC_URL/?room=<public room id>" Anna Max`.
- To fill a call's history, script a guest's page over DevTools (`http://127.0.0.1:9411/json` for the first guest, `window.cubieRoom` in development): `cubieRoom.localParticipant.setAttributes({"quipcall.recording":"audio"})`, a `"quipcall.transcribing"` claim (`"<model>@<unix ms>"`) and then `publishData` of `{"type":"lines","lines":[…]}` on the topic `quipcall.transcript`, and a canvas `captureStream()` track published with `source: "screen_share"` for a share that captures no screen.
- After changing `crates/mobile`'s public items, `dev/ios-core.sh` rewrites `apps/ios/Cubie/Generated/cubie_mobile.swift`; names that are Swift keywords (`private`, `public`) come out in backticks, so avoid them.

## macOS gotchas
- Ask for mic and camera access with AVCaptureDevice before WebRTC touches them. A prompt raised by WebRTC itself blocks its audio thread and freezes the call.
- Any thread that handles libwebrtc Objective-C frames needs an autorelease pool per frame (`autorelease_pool`).
- Never change a window's style mask at runtime: it crashes Macs with a Touch Bar. Floating windows are created borderless through the patched GPUI in `vendor/`.
- Never change an NSView's class at runtime (`object_setClass`, AccessKit's `SubclassingAdapter`). AppKit's Touch Bar observes the responder chain with KVO and can throw when removing its observer after a class change. Use a fixed view class and AccessKit's plain adapter.
- Defer AppKit frame changes with a `foreground_executor` spawn. Defer opening a window from inside an entity update with `cx.defer`, or GPUI panics.
- Keep the camera on H.264 with zero-copy rendering: it uses about 40% less CPU than VP8.
- Mac builds need `MACOSX_DEPLOYMENT_TARGET=12.3`. Below that, the Swift libraries link via `@rpath` and the app won't start. Dev builds are ad-hoc signed, so macOS asks for permissions again after every build. Mac releases keep permissions by reusing the shipping certificate in `~/Library/Keychains/quipcall-signing.keychain-db`. Preserve it when moving the build to another Mac. Fresh signing identities use `cubie-signing.keychain-db`.
- GPUI compiles its shaders at runtime (the `runtime_shaders` feature), so no Metal toolchain is needed. `apps/desktop/build.rs` adds `-ObjC`; without it, libwebrtc's Objective-C categories are dropped.
- This Mac's network runs a fake-IP proxy, so local `dig` gives wrong answers. Resolve names from the VPS instead.
