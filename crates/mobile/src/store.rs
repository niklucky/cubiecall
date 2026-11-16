//! What the phone keeps for itself: the calls you made (the Calls tab) and
//! each contact's own room. One small JSON file, written whole on every
//! change (a tap's worth of them a day), into a new file first and renamed
//! into place, so a crash never leaves half of one.
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

#[derive(Default, Serialize, Deserialize)]
struct Data {
    /// Newest first.
    #[serde(default)]
    calls: Vec<CallRecord>,
    /// A contact (the address book's identifier) → the room for calls with
    /// them.
    #[serde(default)]
    lines: HashMap<String, String>,
}

pub(crate) struct Store {
    /// Where it's kept; nowhere in tests.
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

impl Store {
    /// The store for `server` under `dir`: test and real servers each have
    /// their own.
    pub(crate) fn open(dir: &str, server: &str) -> Self {
        let name: String = server
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

    pub(crate) fn add_call(&self, call: CallRecord) {
        self.change(|data| {
            data.calls.insert(0, call);
            data.calls.truncate(KEEP);
        });
    }

    pub(crate) fn remove_call(&self, id: u64) {
        self.change(|data| data.calls.retain(|c| c.id != id));
    }

    pub(crate) fn clear_calls(&self) {
        self.change(|data| data.calls.clear());
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
    }

    fn change(&self, change: impl FnOnce(&mut Data)) {
        let mut data = self.data.lock();
        change(&mut data);
        let Some(path) = &self.path else { return };
        let written = serde_json::to_vec(&*data)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let new = path.with_extension("json.new");
                std::fs::write(&new, bytes)?;
                std::fs::rename(&new, path)
            });
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
        let dir = std::env::temp_dir().join(format!("quipcall-store-{}", std::process::id()));
        let dir = dir.to_str().unwrap();
        let store = Store::open(dir, "http://127.0.0.1:8180");
        store.wipe();
        store.add_call(call(1000));
        store.add_call(call(2000));
        store.set_line("contact-1", "room-1");
        store.remove_call(1000);

        let again = Store::open(dir, "http://127.0.0.1:8180");
        assert_eq!(again.calls(), [call(2000)]);
        assert_eq!(again.line("contact-1").as_deref(), Some("room-1"));
        // Another server's store is another file.
        assert!(Store::open(dir, "https://quipcall.com").calls().is_empty());

        again.wipe();
        assert!(Store::open(dir, "http://127.0.0.1:8180").calls().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_newest_calls_are_kept() {
        let store = Store::in_memory();
        for id in 0..(KEEP as u64 + 5) {
            store.add_call(call(id));
        }
        let calls = store.calls();
        assert_eq!(calls.len(), KEEP);
        assert_eq!(calls[0].id, KEEP as u64 + 4);
    }
}
