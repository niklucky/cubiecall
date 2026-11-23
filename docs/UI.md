# Cubie UI

Less is more. Outside a call the app is one window: your rooms and their calls. During a call there is no window by default, just one small widget with the call floating over whatever you're working on.

The clickable prototype these screens were built from is [prototype/ui.html](prototype/ui.html) (open it in a browser). Some parts remain planned, including contacts, ringing and shared files.

## Signing in

Every screen asks for one thing and has one button. No passwords, and no words like "guest", "member" or "session". The clickable design for all of it, web and apps, is [prototype/onboarding.html](prototype/onboarding.html).

A Mac without an account opens on a small window (400 × 500): Quby, "Welcome to Cubie", a field for your email and **Continue**. The server emails a 6-digit code; the window's second step shows six boxes for it (typed or pasted, sent on the sixth digit), with **Send a new code** (after half a minute) and **Use a different email**. A wrong code clears the boxes and says so. No browser opens. An expired session comes back to the same window, which says why. Closing it quits the app, as there is nothing else open.

A new account has no name yet, so a second window asks what people should call you, with your avatar updating as you type and the name on this Mac's account as a first guess. Then Home opens, or the call a link asked for. `--name` skips sign-in and opens Home without an account (development and tests).

Cubie is invite-only: a code is sent only to an email that has an account or an invitation, and accounts are made on the web, from an invitation to Cubie or to a private room. The session stays in Keychain. Apple and other OAuth providers are planned.

## Home

One window (960 × 640), two panes:

```
┌──────────────┬───────────────────────────────────────────────┐
│ ●●●  ⚡Quick │ Team standup ✎                  🔗 Access [Call] │
│ [Rooms|History] +  (A)(M)(L) Anna, Max and you  ‹ Today 10:02 · 32 min › │
│ (A+3) Team standup │ Transcript   Recordings 0   Files 0    │
│       Today 10:02 · 32 min │ ┌ Transcripts are on their way… ┐ │
│ (M+1) Family │ ── Call started · 10:02 ──                     │
│ …            │ ── Anna joined · 10:02 ──                      │
│ (N) Nikita ✎ │ ── Call ended · 10:34 · 32 min ──              │
└──────────────┴───────────────────────────────────────────────┘
```

- **Rooms**: one avatar (whoever you've talked to most there) with "+N", the name, room type, and the last call's day, time and length. Clicking a row shows its latest call; the phone that appears on hover joins.
- **History**: every call by day (Today, Yesterday, weekday, then the date), quick calls included.
- **The selected call**: title (owners rename shared rooms; saved links have a local name), copy link, **Access**, **Call**. Below it, who was there and ‹ › through the room's earlier calls. A quick call stays in History and offers **Keep as room** and **Call again** instead. Keeping it saves a bookmark for the current account; renaming that bookmark changes only its name on this Mac, including for someone else’s room. Bookmarks survive server refresh and account switching.
- **Tabs**: *Transcript* shows the call's timeline (who joined, left, shared, recorded and transcribed, with times) with what was said between, as a chat, if the call was transcribed. *Recordings* lists what you recorded in the call (time, length, size), with Open (QuickTime Player) and Show in Finder, and *Voices* for the folder of each voice on its own. A recorded call's Transcript tab has a player at the bottom (▶, the time, a bar to jump along); click a line to play from where it was said, and the line playing is outlined. *Files* shows an empty state until files exist.
- Unused **Quick call** links are cleaned up after 24 hours unless the owner chooses **Keep as room** (in History or Access). Temporary calls stay out of the web and native Rooms lists. Active calls stay open. **Remove from your rooms on this Mac** asks for confirmation and hides a saved link locally without closing the room or changing anyone’s access. **Restore hidden rooms** restores hidden entries, including rooms with no call history. Owners rename shared rooms; local aliases apply to rooms owned by someone else.
- **Quick call** is top left; **+** opens a name field and the choice of **Private** (default) or **Public**. Creation requires an account.
- When a call ends, home opens on it.

## Joining from a link

A link opens the door to a call: your face (initials on your colour, filling in as you type), two round switches, the headline, one field for your name and **Join call**. The headline says who invited you ("Anna invited you to a call") or the room's name with whose it is under it. The page checks access without creating anyone, claiming an invitation or reserving a 1-to-1 slot. It never offers the app and never opens it.

The microphone starts on and the camera off. The browser asks for the microphone when the call starts; refused, the call still opens, muted, and says how to allow it. Turning the camera on shows it in the circle first ("Only you see your camera until you join"). With more than one microphone or camera, a quiet "Change microphone or camera" line opens the choice. The name is remembered in the browser, so coming back is one tap.

**A private room asks who you are first.** Its invitation opens "Anna invited you to Family" with a field for your email, then the six boxes for the code from the email, then the door. The invitation is also how a new person gets an account: there is no other sign-up. A new account is asked its name on the door itself ("You're in. What's your name?"); someone with an account goes email, code, Join. The code is typed on the page that asked for it. The email's button, for people reading it on the same device, opens a page with one button (mail scanners open links, and a link that signed in by itself could be used up before you see it) and goes on to the same door.

After accepting an invitation, the address becomes the ordinary room link so the same browser can return. Unclaimed invitations survive reload and sign-in in the current history entry. Everything that can go wrong has one sentence and at most one button: a private room's link without an invitation (sign in; then "You need an invitation", naming the email to ask for), an invitation made for another email, a used or expired invitation, a call for two that is full, access removed, a call that has ended, no connection, and a messenger's built-in browser that can't make calls ("Open this link in Safari or Chrome"). After leaving: "You left the call" and **Join again**.

**The website itself** (no link) is the same email and code, then Home: **Start a call** (a link to send, then the call), your rooms, **+ New room** (private, or anyone with the link), and a room's page (join, invite, people, rename, close). A private room has one **Invite someone**: a link that works for one person. **Open in the Mac app** is a quiet link on a room's page there, for people who are signed in; it uses `quipcall://open/<room>`, which both generations of the app register. Apps from before in-app sign-in still open the browser to be approved: that page signs you in the same way, then asks for the code the app shows.

## Room access

**Access** on Home and **Invite** during a call open the same native window. Public rooms admit anyone with their address. A private room address only works for people who already have access.

Owners of private rooms can create an invitation link that works for one person, or one bound to an email. The app displays the generated link for copying; it does not email the room invitation. Whoever opens it signs in, or makes their account with it, and has access until manually removed. Unclaimed invitations expire after seven days.

Owners see people and pending invitations, can revoke unused invitations, and can remove someone after confirmation. Access is revoked even if disconnecting the ongoing call fails; the error remains visible and **Disconnect again** retries it. **Close room** requires confirmation, prevents new joins and disconnects everyone while keeping local history. Closing permanently removes local bookmarks; restoring hidden rooms cannot bring them back. Removing a direct guest also locks their slot; start a new Quick call to invite someone else. Non-owners see the room address and their room information. Refresh updates people and invitations; switching accounts clears the access window.

## Call: minimal mode (default)

```
                           (Anna)  (You)         ← camera on: bubbles ride on the pill
        ╭────────────────────────────────────╮
        │ (A) (L) (N) ((S))+3 │ ⋯ 🖥 🎙 ☎ │    ← the pill: recent speakers, you, the stack, four buttons
        ╰────────────────────────────────────╯
```

One small solid pill floats above all apps and Spaces and never takes focus. It is exactly as wide as what's in it, and the cursor changes nothing in it: everything else floats by it. Prototype: [prototype/call-widget-5.html](prototype/call-widget-5.html) (round 5; round 4 is [prototype/call-widget.html](prototype/call-widget.html)).

- **The row** shows up to three people who were active last (spoke, unmuted or joined). When someone new speaks and the row is full, they take the place of whoever has been quiet longest; the others stay where they are. **You** are always last in the row (a red badge while muted) and don't count toward the three, so muting moves nothing.
- **The stack**, a grey pill with one dimmed face, the next two peeking out behind it and "+N", holds everyone else muted or quiet for two minutes. Click it for a card with everyone in it (name, mic state). In a call of two or three, you included, nobody goes to the stack: muting only adds the badge.
- **Alone**, a dashed seat sits where the first guest will. Click it to copy the link.
- **Cameras** show as **bubbles**, each a window of its own: a circle up to 180 px, then a tile that widens to 4:3. They ride on the pill in a row ending at its right end (beside a column, from its foot up). Drag one away and it floats by itself; drop it by the pill (the pill's edge lights up) and it rides again; drop it by the screen's edge and it lines up with it. A bubble has no border of its own, only the green ring while that person speaks. One that closes (a camera off, a video hidden) rides on the pill when it's back. Scroll or pinch over one to zoom it, double-click for big or small. A button under the cursor turns your camera off, or puts their video out of sight. Faces in the pill are always avatars, so a camera turning on moves nothing in it.
- **A face is that person's video switch.** Click yours and your camera starts or stops. Click anyone else's with a camera on and their bubble hides (a small camera-off badge on the face) or comes back. Pressed and moved, a face drags the widget like any other spot.
- **In a call of four or more, one bubble follows the speaker**, staying on each person at least three seconds; click a face to put that person in it. Settings › Call can give everyone in the row a bubble instead. In a call of two or three, everyone with a camera on has one.
- **Four buttons, always**: ⋯, Share, Mic, Leave (a handset laid down; the red says the rest). ⋯ opens a card with the room's name and the call's time, then Camera, Invite, Call window, Record and Transcribe.
- **Transcription**: *Transcribe* asks in the same card: which model will run ("Parakeet v3 on this Mac. Everyone sees it.") with *Start*, or *Download 740 MB and start* when the model isn't here yet, and *Settings*. When others' Macs have a model, it asks "Transcribe on whose Mac?" with *This Mac* and up to two names; asking someone puts "Tester asks your Mac to transcribe" with *Accept* and *Not now* by their pill (Settings can accept by itself). If the transcriber leaves, everyone who can carry on gets "Anna left. Transcribe here?". While anyone transcribes, a captions button with a red dot sits left of ⋯ in everyone's widget (right of the record button), and Transcribe leaves the menu: one transcript per call. Click it for "On your Mac · Parakeet v3" or "On Anna's Mac · Parakeet v3", with *Transcript* (opens or closes the transcript window) and *Stop*, which anyone may press. The download's progress shows there too, with Cancel. A dropped connection is tried twice more by itself; if the download still fails, a card by the pill (and the call window's title bar) says "Download failed" with *Settings*, where *Download* goes on from where it stopped.
- **The transcript window** is a floating panel like a share window (above other apps, on all Spaces, never takes focus), opened by itself for everyone when a transcript starts (Settings can turn that off). A chat: others on the left with their name over each turn, you on the right, joins, leaves, shares and recordings as dividers. A line being spoken is grey and grows about once a second, then turns solid. It follows the end while you're there; scroll up to read. ⋯ has Copy transcript, Hand over to … (each Mac that can, while yours transcribes) and Stop for everyone. Drag it by any empty spot, resize it from its corner, close it with ×: the transcript goes on.
- **Recording**: *Record* asks in the same card: *Audio* or *Audio and screen*. Audio records everyone's voice on your Mac (Movies › Cubie). Audio and screen opens the share picker, titled "Record your screen", and records that screen or window with everyone's voice (an .mp4). While anyone records, a red record button sits left of ⋯ in everyone's widget, and Record leaves the menu: one recording per call. Click the red button for "Recording audio · 4:12" with Stop, or "Anna is recording. Only Anna can stop it." Others get a notice when a recording starts; you get "Recording saved" with Show in Finder when it stops. Design: [prototype/transcription.html](prototype/transcription.html).
- **Hovering a face** for a moment shows the name and what they're doing ("speaking", "active 40 s ago") in a small card over the pill. It's the only thing that opens on hover.
- **While you share**, your screen's live preview is a bubble too (once its first frame arrives, in its shape), with a Stop button.
- **Someone sharing** has a screen badge on their face; click it to bring their share window to the front. Anyone sharing stays in the row.
- **The pill never moves**: the window grows around it, and it gets longer or shorter at the end away from its buttons, so Share, Mic and Leave stay under the cursor. Bubbles and the caption ride over it (under it near the top of the screen). A card opens over the pill when nothing else is there, and on the other side when bubbles or the caption are, so they stay put too; where the screen ends first it opens over the bubbles. A card closes on a click anywhere else, or once the cursor has been away from the widget for five seconds: the widget never has focus to lose.
- **At the screen's sides the pill stands as a column.** Along every edge of the screen runs a band a tenth of the screen deep (of its width at the sides, of its height at the top and bottom). Drop the pill with the cursor in one and it goes to stand by that edge: a column at a side, lying down at the top or bottom. One that already lies that way keeps its place along the edge. Pause while dragging and a dashed outline shows what a drop would make. The edges are those of the display under the cursor, and where two displays meet is no edge: the pill is only on its way across. By a column, bubbles, the caption and cards open on the side with the screen.
- **Dropped in the inner half of the band** (a twentieth of the screen) the widget tucks away as a tab with a dot per person in the row: green while they speak, red for you muted. What rode on the pill goes with it. Click the tab and the call comes back.
- **A pill by an edge stays with it.** One that stands by an edge is inside its band already, and by a corner inside the next edge's too. So it slides along its edge, into the corners as well, and comes away from it, as it is; pushed 20 px into the edge (or as far as the screen goes), it tucks. Slid on along its edge until it hangs 20 px out over the next one, it's that edge's. Once it has come away, another edge takes it when the cursor comes 20 px back towards that edge: lifted straight up out of a corner the pill has only moved, and brought out from the side and back to it, it stands there as a column. Where two bands cross, the edge the cursor is deeper into its band counts.
- **The caption line**: while the call is transcribed, the sentence being said shows in a slim line by the pill, with who says it and the newest words ("Listening…" until someone speaks). Settings › Call turns it off.
- **Hold to talk**: while muted, hold the mic button (a quarter of a second or more) or the right Option key, and you're unmuted until you let go; the button is green meanwhile. A short click still unmutes for good. The key talks only held by itself for a fifth of a second: Option is also how "@" and "é" are typed and how files are copied with a drag, so a key or a mouse button pressed with it keeps you muted until Option comes up. Clicking the mic while the key talks mutes you, and the key stays quiet until it has come up.
- **"You're muted"**: a reminder by the pill, a ring on the mic button and one soft beep, at most every five minutes, when you talk while muted. *The reminder, its beep and its limit are built; nothing calls it yet: hearing you while muted means keeping the microphone open, and today it closes when you mute.*
- **Double-click** an empty spot for the call window. **Right-click** for the menu as macOS draws it: Mute, Camera, Share Screen, Copy Invite Link, Open Call Window, Leave Call.
- "Joining…", permission prompts, questions and errors show in a slim card by the pill.
- **Drag** the widget by any empty spot.
- Someone else's **screen share** floats as its own rounded rectangle you can move and resize. **Double-click** it, or press ⤢ on it, to watch it **fullscreen** (macOS fullscreen in its own Space). Esc, a double-click or ⤡ brings it back. When the share ends it says so and closes a moment later, but only once you're on its Space: macOS can't take a window out of fullscreen from another Space, and closing it anyway left a black Space that ignored every key and click.
- **Screen share** opens a picker with live thumbnails of your screens and windows and the frame-rate presets (5 / 15 fps for text, 30 / 60 fps for motion). Clicking share again stops sharing.

## Call: regular mode

One window (1040 × 680): the room's name and the call's time in the title bar, with "Invite" (opens Room access). Tiles sit on a dark stage, a control bar floats under them, and a side panel opens on the right.

- **The layout picks itself**: alone, a card with your face and the call link; 2–4 people, a grid; more, whoever is speaking big with a strip of the others (as many as fit, then "+N"); anyone sharing, the share big and faces in a column. Your own share gets the big tile when nobody else shares.
- **Controls**: mic, camera, share, record, transcribe | side panel, minimal | Leave. Record opens a small menu over the toolbar (Audio, Audio and screen); pressed again while you record, it stops. While the call is recorded, the title bar shows "REC · 4:12" (yours) or "Anna is recording".
- **Side panel**: *Transcript* (the call's timeline with the transcript as a chat, as in the transcript window) and *People* (the call link and everyone, with their mic state, and *Ask to transcribe* or *Hand over* next to those whose Mac can transcribe).
- **Transcribe** sits after Record in the toolbar: it asks to start (or download first), stops the transcript, or cancels the download. The title bar shows "Transcribing" or "Anna's Mac transcribes".
- Double-click someone's share for fullscreen. Closing the window doesn't end the call; it goes back to the widget. Leaving is the red button.

## Decisions so far (easy to change)

- Dark only, with today's blue accent. Colours are named by use in `theme.rs`, so a light set can follow the system later.
- Avatar colours come from the name, not the call identity, so people keep their colour from call to call.
- One pill for everyone's faces instead of a window per face; a window only per camera on show, which gets the compositor's free circle clip. In a call of four or more that's one bubble, not ten.
- The widget is solid, not glass: it reads the same over a white page and a dark editor.
- Recording, files and the device menus (mic/speaker choice, noise filter) wait for their backends; the prototype shows where they go.
- Rooms and access are stored on the server and cached on this Mac. Call history, transcripts and recordings remain local.
- Your own camera isn't mirrored yet (GPUI can't flip video).
- The share picker needs macOS's Screen Recording permission once.
- Shares are always H.264, for both text and motion (VP9 cost too much; see FOOTPRINT.md).

## How it's built

- The widget, each bubble and each floating share are borderless, non-activating panels (GPUI `WindowKind::PopUp`): above other apps, on all Spaces, never stealing focus. The widget's window is transparent and resizes around its pill, keeping the pill's bottom-right corner where it is (`mac::Anchor::Point`); the pill, the caption and the cards each draw their own solid background, so nothing behind the window is blurred. While a card is open the window covers the gap beside it too, and clicks there don't reach the app underneath.
- A bubble is a window, so the compositor cuts its video to a circle or a tile for free (one draw, no bands). The ones riding on the pill are its child windows, under it: macOS moves them with the pill, and a card opens over them. The widget (`capsule.rs`) decides which bubbles exist and where the riding ones go; a bubble (`bubble.rs`) only draws and passes on what's done to it. The pill is drawn again only when what it shows of someone changes (speaking, muted, camera), not with every frame of video; each bubble repaints with its own frames.
- macOS moves a dragged window by itself and doesn't say when the button goes up, so while the pill or a bubble is dragged the widget looks at the cursor 25 times a second: that's how a pause shows the outline and a drop turns, tucks or docks. The outline is a small window of its own, opened the first time it's needed.
- Hold to talk's key needs no permission: an app without focus gets no key events, so the key's state is read instead (`CGEventSourceKeyState`, and how long ago an ordinary key or a mouse button last went down), 20 times a second while you're muted, every two seconds while it's off in Settings. The right-click menu is an `NSMenu` opened from the run loop itself: inside a GPUI event its own event loop would call back into GPUI.
- The transcript window keeps the glass: the system blur behind the window (GPUI's `Blurred` background, an `NSVisualEffectView`) under a dark tint (78%). The blur adds no colour, so over a white page the tint is what keeps it dark.
- GPUI clips video only to rectangles, so rounded tiles and previews are drawn in horizontal bands, each clipped to the shape's width, under an opaque ring that hides the steps (`rounded_video` in `ui.rs`): about 10 draws for a widget tile. Patching GPUI's surface shader for rounded corners would make it one draw, but that means vendoring GPUI itself.
- The text field is GPUI's `input.rs` example, adapted (`text_input.rs`).
- Screenshots and screen recordings show our floating windows. Our own screen shares leave every Cubie window out through their ScreenCaptureKit filter, so nobody sees the widget (or a preview of the preview) in your share.
- Resizing draws the new frame in the same AppKit transaction, so the widget never shows its old frame stretched to the new size. That needed two GPUI patches (see `vendor/README.md`).
- No widget library: the look is custom, and every dependency costs footprint. Shared pieces (avatars, buttons, tooltips, empty states) are in `ui.rs`.
- Icons: [Lucide](https://lucide.dev) (ISC licence), embedded as SVG.

## Phones

Cubie for iPhone (bundle `com.cubiecall.ios`) looks like iOS, not like the Mac app: the system's lists, search fields, bars, buttons and colours, light or dark as the phone is set. Only faces (initials on a person's colour, the same colour as here) and the video tiles are drawn by us. Four tabs:

- **Calls** (where the app opens, once there are any): the calls you were in on this phone, newest first, as the Phone app lists them: the room, who was there and for how long, when. A call nobody joined is red. Tap one for its page; swipe to delete; Edit to clear.
- **Rooms**: your rooms, searchable; + makes one. Tap a room for its page; the small round button on the row calls at once; swipe or long-press to share the link.
- **Contacts**: your address book, searchable by name, email or number. People with an account come first, under "On Cubie", with a call button; everyone else has **Invite**, which opens the system's share sheet (Messages, Telegram, WhatsApp, Mail…) with a link to a room for the two of you (it opens in a browser, no app needed). Until calls can ring, calling someone on Cubie sends them that link first, then takes you into the call.
- **Settings**: you (tap to change your name), and Sign Out.

Signing in is the same two steps as on the Mac, in the system’s own field and button: your email, then the 6-digit code, which iOS offers from Mail above the keyboard. A new account is asked what people should call it before the tabs open.

**A room opens as a group chat**, the way Telegram and WhatsApp show one. The room's face and name are in the bar, with Share and Call. The history runs oldest to newest and opens at the newest end:

- A day's name where a new day begins, and around each call two centred lines: "Call started · 21:44" and "Call finished · 21:45 · 33 sec" (or that nobody joined).
- What was said, when someone's Mac transcribed the call, as chat bubbles: yours on the right in the tint colour, everyone else's on the left in grey, under their name in their own colour, with a face beside the last bubble of a run. Each has its time and a small captions mark: it was transcribed, not typed.
- Who joined or left, who started or stopped sharing a screen or recording, and the transcript starting and stopping, as small centred grey lines between the bubbles; the share, recording and transcript lines lead with their symbol. The "Call finished" line ends with the same three symbols for whichever of them the call had, so a call can be sized up without reading it.

There's nothing to type into yet: messages and files will be bubbles in the same chat once Cubie has them, and a recording is only a line, since the file stays on the recorder's Mac. Bubbles are the one thing here iOS has no component for, so those two cells are ours, in the system's colours and type.

**A call's page** (from the Calls tab) is a plain grouped list: its date, length and people, what happened in it, the earlier calls in the room under "Previous Calls", a way into the room's chat, and **Call** and **Share** on top.

The call is full screen and dark, as the system's own call screens are: a tile per person (a row each up to three, two across from four, a shared screen on top), the room's name and the time, an invite button, and five round buttons (speaker, camera, microphone, flip, leave). White means on and worth noticing.

Earlier drawings: [prototype/mobile.html](prototype/mobile.html) (round 1, in a dark hand-drawn look that was dropped) still shows the screens not built yet: a room's page, a person's, the incoming call, a call of two, the minimised call.

## Settings

⌘, or the gear next to your name on Home. Three tabs:

- *Transcription*: the language spoken (Auto-detect, or one of thirteen, which suggests the model that knows it best and greys out the others), the model (Parakeet v3 or Qwen3-ASR, each with what it's good at, whether it's on this Mac, and Download, Cancel or Remove), *Try it with your voice* (up to 30 s of your microphone through the model, drafts in grey, nothing kept), whether the transcript window opens by itself, whether this Mac starts when someone asks, and the models' credits.
- *Recording*: what Record offers first (Audio, or Audio and screen), each voice in its own file too, and the folder (Show in Finder, Change…).
- *Call*: hold to talk, the beep when you talk while muted, the caption line by the widget, and others' video in calls of four or more (follows the speaker, or a bubble each). All on by default, following the speaker.

Home’s profile row shows your name and email; its Account button opens **Change your name** and **Sign out** (which goes back to the sign-in window). Errors remain visible there. The room list refreshes when Home gains focus. Opening a room link preserves the Mac’s signed-in identity; the sign-in window opens only for missing or expired credentials, and the call is joined once you’re in. Unsaved calls show the server-provided room name in the call and history.
