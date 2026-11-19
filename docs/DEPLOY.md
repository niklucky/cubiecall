# Deploy

The alpha uses one VPS (`root@testron.dev`, Debian 13, shared with other projects). Its canonical origin is **https://cubiecall.com**.

```sh
make deploy        # server + web client + VPS config
make release-mac   # build the Mac app and publish it at https://cubiecall.com/download/
make mac-app       # build only → target/mac/
```

Set `CUBIE_HOST=user@host` to deploy somewhere else.

## First deployment under the Cubie name

1. Point `cubiecall.com`, `cubycall.com` and `quipcall.com` to the VPS. Certbot must reach the HTTP challenge on all three domains. The installer also requests certificates for their resolving `www` names. To enable a missing `www` alias later, add its DNS record and rerun the deployment.
2. Verify `cubiecall.com` in Resend before setting `CUBIE_MAIL_FROM="Cubie <login@cubiecall.com>"` in the local auth configuration. The installer can also reuse the existing verified sender address and credentials.
3. Take an online SQLite backup of `/var/lib/quipcall/quipcall.db`. Publish the Cubie Mac archives with `make release-mac` so the new download page works as soon as the site switches.
4. Run `make deploy` in a maintenance window. The installer obtains the certificate, validates nginx, stops the old services, copies the stopped database with its journals, starts the Cubie services, and switches nginx after both services pass their checks. Before nginx switches, a failed cutover restores the original services and database. Once nginx can route writes to Cubie, retries keep the Cubie database. If the deployment is interrupted, rerun it to finish the switch and resume queued email and cleanup jobs. This first switch disconnects active calls. The installer retains the existing LiveKit keys and old Mac download archives. On upgraded hosts, keep `/var/www/quipcall-acme`. The Cubie ACME webroot points there for renewals.
5. Check a new invite on `cubiecall.com` and an existing room link on each alias. A fresh visitor lands on the canonical site with the original path, query and fragment. A browser with a session on `quipcall.com` can keep using its original link and guest membership there. Browser cookies are scoped to each host. Both apex aliases serve the API and authenticated LiveKit signaling directly for native clients.

Browser handoffs use `quipcall://` to open installed apps from either release. The Mac and iPhone apps register both `cubie://` and `quipcall://`. The Mac copies an existing Quipcall session into its own Keychain entry the first time macOS grants access. Choosing **Allow** once is sufficient for this migration. The Mac app also moves managed history and downloaded models to `~/Library/Application Support/Cubie`. Previously recorded files stay at their saved paths. The phone uses the same Keychain and history keys across the production aliases. Email sign-in returns to the browser origin that requested it, including a legacy tab without a session cookie.

## What runs where

| | |
|---|---|
| `https://cubiecall.com/` | web client (`/opt/cubie/web`, static) |
| `/api/` | `cubie-server` on `127.0.0.1:4601` |
| `/rtc` | LiveKit signalling (WebSocket) on `127.0.0.1:7880` |
| `/download/` | Mac app, install script, instructions page (`/opt/cubie/download`) |
| UDP `7882` | LiveKit media, all participants on one port |
| TCP `7881` | LiveKit media over TCP, for networks that block UDP |

- nginx terminates TLS: site `/etc/nginx/sites-available/cubiecall.com.conf` (source: `deploy/nginx/`). `www` redirects to the bare domain.
- Let's Encrypt certificate via certbot's webroot (`/var/www/cubie-acme`), renewed by `certbot.timer`. A deploy hook already on the box reloads nginx after renewals.
- systemd units `cubie-livekit` and `cubie-server`, both running as the `cubie` user.
- `/opt/cubie/secrets.env` (root, 0600) holds the LiveKit API key and secret. `install.sh` generates it on the first deploy and it never leaves the box. It also holds the auth settings below. Rotate LiveKit keys in place; do not delete the email configuration.
- No host firewall. LiveKit binds only to `eth0` (public IPv4 + IPv6), not to the Docker bridges.

## How `make deploy` works

`deploy/deploy.sh` builds locally and runs `deploy/install.sh` on the box:

1. The server, cross-compiled to a static Linux binary with `cargo zigbuild` (`x86_64-unknown-linux-musl`; Zig compiles bundled SQLite and TLS C code), and the web client (`pnpm build`). Install `zig` and `cargo-zigbuild` on the build Mac first.
2. One tarball over SSH to a private temporary directory under `/opt/cubie-incoming.*`, removed afterwards.
3. `install.sh` (idempotent): user and directories, LiveKit at the pinned version, keys, configs, units, web files, nginx site, then health checks (`/api/health` through nginx, LiveKit on 7880).

Guard rails, because the box is shared:
- LiveKit restarts only when its binary, config or unit changed. A restart drops every call in progress. The API server always restarts; that takes milliseconds and keeps calls alive.
- If `nginx -t` rejects the new site, the previous one is restored, so other sites keep working and future reloads don't fail.
- Subsequent deploys update the Cubie site, two systemd units, and the application, database and ACME directories listed above.

## Mac app

### GitHub Actions releases

The **Release Mac app** workflow (`.github/workflows/release-mac.yml`) builds Apple Silicon on `macos-15` and Intel on `macos-15-intel`, using Rust 1.99.0. Push an exact `vX.Y.Z` tag to publish:

```sh
git tag v0.1.1
git push origin v0.1.1
```

The tag sets the app's displayed version; a separate `CUBIE_BUILD` keeps Sparkle updates increasing independently of the Cargo workspace version. The shared build number is `1000 + github.run_number * 100 + github.run_attempt`, above the last pre-CI build 108. A retry gets a new immutable release directory. The workflow rejects prerelease tags, leading zeros and other non-`vX.Y.Z` names before it accesses signing credentials. Use Actions → **Release Mac app** → **Run workflow** to test both signed builds without changing the website or creating a GitHub Release; a manual run never publishes, even when a tag is selected.

Both jobs must finish notarization, stapling, signature verification and an extracted-app distribution check before publication. `deploy/mac/verify-release.py` verifies each archive's architecture, bundle ID, version and build, the pinned Sparkle public key, and both the Ed25519 appcast and archive signatures. The publication job checks both artifacts again, uploads them over SSH, verifies SHA-256 on the host, switches both feeds together with the existing publisher, updates the download instructions, and verifies public downloads before creating a GitHub Release. Publication is serialized across tags and the host refuses stale build numbers. Failed builds leave the current public release in place. If a job fails after the website switch, check `/download/version.txt` before retrying; the next attempt uses a newer build number.

The repository secrets are:

| Secret | Purpose |
| --- | --- |
| `MACOS_CERTIFICATE` | Base64-encoded PKCS#12 export of the shipping Developer ID certificate and private key |
| `MACOS_CERTIFICATE_PASSWORD` | Password for that export |
| `APPLE_ID` | Apple account used for notarization |
| `APPLE_NOTARY_PASSWORD` | Apple app-specific password, not the account's normal password |
| `SPARKLE_PRIVATE_KEY` | Existing `com.quipcall.desktop` key exported by Sparkle's `generate_keys -x` |
| `MACOS_DEPLOY_KEY` | Dedicated SSH private key for the download host |
| `MACOS_DEPLOY_KNOWN_HOSTS` | Host key verified through the existing trusted server connection |

`CUBIE_RELEASE_HOST` is a repository variable set to `cubie-release@testron.dev`. That Unix account has access to `/opt/cubie/download` and `/opt/cubie/mac-staging`, without sudo or Docker access. The workflow does not use the build Mac's root SSH credentials. Mac jobs import signing credentials into a temporary Keychain, use `CUBIE_NOTARY_KEYCHAIN` to select its notarization profile, and delete the Keychain after success or failure. Notary diagnostics are uploaded separately on failure; private keys and Keychains are never workflow artifacts. All actions are pinned to commits. The noninteractive notarytool command receives its app-specific password from a masked GitHub secret; local setup should use the secure prompt below instead.

Sparkle feed generation reads the exported key from a mode-0600 file under `RUNNER_TEMP` (`CUBIE_SPARKLE_KEY_FILE`), avoiding interactive Keychain authorization between Sparkle's tools. The workflow deletes this file in its always-run cleanup step. For a replacement certificate, update `MACOS_CERTIFICATE` and its password and set the new fingerprint in `package.sh`. Keep the same Apple team and bundle ID. Preserve the existing Sparkle key so installed apps continue to trust updates.

Run `python3 -m unittest discover -s deploy/mac -p 'test_ci*.py'` for tag/version and signed-artifact tampering checks; `deploy/mac/test-notarize.sh` checks failed-submission gating. The workflow also runs `deploy/mac/test-publish.sh` on Linux to verify atomic updates and stale-build rejection.

### Local packaging and signing

The Quby icon is bundled from `apps/desktop/assets/$ICON.icns` into `Contents/Resources` before signing and selected by `CFBundleIconFile`; `ICON` in `deploy/mac/package.sh` is `QubyTransparent` (the cube on transparency) or `Quby` (the earlier black tile). Artwork sources, transparent exports, and regeneration instructions are in [assets/branding](../assets/branding/README.md). The web favicon files ship from `apps/web/public` through the normal Vite build.

`deploy/mac/package.sh` builds one app per architecture (Apple Silicon and Intel), signs with Developer ID and the Hardened Runtime, notarizes each app with Apple, staples its ticket, verifies the signature and Gatekeeper assessment, and creates the final ZIP. Sparkle's helpers and framework are signed from the inside out with the same identity; the app has camera and audio-input entitlements, without disabling library validation. Notarization failure prevents publishing archives or update feeds. Submission results and full Apple logs remain in `target/mac/Cubie-<architecture>-notary*.json`. Two slices in one binary would be about 115 MB. Sparkle adds about 3 MiB to each app bundle. Release builds have `https://cubiecall.com` baked in (`CUBIE_SERVER` at build time), and `MACOSX_DEPLOYMENT_TARGET=12.3`, the release ScreenCaptureKit appeared in. Below 10.14.4 the linker would also point the Swift overlay `libswiftCoreMedia` at `@rpath`, and the Intel app wouldn't launch. The main executable links only macOS libraries and loads Sparkle from its own bundle. For a custom `CUBIE_SERVER`, use `make mac-app` and distribute the notarized ZIPs manually.

Download the matching ZIP in a browser, unzip it, and move Cubie into Applications before opening it so Sparkle can replace it in place. macOS may show its normal downloaded-app confirmation; notarized releases do not need the Privacy & Security → Open Anyway override. Alternatively, `curl -fsSL https://cubiecall.com/download/install.sh | sh` picks the architecture and installs the same release.

The release identity is `Developer ID Application: highglossy FZE (8BSJZG6B89)` in the build Mac's login Keychain, including its private key. Packaging pins its SHA-1 fingerprint `FB9D41EDF844DF368B68B049D8A9895D5C5F0B96` and verifies team `8BSJZG6B89`. After renewing the certificate, set `CUBIE_SIGNING_IDENTITY=<new fingerprint>`; `CUBIE_SIGNING_KEYCHAIN=<path>` optionally selects a different keychain. **Back up the certificate and private key securely** and restore them on a replacement build Mac. `deploy/mac/signing.sh` is only a legacy self-signed development helper and is never used for release packaging.

Notarization uses the `cubie-notary` Keychain profile (`CUBIE_NOTARY_PROFILE` overrides it). Set it up once on the release Mac with `xcrun notarytool store-credentials cubie-notary --apple-id <account-email> --team-id 8BSJZG6B89`, entering an Apple app-specific password at the secure prompt. Never put that password in source or command arguments. Both the signing identity and the notarization profile are checked before compilation. `make mac-app` builds and notarizes locally; `deploy/mac/package.sh --appcast` also signs the feeds without publishing; `make release-mac` publishes only after both architectures pass verification. `deploy/mac/test-notarize.sh` tests failure gating without Apple credentials.

macOS still requires user consent for microphone, camera and Screen Recording. The shipping bundle ID stays `com.quipcall.desktop`. Existing users moving from the old self-signed release to Developer ID may need to grant permissions again or approve access to their existing Keychain session. Keep the old code-signing keychain backed up for legacy releases, and keep the Developer ID team and bundle ID stable for future updates.

The first Sparkle-enabled release needs a one-time install from the download page. After that, Cubie checks for updates in the background and installs them when the app quits. Users can also choose **Cubie → Check for Updates…** to install and relaunch immediately.

`make release-mac` signs the appcasts and final stapled ZIPs with the existing Sparkle Ed25519 key stored under the account `com.quipcall.desktop` in the release Mac's login Keychain. Keeping this key lets installed self-signed releases verify the transition to Developer ID. The public key is pinned in `deploy/mac/Info.plist`. If macOS asks whether `generate_appcast` may use the key, choose **Always Allow**. **Back up the private key** using `"$(deploy/mac/sparkle.sh)/bin/generate_keys" --account com.quipcall.desktop -x <backup-file>` and keep that file in protected storage. To restore it on a new release Mac, use the same command with `-f <backup-file>` in place of `-x <backup-file>`. The Developer ID signing identity and notarization profile are also needed. Run `deploy/mac/package.sh --appcast` after `make vendor` to generate signed feeds locally. Update the download page's installation instructions only alongside the first notarized public release.

The publisher puts immutable ZIPs at `/download/releases/<build>/` and switches both architecture feeds at `/download/current/` together. Existing manual download URLs point to the current release. Keep earlier ZIPs available for copies of Cubie that already fetched an earlier appcast. Local builds default to the Git commit count; after CI takes over, set `CUBIE_BUILD` explicitly higher than `/download/version.txt` for any local recovery release. `CUBIE_VERSION` overrides the displayed version; otherwise the Cargo workspace version is used. If a failed publish already placed the numbered release directory on the server, retry with a higher `CUBIE_BUILD`.

The app logs to `~/Library/Logs/Cubie/cubie.log` when started from Finder (the previous run is kept as `cubie.previous.log`).

## Logs and status

```sh
ssh root@testron.dev journalctl -u cubie-server -u cubie-livekit -f
ssh root@testron.dev systemctl status cubie-livekit cubie-server
```

## Invite-only authentication

Keep local credentials in ignored `deploy/auth.env.local` (mode 0600), using [deploy/auth.env.example](../deploy/auth.env.example) as the template. `make deploy` securely includes that file when present (or use `CUBIE_AUTH_ENV=/path/to/file`). The installer merges these settings into `/opt/cubie/secrets.env`:

- `RESEND_API_KEY`: a Resend key with permission to send emails.
- `CUBIE_MAIL_FROM`: sender on a domain verified in Resend. Disable email click tracking for sign-in messages.
- `CUBIE_BOOTSTRAP_EMAIL`: your email. Verify it through the normal magic-link flow to create the first admin account; only admins issue account registration invitations.

The installer requires Python 3, validates these settings before replacing the running application, and writes the merged secrets atomically with mode 0600. It initializes missing LiveKit keys even if secrets.env already contains only auth settings; existing key pairs are preserved. On later deploys the local auth file is optional if the server already has these settings. Credentials are never logged. No deployment is needed to run tests.

The systemd unit sets the public origin to `https://cubiecall.com`, creates a private state directory, and stores SQLite at `/var/lib/cubie/cubie.db`. Back this database up using SQLite's online backup command (or stop the API before copying it). It contains accounts, sessions, invitations, rooms and memberships. Restarting the API preserves access. Migrations run at startup. The API checks readiness by opening and migrating the database before binding its port.

Sign in on the web, then use **Invite a new account** to make a single-use, email-bound registration link. Share the link yourself. It expires after seven days. The recipient enters the invited email and verifies it through Resend. Existing accounts only need an email sign-in link. Magic links expire after 15 minutes and are consumed only by an explicit confirmation POST, so email-link scanners don't consume them. Credential hashes, rather than bearer credentials, are stored in SQLite.

Browser sessions use HttpOnly, Secure, SameSite=Lax cookies on production. Native sessions use macOS Keychain, scoped to the server and (for tests) `CUBIE_DATA_DIR`. Desktop sign-in opens the browser and requires typing the eight-character code shown by that Mac. The code is absent from the URL, stored hashed on the server, and limited to five approval attempts. Pending sign-in can be cancelled immediately. HTTP 401 clears the exact failed native credential; an admission failure returns Home and starts sign-in for that room. Stale responses cannot clear a newer session. The app exchanges a separate private polling secret for a session; no session credential is passed through a custom URL scheme. User sessions last 30 days and guest sessions one year, with renewal on active use. Authenticated use renews the deadline once less than half remains, and browser cookies follow that deadline; expired or revoked credentials are never renewed. A room membership persists independently of session expiry; a guest who loses their credential needs a fresh invitation. Apple and other OAuth providers are deferred; the `identities` table already separates provider identity from the internal account ID.

## Calls and access

- A signed-in user creates rooms. A **guest 1-to-1** reserves one identity for the creator and one for the first guest; the same guest can reconnect, and a third principal cannot join. Removing the guest keeps that slot claimed and blocks the removed principal; invite a replacement with a new call link.
- A **public room** admits anyone with its unguessable link, including guests.
- A **private room** admits its owner, invited registered members and explicitly invited guests. Account registration and room membership are separate permissions.
- A **single-use guest invitation** can be claimed by one identity; that identity can leave and return until manually removed. The unclaimed link expires after seven days, but claimed membership and the original return link have no time limit. Acceptance is atomic. A lost HTTP response can be retried by the same identity, but a consumed link never restores removed access.
- Only the room owner can issue/revoke room invitations, remove access, rename or close the room. Membership invitations are bound to an account email. Guests cannot create rooms or issue invitations.
- **Remove access** commits revocation before requesting LiveKit disconnection. If disconnection fails, the API reports failure and the UI offers a retry; access remains revoked. Closing a room works the same way.

LiveKit signaling **must** pass through nginx's `auth_request` gate on `/rtc` (including its subpaths). Each join has a distinct signed LiveKit identity (`principal:connection`) so one account can use multiple clients. Removal enumerates and disconnects every connection for that principal, including legacy identities. The gate validates the signed LiveKit token and checks current room access before every handshake, including refreshed tokens. The LiveKit HTTP/signaling port remains bound to loopback. This is required for manual removal: self-hosted LiveKit alone does not reliably revoke a previously issued token. Do not expose port 7880 directly, cache authorization responses, or remove this gate. nginx must include `http_auth_request_module`; the installer runs `nginx -t` before reload. Signaling access logs are disabled because URLs contain tokens.

The old unauthenticated GET `/api/join` is removed. Unknown room IDs no longer create rooms. Existing alpha room bookmarks are not automatically claimed by whoever presents them; create replacement rooms while signed in. Ship the updated desktop app with the backend/web update; old clients cannot authenticate. Current call history and recordings remain local and aren't exposed by a room invitation. Desktop **+** creates public/private rooms. **Access** on Home and **Invite** during a call manage guest/member links, pending invitations, member removal, renaming and closing. The native list refreshes when Home gains focus. Account actions are behind the profile row’s Account button. Registration invitations remain in the web admin controls.

## Local auth development

`make dev` uses `dev/dev.env`: SQLite under `dev/data`, bootstrap email `admin@example.test`, and a private-file outbox in `dev/data/mail`. Request a magic link from the local web UI, then open the URL from the outbox file. Nothing is sent to Resend in this mode. The outbox is explicitly restricted to localhost and never logged. Real deployment must use Resend.

For an isolated second stack, source `dev/isolated-env.sh` **once** from the repository root, then launch all processes from that shell (or pass its exported values to each terminal):

```sh
source dev/isolated-env.sh
livekit-server --config "$CUBIE_LIVEKIT_CONFIG" > "$CUBIE_TEST_DIR/livekit.log" 2>&1 &
cargo run -p cubie-server > "$CUBIE_TEST_DIR/server.log" 2>&1 &
(cd apps/web && pnpm dev --port 5273 --strictPort) > "$CUBIE_TEST_DIR/web.log" 2>&1 &
# Native clients also need the web origin for browser approval:
MACOSX_DEPLOYMENT_TARGET=12.3 cargo run -p cubie-desktop -- --server "$CUBIE_API" --web "$CUBIE_PUBLIC_URL"
```

The helper creates fresh SQLite/outbox/native state under `$CUBIE_TEST_DIR`, uses server 8180, Vite 5273 with a matching allowed Origin, and LiveKit 7980/7981/7982. Do not use ordinary `make dev` for this stack: its included dev.env points at the user's shared development state. Keep track of your own process IDs when stopping the stack. Authenticate with `admin@example.test` via the isolated outbox, create a public room, then use that room ID for a headless participant. `?room=<id>&autojoin=1&mic=0` works only in local Vite development mode and still obeys server authorization; production always shows the joining screen. Use Chrome's fake-media flags from CLAUDE.md.

To test token revocation end to end, put a local nginx auth-request gateway in front of LiveKit and use its URL as `LIVEKIT_PUBLIC_URL`. The minimal development stack connects directly to LiveKit and therefore does not enforce the signaling gate; it still enforces API admission.

Verification: `cargo test -p cubie-server` covers signup gating, email confirmation/reuse/expiry, session revocation, role enforcement, simultaneous invite/direct-slot claims, private access persistence/removal, cached-token authorization, desktop proof and CSRF/rate limits. `make check` checks Rust and Svelte; `pnpm build` in `apps/web` checks the production bundle.

Native room checks: `cargo test -p cubie-desktop rooms::tests` covers creation responses, retaining issued links when refresh fails, and revocation/disconnection partial failure. With the `snapshot` feature, `room-new`, `room-create-private`, `room-create-public`, `room-manage`, `room-guest`, `room-member`, `room-refresh`, `room-remove`, `room-retry-remove`, `room-revoke` and `room-close` exercise the native controls against an isolated API. `CUBIE_TEST_SESSION_FILE` supplies a fixture credential only on localhost; `CUBIE_ROOM_TEST_STATE` exports the result for assertions. These hooks are absent from release packages. Always use an isolated `CUBIE_DATA_DIR`, `CUBIE_NO_MIC=1`, `CUBIE_FAKE_CAMERA=1` and `CUBIE_FAKE_SCREEN=1`.

Join preflight: `POST /api/entry` accepts exactly one of `room` or `invite`. It returns only permitted room metadata and an access state, never consumes invitations or reserves a direct-call slot, and never mints a media token. Ship its server change with the web join page. Both `cubie://open/` and `cubie://join/` use the Mac’s current identity. Browser approval is requested only when the Mac lacks a valid session or the user explicitly switches accounts. Admission supplies the room’s display name for unsaved links. `pnpm test` in `apps/web` (Node 22.18+ or 24+) checks media request cancellation, device changes, cleanup and permission denial. API tests check preflight privacy and unchanged admission rules.

Desktop sign-in polling uses a separate IP allowance from email/approval requests, plus a per-challenge quota after verifying the private proof. The app backs off on transient network errors and HTTP 408/429/5xx within its original sign-in deadline. Cached room lists are bound to the principal and cleared before a new account refresh, including when that refresh fails. Browser join continuations are cancelled when navigating away, so late identity/redemption responses cannot trigger another join or approval.

Auth requests are limited per IP; unrelated recipients and auth endpoints do not share a global budget. Email requests enter a durable SQLite queue before any account lookup. The worker drains at startup and wakes via Notify after enqueue, at a scheduled retry, or a five-minute recovery poll. Failed delivery retries up to five total attempts with 30/60/120/240-second backoff, within the request’s 15-minute lifetime. Each requester gets three new requests per eligible recipient per 15-minute window, while a recipient-wide cap permits at most six delivery attempts across all IPs, including retries. Excess requests return the same public success response and preserve delivered links. Successful verification resets requester suppression, not the recipient cap. Ambiguous failures retain their token and consume the delivery attempt because the provider may have accepted the email before timing out. Delivery errors and retry exhaustion are logged without exposing account eligibility.

Migration 0002 adds queued mail and device-code approval. Deploy updated web, API and Mac clients together; existing pending challenges and older Mac builds cannot complete code approval. Existing signed-in sessions remain valid.

Migration 0003 adds durable mail retry scheduling and temporary direct-call expiry. Unused Quick-call links become eligible for cleanup after 24 hours; older open direct rooms receive 24 hours of grace from migration. Owners can choose **Keep as room** in native Room access, Home’s history, or the web room details before expiry to make the room reusable. Temporary calls stay out of `/api/rooms`; private/public rooms and durable private guest memberships do not expire. New admission and signed-token reconnects use the same persisted closed state; either can renew an open room until cleanup commits closure. Cleanup checks presence, renews occupied rooms, and closes only after two empty observations ten minutes apart. Each candidate has its own retry schedule, so failed deletions cannot starve other rooms. Deploy updated clients with this migration so keeping a direct call updates the server as well as the local bookmark.

Migration 0004 adds provisional desktop sessions and idle cleanup scheduling. Approval polling leaves the previous session valid and returns a ten-minute provisional credential. After saving it in Keychain, the Mac’s first authenticated request activates it and revokes the previous credential atomically. Uninstalled or lost responses expire without revoking the previous session; cancelled issued credentials are submitted for logout. Other devices remain signed in. Ship updated native clients with the server so Keychain failures preserve the previous credential.

Guest creation is capped separately at 30 new identities/hour and 100/day per IP. Existing sessions are reused without consuming that quota. Auth cleanup removes expired credentials and unreferenced guest identities in batches, retaining durable membership/revocation and pending approval references. It runs separately from LiveKit room cleanup. Migration 0005 indexes guest-reference lookups so pruning does not scan entire child tables for each guest. Mac browser approval does not preflight or redeem the destination room; admission is checked by the Mac after identity approval.

**iPhone app.** `make release-ios CUBIE_IOS_TEAM=<team id>` (`deploy/ios/release.sh`) archives the app for a real iPhone, lets Xcode sign it for that team and uploads it to App Store Connect, where TestFlight picks it up; `--no-upload` stops at the `.ipa`. It signs in as the account in Xcode's settings, or with an App Store Connect API key (`ASC_KEY_ID`, `ASC_ISSUER_ID`, the key in `~/.appstoreconnect/private_keys/`). The app's record has to exist in App Store Connect first, made by hand with the same bundle id (`com.cubie.ios` unless `CUBIE_IOS_BUNDLE` says otherwise). Every upload needs a build number above the last; the script uses the time. The app talks to cubiecall.com; its Contacts tab needs the server below deployed.

`POST /api/contacts/match` (for the phone's Contacts tab) takes up to 1,000 SHA-256 hashes of lowercased email addresses and returns the hash and display name of each that belongs to another account. Signed-in accounts only (guests get 403), 20 requests an hour and 10,000 addresses a day per account (a hit gives a name back, so guessing is rationed); nothing from the request is stored or logged. The route has its own body limit (70 KB); every other route keeps 8 KB. It needs no migration; older apps never call it.

SQLite uses WAL and a four-connection pool so media admission reads can proceed during unrelated writes. Quick-call signaling renews activity only in the latter half of its lifetime, avoiding a write on each reconnect. Use SQLite online backup; copying only the live `.db` file omits uncheckpointed WAL data.
