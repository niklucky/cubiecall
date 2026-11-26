// Release builds on Windows are a GUI app: no console window beside it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// A shortcut with the system's main modifier: ⌘ on the Mac, Ctrl on
/// Windows. GPUI's `cmd-` is the Windows key there.
macro_rules! primary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

mod assets;
mod auth;
mod bubble;
mod call;
mod capsule;
mod fullscreen;
mod geometry;
mod handle;
mod home;
mod models;
mod onboarding;
/// Window shapes and placement, the cursor and system sounds: what GPUI
/// doesn't expose, from AppKit or Win32.
#[cfg_attr(target_os = "macos", path = "platform_mac.rs")]
#[cfg_attr(windows, path = "platform_win.rs")]
mod platform;
/// Cmd+Q's hold or second press. Windows apps quit from their windows.
#[cfg(target_os = "macos")]
mod quit;
mod regular;
mod rooms;
mod screen_window;
mod settings;
/// ScreenCaptureKit's screens and windows, with previews. Elsewhere sharing
/// takes the main screen (`Call::toggle_share`).
#[cfg(target_os = "macos")]
mod share_picker;
mod sign_in;
#[cfg(feature = "snapshot")]
mod snapshot;
mod store;
#[cfg(test)]
mod test_support;
mod text_input;
mod theme;
mod timeline;
mod transcript;
mod ui;
#[cfg(target_os = "macos")]
mod update;
/// Decoded video frames, as each platform's GPUI draws them.
#[cfg_attr(target_os = "macos", path = "video_mac.rs")]
#[cfg_attr(windows, path = "video_win.rs")]
mod video;

use std::io::IsTerminal as _;

use gpui::{App, AppContext as _, Context, Entity, Global, Subscription, WindowHandle};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::call::{Call, Ended, RoomRef};
use crate::home::Home;
use crate::store::Store;

/// Launch options. Login and contacts replace the dev parts in Stage 4.
#[derive(Clone)]
pub struct Args {
    pub session: Option<String>,
    pub server: String,
    /// Base URL of the web client, for call links.
    pub web: String,
    pub name: String,
    /// `--name` was given: don't ask for a name on first launch.
    pub name_given: bool,
    /// Join this room right away instead of showing home (dev and tests).
    pub room: Option<String>,
    /// Turn the camera on as soon as the call starts.
    pub camera: bool,
    /// Start calls in regular mode instead of minimal.
    pub regular: bool,
}

impl Args {
    fn web_origin(&self) -> &str {
        auth::web_origin(&self.server, &self.web)
    }
}

/// The release build's primary server.
const SERVER: &str = match option_env!("CUBIE_SERVER") {
    Some(url) => url,
    None => "http://127.0.0.1:8080",
};

/// Where call links point. Deployed, the web client shares the server's origin.
const WEB: &str = match (option_env!("CUBIE_WEB"), option_env!("CUBIE_SERVER")) {
    (Some(url), _) | (None, Some(url)) => url,
    (None, None) => "http://127.0.0.1:5173",
};

fn parse_args() -> Args {
    let mut args = Args {
        session: None,
        server: SERVER.into(),
        web: WEB.into(),
        name: default_name(),
        name_given: false,
        room: None,
        camera: false,
        regular: false,
    };
    let mut it = std::env::args().skip(1);
    let mut explicit_server = false;
    let mut explicit_web = false;
    let mut reserve = None;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--camera" => args.camera = true,
            "--regular" => args.regular = true,
            "--server" | "--reserve" | "--web" | "--room" | "--name" => {
                let Some(value) = it.next() else { usage() };
                match flag.as_str() {
                    "--server" => {
                        args.server = value;
                        explicit_server = true;
                    }
                    "--reserve" => reserve = Some(value),
                    "--web" => {
                        args.web = value;
                        explicit_web = true;
                    }
                    "--room" => args.room = Some(value),
                    _ => {
                        args.name = value;
                        args.name_given = true;
                    }
                }
            }
            _ => usage(),
        }
    }
    if explicit_server && !explicit_web {
        args.web = args.server.clone();
    }
    if !explicit_server && reserve.is_none() {
        reserve = option_env!("CUBIE_RESERVE_URL")
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
    }
    if let Some(reserve) = reserve {
        let primary = args.server.trim_end_matches('/');
        let reserve = reserve.trim_end_matches('/');
        if !https_origin(primary) || !https_origin(reserve) || primary.eq_ignore_ascii_case(reserve)
        {
            eprintln!("Failover requires two distinct HTTPS origins.");
            usage();
        }
        let _ = auth::FAILOVER.set(cubie_core::http::Failover::new(primary, reserve));
    }
    args.session = auth::load(&args.server);
    #[cfg(feature = "snapshot")]
    if let Ok(path) = std::env::var("CUBIE_TEST_SESSION_FILE") {
        let url = cubie_core::http::Url::parse(&args.server).expect("test server URL");
        assert!(
            matches!(url.host_str(), Some("127.0.0.1" | "localhost")),
            "test sessions are local only"
        );
        args.session = Some(
            std::fs::read_to_string(path)
                .expect("test session")
                .trim()
                .into(),
        );
    }
    args
}

fn https_origin(value: &str) -> bool {
    cubie_core::http::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.path() == "/"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && value.eq_ignore_ascii_case(&url.origin().ascii_serialization())
    })
}

/// The account's full name ("Jane Appleseed"), else the login name.
pub(crate) fn default_name() -> String {
    #[cfg(target_os = "macos")]
    {
        let full = objc2_foundation::NSFullUserName().to_string();
        if !full.trim().is_empty() {
            return full.trim().to_owned();
        }
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "desktop".into())
}

/// The room in a call link: `cubie://join/<room>`.
fn room_from_link(url: &str) -> Option<String> {
    let (scheme, path) = url.split_once("://")?;
    if !matches!(scheme, "cubie" | "quipcall") {
        return None;
    }
    let room = path
        .strip_prefix("join/")
        .or_else(|| path.strip_prefix("open/"))?;
    let room = room.split(['/', '?', '#']).next()?;
    cubie_protocol::is_valid_room_id(room).then(|| room.to_owned())
}

fn usage() -> ! {
    eprintln!(
        "usage: cubie-desktop [--server URL] [--reserve URL] [--web URL] [--name NAME] [--room ROOM] [--camera] [--regular]"
    );
    std::process::exit(2);
}

/// App-wide state: the home window, the current call and local data.
pub struct Shell {
    pub principal: Option<auth::Principal>,
    pub room_creation: auth::PendingOperation,
    pub auth_attempt: auth::SignInAttempt,
    pub auth_message: String,
    /// The call a link asked for, joined once signed in and named.
    pending_room: Option<String>,
    pub args: Args,
    pub store: Store,
    call: Option<(Entity<Call>, Subscription)>,
    home: Option<WindowHandle<Home>>,
    home_pending: bool,
    quitting: bool,
}

struct GlobalShell(Entity<Shell>);
impl Global for GlobalShell {}

impl Shell {
    pub fn global(cx: &App) -> Entity<Shell> {
        cx.global::<GlobalShell>().0.clone()
    }

    pub fn in_call(&self) -> bool {
        self.call.is_some()
    }

    pub fn call(&self) -> Option<Entity<Call>> {
        self.call.as_ref().map(|(call, _)| call.clone())
    }

    pub fn quick_call(&mut self, cx: &mut Context<Self>) {
        self.create_room("direct", true, cx);
    }

    pub fn join_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(room) = self.store.rooms.iter().find(|r| r.id == id) {
            let room = RoomRef {
                id: room.id.clone(),
                name: room.name.clone(),
                one_time: false,
            };
            self.start_call(room, cx);
        }
    }

    /// Join the room of a call link someone sent.
    pub fn join_link(&mut self, id: String, cx: &mut Context<Self>) {
        if self.in_call() {
            tracing::info!("ignoring link to {id}: already in a call");
            return;
        }
        let saved = self.store.rooms.iter().find(|r| r.id == id);
        let room = RoomRef {
            name: saved.map_or_else(|| "Call".into(), |r| r.name.clone()),
            one_time: saved.is_none(),
            id,
        };
        self.start_call(room, cx);
    }

    pub fn rename_room(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        if self.store.is_bookmarked(id) && !self.store.owns_room(id) {
            self.store.rename_bookmark(id, name);
            self.store.save();
            cx.notify();
        } else {
            self.rename_remote_room(id, name, cx);
        }
    }

    /// Save a link locally, first making an owned Quick call reusable.
    pub fn keep_room(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let Some(principal) = self.store.rooms_principal.clone() else {
            self.auth_message = "Sign in before saving a room.".into();
            cx.notify();
            return;
        };
        let args = self.args.clone();
        let id = id.to_owned();
        let name = name.to_owned();
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<crate::store::SavedRoom> = async {
                let detail = auth::request(
                    &args.server,
                    args.session.as_deref(),
                    &format!("/rooms/{id}"),
                    None,
                    "GET",
                )
                .await?;
                let mut room: crate::store::SavedRoom =
                    serde_json::from_value(detail["room"].clone())?;
                if room.owner == principal && room.expires.is_some() {
                    room = serde_json::from_value(
                        auth::request(
                            &args.server,
                            args.session.as_deref(),
                            &format!("/rooms/{id}/keep"),
                            Some(serde_json::json!({})),
                            "POST",
                        )
                        .await?,
                    )?;
                }
                Ok(room)
            }
            .await;
            let _ = this.update(cx, |s, cx| {
                if s.args.session != args.session {
                    return;
                }
                match result {
                    Ok(room) => {
                        s.store.keep_room(&id, &name);
                        s.store.cache_room(room);
                        s.store.save();
                        s.auth_message.clear();
                        home::with_home(cx, move |home, window, cx| {
                            home.select_room(id.clone(), cx);
                            home.start_rename(id.clone(), window, cx);
                        });
                    }
                    Err(e) => {
                        if !s.recover_auth(&e, args.session.as_deref(), cx) {
                            s.auth_message = format!("Could not save room: {e}");
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Call a room from history again (same room, same link).
    pub fn join_again(&mut self, id: &str, name: &str, one_time: bool, cx: &mut Context<Self>) {
        self.start_call(
            RoomRef {
                id: id.to_owned(),
                name: name.to_owned(),
                one_time,
            },
            cx,
        );
    }

    /// The call link for a room.
    pub fn link_for(&self, id: &str) -> String {
        call::link(self.args.web_origin(), id)
    }

    pub fn set_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if self.args.session.is_some() {
            // Shown at once: a call that starts now already carries it.
            self.args.name = name.to_owned();
            self.rename_profile(name, cx);
            cx.notify();
            return;
        }
        self.args.name = name.to_owned();
        self.store.name = Some(name.to_owned());
        self.store.save();
        cx.notify();
    }

    fn start_call(&mut self, room: RoomRef, cx: &mut Context<Self>) {
        if self.args.session.is_none() {
            self.sign_in(Some(room.id), cx);
            return;
        }
        if self.call.is_some() {
            return;
        }
        let call = Call::start(room, &self.args, cx);
        let subscription = cx.subscribe(&call, |shell, _, ended: &Ended, cx| {
            shell.call_ended(ended.clone(), cx)
        });
        self.call = Some((call, subscription));
        // The call's windows open first (deferred), then home goes away.
        if let Some(home) = self.home.take() {
            cx.defer(move |cx| {
                let _ = home.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    fn call_ended(&mut self, ended: Ended, cx: &mut Context<Self>) {
        self.store.record(&ended);
        self.store.save();
        // Keep the model alive until its windows have closed.
        let call = self.call.take();
        cx.defer(move |_| drop(call));
        self.open_home(cx);
        cx.notify();
    }

    /// Quitting mid-call: leave properly, or the server shows a ghost of you
    /// in the room for another 20 s. Resolves once it has heard you leave.
    fn leave_for_quit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl Future<Output = ()> + use<>> {
        self.quitting = true;
        let call = self.call()?;
        Some(call.update(cx, |call, cx| {
            call.leave_now(cx);
            call.closed()
        }))
    }

    /// Deferred: the home window reads the shell while it's being built.
    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        if self.home.is_some() || self.home_pending || self.quitting {
            return;
        }
        self.home_pending = true;
        let shell = cx.entity();
        cx.defer(move |cx| {
            let home = home::open(cx);
            shell.update(cx, |s, _| {
                s.home = home;
                s.home_pending = false;
            });
        });
    }
}

/// Logs go to the terminal, or to ~/Library/Logs/Cubie/cubie.log when
/// started from Finder (`%LOCALAPPDATA%\Cubie\Logs\cubie.log` on Windows).
/// Each launch starts a fresh file; the last one is kept.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cubie_desktop=debug,cubie_core=debug,livekit=info,warn".into());
    // DeepFilterNet warns of "clipping" on every frame above full scale, from
    // the audio thread. Our input gain puts loud voices there on purpose, and
    // the model works in floating point, so nothing clips. A separate filter,
    // so no RUST_LOG directive brings it back.
    let quiet_model = tracing_subscriber::filter::filter_fn(|meta| {
        let model = meta.target() == "df" || meta.target().starts_with("df::");
        !model || *meta.level() <= tracing::Level::ERROR
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let file = if std::io::stderr().is_terminal() {
        None
    } else {
        log_file()
    };
    match file {
        Some(file) => builder
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .finish()
            .with(quiet_model)
            .init(),
        None => builder.finish().with(quiet_model).init(),
    }
    // Panics would otherwise vanish with stderr.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{info}");
        default_hook(info);
    }));
}

fn log_file() -> Option<std::fs::File> {
    #[cfg(windows)]
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Cubie\\Logs");
    #[cfg(not(windows))]
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/Cubie");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("cubie.log");
    let _ = std::fs::rename(&path, dir.join("cubie.previous.log"));
    std::fs::File::create(path).ok()
}

fn finish_call_before_quit(
    closed: Option<impl Future<Output = ()>>,
    cx: &App,
) -> std::future::Ready<()> {
    // GPUI only gives the returned future 200 ms, but final transcripts
    // alone may take two seconds before LiveKit gets our goodbye. Wait
    // here, before returning that future, while the engine's own runtime
    // keeps working. Bound the wait so an offline server can't hold up quit.
    if let Some(closed) = closed {
        match cx
            .foreground_executor()
            .block_with_timeout(std::time::Duration::from_secs(3), closed)
        {
            Ok(()) => tracing::info!("left the call before quitting"),
            Err(_) => tracing::warn!("leaving the call before quitting timed out"),
        }
    }
    std::future::ready(())
}

fn main() {
    init_logging();
    let args = parse_args();
    tracing::info!(
        "cubie {} ({}), server {}",
        option_env!("CUBIE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        std::env::consts::ARCH,
        args.server
    );

    let app = gpui_platform::application().with_assets(assets::Assets);
    // Call links (`cubie://join/<room>`) arrive here, before or after launch.
    let (links_tx, links) = async_channel::unbounded::<String>();
    app.on_open_urls(move |urls| {
        for url in urls {
            let _ = links_tx.try_send(url);
        }
    });
    app.run(move |cx: &mut App| {
        let store = Store::load();
        let mut args = args.clone();
        // A Mac without an account starts at sign-in. `--name` is how tests
        // and development look at home without one.
        let signed_out = args.session.is_none() && !args.name_given && args.room.is_none();
        if !args.name_given
            && let Some(name) = &store.name
        {
            args.name = name.clone();
        }
        let shell = cx.new(|_| Shell {
            principal: None,
            room_creation: auth::PendingOperation::default(),
            auth_attempt: auth::SignInAttempt::default(),
            auth_message: String::new(),
            pending_room: None,
            args,
            store,
            call: None,
            home: None,
            home_pending: false,
            quitting: false,
        });
        cx.set_global(GlobalShell(shell.clone()));
        shell.update(cx, |s, cx| s.refresh_account(cx));
        #[cfg(target_os = "macos")]
        share_picker::bind_keys(cx);
        settings::bind_keys(cx);
        fullscreen::bind_keys(cx);
        text_input::bind_keys(cx);
        #[cfg(target_os = "macos")]
        quit::start(cx);
        platform::log_window_events();

        shell.update(cx, |shell, cx| match shell.args.room.clone() {
            Some(id) => shell.start_call(
                RoomRef {
                    name: id.clone(),
                    id,
                    one_time: false,
                },
                cx,
            ),
            None if signed_out => shell.sign_in(None, cx),
            None => shell.open_home(cx),
        });

        // Quit once home is closed and there's no call. Not merely when no
        // window is open: switching call modes swaps windows.
        cx.on_window_closed(|cx, _| {
            let shell = Shell::global(cx);
            let s = shell.read(cx);
            if cx.windows().is_empty() && !s.in_call() && !s.home_pending {
                cx.quit();
            }
        })
        .detach();

        cx.on_app_quit(|cx| {
            let closed = Shell::global(cx).update(cx, |shell, cx| shell.leave_for_quit(cx));
            finish_call_before_quit(closed, cx)
        })
        .detach();

        cx.spawn(async move |cx| {
            while let Ok(url) = links.recv().await {
                match room_from_link(&url) {
                    Some(room) => cx.update(|cx| {
                        Shell::global(cx).update(cx, |s, cx| {
                            if s.in_call() {
                                return;
                            }
                            s.join_link(room, cx);
                        })
                    }),
                    None => tracing::warn!("not a call link: {url}"),
                }
            }
        })
        .detach();

        #[cfg(target_os = "macos")]
        update::start(cx);
        #[cfg(feature = "snapshot")]
        snapshot::start(cx);
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::room_from_link;

    // The snapshot feature enables GPUI's test executor.
    #[cfg(feature = "snapshot")]
    #[gpui::test]
    fn quit_waits_for_a_leave_slower_than_gpuis_shutdown_budget(cx: &mut gpui::TestAppContext) {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::Duration;

        // Shutdown uses a wall-clock deadline; don't fast-forward the timer.
        cx.executor().allow_parking();
        let left = Arc::new(AtomicBool::new(false));
        cx.update(|cx| {
            let left = left.clone();
            cx.on_app_quit(move |cx| {
                let timer = cx.background_executor().timer(Duration::from_millis(350));
                let left = left.clone();
                super::finish_call_before_quit(
                    Some(async move {
                        timer.await;
                        left.store(true, Ordering::SeqCst);
                    }),
                    cx,
                )
            })
            .detach();
            cx.shutdown();
        });
        assert!(
            left.load(Ordering::SeqCst),
            "quit cut off the leave message"
        );
    }

    #[cfg(feature = "snapshot")]
    #[gpui::test]
    fn quit_is_bounded_when_leaving_never_finishes(cx: &mut gpui::TestAppContext) {
        use std::time::{Duration, Instant};

        cx.executor().allow_parking();
        let started = Instant::now();
        cx.update(|cx| {
            cx.on_app_quit(|cx| {
                super::finish_call_before_quit(Some(std::future::pending::<()>()), cx)
            })
            .detach();
            cx.shutdown();
        });
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_secs(3));
        assert!(elapsed < Duration::from_secs(6), "quit took {elapsed:?}");
    }

    #[test]
    fn call_links() {
        let room = |url| room_from_link(url);
        assert_eq!(
            room("cubie://join/quick-1a2b3c4d").as_deref(),
            Some("quick-1a2b3c4d")
        );
        assert_eq!(
            room("cubie://join/room-1-ab12/").as_deref(),
            Some("room-1-ab12")
        );
        assert_eq!(room("cubie://join/spike?x=1").as_deref(), Some("spike"));
        assert_eq!(room("cubie://open/spike").as_deref(), Some("spike"));
        assert_eq!(room("quipcall://join/spike").as_deref(), Some("spike"));
        assert_eq!(room("quipcall://open/spike").as_deref(), Some("spike"));
        assert_eq!(room("cubie://open/"), None);
        assert_eq!(room("cubie://join/"), None);
        assert_eq!(room("cubie://join/a%20b"), None);
        assert_eq!(room("https://cubiecall.com/?room=spike"), None);
    }
}
