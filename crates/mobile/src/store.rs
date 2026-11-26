//! What the phone keeps for itself: the calls you made (the Calls tab) and
//! each contact's own room. One small JSON file, written whole on every
//! change (a tap's worth of them a day), into a new file first and renamed
//! into place, so a crash never leaves half of one.
//!
//! What happened in a call (its [`Moment`]s, a transcript among them) is a
//! file of its own beside it, read when that call is opened: the list stays
//! small in memory however much was said.
//!
//! It belongs to one account: another one signing in on this phone starts
//! with none of it.
//!
//! The server knows nothing of either yet: call records come with its
//! presence (PLAN.md, Stage 3).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The most calls kept; the oldest go first.
const KEEP: usize = 500;

/// A call you were in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct CallRecord {
    /// Its number in the list (when it began, unix milliseconds).
    pub id: u64,
    /// The room's id, to call it again.
    pub room: String,
    /// What the room was called then.
    pub name: String,
    /// When you got in, unix seconds.
    pub started: u64,
    pub seconds: u64,
    /// Everyone else who was there at some point, by name, in the order
    /// they came. Empty: nobody joined.
    pub people: Vec<String>,
}

/// What a [`Moment`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum MomentKind {
    Joined,
    Left,
    /// Someone began sharing their screen, or stopped.
    ShareStarted,
    ShareStopped,
    /// Someone began recording the call (on their Mac), or stopped.
    RecordingStarted,
    RecordingStopped,
    /// Someone's Mac began transcribing the call, or stopped.
    TranscriptStarted,
    TranscriptStopped,
    /// Something was said: a line of the transcript, in `text`.
    Said,
}

/// Something that happened in a call: its history is these, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Moment {
    /// When, unix seconds.
    pub at: u64,
    pub kind: MomentKind,
    /// Whose doing it was, by name.
    pub who: String,
    /// It was you.
    pub mine: bool,
    /// What was said ([`MomentKind::Said`]); empty otherwise.
    pub text: String,
    /// The transcript line's own id, for the engine; empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Data {
    /// Whose it is: the account's id. Nobody's yet until someone signs in.
    #[serde(default)]
    owner: Option<String>,
    /// Newest first.
    #[serde(default)]
    calls: Vec<CallRecord>,
    /// A contact (the address book's identifier) → the room for calls with
    /// them.
    #[serde(default)]
    lines: HashMap<String, String>,
}

pub(crate) struct Store {
    /// Where it's kept; nowhere in tests (which then keep no moments).
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

/// `bytes` into `path` whole, or not at all.
fn write(path: &Path, bytes: Vec<u8>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let new = path.with_extension("json.new");
    std::fs::write(&new, bytes)?;
    std::fs::rename(&new, path)
}

impl Store {
    /// The store for `server` under `dir`: test and real servers each have
    /// their own.
    pub(crate) fn open(dir: &str, server: &str) -> Self {
        let name: String = cubie_core::http::storage_origin(server)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = Path::new(dir).join(format!("store-{name}.json"));
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            data: Mutex::new(data),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            data: Mutex::default(),
        }
    }

    pub(crate) fn calls(&self) -> Vec<CallRecord> {
        self.data.lock().calls.clone()
    }

    /// The calls made in `room`, newest first.
    pub(crate) fn room_calls(&self, room: &str) -> Vec<CallRecord> {
        let data = self.data.lock();
        data.calls
            .iter()
            .filter(|c| c.room == room)
            .cloned()
            .collect()
    }

    /// Where the calls' moments are: a folder beside the file.
    fn moments_dir(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.with_extension("calls"))
    }

    fn moments_path(&self, id: u64) -> Option<PathBuf> {
        self.moments_dir().map(|dir| dir.join(format!("{id}.json")))
    }

    /// What happened in call `id`, in order.
    pub(crate) fn moments(&self, id: u64) -> Vec<Moment> {
        self.moments_path(id)
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn forget_moments(&self, id: u64) {
        if let Some(path) = self.moments_path(id) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn forget_all_moments(&self) {
        if let Some(dir) = self.moments_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    pub(crate) fn add_call(&self, call: CallRecord, moments: Vec<Moment>) {
        if let Some(path) = self.moments_path(call.id).filter(|_| !moments.is_empty()) {
            let kept = serde_json::to_vec(&moments)
                .map_err(std::io::Error::other)
                .and_then(|bytes| write(&path, bytes));
            if let Err(e) = kept {
                tracing::warn!("keeping {}: {e}", path.display());
            }
        }
        let mut dropped = Vec::new();
        self.change(|data| {
            data.calls.insert(0, call);
            // The oldest calls go, and their histories with them.
            dropped = data.calls.split_off(KEEP.min(data.calls.len()));
        });
        for old in dropped {
            self.forget_moments(old.id);
        }
    }

    pub(crate) fn remove_call(&self, id: u64) {
        self.change(|data| data.calls.retain(|c| c.id != id));
        self.forget_moments(id);
    }

    pub(crate) fn clear_calls(&self) {
        self.change(|data| data.calls.clear());
        self.forget_all_moments();
    }

    pub(crate) fn line(&self, contact: &str) -> Option<String> {
        self.data.lock().lines.get(contact).cloned()
    }

    pub(crate) fn set_line(&self, contact: &str, room: &str) {
        self.change(|data| {
            data.lines.insert(contact.to_owned(), room.to_owned());
        });
    }

    /// Signing out takes everything with it.
    pub(crate) fn wipe(&self) {
        self.change(|data| *data = Data::default());
        self.forget_all_moments();
    }

    /// `account` (its id) is signed in: what's here is theirs from now on.
    /// Whatever someone else left (a session that ended without signing
    /// out) goes first.
    pub(crate) fn claim(&self, account: &str) {
        let owner = self.data.lock().owner.clone();
        match owner.as_deref() {
            Some(owner) if owner == account => return,
            Some(_) => self.wipe(),
            None => {}
        }
        self.change(|data| data.owner = Some(account.to_owned()));
    }

    fn change(&self, change: impl FnOnce(&mut Data)) {
        let mut data = self.data.lock();
        change(&mut data);
        let Some(path) = &self.path else { return };
        let written = serde_json::to_vec(&*data)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write(path, bytes));
        if let Err(e) = written {
            tracing::warn!("keeping {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: u64) -> CallRecord {
        CallRecord {
            id,
            room: "r".into(),
            name: "Team".into(),
            started: id / 1000,
            seconds: 60,
            people: vec!["Anna".into()],
        }
    }

    #[test]
    fn calls_and_lines_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("cubie-store-{}", std::process::id()));
        let dir = dir.to_str().unwrap();
        let store = Store::open(dir, "http://127.0.0.1:8180");
        store.wipe();
        let joined = Moment {
            at: 2,
            kind: MomentKind::Joined,
            who: "Anna".into(),
            mine: false,
            text: String::new(),
            key: String::new(),
        };
        store.add_call(call(1000), vec![joined.clone()]);
        store.add_call(call(2000), vec![joined.clone()]);
        store.set_line("contact-1", "room-1");
        store.remove_call(1000);

        let again = Store::open(dir, "http://127.0.0.1:8180");
        assert_eq!(again.calls(), [call(2000)]);
        assert_eq!(again.room_calls("r"), [call(2000)]);
        assert!(again.room_calls("another").is_empty());
        assert_eq!(again.moments(2000), [joined]);
        assert!(again.moments(1000).is_empty());
        assert_eq!(again.line("contact-1").as_deref(), Some("room-1"));
        // Another server's store is another file.
        assert!(
            Store::open(dir, "https://call.example.test")
                .calls()
                .is_empty()
        );

        again.wipe();
        assert!(Store::open(dir, "http://127.0.0.1:8180").calls().is_empty());
        assert!(again.moments(2000).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_custom_server_reopens_existing_history() {
        let dir = std::env::temp_dir().join(format!("cubie-alias-{}", std::process::id()));
        let dir = dir.to_str().unwrap();
        let key = "store-https---legacy-example-test";
        let moment = Moment {
            at: 1,
            kind: MomentKind::Joined,
            who: "Anna".into(),
            mine: false,
            text: String::new(),
            key: String::new(),
        };
        // The filenames were derived from the old default server URL.
        write(
            &Path::new(dir).join(format!("{key}.json")),
            serde_json::to_vec(&serde_json::json!({"calls": [call(1000)]})).unwrap(),
        )
        .unwrap();
        write(
            &Path::new(dir).join(format!("{key}.calls/1000.json")),
            serde_json::to_vec(&[&moment]).unwrap(),
        )
        .unwrap();
        let store = Store::open(dir, "https://legacy.example.test");
        assert_eq!(store.calls(), [call(1000)]);
        assert_eq!(store.moments(1000), std::slice::from_ref(&moment));
        assert!(
            Store::open(dir, "https://other.example.test")
                .calls()
                .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_account_starts_with_nothing() {
        let dir = std::env::temp_dir().join(format!("cubie-owner-{}", std::process::id()));
        let dir = dir.to_str().unwrap();
        let store = Store::open(dir, "http://127.0.0.1:8180");
        store.wipe();
        let said = Moment {
            at: 2,
            kind: MomentKind::Said,
            who: "Anna".into(),
            mine: false,
            text: "Hello".into(),
            key: "anna:1".into(),
        };
        // Calls made before anyone is known to own them become the first
        // account's; signing in again as the same person keeps them.
        store.add_call(call(1000), vec![said.clone()]);
        store.set_line("contact-1", "room-1");
        store.claim("anna");
        store.claim("anna");
        assert_eq!(store.calls(), [call(1000)]);
        assert_eq!(store.moments(1000), [said]);

        // Someone else: none of it, on disk either.
        let again = Store::open(dir, "http://127.0.0.1:8180");
        again.claim("max");
        assert!(again.calls().is_empty());
        assert!(again.moments(1000).is_empty());
        assert!(again.line("contact-1").is_none());
        let reopened = Store::open(dir, "http://127.0.0.1:8180");
        assert!(reopened.calls().is_empty());
        reopened.claim("anna");
        assert!(reopened.calls().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_newest_calls_are_kept() {
        let store = Store::in_memory();
        for id in 0..(KEEP as u64 + 5) {
            store.add_call(call(id), Vec::new());
        }
        let calls = store.calls();
        assert_eq!(calls.len(), KEEP);
        assert_eq!(calls[0].id, KEEP as u64 + 4);
    }
}
