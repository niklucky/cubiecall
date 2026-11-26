//! The account on this computer: its session in the Keychain (Credential
//! Manager on Windows), and signing in with a code the server emails. Only the app that asked for the code holds the
//! challenge it goes with.
use crate::{Shell, call::RoomRef, store::SavedRoom};
use gpui::Context;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Deserialize)]
pub struct Principal {
    pub id: String,
    /// Empty for a new account, until its owner has said what to be called.
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub email: Option<String>,
}
#[cfg(any(target_os = "macos", windows))]
const SESSION_SERVICE: &str = "com.cubiecall.session";
#[cfg(target_os = "macos")]
const PREVIOUS_SERVICE: &str = "com.quipcall.session";
#[cfg(target_os = "macos")]
fn key_account(server: &str) -> String {
    format!(
        "{}:{}",
        cubie_core::http::storage_origin(server.trim_end_matches('/')),
        crate::store::data_override().unwrap_or_default().display()
    )
}
#[cfg(target_os = "macos")]
fn restore_session(
    read: impl Fn(&str) -> Result<Vec<u8>, i32>,
    add_current: impl FnOnce(&[u8]) -> Result<(), i32>,
) -> Option<String> {
    const ITEM_NOT_FOUND: i32 = -25300; // errSecItemNotFound
    const DUPLICATE_ITEM: i32 = -25299; // errSecDuplicateItem
    match read(SESSION_SERVICE) {
        Ok(bytes) => return String::from_utf8(bytes).ok(),
        Err(ITEM_NOT_FOUND) => {}
        Err(_) => return None,
    }
    let previous = String::from_utf8(read(PREVIOUS_SERVICE).ok()?).ok()?;
    // Copy after the user grants access. Keep the original for Quipcall and
    // add without overwriting a session another Cubie instance just saved.
    match add_current(previous.as_bytes()) {
        Ok(()) => Some(previous),
        Err(DUPLICATE_ITEM) => String::from_utf8(read(SESSION_SERVICE).ok()?).ok(),
        Err(status) => {
            tracing::warn!(status, "Could not migrate the saved Quipcall session");
            Some(previous)
        }
    }
}
pub fn load(server: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::{
            os::macos::keychain::SecKeychain, passwords::get_generic_password,
        };
        let account = key_account(server);
        restore_session(
            |service| get_generic_password(service, &account).map_err(|e| e.code()),
            |token| {
                SecKeychain::default()
                    .and_then(|kc| kc.add_generic_password(SESSION_SERVICE, &account, token))
                    .map_err(|e| e.code())
            },
        )
    }
    #[cfg(windows)]
    {
        vault::read(server)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = server;
        None
    }
}
fn save(server: &str, token: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let account = key_account(server);
        security_framework::passwords::set_generic_password(
            SESSION_SERVICE,
            &account,
            token.as_bytes(),
        )?;
        let _ = security_framework::passwords::delete_generic_password(PREVIOUS_SERVICE, &account);
    }
    #[cfg(windows)]
    vault::write(server, token)?;
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (server, token);
        anyhow::bail!("Secure credential storage is not implemented on this platform");
    }
    #[allow(unreachable_code)]
    Ok(())
}
fn clear(server: &str) {
    #[cfg(target_os = "macos")]
    {
        let account = key_account(server);
        for service in [SESSION_SERVICE, PREVIOUS_SERVICE] {
            let _ = security_framework::passwords::delete_generic_password(service, &account);
        }
    }
    #[cfg(windows)]
    vault::delete(server);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = server;
}

/// The session in Windows' Credential Manager: a generic credential that
/// only this Windows account can read, as the Keychain item is on the Mac.
/// One per server and data directory, as there, so test instances keep
/// sessions of their own.
#[cfg(windows)]
mod vault {
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use windows::core::{HSTRING, PWSTR};

    /// `com.cubiecall.session/<server>/<data dir>`, each part in hex:
    /// Credential Manager matches names ignoring case, so two servers whose
    /// paths differ only in case would share a session, and parts kept apart
    /// can't run into each other as joined text could.
    fn target(server: &str) -> String {
        let hex = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let origin = cubie_core::http::storage_origin(server.trim_end_matches('/'));
        let data = crate::store::data_override().unwrap_or_default();
        format!(
            "{}/{}/{}",
            super::SESSION_SERVICE,
            hex(origin),
            hex(&data.to_string_lossy())
        )
    }

    pub fn read(server: &str) -> Option<String> {
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: on success CredReadW hands us a credential to free with
        // CredFree, its blob `CredentialBlobSize` bytes long.
        unsafe {
            CredReadW(
                &HSTRING::from(target(server)),
                CRED_TYPE_GENERIC,
                None,
                &mut credential,
            )
            .ok()?;
            let c = &*credential;
            // An empty credential may come with no blob at all.
            let token = (!c.CredentialBlob.is_null() && c.CredentialBlobSize > 0)
                .then(|| {
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
                })
                .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());
            CredFree(credential.cast());
            token
        }
    }

    pub fn write(server: &str, token: &str) -> anyhow::Result<()> {
        let mut name: Vec<u16> = target(server).encode_utf16().chain([0]).collect();
        let mut user: Vec<u16> = "Cubie".encode_utf16().chain([0]).collect();
        let mut blob = token.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            UserName: PWSTR(user.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: every pointer in `credential` outlives the call, which
        // copies what it points at.
        unsafe { CredWriteW(&credential, 0) }?;
        Ok(())
    }

    pub fn delete(server: &str) {
        // SAFETY: a plain call; a missing credential just fails.
        let _ = unsafe { CredDeleteW(&HSTRING::from(target(server)), CRED_TYPE_GENERIC, None) };
    }
}
pub(crate) use cubie_core::http::RequestError;

pub(crate) static FAILOVER: std::sync::OnceLock<cubie_core::http::Failover> =
    std::sync::OnceLock::new();

pub(crate) fn endpoint(server: &str) -> cubie_core::http::Endpoint<&str> {
    use cubie_core::http::Endpoint;
    if let Some(route) = FAILOVER
        .get()
        .filter(|route| route.primary() == server.trim_end_matches('/'))
    {
        Endpoint::Redundant(route)
    } else {
        Endpoint::Direct(server)
    }
}

pub(crate) fn web_origin<'a>(server: &str, web: &'a str) -> &'a str {
    match endpoint(server) {
        cubie_core::http::Endpoint::Redundant(route)
            if web.trim_end_matches('/') == route.primary() =>
        {
            route.origin()
        }
        _ => web,
    }
}

pub(crate) fn request(
    server: &str,
    token: Option<&str>,
    path: &str,
    body: Option<serde_json::Value>,
    method: &str,
) -> impl Future<Output = anyhow::Result<serde_json::Value>> + Send + 'static + use<> {
    endpoint(server).request(token, path, body, method)
}

/// A completion can only modify the operation generation that started it.
pub struct Generation<T> {
    generation: u64,
    pub payload: Option<T>,
}
impl<T> Default for Generation<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            payload: None,
        }
    }
}
impl<T> Generation<T> {
    fn start(&mut self, payload: T) -> u64 {
        self.generation += 1;
        self.payload = Some(payload);
        self.generation
    }
    fn current(&self, generation: u64) -> bool {
        self.pending() && self.generation == generation
    }
    fn cancel(&mut self) {
        self.payload = None;
    }
    fn finish(&mut self, generation: u64) -> bool {
        if !self.current(generation) {
            return false;
        }
        self.cancel();
        true
    }
    pub fn pending(&self) -> bool {
        self.payload.is_some()
    }
}
/// Why the sign-in window opened without being asked for.
pub const SESSION_EXPIRED: &str = "Your session expired. Sign in to continue.";
/// A sign-in on its way: the window is open. `room`: the call a link asked
/// for, joined once signed in.
#[derive(Default)]
pub struct SignIn {
    room: Option<String>,
}
pub type SignInAttempt = Generation<SignIn>;
pub type PendingOperation = Generation<()>;
impl SignInAttempt {
    /// One sign-in at a time: a link that arrives meanwhile changes where it
    /// leads, not the window.
    fn request(&mut self, room: Option<String>) -> u64 {
        if !self.pending() {
            return self.start(SignIn { room });
        }
        if room.is_some() {
            self.payload.as_mut().unwrap().room = room;
        }
        self.generation
    }
    fn room(&self) -> Option<String> {
        self.payload.as_ref().and_then(|p| p.room.clone())
    }
}
/// Ask the server to email a sign-in code. The answer is the challenge the
/// code goes with. An address with no account is refused, in words for the
/// person to read.
pub async fn request_code(server: &str, email: &str) -> anyhow::Result<String> {
    let sent = request(
        server,
        None,
        "/auth/request",
        Some(json!({"email":email,"app":true})),
        "POST",
    )
    .await?;
    sent["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("Update the server to sign in with a code"))
}
/// Exchange the emailed code for a session. The server keeps `previous`, this
/// Mac's session until now, working until the new one has been used.
pub async fn verify_code(
    server: &str,
    previous: Option<&str>,
    challenge: &str,
    code: &str,
) -> anyhow::Result<(String, Principal)> {
    let value = request(
        server,
        previous,
        "/auth/verify",
        Some(json!({"id":challenge,"code":code,"app":true})),
        "POST",
    )
    .await?;
    let token = value["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing session"))?
        .to_owned();
    Ok((token, serde_json::from_value(value["principal"].clone())?))
}
/// Sign a session out that this Mac won't keep after all.
pub async fn discard(server: &str, token: &str) {
    let _ = request(server, Some(token), "/auth/logout", Some(json!({})), "POST").await;
}
pub(crate) fn unauthorized(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<RequestError>()
        .is_some_and(|e| e.status == 401)
}
fn forget_session(
    session: &mut Option<String>,
    principal: &mut Option<Principal>,
    store: &mut crate::store::Store,
    clear_saved: impl FnOnce(),
) {
    clear_saved();
    *session = None;
    *principal = None;
    store.rooms.clear();
    store.bind_rooms(None);
}

async fn load_account(
    server: &str,
    session: Option<&str>,
) -> anyhow::Result<(Principal, Vec<SavedRoom>)> {
    let (principal, rooms) = cubie_core::http::join(
        request(server, session, "/auth/me", None, "GET"),
        request(server, session, "/rooms", None, "GET"),
    )
    .await;
    // A 401 takes precedence over an unrelated transport error from its peer.
    if let Err(e) = &principal
        && unauthorized(e)
    {
        return Err(principal.unwrap_err());
    }
    if let Err(e) = &rooms
        && unauthorized(e)
    {
        return Err(rooms.unwrap_err());
    }
    Ok((
        serde_json::from_value(principal?)?,
        serde_json::from_value(rooms?)?,
    ))
}

impl Shell {
    pub fn auth_busy(&self) -> bool {
        self.auth_attempt.pending() || self.room_creation.pending()
    }
    pub fn expire_session(&mut self, expected: Option<&str>, cx: &mut Context<Self>) -> bool {
        if self.args.session.as_deref() != expected {
            return false;
        }
        self.room_creation.cancel();
        forget_session(
            &mut self.args.session,
            &mut self.principal,
            &mut self.store,
            || {
                if expected.is_some() {
                    clear(&self.args.server);
                }
            },
        );
        self.store.save();
        self.auth_message = SESSION_EXPIRED.into();
        cx.notify();
        true
    }

    pub(crate) fn recover_auth(
        &mut self,
        error: &anyhow::Error,
        expected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !unauthorized(error) {
            return false;
        }
        if self.expire_session(expected, cx) {
            self.sign_in(None, cx);
        }
        true
    }

    /// Open the sign-in window. `room`: a call to join once signed in.
    pub fn sign_in(&mut self, room: Option<String>, cx: &mut Context<Self>) {
        if self.room_creation.pending() {
            if room.is_none() {
                return;
            }
            self.room_creation.cancel();
        }
        self.auth_attempt.request(room);
        cx.defer(crate::sign_in::open);
        cx.notify();
    }
    /// The code was right: keep the session, then go on to a name, the call
    /// a link asked for, or home. An error is for the sign-in window to show;
    /// its caller signs the unused session out.
    pub fn signed_in(
        &mut self,
        token: String,
        p: Principal,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.auth_attempt.pending() {
            return Err("Sign-in cancelled".into());
        }
        save(&self.args.server, &token)
            .map_err(|e| format!("Cannot save session in Keychain: {e}"))?;
        self.pending_room = self.auth_attempt.room();
        self.auth_attempt.cancel();
        self.store.bind_rooms(Some(&p.id));
        self.store.save();
        self.args.session = Some(token);
        self.args.name = p.name.clone();
        let named = !p.name.is_empty();
        self.principal = Some(p);
        self.auth_message.clear();
        self.refresh_account(cx);
        if named {
            self.enter(cx);
        } else {
            cx.defer(crate::onboarding::open_first_run);
        }
        cx.notify();
        Ok(())
    }
    /// Signed in, with a name: on to the call a link asked for, or home.
    pub fn enter(&mut self, cx: &mut Context<Self>) {
        match self.pending_room.take() {
            Some(room) => self.join_link(room, cx),
            None => self.open_home(cx),
        }
    }
    pub fn refresh_account(&mut self, cx: &mut Context<Self>) {
        let args = self.args.clone();
        if args.session.is_none() {
            self.principal = None;
            self.store.bind_rooms(None);
            self.store.save();
            cx.notify();
            return;
        }
        let expected = args.session.clone();
        cx.spawn(async move |this, cx| {
            let result = load_account(&args.server, args.session.as_deref()).await;
            let _ = this.update(cx, |s, cx| {
                if s.args.session != expected {
                    return;
                }
                match result {
                    Ok((p, rooms)) => {
                        s.store.bind_rooms(Some(&p.id));
                        // An account that never got its name (the app was
                        // quit at the question) is asked again at launch.
                        if p.name.is_empty() && s.principal.is_none() {
                            cx.defer(crate::onboarding::open_first_run);
                        }
                        s.args.name = p.name.clone();
                        s.principal = Some(p);
                        s.store.refresh_rooms(rooms);
                        s.store.save();
                        s.auth_message.clear();
                    }
                    Err(e) if unauthorized(&e) => {
                        // Home has nothing to show without an account.
                        if s.expire_session(expected.as_deref(), cx) {
                            s.sign_in(None, cx);
                        }
                    }
                    Err(e) => {
                        s.auth_message = e.to_string();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// The sign-in window was closed before it was done.
    pub fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if !self.auth_attempt.pending() {
            return;
        }
        self.auth_attempt.cancel();
        cx.notify();
    }
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        let args = self.args.clone();
        self.auth_attempt.cancel();
        self.room_creation.cancel();
        forget_session(
            &mut self.args.session,
            &mut self.principal,
            &mut self.store,
            || clear(&args.server),
        );
        self.store.save();
        self.auth_message.clear();
        // Back to where a Mac without an account starts. The sign-in window
        // opens before home closes, or the app would quit with no window.
        self.sign_in(None, cx);
        if let Some(home) = self.home.take() {
            cx.defer(move |cx| {
                let _ = home.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
        cx.spawn(async move |_, _| {
            let _ = request(
                &args.server,
                args.session.as_deref(),
                "/auth/logout",
                Some(json!({})),
                "POST",
            )
            .await;
        })
        .detach();
    }
    pub fn create_room(&mut self, kind: &str, join: bool, cx: &mut Context<Self>) {
        if self.auth_busy() {
            return;
        }
        if self.args.session.is_none() {
            self.sign_in(None, cx);
            return;
        }
        let operation = self.room_creation.start(());
        let args = self.args.clone();
        let expected = args.session.clone();
        let kind = kind.to_owned();
        let name = if join {
            "Quick call".to_owned()
        } else {
            self.store.next_room_name()
        };
        cx.spawn(async move |this, cx| {
            let result = async {
                let r = request(
                    &args.server,
                    args.session.as_deref(),
                    "/rooms",
                    Some(json!({"name":name,"kind":kind})),
                    "POST",
                )
                .await?;
                serde_json::from_value::<SavedRoom>(r).map_err(anyhow::Error::from)
            }
            .await;
            let _ = this.update(cx, |s, cx| {
                if !s.room_creation.finish(operation) || s.args.session != expected {
                    return;
                }
                match result {
                    Ok(r) => {
                        s.store.bind_rooms(Some(&r.owner));
                        s.store.cache_room(r.clone());
                        s.store.save();
                        if join {
                            s.start_call(
                                RoomRef {
                                    id: r.id,
                                    name: r.name,
                                    one_time: true,
                                },
                                cx,
                            );
                        }
                    }
                    Err(e) if unauthorized(&e) => {
                        if s.expire_session(expected.as_deref(), cx) {
                            s.sign_in(None, cx);
                        }
                    }
                    Err(e) => s.auth_message = e.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn rename_profile(&mut self, name: &str, cx: &mut Context<Self>) {
        let args = self.args.clone();
        let name = name.to_owned();
        let expected = args.session.clone();
        cx.spawn(async move |this, cx| {
            let result = request(
                &args.server,
                args.session.as_deref(),
                "/auth/me",
                Some(json!({"name":name})),
                "PATCH",
            )
            .await;
            let _ = this.update(cx, |s, cx| {
                if s.args.session != expected {
                    return;
                }
                match result {
                    Ok(value) => {
                        if let Ok(p) = serde_json::from_value::<Principal>(value) {
                            s.args.name = p.name.clone();
                            s.store.name = Some(p.name.clone());
                            s.principal = Some(p);
                            s.store.save();
                        }
                    }
                    Err(e) => {
                        if !s.recover_auth(&e, expected.as_deref(), cx) {
                            s.auth_message = e.to_string();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn rename_remote_room(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let args = self.args.clone();
        let expected = args.session.clone();
        let id = id.to_owned();
        let name = name.to_owned();
        cx.spawn(async move |this, cx| {
            let result = request(
                &args.server,
                args.session.as_deref(),
                &format!("/rooms/{id}"),
                Some(json!({"name":name})),
                "PATCH",
            )
            .await;
            let _ = this.update(cx, |s, cx| {
                if s.args.session != expected {
                    return;
                }
                match result {
                    Ok(_) => {
                        s.store.rename_room(&id, &name);
                        s.store.save();
                    }
                    Err(e) => {
                        if !s.recover_auth(&e, expected.as_deref(), cx) {
                            s.auth_message = e.to_string();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[cfg(target_os = "macos")]
    #[test]
    fn approved_legacy_session_is_imported_on_first_launch_only() {
        use std::cell::{Cell, RefCell};
        let current = RefCell::new(None::<Vec<u8>>);
        let legacy_reads = Cell::new(0);
        for _ in 0..2 {
            let restored = restore_session(
                |service| {
                    if service == SESSION_SERVICE {
                        return current.borrow().clone().ok_or(-25300);
                    }
                    assert_eq!(service, PREVIOUS_SERVICE);
                    legacy_reads.set(legacy_reads.get() + 1);
                    Ok(b"old-session".to_vec())
                },
                |token| {
                    assert!(current.replace(Some(token.to_vec())).is_none());
                    Ok(())
                },
            );
            assert_eq!(restored.as_deref(), Some("old-session"));
        }
        assert_eq!(legacy_reads.get(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn denied_keychain_read_does_not_import_a_session() {
        for denied_service in [SESSION_SERVICE, PREVIOUS_SERVICE] {
            let restored = restore_session(
                |service| {
                    if service == denied_service {
                        Err(-25293)
                    } else {
                        assert_eq!(service, SESSION_SERVICE);
                        Err(-25300)
                    }
                },
                |_| panic!("access was denied"),
            );
            assert!(restored.is_none());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn migration_uses_concurrent_sign_in_and_tolerates_a_failed_write() {
        use std::cell::Cell;
        for (error, expected) in [(-25299, "new"), (-25293, "old")] {
            let added = Cell::new(false);
            let restored = restore_session(
                |service| match service {
                    PREVIOUS_SERVICE => Ok(b"old".to_vec()),
                    SESSION_SERVICE if added.get() => Ok(b"new".to_vec()),
                    _ => Err(-25300),
                },
                |_| {
                    added.set(true);
                    Err(error)
                },
            );
            assert_eq!(restored.as_deref(), Some(expected));
        }
    }

    #[test]
    fn unbinding_identity_clears_private_room_cache() {
        let mut store = crate::store::Store::default();
        store.bind_rooms(Some("old-account"));
        store.cache_room(SavedRoom {
            id: "private".into(),
            ..Default::default()
        });
        store.bind_rooms(None);
        assert!(store.rooms.is_empty());
        assert!(store.rooms_principal.is_none());
    }

    #[test]
    fn a_link_during_sign_in_changes_where_it_leads_not_the_attempt() {
        let mut attempt = SignInAttempt::default();
        let home = attempt.request(None);
        assert_eq!(attempt.request(None), home);
        assert!(attempt.room().is_none());
        assert_eq!(attempt.request(Some("room-a".into())), home);
        assert_eq!(attempt.room().as_deref(), Some("room-a"));
        // Asking again without a link keeps the call that was asked for.
        assert_eq!(attempt.request(None), home);
        assert_eq!(attempt.request(Some("room-b".into())), home);
        assert_eq!(attempt.room().as_deref(), Some("room-b"));
        assert!(attempt.current(home));
        attempt.cancel();
        assert!(attempt.room().is_none());
        assert_ne!(attempt.request(None), home);
    }
    #[test]
    fn profile_and_rooms_refresh_are_in_flight_together() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let responder = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut sockets = vec![];
            while sockets.len() < 2 {
                match listener.accept() {
                    Ok((stream, _)) => sockets.push(stream),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "second request waited for first response"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            for mut stream in sockets {
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut headers = vec![];
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                }
                let headers = String::from_utf8(headers).unwrap();
                let body = if headers.starts_with("GET /api/auth/me ") {
                    r#"{"id":"me","name":"Me","kind":"user"}"#
                } else {
                    assert!(headers.starts_with("GET /api/rooms "));
                    "[]"
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let (p, rooms) =
            crate::test_support::block_on(load_account(&server, Some("session"))).unwrap();
        assert_eq!(p.id, "me");
        assert!(rooms.is_empty());
        responder.join().unwrap();
    }

    #[test]
    fn expired_create_releases_busy_without_unlocking_a_newer_operation() {
        let mut creation = PendingOperation::default();
        let old = creation.start(());
        // Session invalidation releases the in-flight create immediately.
        creation.cancel();
        assert!(!creation.pending());
        let mut signin = SignInAttempt::default();
        let browser = signin.start(SignIn::default());
        assert!(!creation.finish(old));
        assert!(signin.current(browser));
        signin.finish(browser);
        let retry = creation.start(());
        assert!(!creation.finish(old));
        assert!(creation.pending());
        assert!(creation.finish(retry));
        assert!(!creation.pending());
    }

    #[test]
    fn cancelled_signin_cannot_install_a_late_session() {
        let mut attempt = SignInAttempt::default();
        let old = attempt.start(SignIn::default());
        attempt.cancel();
        assert!(!attempt.pending());
        assert!(!attempt.current(old));
        let retry = attempt.start(SignIn::default());
        assert!(!attempt.finish(old)); // a delayed answer cannot replace the retry
        assert!(attempt.current(retry));
        assert!(attempt.finish(retry));
        assert!(!attempt.pending());
    }

    #[test]
    fn expired_session_clears_credentials_identity_and_cached_rooms() {
        let error: anyhow::Error = RequestError {
            status: 401,
            message: "Expired".into(),
        }
        .into();
        assert!(unauthorized(&error));
        let network: anyhow::Error = RequestError {
            status: 503,
            message: "Offline".into(),
        }
        .into();
        assert!(!unauthorized(&network));
        let guest_role: anyhow::Error = RequestError {
            status: 403,
            message: "Account required".into(),
        }
        .into();
        assert!(
            !unauthorized(&guest_role),
            "a valid guest credential must survive a role denial"
        );
        let mut session = Some("old-token".into());
        let mut principal = Some(Principal {
            id: "a".into(),
            name: "A".into(),
            kind: "user".into(),
            email: None,
        });
        let mut store = crate::store::Store::default();
        store.bind_rooms(Some("a"));
        store.cache_room(SavedRoom {
            id: "private".into(),
            ..Default::default()
        });
        let mut cleared_keychain = false;
        forget_session(&mut session, &mut principal, &mut store, || {
            cleared_keychain = true
        });
        assert!(cleared_keychain);
        assert!(session.is_none()); // start_call now takes the sign_in path
        assert!(principal.is_none());
        assert!(store.rooms.is_empty());
        assert!(store.rooms_principal.is_none());
    }

    #[test]
    fn throttled_non_json_response_keeps_its_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0];
            while !bytes.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 4\r\nConnection: close\r\n\r\nBusy").unwrap();
        });
        let error =
            crate::test_support::block_on(request(&server, None, "/auth/verify", None, "GET"))
                .unwrap_err();
        assert_eq!(error.downcast_ref::<RequestError>().unwrap().status, 429);
        task.join().unwrap();
    }
}
