//! The Cubie phone app: everything under the screens.
//!
//! The iOS app (Swift) and, later, the Android app (Kotlin) are views only.
//! Accounts, rooms and the call itself live here, on top of `cubie-core`,
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
mod log;
mod secrets;
mod store;
#[cfg(target_vendor = "apple")]
mod video;

pub use account::{Account, Client, ContactMatch, Room, RoomKind, SignIn};
pub use call::{Call, CallEvent, CallListener, Quality};
pub use store::{CallRecord, Moment, MomentKind};

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
        let answered = error.downcast_ref::<cubie_core::http::RequestError>();
        let signed_out = answered.is_some_and(|e| e.status == 401);
        // The server never answered (no network, or it's down): say that,
        // not the transport's own words.
        let message = if answered.is_none() && cubie_core::http::transient(&error) {
            tracing::warn!("request failed: {error:#}");
            "Can't reach Cubie. Check your connection and try again.".to_owned()
        } else {
            format!("{error:#}")
        };
        if signed_out {
            Self::SignedOut { message }
        } else {
            Self::Failed { message }
        }
    }
}

/// Logs go to stderr (Xcode's console, `simctl launch --console`), at the
/// level in `CUBIE_LOG` (`info` when unset).
fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_env("CUBIE_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_cant_be_reached_is_said_plainly() {
        // A port nothing listens on.
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", socket.local_addr().unwrap());
        drop(socket);
        let request = cubie_core::http::request(&server, None, "/rooms", None, "GET");
        let error = AppError::from(block::on(request).unwrap_err());
        assert!(matches!(&error, AppError::Failed { .. }));
        assert_eq!(
            error.to_string(),
            "Can't reach Cubie. Check your connection and try again."
        );
    }

    #[test]
    fn what_the_server_says_is_passed_on() {
        let refused = cubie_core::http::RequestError {
            status: 403,
            message: "Only the owner can do that".into(),
        };
        assert_eq!(
            AppError::from(anyhow::Error::from(refused)).to_string(),
            "Only the owner can do that"
        );
        let expired = cubie_core::http::RequestError {
            status: 401,
            message: "Sign in to continue".into(),
        };
        assert!(matches!(
            AppError::from(anyhow::Error::from(expired)),
            AppError::SignedOut { .. }
        ));
    }
}
