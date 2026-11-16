//! Quipcall for phones: everything under the screens.
//!
//! The iOS app (Swift) and, later, the Android app (Kotlin) are views only.
//! Accounts, rooms and the call itself live here, on top of `quipcall-core`,
//! the engine the desktop app runs. UniFFI writes the Swift and Kotlin side of
//! this crate's public items (`dev/ios-core.sh`).
//!
//! - [`Client`]: who you are, your rooms and which contacts are here, over
//!   the server's HTTP API; and the phone's own list of calls.
//! - [`Call`]: one call. Things you do are methods; what happens comes back
//!   through a [`CallListener`].
//! - Video never crosses the bridge: a view hands over its system display
//!   layer and frames go into it on an engine thread (`video.rs`).

mod account;
mod block;
mod call;
mod secrets;
mod store;
#[cfg(target_vendor = "apple")]
mod video;

pub use account::{Account, Client, ContactMatch, Room, RoomKind, SignIn};
pub use call::{Call, CallEvent, CallListener, Quality};
pub use store::CallRecord;

uniffi::setup_scaffolding!();

/// Why something didn't work, in words a person can read.
#[derive(Debug, uniffi::Error)]
pub enum AppError {
    /// The session is gone or expired: sign in again.
    SignedOut {
        message: String,
    },
    Failed {
        message: String,
    },
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignedOut { message } | Self::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for AppError {}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        let signed_out = error
            .downcast_ref::<quipcall_core::http::RequestError>()
            .is_some_and(|e| e.status == 401);
        let message = format!("{error:#}");
        if signed_out {
            Self::SignedOut { message }
        } else {
            Self::Failed { message }
        }
    }
}

/// Logs go to stderr (Xcode's console, `simctl launch --console`), at the
/// level in `QUIPCALL_LOG` (`info` when unset).
fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_env("QUIPCALL_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .try_init();
    });
}
