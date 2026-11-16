//! Who you are and your rooms, over the server's HTTP API.
//!
//! Signing in is the desktop app's: the phone shows a code, the browser (the
//! email link, then the code) approves it, and the phone picks up its session
//! by polling with a secret only it has. The session stays in the Keychain.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use quipcall_core::http::{self, request};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::store::{CallRecord, Moment, Store};
use crate::{AppError, secrets};

/// The signed-in person.
#[derive(Debug, Clone, PartialEq, Deserialize, uniffi::Record)]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RoomKind {
    /// A call between two people.
    Direct,
    /// Anyone with the link gets in.
    Open,
    /// Only its members get in.
    Members,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub kind: RoomKind,
    /// You own it: you may rename it, invite to it and close it.
    pub mine: bool,
}

#[derive(Deserialize)]
struct RoomJson {
    id: String,
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    owner: String,
}

impl RoomJson {
    fn room(self, you: Option<&str>) -> Room {
        Room {
            kind: match self.kind.as_str() {
                "direct" => RoomKind::Direct,
                "public" => RoomKind::Open,
                _ => RoomKind::Members,
            },
            mine: you == Some(self.owner.as_str()),
            id: self.id,
            name: self.name,
        }
    }
}

/// A sign-in on its way: open `url` in the browser and type `code` there.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignIn {
    pub url: String,
    pub code: String,
}

/// Someone in your address book who has an account.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ContactMatch {
    /// The address you asked about, as you gave it.
    pub email: String,
    /// What they call themselves here.
    pub name: String,
}

struct Pending {
    id: String,
    secret: String,
}

#[derive(Default)]
struct State {
    session: Option<String>,
    account: Option<Account>,
    pending: Option<Pending>,
}

#[derive(uniffi::Object)]
pub struct Client {
    pub(crate) server: String,
    web: String,
    state: Mutex<State>,
    pub(crate) store: Arc<Store>,
}

#[uniffi::export]
impl Client {
    /// `server` is the API's address and `web` the site's (they differ only
    /// in development); `data_dir` is where the phone keeps its own list of
    /// calls. Picks up the session kept from last time.
    #[uniffi::constructor]
    pub fn new(server: String, web: String, data_dir: String) -> Arc<Self> {
        crate::init_logging();
        let session = secrets::load(&server);
        tracing::info!(%server, signed_in = session.is_some(), "client");
        let store = Arc::new(Store::open(&data_dir, &server));
        Arc::new(Self {
            server,
            web,
            state: Mutex::new(State {
                session,
                ..Default::default()
            }),
            store,
        })
    }

    /// The calls you were in on this phone, newest first.
    pub fn calls(&self) -> Vec<CallRecord> {
        self.store.calls()
    }

    /// The calls you were in in `room` (its id), newest first.
    pub fn room_calls(&self, room: String) -> Vec<CallRecord> {
        self.store.room_calls(&room)
    }

    /// What happened in call `id`, in order: who came and went, screen
    /// shares, recordings, and what was said if it was transcribed.
    pub fn call_moments(&self, id: u64) -> Vec<Moment> {
        self.store.moments(id)
    }

    pub fn delete_call(&self, id: u64) {
        self.store.remove_call(id);
    }

    pub fn clear_calls(&self) {
        self.store.clear_calls();
    }

    /// Which of these email addresses (from the address book) have an
    /// account. Only a SHA-256 of each leaves the phone, and the server
    /// keeps none of them.
    pub async fn match_contacts(&self, emails: Vec<String>) -> Result<Vec<ContactMatch>, AppError> {
        let mut by_hash: HashMap<String, String> = HashMap::new();
        for email in emails {
            by_hash.entry(email_hash(&email)).or_insert(email);
        }
        let hashes: Vec<&String> = by_hash.keys().collect();
        let mut matches = Vec::new();
        // The server takes a thousand at a time.
        for chunk in hashes.chunks(1000) {
            let value = self
                .authorized("/contacts/match", Some(json!({"hashes": chunk})), "POST")
                .await?;
            for found in value["matches"].as_array().into_iter().flatten() {
                let (Some(hash), Some(name)) = (found["hash"].as_str(), found["name"].as_str())
                else {
                    continue;
                };
                if let Some(email) = by_hash.get(hash) {
                    matches.push(ContactMatch {
                        email: email.clone(),
                        name: name.to_owned(),
                    });
                }
            }
        }
        Ok(matches)
    }

    /// The room for calls with one contact: theirs and yours, kept for as
    /// long as you like, so its link (the invitation you text them) keeps
    /// working. Made the first time; `contact` is the address book's
    /// identifier, `name` what to call them.
    pub async fn contact_room(&self, contact: String, name: String) -> Result<Room, AppError> {
        if let Some(id) = self.store.line(&contact) {
            // Still open? (You, or its clean-up, may have closed it.)
            if let Some(room) = self.rooms().await?.into_iter().find(|r| r.id == id) {
                return Ok(room);
            }
        }
        let you = self.account().map(|a| a.name).unwrap_or_default();
        let title = if you.is_empty() {
            name
        } else {
            format!("{you} & {name}")
        };
        let body = json!({"name": title, "kind": "direct"});
        let value = self.authorized("/rooms", Some(body), "POST").await?;
        let room: RoomJson = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        // A call for two expires in a day unless it's kept: this one is an
        // address, so keep it.
        let keep = format!("/rooms/{}/keep", room.id);
        self.authorized(&keep, Some(json!({})), "POST").await?;
        self.store.set_line(&contact, &room.id);
        let you = self.state.lock().account.as_ref().map(|a| a.id.clone());
        Ok(room.room(you.as_deref()))
    }

    pub fn signed_in(&self) -> bool {
        self.state.lock().session.is_some()
    }

    /// Who you are, as last heard from the server ([`Client::refresh`]).
    pub fn account(&self) -> Option<Account> {
        self.state.lock().account.clone()
    }

    /// The address of a room, to send to people.
    pub fn room_link(&self, room: String) -> String {
        format!("{}/?room={room}", self.web.trim_end_matches('/'))
    }

    /// Start signing in. Then [`Client::sign_in_poll`] every couple of
    /// seconds until it returns you.
    pub async fn sign_in_start(&self) -> Result<SignIn, AppError> {
        let start = request(
            &self.server,
            None,
            "/auth/desktop/start",
            Some(json!({})),
            "POST",
        )
        .await?;
        let field = |name: &str| {
            start[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("The server's sign-in answer makes no sense"))
        };
        let (id, secret, code) = (field("id")?, field("secret")?, field("code")?);
        let url = format!("{}/?desktop={id}", self.web.trim_end_matches('/'));
        self.state.lock().pending = Some(Pending { id, secret });
        Ok(SignIn { url, code })
    }

    /// Whether the browser has approved yet: you, once it has. A connection
    /// that drops for a moment counts as "not yet".
    pub async fn sign_in_poll(&self) -> Result<Option<Account>, AppError> {
        let (id, body, previous) = {
            let s = self.state.lock();
            let pending = s.pending.as_ref().ok_or_else(cancelled)?;
            (
                pending.id.clone(),
                json!({"id": pending.id, "secret": pending.secret}),
                s.session.clone(),
            )
        };
        let poll = request(
            &self.server,
            previous.as_deref(),
            "/auth/desktop/poll",
            Some(body),
            "POST",
        );
        let value = match poll.await {
            Err(e) if http::transient(&e) => return Ok(None),
            result => result?,
        };
        if value["pending"] == true {
            return Ok(None);
        }
        let token = value["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("The server sent no session"))?
            .to_owned();
        let account: Account =
            serde_json::from_value(value["principal"].clone()).map_err(anyhow::Error::from)?;
        {
            let mut s = self.state.lock();
            // Cancelled, or started again, while this poll was on its way.
            if s.pending.as_ref().map(|p| p.id.as_str()) != Some(id.as_str()) {
                return Err(cancelled().into());
            }
            secrets::save(&self.server, &token)?;
            s.pending = None;
            s.session = Some(token);
            s.account = Some(account.clone());
        }
        // The session is provisional until its first use: this makes it last,
        // and ends the one it replaces.
        Ok(Some(self.refresh().await.unwrap_or(account)))
    }

    pub fn sign_in_cancel(&self) {
        self.state.lock().pending = None;
    }

    /// Ask the server who you are. A session that has ended is forgotten
    /// here, and the error says so.
    pub async fn refresh(&self) -> Result<Account, AppError> {
        let value = self.authorized("/auth/me", None, "GET").await?;
        let account: Account = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        self.state.lock().account = Some(account.clone());
        Ok(account)
    }

    /// Your rooms, newest first.
    pub async fn rooms(&self) -> Result<Vec<Room>, AppError> {
        let value = self.authorized("/rooms", None, "GET").await?;
        let rooms: Vec<RoomJson> = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        let you = self.state.lock().account.as_ref().map(|a| a.id.clone());
        Ok(rooms.into_iter().map(|r| r.room(you.as_deref())).collect())
    }

    pub async fn create_room(&self, name: String, members_only: bool) -> Result<Room, AppError> {
        let kind = if members_only { "private" } else { "public" };
        let body = json!({"name": name, "kind": kind});
        let value = self.authorized("/rooms", Some(body), "POST").await?;
        let room: RoomJson = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        let you = self.state.lock().account.as_ref().map(|a| a.id.clone());
        Ok(room.room(you.as_deref()))
    }

    /// End the session here and on the server (if it can be reached).
    pub async fn sign_out(&self) {
        let session = {
            let mut s = self.state.lock();
            s.account = None;
            s.pending = None;
            s.session.take()
        };
        secrets::clear(&self.server);
        self.store.wipe();
        if let Some(session) = session {
            let logout = request(
                &self.server,
                Some(&session),
                "/auth/logout",
                Some(json!({})),
                "POST",
            );
            if let Err(e) = logout.await {
                tracing::warn!("sign out: {e:#}");
            }
        }
    }

    /// Development builds only: use `session` for this run, without touching
    /// the Keychain (a test signs in with `curl` and hands the token over).
    pub fn use_session(&self, session: String) {
        self.state.lock().session = Some(session);
    }
}

impl Client {
    pub(crate) fn session(&self) -> Option<String> {
        self.state.lock().session.clone()
    }

    /// A request as you. If the server says the session has ended, it's
    /// forgotten: the app goes back to signing in.
    async fn authorized(
        &self,
        path: &str,
        body: Option<Value>,
        method: &str,
    ) -> Result<Value, AppError> {
        let session = self.session().ok_or_else(|| AppError::SignedOut {
            message: "Sign in to continue".into(),
        })?;
        let result = request(&self.server, Some(&session), path, body, method).await;
        let result = result.map_err(AppError::from);
        if matches!(result, Err(AppError::SignedOut { .. })) {
            self.forget(&session);
        }
        result
    }

    /// `session` has ended: forget it, unless a newer one took its place.
    pub(crate) fn forget(&self, session: &str) {
        let mut s = self.state.lock();
        if s.session.as_deref() == Some(session) {
            s.session = None;
            s.account = None;
            secrets::clear(&self.server);
        }
    }
}

/// What the server compares: SHA-256 of the address, trimmed and lowercased.
fn email_hash(email: &str) -> String {
    use sha2::{Digest as _, Sha256};
    format!(
        "{:x}",
        Sha256::digest(email.trim().to_lowercase().as_bytes())
    )
}

fn cancelled() -> anyhow::Error {
    anyhow::anyhow!("Sign-in cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_as_the_server_sends_them() {
        let rooms: Vec<RoomJson> = serde_json::from_str(
            r#"[{"id":"a1","name":"Team","kind":"private","owner":"me","guest":null,"closed":false,"expires":null},
                {"id":"b2","name":"Open","kind":"public","owner":"anna","guest":null,"closed":false,"expires":null}]"#,
        )
        .unwrap();
        let rooms: Vec<Room> = rooms.into_iter().map(|r| r.room(Some("me"))).collect();
        assert_eq!(
            rooms,
            [
                Room {
                    id: "a1".into(),
                    name: "Team".into(),
                    kind: RoomKind::Members,
                    mine: true
                },
                Room {
                    id: "b2".into(),
                    name: "Open".into(),
                    kind: RoomKind::Open,
                    mine: false
                },
            ]
        );
    }

    #[test]
    fn an_address_is_hashed_as_the_server_does() {
        // `printf admin@example.com | shasum -a 256`
        assert_eq!(
            email_hash("  Admin@Example.com "),
            "258d8dc916db8cea2cafb6c3cd0cb0246efe061421dbd83ec3a350428cabda4f"
        );
    }

    #[test]
    fn a_room_link_is_the_sites() {
        let client = Client {
            server: "http://127.0.0.1:8180".into(),
            web: "http://127.0.0.1:5273/".into(),
            state: Default::default(),
            store: Arc::new(Store::in_memory()),
        };
        assert_eq!(
            client.room_link("a1".into()),
            "http://127.0.0.1:5273/?room=a1"
        );
    }
}
