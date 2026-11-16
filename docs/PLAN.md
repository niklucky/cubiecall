# Quipcall — Implementation Plan

How to read this file:

- A finished task gets `[x]` and ~~strikethrough~~.
- A finished stage gets its heading struck through and a ✅.
- Each stage ends with **Done when**: the checks that have to pass before we cross it out.
- Decisions and the reasoning behind them live in [ARCHITECTURE.md](ARCHITECTURE.md).

**Current focus:** Stage 2 — Call UI (Stage 1 waits on a manual test of camera, speaker and screen share). An early alpha (Stage 7, partly) runs at https://quipcall.com so friends can test calls. The phone apps have their own stages, [Mobile](#mobile-ios-first-then-android) below: iOS first (M0, the media spike, is built and waits on a real iPhone), then Android.

---

## Footprint budget

These are targets. We measure them at the end of Stage 1 and at every milestone, and record the results in `docs/FOOTPRINT.md`.

| Metric | Target |
|---|---|
| macOS app bundle (without speech-to-text models) | ≤ 60 MB per architecture *(40 MB until 2026-11-10: quality over a few megabytes; the Apple Silicon app is 53.6 MB with transcription)* |
| Idle in the menu bar, logged in | ≤ 60 MB RAM, ~0% CPU |
| 1:1 call, 720p camera both ways | ≤ 200 MB RAM, ≤ 15% CPU on an M-series Mac |
| 10-person grid call | ≤ 350 MB RAM |
| Screen share, 1080p at 30 fps (sending) | ≤ 20% CPU |

---

## ~~Stage 0 — Foundations~~ ✅

- [x] ~~Git repo, Cargo workspace, `.gitignore`, README~~
- [x] ~~`docs/PLAN.md` and `docs/ARCHITECTURE.md`~~
- [x] ~~Local LiveKit dev server config (`dev/livekit.yaml`) and Makefile tasks~~
- [x] ~~`apps/server` skeleton: Axum, `/health`, and a dev-only LiveKit token endpoint~~
- [x] ~~`crates/protocol` with the first shared types~~

**Done when:** `make dev` starts LiveKit and the server, `cargo build --workspace` passes, and `curl` returns a token.

## Stage 1 — Media spike (riskiest part first)

- [x] ~~`crates/core`: room connection, events, and subscribing to remote tracks, with no UI code (own tokio runtime)~~
- [x] ~~Desktop: GPUI window that connects to a room with a dev token~~
- [x] ~~Desktop: render remote video. I420 → full-range NV12 `CVPixelBuffer` → GPUI `surface`~~
- [ ] Desktop: microphone and speaker via `PlatformAudio`, with echo cancellation, noise suppression and auto gain from WebRTC. *Mic → web verified (audio arrives with signal, no concealment). Speaker playback still needs a human ear.*
- [ ] Desktop: neural noise suppression (DeepFilterNet 3) after WebRTC's echo cancellation, then AGC2, through a `webrtc-sys` hook (`vendor/README.md`). *First real call: voice dipped; the model was muting quiet voices (fixed with a 12 dB input gain and a 12 dB limit). Second call: better, hard rustling still faintly audible, so pauses now allow 30 dB ([FOOTPRINT.md](FOOTPRINT.md)). Needs a third listening check, and a look at double talk on laptop speakers.*
- [x] ~~Desktop: screen share via ScreenCaptureKit, with frame-rate presets 5/15/30/60 and a text/motion mode~~ *Verified by sharing one of Quipcall's own windows: the web client receives 2200×1440 at 15 fps (text) and 1920×1256 at 30 fps (motion), 0 dropped frames.*
- [x] ~~Desktop: ask for mic and camera permission up front (AVCaptureDevice `requestAccess`) before opening the audio device. Otherwise a pending prompt blocks WebRTC's audio thread and freezes the call (found in testing).~~ *The header shows "Waiting for … access" while the prompt is open; if the mic is denied, the call continues listen-only.*
- [x] ~~Desktop: pick the simulcast layer per tile from its on-screen size (the Rust SDK's adaptive stream doesn't report sizes)~~
- [ ] Desktop: camera capture and publish (AVFoundation → full-range NV12 `CVPixelBuffer` → VideoToolbox H.264, simulcast 720p/360p/180p). *Publish path verified with the synthetic camera (`QUIPCALL_FAKE_CAMERA=1`): the web client receives 720p30 H.264 with 0 dropped frames. The AVFoundation capture itself still needs a first run by a person (camera permission prompt).*
- [ ] Desktop: local self-view for the camera and for your screen share, drawn straight from the capture buffers. *Implemented; the camera isn't mirrored yet. The share preview needed no conversion after all: ScreenCaptureKit already delivers full-range NV12 IOSurfaces.*
- [x] ~~Web: minimal Svelte page that joins the same room with camera, mic, and screen share (with frame-rate picker)~~
- [x] ~~Zero-copy receive path for hardware-decoded frames~~ *VideoToolbox already outputs full-range NV12, so the decoded `CVPixelBuffer` goes to GPUI as is. Receiving 720p: ~7.5% CPU with H.264 vs ~12.5% with VP8. The web client now publishes H.264 by default.*
- [x] ~~Direct ScreenCaptureKit capture (`CVPixelBuffer` → `NativeBuffer` → VideoToolbox)~~ *Replaced libwebrtc's capturer on macOS; needed for our own picker anyway. Text mode moved from VP9 to H.264: 460 → 173 MB, 24 → 11% CPU.*
- [x] ~~Fix: every video frame leaked an IOSurface (libwebrtc's autoreleased Objective-C wrappers on threads without an autorelease pool)~~ *Found while measuring.*
- [ ] Measure the footprint and write `docs/FOOTPRINT.md`. *Recorded (as physical footprint): receive only ~170 MB / 6%, 1:1 both ways ~189 MB / 9%, 10-person grid ~199 MB / 15%, all within budget. Screen share (sending, plus receiving one 720p camera): 15 fps text at 2200 px ~173 MB / ~11%, 30 fps motion at 1920 px ~188 MB / ~13%.*
- [ ] Find out why the mic track sometimes shows as muted on the server right after joining. *Seen twice, not reproduced in 10 scripted runs. Mic toggles and mute changes are logged now.*
- [ ] GPUI go/no-go. *Recommendation: go (see the notes below the stage).*

**Done when:** a 1:1 call between desktop and web works locally with audio, video, and screen share at a chosen frame rate. Footprint numbers are recorded, and we've made a go/no-go call on GPUI.

**GPUI so far.** For: video frames are drawn straight from `CVPixelBuffer`s with no copy, the whole call stays within the CPU budget, the binary is 25 MB, and Zed proves it on Windows and Linux. Against: about 90 MB of the ~170 MB footprint is GPU memory held by the window, `gpui-pre` is a snapshot of an unstable API (pinned to an exact version), and some basics are missing (no transforms, so no mirrored self-view yet; no widget library, so we build our own controls). None of these is a blocker, and the alternatives (Tauri/web view, Slint, egui) would need their own zero-copy video path.

## Stage 2 — Call UI (desktop): home, minimal and regular modes

Design: [UI.md](UI.md).

- [x] ~~Design: home screen, minimal mode (floating circles + glass toolbar), regular mode~~
- [x] ~~Design round 2: clickable prototype of every screen ([prototype/ui.html](prototype/ui.html)): onboarding, two-pane home, a finished call, regular and minimal modes~~ *Approved, with one change: while you share, your face in the capsule turns into the preview.*
- [x] ~~App structure: one call model shared by several windows; each video view repaints only itself~~
- [ ] Minimal mode as one glass capsule (replaces the window per circle and the toolbar): faces with camera or avatar, speaking ring, muted badge, mic button on your face; mic and Leave always shown, time, camera, share, link and expand on hover; your face becomes your share preview; "+N" past 6 people with the speaker always shown; a column at the screen's left or right edge; scroll or pinch to resize faces (32–96 px). *Built; checked with snapshots (faces with video, share preview, "+N", column). Hover, dragging, docking and resizing need a look on a real screen.*
- [ ] Floating circle per person. *Replaced by the capsule above (the code is gone).*
- [x] ~~Design round 4 for the minimal widget ([prototype/call-widget.html](prototype/call-widget.html)), after live testing found the capsule unusable: the row shows the 3 people active last plus you, the muted and the quiet fold into a stack, cameras become tiles above the row, hover rows open underneath instead of controls sliding out, your share preview grows on top, sharers get a badge~~
- [ ] Minimal widget, round 4. *Built; checked with snapshots (tiles, stack and its row, controls row, your share preview, the flip near the screen's bottom). First live test: hover rows never closed (GPUI reports no hover change once the cursor leaves a window; the cursor is now polled while a row is open), muting shifted the row (you now stay in it with a badge, and the stack keeps one width), and the widget jumped as rows changed (content now stays where the window is until a resize lands, and the window grows from its left end). Second live test: still jumped when sharing (two resizes in a row confused that bookkeeping; the main row's place is now set right before each frame change, and the preview waits for its first frame), the stack showed grey placeholders (now real faces, dimmed), and screen recordings showed nothing (floating windows were hidden from all capture; now only our own shares leave them out). Third live test (a recording, alone in a call): empty space left of your face (the widget had a 240 px floor; it now fits its row, and the hover rows fit in that, so hovering never changes the width), the widget skewed on every hover (the frame AppKit asks for during a resize was throttled, since GPUI caps windows that aren't key at 30 fps: on a 1× display all 24 of 24 resize frames were skipped in a scripted run, and the old frame showed stretched; patched in GPUI, now 25 of 25 draw), once for seconds (not reproduced; frames drawn into a buffer of the old size are now skipped too), a black ring round the mute badge, and pale glass over white pages (the tint went from 38% to 78%). Code review: alone with your camera on the widget was narrower than the hover row (it now never gets narrower than that row, which sits right-aligned under the buttons so a change of width moves nothing in it), and near the top of the screen a tile or your preview pushed the whole widget down (they now open below the main row there; checked with a scripted run at the top of the screen: the window's top and right edge stayed put through a share and a hover).*
- [x] ~~Resize knob on hover: a dark knob with arrows on the circle's rim at 45° (the window mask grows to include it while you hover); screen shares get it in the corner~~
- [x] ~~Share picker: a window with live thumbnails of screens and windows, frame-rate presets, Share / Cancel (Esc, Enter, double-click)~~ *Explains the Screen Recording permission when it's off.*
- [x] ~~Fix: the expand button quit the app~~ *Switching modes closed every window before opening the new one, and the app quit when no window was left. Windows now close a tick later, and the app only quits when there's no call and no home window.*
- [x] ~~Glass toolbar: Lucide icons; mic, camera, screen share with fps presets, copy link, invite, expand, leave; draggable~~ *Folded into the capsule.*
- [ ] Minimal widget, round 5 ([prototype](prototype/call-widget-5.html)). *Built: a solid pill as long as what's in it, nothing in it changes under the cursor, a ⋯ button and cards that open on a click (menu, the stack's people, recording, transcript), names on hover, no stack in a call of two or three, a seat that copies the link when you're alone, Leave without the line; cameras and your share's preview as bubbles (a window each, riding on the pill, dragged away and dropped back, zoomed, a tile past 180 px), faces as video switches, one bubble that follows the speaker in calls of four or more; the pill as a column at the screen's sides and a tab in an edge, with an outline on a pause; the caption line; hold to talk (mic button and right Option); double-click for the call window, right-click for a native menu; Settings › Call. Checked by script with one, three and six people (window frames from the log, a picture of every window): rows and columns of bubbles, closing ranks, hiding, tiles, the share bubble, both video modes, turning, tucking and coming back, the outline, the caption, the reminder with its beep. By hand, in a real call with two test guests (`dev/guests.mjs`): bubbles dragged, zoomed and pressed. Reviewed by two other models; their findings are fixed (the key no longer talks while typing, drops go by the display under the cursor, a held mic is muted if the widget closes first). Still to try by hand: dragging the pill to an edge, two displays, holding the mic button and the key, the right-click menu.*
- [ ] Hear that you talk while muted. *The reminder, its beep (at most every 5 minutes) and its switch are built and can be set off by script. Nothing sets it off in a call yet: the microphone closes when you mute (macOS shows it off), so hearing you would mean keeping it open while muted. To decide.*
- [ ] Footprint of the bubbles against the one-window widget, side by side, and with ten people. *First numbers (six people, all cameras on): ~15% CPU and ~147 MB with the default two bubbles, ~165 MB with four; about 9 MB a bubble (docs/FOOTPRINT.md).*
- [ ] Other participants in the capsule, screen shares as floating rectangles. *Circles verified with 10 people; screen-share windows checked with snapshots (synthetic screens), dragging and resizing them not yet.*
- [ ] Your own share as a preview: your face in the capsule turns into a rounded rectangle of what you share (same IOSurfaces, ≤ 15 fps repaints); the big tile in regular mode when nobody else shares. *Built; checked with snapshots using the synthetic screen and camera.*
- [ ] Fullscreen for someone's share: double-click it or press ⤢ (floating window), double-click its tile (regular mode); Esc, a double-click or ⤡ goes back. Web: double-click or ⤢ on a share tile. *Built; a scripted run entered native fullscreen and came back to the floating window. Clicks and Esc need a test by hand. Web fullscreen verified in headless Chrome. Live test (build 35): a share that ended while its fullscreen Space wasn't current left a dead black Space (Dock: "trying to take pid … out of fullscreen but it is not the current space"); now the window stays, says the share ended, and closes once its Space is showing. Not yet retested live. Live test: leaving fullscreen put the floating window on another display, partly off screen (GPUI gives bounds relative to the window's own display but opened it on the primary one); it now comes back at its exact AppKit frame, also after a mode switch, or at the default spot if that display is gone.*
- [x] ~~Several people sharing at once~~ *Already worked (one share per person). Verified: two desktops sharing at the same time each receive the other's share, and a web participant sees both.*
- [x] ~~Synthetic screen for tests: `QUIPCALL_FAKE_SCREEN=1` (or `=WIDTHxHEIGHT`) shares a test pattern through the real publish path, so tests never capture the screen~~
- [ ] Regular mode: title bar with the time and Invite; layouts that pick themselves (alone with the link, grid up to 4, speaker with a strip that fits, share with faces in a column); a floating control bar; a side panel with the call's timeline and its people; switch both ways. *Built; each layout and both panel tabs checked with snapshots (synthetic camera and screen, `lk` bots). Needs a click test.*
- [ ] Home window: two panes. Rooms (avatar and "+N", last call) and history by day on the left; the selected call on the right with rename, copy link, remove, Call, ‹ › through the room's calls, and Transcript / Recordings / Files tabs (the timeline for now, empty states for the rest); quick calls get Keep as room and Call again. Opens on the call you just left. Stored locally until Stage 3. *Built; checked with snapshots on seeded data and after a scripted call. Needs a click test, renaming by hand included.*
- [ ] A call's timeline: who joined, left and started or stopped sharing, with times, saved with the call. *Built; checked after a scripted call.*
- [ ] First launch asks for your name (also behind the pencil on home); `--name` skips it. *Built; checked with a snapshot. Needs typing by hand.*
- [x] ~~Text field for naming rooms (adapt GPUI's `input.rs` example)~~ *`text_input.rs`: IME, selection, copy and paste; Enter and Esc as events.*
- [ ] Mirror your own circle
- [ ] Mic button on your own circle (also on your screen preview). *Built; checked with snapshots, needs a click.*
- [x] ~~Fix: after leaving, you stayed in the room as a ghost for 15–40 s, so rejoining showed you twice~~ *The engine's runtime was dropped with the call, cancelling the goodbye to LiveKit mid-way. It now gets a bounded grace period on the engine runtime after the call model is dropped, and quitting mid-call leaves first. Quit waits synchronously for up to three seconds before returning to GPUI, whose 200 ms async cleanup budget could cut off final transcript delivery and the goodbye. Four immediate quit/rejoin cycles, with the synthetic camera on and off, left zero participants after exit and logged `CLIENT_REQUEST_LEAVE` each time.*
- [x] ~~Fix: after a sharer quit, others still saw their screen for ~20 s~~ *The goodbye didn't reach LiveKit before the quit deadline once (server: `PEER_CONNECTION_DISCONNECTED` 21 s after the data channel died; the next quit logged `CLIENT_REQUEST_LEAVE`). Leaving now unpublishes the screen first, before final transcripts and the goodbye, and logs how long leaving took. Scripted quit: the viewer's share window closed 7 ms after the sharer quit.*
- [x] ~~Footprint with 10 circles open~~ *~207 MB, ~23% CPU with 10 circles + toolbar while sending the camera. Memory is the same as one grid window; CPU is higher because every circle draws itself.*
- [x] ~~Footprint of the capsule with 10 people~~ *~19% CPU, ~135 MB (no mic) once repaints were capped at 30 fps; ~24% before ([FOOTPRINT.md](FOOTPRINT.md)).*

**Done when:** a 3+ person call runs in minimal mode in the capsule (moved, docked, faces resized); switching to regular mode and back works; home lists rooms and history and shows a finished call; footprint with the capsule is recorded.

## Stage 3 — Backend core

- [x] ~~SQLite via sqlx, with migrations~~
- [x] ~~Accounts: invite-only registration, email magic links through Resend, session tokens~~ *Web cookies, desktop Keychain/browser approval, bootstrap admin, separate provider identities ready for future OAuth.*
- [ ] `protocol`: REST request/response types and WebSocket messages, with TypeScript types generated for web
- [ ] WebSocket hub: authenticated connection, presence (online / away / in call)
- [ ] LiveKit token minting for calls; LiveKit webhooks (joined / left / room finished) → call records
- [x] ~~Remove the unauthenticated token endpoint~~ *POST join requires an account/guest session and room access; nginx gates LiveKit signaling against current membership.*

**Done when:** integration tests for auth, token minting, and WebSocket presence pass.

## Stage 4 — Desktop app shell

- [x] ~~Login, with the session stored in the macOS Keychain~~ *Browser approval with a private polling secret; refresh rooms, switch account and sign out.*
- [ ] Menu-bar mode: runs in the background, stays connected for presence, small idle footprint. Check that GPUI's GPU memory (~90 MB with the call window open) is released when no window is open.
- [ ] `core`: call state machine with no UI code
- [ ] Contacts: add by username, see who's online
- [ ] Settings: mic, speaker, camera, default screen-share preset
- [ ] Noise suppression switch (off for music), desktop and web; the web client gets the same model in WASM. Then shrink the model's footprint: a pre-converted model to cut load-time memory, or our own inference on Accelerate instead of tract.

**Done when:** the app starts in the menu bar, logs in, shows contacts with live presence, and meets the idle budget.

## Stage 5 — 1:1 calls

- [x] ~~Guest 1-to-1 calls: authenticated creator, one guest identity, reconnects, no third participant~~
- [x] ~~Public rooms and private rooms with membership and single-use guest links granting access until removal~~ *Web and native room creation/access management: public/private choice, guest/member invitations, revoke/remove, rename/close, and in-call Invite. Registration and room invitations are independent.*

- [ ] Ringing protocol: invite → ringing → accept / decline / cancel / timeout / busy. With multiple devices, the first accept wins.
- [ ] Incoming-call window and ringtone
- [ ] Device switching (mic, speaker, camera) during a call
- [ ] Call history

**Done when:** two desktops can call each other through the backend, and every ringing state is covered by tests.

## Stage 6 — Web client

- [x] ~~Web magic-link login, room creation/joining and invitation/access management~~
- [x] ~~Focused guest join flow: name, camera preview, mic level and input selection, permission recovery, invitation/access states, mobile layout and explicit Mac identity handoff~~ *Verified against an isolated LiveKit server with synthetic tracks; preview cleanup, enabled/disabled media, return visits, live removal and browser-to-Mac identity approval checked.*
- [ ] Contacts, ringing, place and answer contact calls from the browser
- [ ] Call UI on par with desktop: grid, controls, screen share with frame-rate picker
- [ ] Mobile layout (active speaker + up to 4 tiles), tested in Chrome on Android

**Done when:** calls work web ↔ desktop, including from an Android phone.

## Stage 7 — Alpha deploy to the VPS (friends start testing here)

Started early (2026-11-03), ahead of Stages 3–6, with open room links. Invite-only auth and room policies shipped on 2026-11-11 with the web/API update and signed Mac build 54 (Apple Silicon and Intel). Production checks passed for secure guest sessions, admission rules, retired open joins and the nginx signaling gate; the Resend sender domain is verified. Signed Mac build 55 adds native room creation, invitations and access management; isolated native/API checks cover guest redemption, return, removal, invitation revocation, public admission, non-owner controls and room closure. The focused web join flow and read-only access preflight shipped on 2026-11-12; signed Mac build 56 adds the explicit browser-identity handoff. Runbook: [DEPLOY.md](DEPLOY.md).

- [x] ~~LiveKit on a single node: one UDP port (7882), TCP fallback (7881)~~ — calls from outside connect over UDP
- [ ] Built-in TURN/TLS for strict networks (only 443 open)
- [x] ~~HTTPS, systemd units, server, and static web files~~ — nginx + Let's Encrypt (the box already runs them for other projects, so not Caddy); `make deploy`
- [ ] SQLite backups
- [x] ~~macOS build: `.app` bundle, ad-hoc signing for friends~~ — one app per architecture, `curl … | sh` installer, call links (`quipcall://join/<room>`); `make release-mac`
- [x] ~~Quby visual assets: macOS bundle icon, web favicon, transparent 1024px PNG, and editable SVG~~ — wired into packaging and the web build; assets prepared locally for the next release.
- [x] ~~App icon: the cube alone on transparency, filling the icon, instead of the black tile~~ *`QubyTransparent.icns`; the black tile stays as `Quby.icns`, and `ICON` in `deploy/mac/package.sh` switches back.*
- [ ] Updates keep their permissions: releases are signed with one self-signed certificate (`deploy/mac/signing.sh`), so macOS's requirement for the app stays the same across builds. *Built; needs two releases in a row to confirm.*
- [ ] Notarization (Apple Developer ID), so a downloaded zip opens without the manual "Open"; simple update check
- [x] ~~Email-bound account invitations for friends~~
- [x] ~~Session and handoff polish: preserve native identity on room links, return room names, renew active sessions and cookies, WAL admission reads, safe task registration, compact account controls, and idempotent secrets initialization~~
- [x] ~~PR auth hardening: device-code approval, private queued email delivery, cancellable native sign-in and expired-session recovery, per-connection media identity, revoked direct slots and durable invitation return links~~
- [x] ~~Lifecycle and auth review: drain final transcript sends before goodbye with bounded fallback, handle teardown-thread creation failure, separate Mac approval from room admission, rate-limit/prune orphan guests, separate media/auth cleanup workers, and preserve nonempty bookmark names~~
- [x] ~~Further review: locked removed direct slots, consistent idle admission, action-specific service failures, transient sign-in retry, shared generation guard, missing-session recovery, permanent bookmark closure, per-engine runtime teardown, and shared permission policy~~
- [x] ~~Review follow-up: shared owner names, recoverable room hiding, fair idle cleanup, provisional session activation, deep-link sign-in restart, parallel account refresh, unified 401 recovery, and common Home dispatch~~
- [x] ~~Review follow-up: bounded durable mail retries and recipient caps, non-consuming owner invite previews, checked concurrent claims, expiring Quick calls with owner keep, removable local saved links, stable startup Home selection, atomic native session replacement, and engine-owned async task cancellation~~
- [x] ~~Native recovery and saved-room fixes: permission before admission, typed 401 recovery, session-safe room requests, persistent local bookmarks, temporary quick calls, and reusable async HTTP runtime; notify-driven mail delivery~~
- [x] ~~Fix: crash on join on Macs with a Touch Bar~~ — floating windows are created borderless (patched GPUI) instead of switching style afterwards; the accessibility content view also keeps its class when home closes so Touch Bar observers survive the transition
- [ ] Verify on the affected Intel Mac (macOS 15.6, two displays, Touch Bar): call windows stay visible when sharing and switching apps. *Fixed NSPanel's default hide-on-deactivate behavior for floating windows and popups; audio continued while the UI disappeared. Needs a manual check with the updated app.*
- [ ] Verify share picker responsiveness with many windows. *Permission checks and enumeration run off the UI thread; choices arrive before previews. Native submissions run on isolated workers with deadlines and a shared limit of four requests across picker sessions. In-flight requests keep their slot through timeouts and cancellation until the native callback completes; listing failures offer Retry.*

**Done when:** friends install the app, sign up, and call each other over the internet, including through strict NAT.

## Stage 8 — Group calls (up to ~10 people)

- [ ] Groups: create, add/remove members, owner role
- [ ] Start a group call (rings online members); join a call already in progress; show which groups are in a call
- [x] ~~Single-use guest links → durable private-room access with a display name; the owner can revoke access and disconnect guests~~ *Opaque hashed invitation tokens, atomic redemption, persistent membership and signaling admission checks.*
- [ ] Layouts: grid / speaker / screen-share focus; active speaker; pagination
- [ ] Performance: adaptive stream, dynacast, pause off-screen tiles, tuned simulcast layers

**Done when:** a 10-participant call with a mix of desktop, web, and bots stays smooth, and CPU is measured.

## Stage 9 — Transcription (single transcriber) and recording

- [x] ~~Design round 1: clickable prototype ([prototype/transcription.html](prototype/transcription.html)): Transcribe in the widget's hover row, choosing whose Mac transcribes (ask, accept, hand over, take over when the transcriber leaves), the transcript as a chat in its own floating window, the call window's side panel, and Settings › Transcription (language and model, downloads, try it with your voice)~~ *Approved as drawn: anyone in the call can stop the transcript, the window opens for everyone (a switch turns that off), one model per call, and the window's footprint gets measured.*
- [x] ~~Design round 2, same prototype: a separate Record button that saves the call's audio, or audio and video, on the recorder's Mac. Its panel, the indicator and notices for everyone, toolbar buttons in the call window, Settings › Recording, and Home after the call (the recording plays under the transcript)~~ *Decided: video means your own screen, picked as when sharing; one recording per call; only whoever started it can stop it; sharing the file with the room later.*
- [x] ~~Design round 3, same prototype: "Audio and screen" through the share picker in record mode, one recording per call (Record leaves everyone else's hover row), and the recorder leaving.~~ *Approved.*
- [x] ~~`crates/stt`: VAD (Silero), model manager (download on demand, checksum)~~ *transcribe.cpp runs Silero VAD, Parakeet v3 and Qwen3-ASR (no trait needed: one engine for every model). Downloads resume and check SHA-256 (`crates/core/src/models.rs`).*
- [x] ~~Model bake-off for our languages (accuracy and speed on Apple Silicon); record the decision~~ *Parakeet v3 and Qwen3-ASR on one paragraph in six languages (macOS voices, `crates/stt/examples/bakeoff.rs`): both perfect or one word off in English, Russian, German and French; Qwen3-ASR also gets Chinese and Japanese, Parakeet doesn't know them. Parakeet is 4× faster (160× real time on the GPU, 42× on the CPU, against 39× and 18×). Decided: Parakeet by default, Qwen3-ASR for Chinese, Japanese and Korean (Settings suggests it). Whisper, GigaAM and Nemotron would need transcribe.cpp support or another engine; not now.*
- [x] ~~Transcriber role: claim, lease, and heartbeat; capability shown via participant attributes; handoff prompt~~ *Built without the backend: LiveKit's presence is the lease (a transcriber's attribute goes when they leave). `quipcall.can_transcribe` marks Macs with a model ready; Transcribe asks "whose Mac?", the other Mac gets Accept / Not now (or accepts by itself, a Setting); hand over from the transcript window or the People tab; when the transcriber leaves, everyone who can is asked to take over. Two Macs transcribing at once: the newest carries on, the other stops. Verified with two app instances: handover, and the take-over prompt.*
- [x] ~~Pipeline: local mic (before encoding) + each remote track → 16 kHz → VAD → shared ASR queue; partial and final results~~ *Verified in a call: a web participant speaking English and Russian (macOS `say` as its microphone) came out right, with drafts about once a second.*
- [x] ~~Transcript flow to all participants; late joiners get the history~~ *LiveKit data messages from the transcriber (`quipcall.transcript`) instead of a backend WebSocket: lines to everyone, the history to late joiners, and anyone can ask it to stop. Verified with two app instances and a web participant. Everyone keeps the transcript on their own device; storing it on the server waits for accounts (Stage 3).*
- [x] ~~Transcript panel (chat style) on desktop and web; "Transcribed by X" indicator~~ *Desktop: the transcript window, the call window's side panel, the widget's captions button, Home's Transcript tab. Web: a side panel (below the video on phones) that opens when a transcript starts, with Stop for everyone.*
- [x] ~~Raise Opus bitrate while a transcription is running~~ *Not needed: lines transcribed through a call (a web participant's Opus) came out word for word as from the clean audio.*
- [x] ~~Settings window (⌘,) with Recording and Transcription tabs: language, model downloads (on demand, checksum, remove), try it with your voice~~ *Transcription: language, model, downloads (resumed, retried, checked), remove, the window switch, accept asks by itself, Try it (your mic through the model, live), credits. Recording: what Record offers first, each voice in its own file too, the folder.*
- [x] ~~Everyone's voice, 10 ms at a time, for recording and transcription (`audio_tap.rs`): your mic through a new hook in the webrtc-sys patch (after processing, only while unmuted), remote mics through LiveKit's audio sinks; nothing is copied unless something listens~~ *Remote audio verified end to end; your own voice in a real call recording (2026-11-09).*
- [x] ~~Recording, audio: everyone mixed into one AAC .m4a with AVAssetWriter, a fragment every 5 s; optionally one track per voice~~ *Verified with a headless web participant: its 2-per-second beeps land in the file exactly 0.5 s apart, a 14 s recording is 14.1 s, quitting mid-recording finishes the file, and a test that aborts the writer keeps 10 of 12 s. Each voice in its own file too (a Setting): one .m4a per person in a folder next to the mix, the same length as the mix.*
- [ ] Recording, screen: the share picker in record mode (same presets), ScreenCaptureKit and VideoToolbox H.264 plus the audio mix in one .mp4. Measure CPU. *Built. Checked with the synthetic screen and a web participant: 1280×800 H.264 at 15 fps, 150 frames in 10 s (none dropped), its beeps in the sound, and "… is recording this call and their screen" on the web. The choice of Audio or Audio and screen looked right in the widget (2026-11-10). CPU: about +1.5% while recording a 1920×1200 screen at 15 fps (release, M5 Pro). Sharing and recording the same screen runs two captures; sharing one capture is for later. Known issue: at 1920×1200, AVAssetWriter can fail (-11800, underneath -16341) after the process has been suspended a few times (reproduced only with `vmmap` attached; 1280×800 survives it). A failure now shows at once, clears the indicator for everyone, and the file keeps what came before its last 5 s fragment.*
- [x] ~~One recording per call: a participant attribute marks the recorder, Record is hidden for everyone else, and only the recorder can stop it. An indicator for everyone, a notice when it starts, a "saved" notice, and a stop when the recorder leaves~~ *In the widget, the call window and the web client; checked with two app instances and a web participant. Out since 2026-11-10 (build 40).*
- [x] ~~Home: the Recordings tab (opens the file in QuickTime Player) and a player under the transcript that plays the recording's sound~~ *AVPlayer under the transcript: any line plays from where it was said, the line playing is outlined, a bar jumps along.*
- [x] ~~Footprint of a long transcribed call~~ *30 minutes of one voice transcribed with the window open (release, M5 Pro): about 240 MB between utterances and 320–330 MB while the model runs, the same at the end as at the start; the baseline rose by about 0.5 MB a minute (244 lines). CPU about 9% with the speaker's video.*

**Done when:** a 1-hour mixed-language test call produces a transcript with correct speakers, handing off the transcriber role works, and a recording of the same call plays under its transcript in Home.

*Status 2026-11-10: everything above is built and out (web deployed, Mac build 46). Checked on quipcall.com: a Mac transcribing a web participant in English and Russian, a late joiner getting the history, the web panel. Still to do with people: the 1-hour call with several real voices. A code review (Codex) then found and we fixed: a dropped connection leaving the recording running; two Macs starting at once both carrying on, or a handover losing to a clock that runs ahead; the history after a handover missing what came before it; the transcriber's own history missing the line cut off by leaving (verified: it's kept now); a stopped voice trial still opening the microphone; a whole `.part` download never finishing, and two models fetching the VAD at once; a failed encoder going on "recording"; an unmuted voice's first line dated before the mute. Review of the pull request then found and we fixed: anyone in the call could post transcript lines in anyone's name, and a malformed one broke the web panel (lines now come only from the transcriber, checked on both apps); a recording still being finished after you left could take in the next call's voices (each call has its own audio now); a dropped connection could still let a recording or the transcript start; a handover going ahead after the transcript was stopped; the recording notice vanishing while a second recorder carried on; relayed history showing a line twice; two voices' files colliding on a name that differs only in case; and a slow voices' file holding frames without limit. Checked with test instances: forged lines dropped, a late joiner after a two-Mac handover gets the whole transcript once. A second review then found and we fixed: a relay could rewrite or take back another Mac's line; a handover could still go ahead after Stop, while the last words were being transcribed, or after the asker stopped and started again (checked with two test instances); the screen recording ran on after leaving until the last words were in; a room still closing could reach the next call's state on a reused engine; long late-join history in Cyrillic could exceed LiveKit's packet limit. A third review then found and we fixed: Stop could still lose to a handover while its claim went out (a start can now be stopped at any point before it listens, and the asker passes a Stop on; checked with two test instances: a Stop sent to the old transcriber after a handover stops the new one); an ask from an older app could take over a restarted transcript; an overtaken connect could replace a newer one; a relayed final couldn't finish a draft on the web; the web took last words for 10 s, the Mac 15. A fourth review then found and we fixed: Stop waited on sending before it stopped listening; with two handover asks out, a Stop went on to the wrong Mac; a start for a call you left blocked the next call's; a late draft could replace a final. A fifth review then found and we fixed: with two Macs taking over at once, a Stop could miss the one still loading; a dropped connection kept the call's lines and a start on its way for the next call; a forged relayed final kept the real one out. Web deployed with the third round's fixes and checked on quipcall.com (forged and malformed lines dropped, late joiner's history intact). Still open: the transcriber's claim is an attribute anyone can set, so someone could add made-up lines, until the backend lease.*

## Stage 10 — LLM summaries

- [ ] `Summarizer` trait: Claude API (`claude-haiku-5-5` / `claude-sonnet-5-5`), plus a local backend (llama.cpp-compatible)
- [ ] Summaries on demand and at the end of a call, stored in call history
- [ ] Prompt covers decisions, action items, and highlights per speaker, written in the transcript's language

**Done when:** a summary of a test call is generated and visible on both desktop and web.

## Mobile (iOS first, then Android)

Native apps: Swift and UIKit on iOS, Kotlin on Android, both thin views over the desktop's call engine (`crates/core`) through `crates/mobile`. Decisions: [ARCHITECTURE.md](ARCHITECTURE.md#mobile-ios-first-then-android). Design: [prototype/mobile.html](prototype/mobile.html), with Telegram as the model: rooms are its groups, contacts are for personal calls. We finish iOS (build, tune, test) before starting Android.

### Mobile budget

Set to the least we think can work. We raise a line only when a real call hits it, and write down why. Measured on a real iPhone, release build (`docs/FOOTPRINT.md`); the first numbers are from the Simulator and only show the direction.

| Metric | Limit | So far (Simulator, 2026-11-16) |
|---|---|---|
| App on the phone (arm64, stripped) | ≤ 20 MB | 18.3 MB |
| On the list, signed in | ≤ 30 MB, 0% CPU, nothing on a timer | 34 MB with the test runner attached |
| Audio call | ≤ 40 MB, ≤ 5% of one core | not measured (the Simulator runs without audio) |
| Call of two, video both ways | ≤ 70 MB, ≤ 15% | receiving two: 44 MB, ~8%; sending too: 215 MB (the Simulator's software encoder, see below) |
| Room call, six on screen | ≤ 90 MB, ≤ 25% | not measured |
| Camera you send | 540 × 960 at 24 fps, ≤ 800 kbps, with one 180 × 320 layer (150 kbps, 15 fps) under it | as set |
| Video you receive | no more than its tile shows: the full layer only for a tile at least 700 px wide | as set |
| GPU | none of our own: video goes to the system's renderer, the UI redraws only when something changes | as built |
| In the background | audio only: nothing is decoded for a screen nobody sees | not built |

### M0 — Foundations and media spike (riskiest part first)

- [x] ~~Decide the stack: native UI over the Rust engine, bindings by UniFFI; UIKit, no storyboard, no packages~~
- [x] ~~`crates/core` builds for iOS: the noise model and speech to text became features (`denoise`, `transcribe`), on for the desktop and off for phones; camera, permissions and pixel buffers are shared by macOS and iOS~~
- [x] ~~`crates/mobile`: `Client` (sign in, rooms) and `Call` (events, mic, camera, video), with Swift bindings written by `dev/ios-core.sh`~~
- [x] ~~`apps/ios`: an Xcode project with a "Rust core" build phase; sign-in, the room list, settings and a call screen~~ *Checked in the Simulator against an isolated server: the browser-approval sign-in ends in the Keychain, the list loads, a room is created.*
- [x] ~~Receive video with no copy: decoded H.264 `CVPixelBuffer`s go from the engine's thread into each tile's `AVSampleBufferVideoRenderer`~~ *Two web guests (`dev/guests.mjs`) at 640 × 480: "first video frame on screen … zero_copy=true" for both, speaking ring follows them, leaving and rejoining works.*
- [x] ~~Send the camera as H.264 at the budget's size, two layers~~ *With the synthetic camera (the Simulator has none): LiveKit lists 540 × 960 at 800 kbps and 180 × 320 at 150 kbps; your own tile shows the same buffers.*
- [ ] The real camera: front and back, upright (the capture connection turns frames, so nothing rotates on the CPU), 24 fps. *Built; needs an iPhone.*
- [ ] Audio on an iPhone: microphone and speaker through the system's voice processing (echo cancellation, noise suppression, gain), loudspeaker and receiver. *Built blind: the Simulator's microphone is the Mac's, so test runs there open no audio device at all.*
- [ ] Footprint on an iPhone. *Simulator: 34 MB on the list, 44 MB and ~8% CPU receiving two cameras. Sending the camera adds about 170 MB there, all of it given back when the camera goes off; a process sample shows VideoToolbox encoding, which in the Simulator is software. Measure on a phone before anything else is built on it; if it costs that much there too, find what holds it (run with `MallocStackLogging`).*
- [ ] The first join after the first install stayed on "Joining…" once (no request reached LiveKit). *Not reproduced in four more joins, cold and warm; the engine's log now goes to a file with `-log`.*
- [ ] Go/no-go on the Rust engine for phones, against LiveKit's Swift and Android SDKs. *Recommendation: go. One engine for three platforms, the phone already follows the desktop's recording and transcript notices, and the app is 18 MB. What it costs: we own the audio session, CallKit and the camera ourselves.*

**Done when:** a call between an iPhone and the Mac app works over quipcall.com with audio and video both ways, the budget table has phone numbers in it, and we've said go or no-go.

### M1 — Design

- [ ] Round 1 of the prototype ([prototype/mobile.html](prototype/mobile.html)): Calls (rooms and people in one list), a room, contacts, a person, an incoming call, a call of two, a room call, a shared screen, the minimised pill, sign-in, settings. *Drawn; waits for a look.*
- [ ] Decide: one list (rooms and people together, as drawn) or a Rooms tab and people only under Contacts; whether a tap opens a room or calls it.

**Done when:** every screen below is approved as drawn.

### M2 — App shell

- [ ] Sign in with an email link that opens the app (server: a native variant of the link exchange; universal links for quipcall.com). Until then: the Mac's code in the browser.
- [ ] The list: last call and its length per room, who's in a room right now with Join (server: room presence, Stage 3's webhooks), search, long-press actions
- [ ] A room: its calls as a chat, Call or Join, members, the link, invitations and access (the Mac's Room access)
- [ ] Create, rename and close rooms; quick calls; share sheets
- [ ] Room links open the app: `quipcall://` (built) and https://quipcall.com links
- [ ] Call history on the phone; your name; offline and error states
- [ ] Session lifecycle as on the Mac: expired sessions return to sign-in from anywhere (built for the list and for joining)

**Done when:** everything the Mac's Home does with rooms can be done on the phone.

### M3 — The call

- [ ] Layouts: a call of two (they fill the screen, you in a corner you can drag), rows up to three people and a grid from four (built), pages past eight, a shared screen on top with pinch to zoom and landscape
- [ ] The minimised pill inside the app; the system's picture in picture outside it
- [ ] Audio routes: loudspeaker, receiver, Bluetooth and wired headsets, with the system's picker; the proximity sensor on audio calls
- [ ] Interruptions: a phone call, Siri, an alarm; the app in the background (audio goes on, video stops both ways); the screen locked
- [ ] Reconnecting when the network changes (Wi-Fi to mobile data and back)
- [ ] Join muted or with the camera off (the engine publishes the microphone unmuted today; the phone mutes it the moment it opens)
- [ ] Flip the camera without republishing the track; turn the phone (your video, and other phones' turned video)
- [ ] Recording and transcript notices (the events arrive already); captions and the transcript itself wait for the transcription round
- [ ] Use less when it matters: Low Power Mode and a hot phone lower what's sent and received

**Done when:** a 30-minute room call with six people stays inside the budget on an iPhone, through a network change and a trip to the background.

### M4 — Contacts and personal calls

- [ ] Server: contacts (add by email, invitations), presence over the WebSocket hub, the ringing protocol (Stages 3–5)
- [ ] Ringing a phone that's asleep: VoIP push and CallKit, so a Quipcall call rings like a phone call, on the lock screen too. *Needs the Apple Developer Program.*
- [ ] Contacts, a person's screen, missed calls; the Mac and the web ring too, and the first to answer takes the call

**Done when:** two phones call each other from Contacts with both apps closed.

### M5 — iOS alpha

- [ ] Apple Developer account, signing, TestFlight for friends; `make release-ios`
- [ ] Privacy manifest, the icon's dark and tinted variants, crash logs
- [ ] Battery: an hour's audio call, an hour's video call

**Done when:** friends install it from TestFlight and call each other.

### M6 — Android

- [ ] Kotlin views over the same `crates/mobile` (UniFFI's Kotlin bindings), the engine's Android pieces (camera, video to a `Surface`, the session in the Keystore), calls through Telecom and FCM
- [ ] The same budget, measured on a mid-range phone

### Later (mobile)

- [ ] Transcription on phones: captions and transcripts from a Mac in the call arrive already; a phone transcribing by itself is to discuss
- [ ] Sharing the phone's screen (ReplayKit, a media projection)
- [ ] iPad and landscape layouts

## Later

- [ ] Windows desktop (Windows.Graphics.Capture, GPUI on DirectX 11)
- [ ] Linux desktop (screen capture through PipeWire and the desktop portal)
- [ ] Server-side transcriber (headless bot on the VPS or a GPU box)
- [ ] Upload recordings to the server so everyone in the room gets them (needs Stage 3)
- [ ] End-to-end encryption
- [ ] Speaker separation for shared microphones
- [ ] Peer-to-peer for 1:1 calls
