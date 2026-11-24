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
/// A new account has no name until its owner gives one; until then people
/// see the part of its email before the @.
pub fn shown_name(name: &str, email: Option<&str>) -> String {
    if !name.is_empty() {
        return name.into();
    }
    email
        .and_then(|email| email.split('@').next())
        .filter(|local| !local.is_empty())
        .unwrap_or("Guest")
        .into()
}
/// [`shown_name`] in SQL, for a `principals` row called `p`.
pub const SHOWN_NAME: &str = "CASE WHEN p.name='' THEN COALESCE(substr(p.email,1,instr(p.email,'@')-1),'Guest') ELSE p.name END";
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
/// What `key` has counted in its current `period`, without counting more.
async fn counted(db: &mut SqliteConnection, key: &str, period: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(count),0) FROM rate_limits WHERE key=? AND window=?",
    )
    .bind(key)
    .bind(now() / period)
    .fetch_one(db)
    .await?)
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
    let users: Vec<(String, String)> = sqlx::query_as(&format!(
        "SELECT p.email,{SHOWN_NAME} FROM principals p WHERE p.kind='user' AND p.email IS NOT NULL AND p.id!=?",
    ))
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
    /// An invitation that lets a new person register: one to Cubie, or one
    /// to a private room.
    pub invite: Option<String>,
    /// Asked from an app, which can only take the code.
    #[serde(default)]
    pub app: bool,
}
/// Whether a sign-in email may go to `email`: it has an account, or
/// `invite` (a hash) is an invitation it can register with: one to Cubie, or
/// one to a private room, since whoever is invited to a room may make their
/// account from it.
async fn may_sign_in(s: &AppState, email: &str, invite: Option<&str>) -> Result<bool> {
    if s.config.bootstrap_email.as_deref() == Some(email) {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM identities WHERE provider='email' AND subject=?2)
         OR EXISTS(SELECT 1 FROM account_invites WHERE hash=?1 AND email=?2 AND expires>?3)
         OR EXISTS(SELECT 1 FROM room_invites i JOIN rooms r ON r.id=i.room
            WHERE i.hash=?1 AND i.used_by IS NULL AND i.expires>?3 AND r.closed=0
            AND (i.email IS NULL OR i.email=?2))",
    )
    .bind(invite)
    .bind(email)
    .bind(now())
    .fetch_one(&s.db)
    .await?)
}
/// Addresses one requester may be told nobody invited, an hour: a typo or
/// three, not a list of addresses to try.
const UNKNOWN_EMAILS_AN_HOUR: i64 = 20;
pub async fn request_link(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<LinkRequest>,
) -> Result<Json<Value>> {
    let email = email(&req.email)?;
    let requester = client_ip(&h);
    let invite = req.invite.as_deref().map(hash);
    // Said plainly: someone waiting for a code that will never come has no
    // way to find out why. It tells a stranger who has an account, so it is
    // counted, and a requester who has used the count up is told nothing
    // about any address, known or not, until the hour is over.
    let unknown = format!("unknown-email:{requester}");
    if counted(&mut *s.db.acquire().await?, &unknown, 3600).await? >= UNKNOWN_EMAILS_AN_HOUR {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Please try again later.",
        ));
    }
    if !may_sign_in(&s, &email, invite.as_deref()).await? {
        rate(&s, &unknown, UNKNOWN_EMAILS_AN_HOUR, 3600).await?;
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            if invite.is_some() {
                "This invitation doesn't work for this email. Use the address it was sent to, or ask for a new one."
            } else {
                "No Cubie account uses this email. Cubie is invite-only for now: ask someone on Cubie to invite you."
            },
        ));
    }
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    // The code in the email only works together with this, so reading
    // someone's code off their screen or guessing it from elsewhere is no use.
    let challenge = random();
    sqlx::query(
        "INSERT INTO mail_requests(email,requester,invite,created,origin,challenge,app) VALUES (?,?,?,?,?,?,?)",
    )
    .bind(email)
    .bind(requester)
    .bind(invite)
    .bind(now())
    .bind(origin.filter(|origin| s.config.trusts_origin(origin)))
    .bind(&challenge)
    .bind(req.app)
    .execute(&s.db)
    .await?;
    s.mail_ready.notify_one();
    // Delivery and suppression never affect the response.
    Ok(Json(json!({"ok":true,"id":challenge})))
}

#[derive(FromRow)]
struct QueuedMail {
    id: i64,
    email: String,
    requester: String,
    invite: Option<String>,
    attempts: i64,
    origin: Option<String>,
    challenge: Option<String>,
    app: bool,
}

/// One bounded worker drains the durable queue; never hold an HTTP request open
/// for delivery or expose a provider failure to an account-enumeration probe.
pub async fn deliver_pending(s: &AppState) -> Result<usize> {
    let requests: Vec<QueuedMail> = sqlx::query_as(
        "SELECT id,email,requester,invite,attempts,origin,challenge,app FROM mail_requests WHERE created>? AND available<=? ORDER BY id LIMIT 100",
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
    // Asked again at delivery: an invitation can be used or cancelled while
    // its email waits for a retry.
    if !may_sign_in(s, email, invite).await? {
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
    let code = sign_in_code();
    sqlx::query(
        "INSERT INTO magic_links(hash,email,invite,expires,challenge,code_hash) VALUES (?,?,?,?,?,?)",
    )
    .bind(hash(&token))
    .bind(email)
    .bind(invite)
    .bind(now() + 15 * 60)
    .bind(&mail.challenge)
    .bind(mail.challenge.as_deref().map(|id| code_hash(id, &code)))
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
    if let Err(e) = send(s, email, &code, (!mail.app).then_some(url.as_str())).await {
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
/// Six digits, each as likely as the next.
fn sign_in_code() -> String {
    loop {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("OS randomness unavailable");
        let n = u32::from_le_bytes(b);
        // The top of the range would favour the low codes.
        if n < 4_294_000_000 {
            return format!("{:06}", n % 1_000_000);
        }
    }
}
fn code_hash(challenge: &str, code: &str) -> String {
    hash(&format!("{challenge}:{code}"))
}
/// The email: its code, and for a browser the link too.
fn mail_text(code: &str, url: Option<&str>) -> String {
    match url {
        Some(url) => format!(
            "Your Cubie code: {code}\n\nType it on the page that asked for it. Or, on the same device, open this link:\n{url}\n\nBoth work once, for 15 minutes. If you didn't ask for this, you can ignore this email."
        ),
        None => format!(
            "Your Cubie code: {code}\n\nType it in the Cubie app. It works once, for 15 minutes. If you didn't ask for this, you can ignore this email."
        ),
    }
}
fn mail_html(code: &str, url: Option<&str>) -> String {
    let spaced = format!("{} {}", &code[..3], &code[3..]);
    let link = url.map_or(String::new(), |url| format!(
        "<p style=\"margin:0 0 12px\">Reading this on the device you're signing in on?</p><p style=\"margin:0 0 20px\"><a href=\"{url}\" style=\"display:inline-block;background:#2f6feb;color:#ffffff;text-decoration:none;font-weight:600;padding:11px 20px;border-radius:10px\">Continue to Cubie</a></p>"
    ));
    let place = if url.is_some() {
        "on the page that asked for it"
    } else {
        "in the Cubie app"
    };
    format!(
        "<div style=\"font:16px/1.5 -apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;color:#16181d;max-width:420px\"><p style=\"margin:0 0 6px\">Your Cubie code. Type it {place}:</p><p style=\"margin:0 0 20px;font-size:34px;font-weight:700;letter-spacing:.12em\">{spaced}</p>{link}<p style=\"margin:0;color:#6b7280;font-size:13px\">It works once, for 15 minutes. If you didn't ask for this, you can ignore this email.</p></div>"
    )
}
async fn send(s: &AppState, email: &str, code: &str, url: Option<&str>) -> anyhow::Result<()> {
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
        // The link stays last: local tools read it off the end of the file.
        writeln!(file, "Your Cubie code: {code}")?;
        if let Some(url) = url {
            writeln!(file, "Sign in to Cubie (expires in 15 minutes):\n{url}")?;
        }
        return Ok(());
    }
    s.http.post("https://api.resend.com/emails").bearer_auth(s.config.resend_api_key.as_deref().unwrap_or_default())
        .json(&json!({"from":s.config.mail_from,"to":[email],"subject":format!("{code} is your Cubie code"),"text":mail_text(code, url),"html":mail_html(code, url)}))
        .send().await?.error_for_status()?;
    Ok(())
}
#[derive(Deserialize)]
pub struct Token {
    pub token: String,
}
/// What proves the email is yours: the link in it, or its code together with
/// the challenge the asking browser or app was given.
#[derive(Deserialize)]
pub struct Verify {
    pub token: Option<String>,
    pub id: Option<String>,
    pub code: Option<String>,
    /// An app: it gets a credential to keep instead of a cookie.
    #[serde(default)]
    pub app: bool,
}
/// Wrong codes one challenge takes before it stops working.
const CODE_TRIES: i64 = 5;
/// Wrong codes one requester may give for one email in a day, across
/// challenges: with six digits that leaves a guesser one chance in fifty
/// thousand a day. Counted per requester, so that a stranger guessing can't
/// stop the owner's own codes from working. Links still work.
const WRONG_CODES_A_DAY: i64 = 20;
/// One answer for every way a code fails (wrong, used up, expired, never
/// sent): which of them it was is nobody's to learn by guessing.
fn wrong_code() -> AppError {
    AppError::bad("That code isn't right, or it has expired. Check it, or ask for a new one.")
}
/// The email a code proves, and the invitation it was asked with.
async fn verify_code(
    tx: &mut SqliteConnection,
    requester: &str,
    challenge: &str,
    code: &str,
) -> Result<Option<(String, Option<String>)>> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    #[derive(FromRow)]
    struct Link {
        hash: String,
        email: String,
        invite: Option<String>,
        code_hash: Option<String>,
        attempts: i64,
    }
    // Every link of this challenge: a retried delivery mints another.
    let links: Vec<Link> = sqlx::query_as(
        "SELECT hash,email,invite,code_hash,attempts FROM magic_links WHERE challenge=? AND expires>?",
    )
    .bind(challenge)
    .bind(now())
    .fetch_all(&mut *tx)
    .await?;
    let Some(email) = links.first().map(|link| link.email.clone()) else {
        return Ok(None);
    };
    let budget = format!("wrong-code:{requester}:{}", hash(&email));
    let wrong_today = counted(&mut *tx, &budget, 86_400).await?;
    let expected = code_hash(challenge, &code);
    let found = links
        .iter()
        .find(|link| link.code_hash.as_deref() == Some(expected.as_str()));
    let spent =
        wrong_today >= WRONG_CODES_A_DAY || links.iter().any(|link| link.attempts >= CODE_TRIES);
    let Some(link) = found.filter(|_| !spent) else {
        sqlx::query("UPDATE magic_links SET attempts=attempts+1 WHERE challenge=?")
            .bind(challenge)
            .execute(&mut *tx)
            .await?;
        // Counting past the limit changes nothing; the answer is the same.
        let _ = spend(tx, &budget, 1, WRONG_CODES_A_DAY, 86_400).await;
        return Ok(None);
    };
    sqlx::query("DELETE FROM magic_links WHERE hash=?")
        .bind(&link.hash)
        .execute(&mut *tx)
        .await?;
    Ok(Some((email, link.invite.clone())))
}
pub async fn verify_link(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(req): Json<Verify>,
) -> Result<Response> {
    let mut tx = s.db.begin_with("BEGIN IMMEDIATE").await?;
    let (email, invite): (String, Option<String>) = if let Some(token) = &req.token {
        sqlx::query_as("DELETE FROM magic_links WHERE hash=? AND expires>? RETURNING email, invite")
            .bind(hash(token))
            .bind(now())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::bad("This sign-in link is invalid or expired"))?
    } else {
        let (Some(id), Some(code)) = (&req.id, &req.code) else {
            return Err(AppError::bad("Enter the code from your email"));
        };
        let Some(proved) = verify_code(&mut tx, &client_ip(&h), id, code).await? else {
            // Keep the count of wrong tries.
            tx.commit().await?;
            return Err(wrong_code());
        };
        proved
    };
    let mut p = sqlx::query_as::<_, Principal>("SELECT p.* FROM principals p JOIN identities i ON i.principal=p.id WHERE i.provider='email' AND i.subject=?").bind(&email).fetch_optional(&mut *tx).await?;
    if p.is_none() {
        let admin = s.config.bootstrap_email.as_deref() == Some(&email);
        // No name yet: the account's owner is asked for one, once.
        let new = Principal {
            id: random(),
            name: String::new(),
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
        let mut invited = admin
            || sqlx::query("DELETE FROM account_invites WHERE hash=? AND email=? AND expires>?")
                .bind(&invite)
                .bind(&email)
                .bind(now())
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
        if !invited {
            // A private room's invitation. It is theirs from here on, so one
            // link makes one account.
            let room: Option<String> = sqlx::query_scalar(
                "UPDATE room_invites SET used_by=?1 WHERE hash=?2 AND used_by IS NULL AND expires>?3
                 AND (email IS NULL OR email=?4)
                 AND EXISTS(SELECT 1 FROM rooms r WHERE r.id=room_invites.room AND r.closed=0)
                 RETURNING room",
            )
            .bind(&new.id)
            .bind(&invite)
            .bind(now())
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(room) = room {
                sqlx::query("INSERT INTO memberships VALUES (?,?,0) ON CONFLICT(room,principal) DO UPDATE SET revoked=0")
                    .bind(room)
                    .bind(&new.id)
                    .execute(&mut *tx)
                    .await?;
                invited = true;
            }
        }
        if !invited {
            // Dropping the transaction takes the half-made account with it.
            return Err(AppError::forbidden(
                "Registration requires an unused invitation",
            ));
        }
        p = Some(new);
    }
    let p = p.unwrap();
    let token = session(&mut tx, &p).await?;
    if req.app {
        // As for an approved device: provisional until the app has saved it
        // and used it once, which is also when its previous credential goes.
        sqlx::query("UPDATE sessions SET pending=1,expires=?,replaces=? WHERE hash=?")
            .bind(now() + 600)
            .bind(credential(&h).map(hash))
            .bind(hash(&token))
            .execute(&mut *tx)
            .await?;
    }
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
    if req.app {
        return Ok(Json(json!({"token":token,"principal":p})).into_response());
    }
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
