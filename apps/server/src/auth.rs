use crate::{AppError, AppState, Result};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
pub fn random() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS randomness unavailable");
    b.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
/// nginx overwrites X-Real-IP; the API listener is loopback-only in deployment.
pub fn client_ip(h: &HeaderMap) -> String {
    hash(
        h.get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("local"),
    )
}
#[derive(Clone, Serialize, FromRow)]
pub struct Principal {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub email: Option<String>,
    pub admin: bool,
}
pub fn credential(h: &HeaderMap) -> Option<&str> {
    if let Some(v) = h.get(header::AUTHORIZATION) {
        return v.to_str().ok()?.strip_prefix("Bearer ");
    }
    h.get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|c| c.strip_prefix("qc_session="))
}
pub async fn current(s: &AppState, h: &HeaderMap) -> Result<Principal> {
    let token = hash(credential(h).ok_or_else(AppError::unauthorized)?);
    #[derive(FromRow)]
    struct Authenticated {
        #[sqlx(flatten)]
        principal: Principal,
        pending: bool,
        expires: i64,
    }
    let found: Authenticated = sqlx::query_as("SELECT p.*,s.pending,s.expires FROM sessions s JOIN principals p ON p.id=s.principal WHERE s.hash=? AND s.expires>?")
        .bind(&token).bind(now()).fetch_optional(&s.db).await?.ok_or_else(AppError::unauthorized)?;
    if found.pending {
        // The native client only uses the new credential after saving it. Lost
        // poll responses and cancelled attempts leave the old session intact.
        let mut tx = s.db.begin_with("BEGIN IMMEDIATE").await?;
        let replaced: Option<Option<String>> = sqlx::query_scalar("UPDATE sessions SET pending=0,expires=? WHERE hash=? AND pending=1 AND expires>? RETURNING replaces")
            .bind(now()+lifetime(&found.principal)).bind(&token).bind(now()).fetch_optional(&mut *tx).await?;
        if let Some(Some(previous)) = replaced {
            sqlx::query("DELETE FROM sessions WHERE hash=? AND hash<>?")
                .bind(previous)
                .bind(&token)
                .execute(&mut *tx)
                .await?;
        } else if replaced.is_none() {
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE hash=? AND expires>?)",
            )
            .bind(&token)
            .bind(now())
            .fetch_one(&mut *tx)
            .await?;
            if !valid {
                return Err(AppError::unauthorized());
            }
        }
        tx.commit().await?;
    } else if found.expires < now() + lifetime(&found.principal) / 2 {
        let renewed =
            sqlx::query("UPDATE sessions SET expires=MAX(expires,?) WHERE hash=? AND expires>?")
                .bind(now() + lifetime(&found.principal))
                .bind(&token)
                .bind(now())
                .execute(&s.db)
                .await?;
        if renewed.rows_affected() == 0 {
            return Err(AppError::unauthorized());
        }
    }
    Ok(found.principal)
}
pub async fn user(s: &AppState, h: &HeaderMap) -> Result<Principal> {
    let p = current(s, h).await?;
    if p.kind != "user" {
        return Err(AppError::forbidden(
            "Sign in with an invited account to create or manage rooms",
        ));
    }
    Ok(p)
}
pub async fn session(db: &mut SqliteConnection, p: &Principal) -> Result<String> {
    let token = random();
    sqlx::query("INSERT INTO sessions(hash,principal,expires) VALUES (?, ?, ?)")
        .bind(hash(&token))
        .bind(&p.id)
        .bind(now() + lifetime(p))
        .execute(db)
        .await?;
    Ok(token)
}
fn lifetime(p: &Principal) -> i64 {
    if p.kind == "guest" {
        365 * 86400
    } else {
        30 * 86400
    }
}
fn cookie(s: &AppState, token: &str, age: i64) -> String {
    format!(
        "qc_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{}",
        if s.config.secure_cookie() {
            "; Secure"
        } else {
            ""
        }
    )
}
/// Align the browser cookie with the server deadline after authenticated use.
/// Never recreate a logged-out/expired session or override a fresh login cookie.
pub async fn renewed_cookie(s: &AppState, token: &str) -> Result<Option<String>> {
    let expiry: Option<i64> =
        sqlx::query_scalar("SELECT expires FROM sessions WHERE hash=? AND pending=0 AND expires>?")
            .bind(hash(token))
            .bind(now())
            .fetch_optional(&s.db)
            .await?;
    Ok(expiry.map(|expires| cookie(s, token, expires - now())))
}
fn signed_in(s: &AppState, token: &str, p: &Principal) -> Response {
    (
        [(header::SET_COOKIE, cookie(s, token, lifetime(p)))],
        Json(p),
    )
        .into_response()
}
pub async fn me(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Principal>> {
    Ok(Json(current(&s, &h).await?))
}
pub fn clean_name(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect()
}
#[derive(Deserialize)]
pub struct Name {
    pub name: String,
}
pub async fn profile(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<Name>,
) -> Result<Json<Principal>> {
    let mut p = current(&s, &h).await?;
    let name = clean_name(&req.name);
    if name.is_empty() {
        return Err(AppError::bad("Enter a display name"));
    }
    sqlx::query("UPDATE principals SET name=? WHERE id=?")
        .bind(&name)
        .bind(&p.id)
        .execute(&s.db)
        .await?;
    p.name = name;
    Ok(Json(p))
}
pub async fn guest(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<Name>,
) -> Result<Response> {
    if let Ok(p) = current(&s, &h).await {
        return Ok(Json(p).into_response());
    }
    let ip = client_ip(&h);
    // Count new identities only; returning guests do not consume this budget.
    rate(&s, &format!("guest-hour:{ip}"), 30, 3600).await?;
    rate(&s, &format!("guest-day:{ip}"), 100, 86400).await?;
    let name = clean_name(&req.name);
    let p = Principal {
        id: random(),
        kind: "guest".into(),
        name: if name.is_empty() {
            "Guest".into()
        } else {
            name
        },
        email: None,
        admin: false,
    };
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO principals(id, kind, name) VALUES (?, 'guest', ?)")
        .bind(&p.id)
        .bind(&p.name)
        .execute(&mut *tx)
        .await?;
    let token = session(&mut tx, &p).await?;
    tx.commit().await?;
    Ok(signed_in(&s, &token, &p))
}
pub async fn logout(State(s): State<AppState>, h: HeaderMap) -> Result<Response> {
    if let Some(token) = credential(&h) {
        sqlx::query("DELETE FROM sessions WHERE hash=?")
            .bind(hash(token))
            .execute(&s.db)
            .await?;
    }
    Ok((
        [(header::SET_COOKIE, cookie(&s, "", 0))],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub fn email(value: &str) -> Result<String> {
    let value = value.trim().to_lowercase();
    if value.len() > 254
        || value.contains(char::is_whitespace)
        || value.contains(char::is_control)
        || value.matches('@').count() != 1
        || value.starts_with('@')
        || !value
            .split('@')
            .nth(1)
            .is_some_and(|s| s.contains('.') && !s.starts_with('.') && !s.ends_with('.'))
    {
        return Err(AppError::bad("Enter a valid email address"));
    }
    Ok(value)
}
pub async fn rate(s: &AppState, key: &str, limit: i64, period: i64) -> Result<()> {
    rate_connection(&mut *s.db.acquire().await?, key, limit, period).await
}
async fn rate_connection(
    db: &mut SqliteConnection,
    key: &str,
    limit: i64,
    period: i64,
) -> Result<()> {
    spend(db, key, 1, limit, period).await
}
/// Count `amount` against `key`'s `limit` for this `period`: a request that
/// asks many things at once (a contact lookup) is counted by what it asks.
async fn spend(
    db: &mut SqliteConnection,
    key: &str,
    amount: i64,
    limit: i64,
    period: i64,
) -> Result<()> {
    let window = now() / period;
    let count: i64 = sqlx::query_scalar("INSERT INTO rate_limits(key,window,count,expires) VALUES (?,?,?,?) ON CONFLICT(key) DO UPDATE SET count=CASE WHEN window=excluded.window THEN count+excluded.count ELSE excluded.count END, window=excluded.window, expires=excluded.expires RETURNING count")
        .bind(key).bind(window).bind(amount).bind(now()+period).fetch_one(db).await?;
    if count > limit {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Please try again later.",
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct AccountInvite {
    pub email: String,
}
pub async fn invite_account(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<AccountInvite>,
) -> Result<Json<Value>> {
    let p = user(&s, &h).await?;
    if !p.admin {
        return Err(AppError::forbidden("Only admins can invite new accounts"));
    }
    let email = email(&req.email)?;
    let token = random();
    sqlx::query("INSERT INTO account_invites VALUES (?, ?, ?, ?)")
        .bind(hash(&token))
        .bind(&email)
        .bind(now() + 7 * 86400)
        .bind(p.id)
        .execute(&s.db)
        .await?;
    Ok(Json(
        json!({"url":format!("{}/#accountInvite={token}",s.config.public_url), "email":email}),
    ))
}
#[derive(Deserialize)]
pub struct ContactLookup {
    /// SHA-256 (hex) of each lowercased email address: the phone's address
    /// book never arrives in the clear, and none of it is stored.
    pub hashes: Vec<String>,
}
/// The most address-book entries one request may ask about.
pub const CONTACT_LOOKUP_MAX: usize = 1000;
/// The most addresses one account may ask about in a day.
const CONTACT_HASHES_A_DAY: i64 = 10_000;
/// Which of a signed-in person's contacts have an account: their name, by
/// the hash that was asked about. Accounts only (no guests), a limited
/// number of times an hour, so the list of accounts can't be walked.
pub async fn match_contacts(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<ContactLookup>,
) -> Result<Json<Value>> {
    let p = user(&s, &h).await?;
    if req.hashes.len() > CONTACT_LOOKUP_MAX {
        return Err(AppError::bad("Too many contacts in one request"));
    }
    rate(&s, &format!("contacts:{}", p.id), 20, 3600).await?;
    // And by how much is asked: a guessed address that hits gets a name
    // back, so an account may try only so many a day (a big address book,
    // twice over).
    spend(
        &mut *s.db.acquire().await?,
        &format!("contact-hashes:{}", p.id),
        req.hashes.len() as i64,
        CONTACT_HASHES_A_DAY,
        86_400,
    )
    .await?;
    let wanted: std::collections::HashSet<&str> = req.hashes.iter().map(String::as_str).collect();
    let users: Vec<(String, String)> = sqlx::query_as(
        "SELECT email,name FROM principals WHERE kind='user' AND email IS NOT NULL AND id!=?",
    )
    .bind(&p.id)
    .fetch_all(&s.db)
    .await?;
    let matches: Vec<Value> = users
        .into_iter()
        .filter_map(|(email, name)| {
            let hash = hash(&email);
            wanted
                .contains(hash.as_str())
                .then(|| json!({"hash":hash,"name":name}))
        })
        .collect();
    Ok(Json(json!({"matches":matches})))
}
#[derive(Deserialize)]
pub struct LinkRequest {
    pub email: String,
    pub invite: Option<String>,
}
pub async fn request_link(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<LinkRequest>,
) -> Result<Json<Value>> {
    let email = email(&req.email)?;
    let requester = client_ip(&h);
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    sqlx::query(
        "INSERT INTO mail_requests(email,requester,invite,created,origin) VALUES (?,?,?,?,?)",
    )
    .bind(email)
    .bind(requester)
    .bind(req.invite.as_deref().map(hash))
    .bind(now())
    .bind(origin.filter(|origin| s.config.trusts_origin(origin)))
    .execute(&s.db)
    .await?;
    s.mail_ready.notify_one();
    // Delivery, eligibility and suppression never affect the public response.
    Ok(Json(json!({"ok":true})))
}

#[derive(FromRow)]
struct QueuedMail {
    id: i64,
    email: String,
    requester: String,
    invite: Option<String>,
    attempts: i64,
    origin: Option<String>,
}

/// One bounded worker drains the durable queue; never hold an HTTP request open
/// for delivery or expose a provider failure to an account-enumeration probe.
pub async fn deliver_pending(s: &AppState) -> Result<usize> {
    let requests: Vec<QueuedMail> = sqlx::query_as(
        "SELECT id,email,requester,invite,attempts,origin FROM mail_requests WHERE created>? AND available<=? ORDER BY id LIMIT 100",
    )
    .bind(now() - 900)
    .bind(now())
    .fetch_all(&s.db)
    .await?;
    let count = requests.len();
    for mail in requests {
        let (id, attempts) = (mail.id, mail.attempts);
        if let Err(e) = deliver_link(s, &mail).await {
            tracing::warn!(
                request = id,
                attempt = attempts + 1,
                "email delivery failed: {}",
                e.message
            );
            if attempts < 4 {
                sqlx::query("UPDATE mail_requests SET attempts=attempts+1,available=? WHERE id=?")
                    .bind(now() + 30 * (1 << attempts))
                    .bind(id)
                    .execute(&s.db)
                    .await?;
                continue;
            }
            tracing::error!(request = id, "email retries exhausted");
        }
        sqlx::query("DELETE FROM mail_requests WHERE id=?")
            .bind(id)
            .execute(&s.db)
            .await?;
    }
    Ok(count)
}
/// Drain on startup and on new requests. Notify retains a permit if a request
/// arrives during delivery, so the transition to sleep cannot lose a wakeup.
pub async fn mail_worker(s: AppState) {
    loop {
        let failed = match deliver_pending(&s).await {
            Ok(100) => continue,
            Ok(_) => false,
            Err(e) => {
                tracing::warn!("mail queue: {}", e.message);
                true
            }
        };
        // A pending retry supplies the next wakeup; an empty queue stays idle.
        let next: Option<i64> =
            sqlx::query_scalar("SELECT MIN(available) FROM mail_requests WHERE created>?")
                .bind(now() - 900)
                .fetch_one(&s.db)
                .await
                .unwrap_or(None);
        let delay = if failed {
            300
        } else {
            next.map_or(300, |at| (at - now()).clamp(0, 300)) as u64
        };
        tokio::select! {
            _ = s.mail_ready.notified() => {},
            _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {},
        }
    }
}
async fn deliver_link(s: &AppState, mail: &QueuedMail) -> Result<()> {
    let email = mail.email.as_str();
    let requester = &mail.requester;
    let invite = mail.invite.as_deref();
    let attempts = mail.attempts;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM identities WHERE provider='email' AND subject=?)",
    )
    .bind(email)
    .fetch_one(&s.db)
    .await?;
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_invites WHERE hash=? AND email=? AND expires>?)",
    )
    .bind(invite)
    .bind(email)
    .bind(now())
    .fetch_one(&s.db)
    .await?;
    if !exists && !allowed && s.config.bootstrap_email.as_deref() != Some(email) {
        return Ok(());
    }
    // One requester cannot use up the whole recipient allowance. The second
    // budget caps delivery attempts across rotating IPs, including retries.
    let mut tx = s.db.begin_with("BEGIN IMMEDIATE").await?;
    let budget = format!("email:{requester}:{}", hash(email));
    if attempts == 0
        && let Err(e) = rate_connection(&mut tx, &budget, 3, 900).await
    {
        if e.status == StatusCode::TOO_MANY_REQUESTS {
            return Ok(());
        }
        return Err(e);
    }
    if let Err(e) =
        rate_connection(&mut tx, &format!("email-recipient:{}", hash(email)), 6, 900).await
    {
        if attempts == 0 && e.status == StatusCode::TOO_MANY_REQUESTS {
            return Ok(());
        }
        return Err(e);
    }
    tx.commit().await?;
    let token = random();
    sqlx::query("INSERT INTO magic_links VALUES (?, ?, ?, ?)")
        .bind(hash(&token))
        .bind(email)
        .bind(invite)
        .bind(now() + 15 * 60)
        .execute(&s.db)
        .await?;
    // Verification must set the host-only cookie on the waiting tab's origin.
    let origin = mail
        .origin
        .as_deref()
        .filter(|origin| s.config.trusts_origin(origin))
        .unwrap_or(&s.config.public_url);
    // This landing path also works when the legacy tab has no cookie yet.
    let url = format!("{origin}/signin#magic={token}");
    if let Err(e) = send(s, email, &url).await {
        // A timeout can happen after provider acceptance. Keep this token
        // valid, and count the attempt, so a late email works and ambiguous
        // failures cannot bypass the recipient cap. Retries mint a fresh link.
        tracing::error!("email delivery failed: {e}");
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Email delivery is unavailable. Please retry later.",
        ));
    }
    Ok(())
}
async fn send(s: &AppState, email: &str, url: &str) -> anyhow::Result<()> {
    if let Some(dir) = &s.config.mail_dir {
        use std::io::Write;
        std::fs::create_dir_all(dir)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(dir.join(format!("{}.txt", hash(email))))?;
        writeln!(file, "Sign in to Cubie (expires in 15 minutes):\n{url}")?;
        return Ok(());
    }
    s.http.post("https://api.resend.com/emails").bearer_auth(s.config.resend_api_key.as_deref().unwrap_or_default())
        .json(&json!({"from":s.config.mail_from,"to":[email],"subject":"Sign in to Cubie","text":format!("Sign in to Cubie:\n{url}\n\nThis link expires in 15 minutes and can only be used once. If you did not request it, ignore this email.")}))
        .send().await?.error_for_status()?;
    Ok(())
}
#[derive(Deserialize)]
pub struct Token {
    pub token: String,
}
pub async fn verify_link(State(s): State<AppState>, Json(req): Json<Token>) -> Result<Response> {
    let mut tx = s.db.begin().await?;
    let (email, invite): (String, Option<String>) = sqlx::query_as(
        "DELETE FROM magic_links WHERE hash=? AND expires>? RETURNING email, invite",
    )
    .bind(hash(&req.token))
    .bind(now())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::bad("This sign-in link is invalid or expired"))?;
    let mut p = sqlx::query_as::<_, Principal>("SELECT p.* FROM principals p JOIN identities i ON i.principal=p.id WHERE i.provider='email' AND i.subject=?").bind(&email).fetch_optional(&mut *tx).await?;
    if p.is_none() {
        let admin = s.config.bootstrap_email.as_deref() == Some(&email);
        let used =
            sqlx::query("DELETE FROM account_invites WHERE hash=? AND email=? AND expires>?")
                .bind(invite)
                .bind(&email)
                .bind(now())
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if !admin && used != 1 {
            return Err(AppError::forbidden(
                "Registration requires an unused invitation",
            ));
        }
        let new = Principal {
            id: random(),
            name: email.split('@').next().unwrap().into(),
            kind: "user".into(),
            email: Some(email.clone()),
            admin,
        };
        sqlx::query("INSERT INTO principals VALUES (?, 'user', ?, ?, ?)")
            .bind(&new.id)
            .bind(&new.name)
            .bind(&email)
            .bind(admin)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO identities VALUES ('email', ?, ?)")
            .bind(&email)
            .bind(&new.id)
            .execute(&mut *tx)
            .await?;
        p = Some(new);
    }
    let p = p.unwrap();
    let token = session(&mut tx, &p).await?;
    sqlx::query("DELETE FROM rate_limits WHERE key LIKE ?")
        .bind(format!("email:%:{}", hash(&email)))
        .execute(&mut *tx)
        .await?;
    // Invalidate other outstanding links for this email after successful use.
    sqlx::query("DELETE FROM magic_links WHERE email=?")
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(signed_in(&s, &token, &p))
}
pub async fn desktop_start(State(s): State<AppState>) -> Result<Json<Value>> {
    let id = random();
    let secret = random();
    // Eight hex characters are typed from the initiating Mac, never put in a URL.
    let code = random()[..8].to_uppercase();
    sqlx::query("INSERT INTO desktop_logins(id,secret_hash,expires,code_hash) VALUES (?,?,?,?)")
        .bind(&id)
        .bind(hash(&secret))
        .bind(now() + 600)
        .bind(hash(&code))
        .execute(&s.db)
        .await?;
    Ok(Json(json!({"id":id,"secret":secret,"code":code})))
}
#[derive(Deserialize)]
pub struct DesktopApproval {
    pub id: String,
    #[serde(default)]
    pub code: String,
}
pub async fn desktop_approve(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<DesktopApproval>,
) -> Result<Json<Value>> {
    let p = current(&s, &h).await?;
    let code = req.code.trim().to_ascii_uppercase();
    let mut tx = s.db.begin().await?;
    let expected: Option<String> = sqlx::query_scalar(
        "UPDATE desktop_logins SET attempts=attempts+1 WHERE id=? AND principal IS NULL AND expires>? AND attempts<5 AND code_hash IS NOT NULL RETURNING code_hash")
        .bind(&req.id).bind(now()).fetch_optional(&mut *tx).await?;
    if expected.as_deref() != Some(hash(&code).as_str()) {
        // Commit failed guesses too; a challenge gets five attempts total.
        tx.commit().await?;
        return Err(AppError::bad(
            "Check the code shown in Cubie on your Mac, or restart sign-in",
        ));
    }
    sqlx::query("UPDATE desktop_logins SET principal=? WHERE id=?")
        .bind(p.id)
        .bind(req.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
pub struct DesktopPoll {
    pub id: String,
    pub secret: String,
}
pub async fn desktop_poll(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<DesktopPoll>,
) -> Result<Json<Value>> {
    let mut tx = s.db.begin_with("BEGIN IMMEDIATE").await?;
    let pending: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT principal FROM desktop_logins WHERE id=? AND secret_hash=? AND expires>?",
    )
    .bind(&req.id)
    .bind(hash(&req.secret))
    .bind(now())
    .fetch_optional(&mut *tx)
    .await?;
    let (principal,) = pending.ok_or_else(|| AppError::bad("Desktop sign-in expired"))?;
    // Only a valid private proof can consume this challenge's quota. Polling
    // has its own IP budget and cannot block browser approval or email login.
    rate_connection(&mut tx, &format!("desktop-poll:{}", req.id), 60, 60).await?;
    let Some(principal) = principal else {
        tx.commit().await?;
        return Ok(Json(json!({"pending":true})));
    };
    sqlx::query("DELETE FROM desktop_logins WHERE id=?")
        .bind(req.id)
        .execute(&mut *tx)
        .await?;
    let p: Principal = sqlx::query_as("SELECT * FROM principals WHERE id=?")
        .bind(principal)
        .fetch_one(&mut *tx)
        .await?;
    let token = session(&mut tx, &p).await?;
    sqlx::query("UPDATE sessions SET pending=1,expires=?,replaces=? WHERE hash=?")
        .bind(now() + 600)
        .bind(credential(&h).map(hash))
        .bind(hash(&token))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"token":token,"principal":p})))
}

/// Database housekeeping has no dependency on the media service.
pub async fn cleanup(s: &AppState) -> Result<()> {
    for table in [
        "sessions",
        "account_invites",
        "magic_links",
        "desktop_logins",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE expires < ?"))
            .bind(now())
            .execute(&s.db)
            .await?;
    }
    crate::rooms::cleanup_invites(&s.db).await?;
    sqlx::query("DELETE FROM mail_requests WHERE created < ?")
        .bind(now() - 900)
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM rate_limits WHERE expires < ?")
        .bind(now())
        .execute(&s.db)
        .await?;
    // Keep durable access (including revocation tombstones) and every foreign
    // key reference. Delete a bounded batch of guests that can no longer return.
    sqlx::query(
        "DELETE FROM principals WHERE id IN (
        SELECT p.id FROM principals p WHERE p.kind='guest'
        AND NOT EXISTS(SELECT 1 FROM sessions WHERE principal=p.id)
        AND NOT EXISTS(SELECT 1 FROM memberships WHERE principal=p.id)
        AND NOT EXISTS(SELECT 1 FROM desktop_logins WHERE principal=p.id)
        AND NOT EXISTS(SELECT 1 FROM rooms WHERE owner=p.id OR guest=p.id)
        AND NOT EXISTS(SELECT 1 FROM room_invites WHERE used_by=p.id)
        AND NOT EXISTS(SELECT 1 FROM identities WHERE principal=p.id)
        AND NOT EXISTS(SELECT 1 FROM account_invites WHERE created_by=p.id)
        LIMIT 1000)",
    )
    .execute(&s.db)
    .await?;
    Ok(())
}
