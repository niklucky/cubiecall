//! Embedded assets: Lucide icons (ISC licence, see assets/icons/LICENSE) and
//! Quby, the mark on the sign-in window.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        // Some are the Mac's share picker's alone.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        pub enum Icon { $($variant),* }

        impl Icon {
            pub fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file, ".svg")),* }
            }
        }

        static FILES: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/icons/", $file, ".svg")))),*
        ];
    };
}

icons! {
    Captions => "captions",
    Check => "check",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    CircleDot => "circle-dot",
    Copy => "copy",
    Download => "download",
    Ellipsis => "ellipsis",
    Link => "link",
    LogIn => "log-in",
    LogOut => "log-out",
    Mail => "mail",
    PanelRight => "panel-right",
    Pause => "pause",
    Play => "play",
    Pencil => "pencil",
    Phone => "phone",
    PhoneDown => "phone-down",
    Trash => "trash-2",
    Upload => "upload",
    User => "user",
    Users => "users",
    Maximize => "maximize-2",
    Mic => "mic",
    Monitor => "monitor",
    AppWindow => "app-window",
    MoveDiagonal => "move-diagonal-2",
    MicOff => "mic-off",
    Minimize => "minimize-2",
    MonitorOff => "monitor-off",
    MonitorUp => "monitor-up",
    PhoneOff => "phone-off",
    Plus => "plus",
    Settings => "settings",
    UserPlus => "user-plus",
    Video => "video",
    VideoOff => "video-off",
    X => "x",
    Zap => "zap",
}

/// Quby in colour, for `gpui::img`: the web client's own file.
pub const QUBY: &str = "quby.svg";
static QUBY_SVG: &[u8] = include_bytes!("../../web/public/favicon.svg");

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == QUBY {
            return Ok(Some(Cow::Borrowed(QUBY_SVG)));
        }
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}
