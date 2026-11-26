//! Application HTTP independent of call lifetimes, usable from any UI executor.
pub use futures_util::future::join;
pub use reqwest::Url;
use serde_json::Value;
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};
use tokio::time::Instant;

/// Local identity for credentials and history. The production aliases share
/// the original key so installations keep their sign-in and previous calls.
/// Custom servers retain their own keys.
pub fn storage_origin(server: &str) -> &str {
    match server.trim_end_matches('/') {
        "https://cubiecall.com"
        | "https://cubiecall.ru"
        | "https://cubycall.com"
        | "https://quipcall.com" => "https://quipcall.com",
        _ => server,
    }
}

#[derive(Debug)]
pub struct RequestError {
    pub status: u16,
    pub message: String,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RequestError {}

/// Retryable polling failures; credential/proof errors and malformed successes
/// remain terminal. The caller owns its deadline, backoff and cancellation.
pub fn transient(error: &anyhow::Error) -> bool {
    if error.is::<Unreachable>() {
        return true;
    }
    if let Some(e) = error.downcast_ref::<RequestError>() {
        return matches!(e.status, 408 | 429 | 500..=599);
    }
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|e| e.is_connect() || e.is_timeout() || e.is_body() || e.is_request())
}

fn client(failover: bool) -> anyhow::Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    static FAILOVER_CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    let cache = if failover { &FAILOVER_CLIENT } else { &CLIENT };
    cache
        .get_or_init(|| {
            let builder = reqwest::Client::builder().timeout(Duration::from_secs(15));
            if failover {
                // A connect error can only be replayed if it was for the original
                // request, before any bytes reached its destination.
                builder
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(3))
                    .build()
            } else {
                builder.build()
            }
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Debug)]
struct Unreachable;

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Can't reach Cubie")
    }
}

impl std::error::Error for Unreachable {}

const PRIMARY_RECHECK: Duration = Duration::from_secs(60);
const OFFLINE_RECHECK: Duration = Duration::from_secs(1);
const REPEATED_FAILURE_WINDOW: Duration = Duration::from_secs(30);
const MAX_TRACKED_FAILURES: usize = 32;
const MAX_ATTEMPTS: u64 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Primary,
    Reserve,
}

impl Route {
    fn other(self) -> Self {
        match self {
            Self::Primary => Self::Reserve,
            Self::Reserve => Self::Primary,
        }
    }
}

struct Selection {
    route: Option<Route>,
    recheck_at: Option<Instant>,
    request_failures: Vec<RequestFailure>,
}

struct RequestFailure {
    method: reqwest::Method,
    path: String,
    expires: Instant,
}

impl Selection {
    fn probe_due(&self) -> bool {
        self.recheck_at.is_none_or(|at| Instant::now() >= at)
    }
}

pub struct Failover {
    primary: &'static str,
    reserve: &'static str,
    // Links continue to use the last working route even during an outage.
    last_used_reserve: AtomicBool,
    selection: tokio::sync::Mutex<Selection>,
}

impl Failover {
    pub const fn new(primary: &'static str, reserve: &'static str) -> Self {
        Self {
            primary,
            reserve,
            last_used_reserve: AtomicBool::new(false),
            selection: tokio::sync::Mutex::const_new(Selection {
                route: None,
                recheck_at: None,
                request_failures: Vec::new(),
            }),
        }
    }

    pub fn origin(&self) -> &'static str {
        if self.last_used_reserve.load(Relaxed) {
            self.reserve
        } else {
            self.primary
        }
    }

    fn origin_for(&self, route: Route) -> &'static str {
        match route {
            Route::Primary => self.primary,
            Route::Reserve => self.reserve,
        }
    }

    fn remember(&self, selection: &mut Selection, route: Option<Route>) {
        if selection.route != route {
            selection.request_failures.clear();
        }
        selection.route = route;
        selection.recheck_at = match route {
            Some(Route::Primary) => None,
            Some(Route::Reserve) => Some(Instant::now() + PRIMARY_RECHECK),
            None => Some(Instant::now() + OFFLINE_RECHECK),
        };
        if let Some(route) = route {
            self.last_used_reserve
                .store(route == Route::Reserve, Relaxed);
        }
    }

    async fn healthy(&self, client: &reqwest::Client, route: Route) -> bool {
        let health = client
            .get(format!("{}/api/health", self.origin_for(route)))
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        if let Ok(response) = health
            && response.status() == reqwest::StatusCode::OK
            && response.content_length().is_none_or(|len| len == 2)
        {
            return response
                .bytes()
                .await
                .is_ok_and(|body| body.as_ref() == b"ok");
        }
        false
    }

    async fn select(&'static self, client: &reqwest::Client) -> Result<Route, Unreachable> {
        let mut selection = self.selection.lock().await;
        if !selection.probe_due() || selection.route == Some(Route::Primary) {
            return selection.route.ok_or(Unreachable);
        }
        if selection.route == Some(Route::Reserve) {
            let next_probe = Instant::now() + PRIMARY_RECHECK;
            selection.recheck_at = Some(next_probe);
            let client = client.clone();
            tokio::spawn(async move {
                if self.healthy(&client, Route::Primary).await {
                    let mut selection = self.selection.lock().await;
                    if selection.route == Some(Route::Reserve)
                        && selection.recheck_at == Some(next_probe)
                    {
                        self.remember(&mut selection, Some(Route::Primary));
                    }
                }
            });
            return Ok(Route::Reserve);
        }
        let route = if self.healthy(client, Route::Primary).await {
            Some(Route::Primary)
        } else if self.healthy(client, Route::Reserve).await {
            Some(Route::Reserve)
        } else {
            None
        };
        self.remember(&mut selection, route);
        route.ok_or(Unreachable)
    }

    async fn alternate(
        &self,
        client: &reqwest::Client,
        failed: Route,
        recheck: Option<&ApiRequest>,
    ) -> Option<Route> {
        let mut selection = self.selection.lock().await;
        let other = failed.other();
        if selection.route == Some(other) {
            return Some(other);
        }
        if selection.route.is_none() && !selection.probe_due() {
            return None;
        }
        // A slow handler or a transient error during a deploy needn't move
        // a healthy route. Repeated errors can be specific to an API path.
        if let Some(request) = recheck
            && self.healthy(client, failed).await
        {
            let now = Instant::now();
            let failures = &mut selection.request_failures;
            failures.retain(|previous| now < previous.expires);
            if !failures
                .iter()
                .any(|previous| previous.method == request.method && previous.path == request.path)
            {
                if failures.len() == MAX_TRACKED_FAILURES {
                    failures.remove(0);
                }
                failures.push(RequestFailure {
                    method: request.method.clone(),
                    path: request.path.clone(),
                    expires: now + REPEATED_FAILURE_WINDOW,
                });
                return None;
            }
        }
        let route = self.healthy(client, other).await.then_some(other);
        self.remember(&mut selection, route);
        route
    }

    /// Resolve a fresh route for an independent consumer such as the updater.
    /// Its probes leave the API's operation-specific failure history alone.
    pub async fn reachable_origin(&'static self, previous: &'static str) -> &'static str {
        if let (Ok(runtime), Ok(client)) = (crate::runtime(), client(true)) {
            return runtime
                .spawn(async move {
                    if self.healthy(client, Route::Primary).await {
                        self.primary
                    } else if self.healthy(client, Route::Reserve).await {
                        self.reserve
                    } else {
                        previous
                    }
                })
                .await
                .unwrap_or(previous);
        }
        previous
    }

    async fn send(
        &'static self,
        client: &reqwest::Client,
        request: &ApiRequest,
        retry: bool,
    ) -> anyhow::Result<Value> {
        let mut attempt = 1;
        let mut route = loop {
            match self.select(client).await {
                Ok(route) => break route,
                Err(_) if retry && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(OFFLINE_RECHECK.max(Duration::from_millis(500 * attempt)))
                        .await;
                    attempt += 1;
                }
                Err(error) => return Err(error.into()),
            }
        };
        let mut attempt = 1;
        let mut switched = false;
        loop {
            let result = request.send(client, self.origin_for(route), false).await;
            let Err(error) = &result else {
                if attempt > 1 {
                    let mut selection = self.selection.lock().await;
                    if selection.route.is_none() {
                        self.remember(&mut selection, Some(route));
                    }
                }
                return result;
            };
            let transport = error.downcast_ref::<reqwest::Error>();
            let status = error.downcast_ref::<RequestError>().map(|e| e.status);
            let gateway_error = status.is_some_and(|s| matches!(s, 502..=504));
            let transport_error = transport.is_some_and(|e| {
                e.is_connect() || e.is_timeout() || e.is_body() || e.is_request() || e.is_decode()
            });
            let redirected = status.is_some_and(|s| matches!(s, 300..=399));
            if !transport_error && !gateway_error && !redirected {
                return result;
            }
            let connect_error = transport.is_some_and(reqwest::Error::is_connect);
            let other = self
                .alternate(client, route, (!connect_error).then_some(request))
                .await;
            // General writes may already have committed. Admission alone
            // opts into retrying ambiguous transport failures.
            let safe_to_retry =
                matches!(request.method, reqwest::Method::GET | reqwest::Method::HEAD)
                    || connect_error
                    || retry;
            if !switched
                && safe_to_retry
                && attempt < MAX_ATTEMPTS
                && let Some(other) = other
            {
                route = other;
                switched = true;
                attempt += 1;
                continue;
            }
            if !retry || !(transport_error || gateway_error) || attempt >= MAX_ATTEMPTS {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
            attempt += 1;
        }
    }
}

#[derive(Clone, Copy)]
pub enum Endpoint<S> {
    Direct(S),
    Redundant(&'static Failover),
}

struct ApiRequest {
    token: Option<String>,
    path: String,
    body: Option<Value>,
    method: reqwest::Method,
}

impl ApiRequest {
    async fn send(
        &self,
        client: &reqwest::Client,
        server: &str,
        retry: bool,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api{}", server.trim_end_matches('/'), self.path);
        let mut attempt = 1;
        let response = loop {
            let mut req = client
                .request(self.method.clone(), &url)
                .header("X-Cubie", "1");
            if let Some(token) = &self.token {
                req = req.bearer_auth(token);
            }
            if let Some(body) = &self.body {
                req = req.json(body);
            }
            match req.send().await {
                Ok(response) => break response,
                Err(e) if retry && attempt < MAX_ATTEMPTS && (e.is_connect() || e.is_timeout()) => {
                    tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        };
        let status = response.status();
        if !status.is_success() {
            let value: Value = response.json().await.unwrap_or_default();
            return Err(RequestError {
                status: status.as_u16(),
                message: value["error"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Request failed ({status})")),
            }
            .into());
        }
        Ok(response.json().await?)
    }
}

pub fn request(
    server: &str,
    token: Option<&str>,
    path: &str,
    body: Option<Value>,
    method: &str,
) -> impl Future<Output = anyhow::Result<Value>> + Send + 'static + use<> {
    Endpoint::Direct(server).request(token, path, body, method)
}

impl Endpoint<&str> {
    pub fn request(
        self,
        token: Option<&str>,
        path: &str,
        body: Option<Value>,
        method: &str,
    ) -> impl Future<Output = anyhow::Result<Value>> + Send + 'static + use<> {
        self.request_with_retry(token, path, body, method, false)
    }

    pub(crate) fn request_with_retry(
        self,
        token: Option<&str>,
        path: &str,
        body: Option<Value>,
        method: &str,
        retry: bool,
    ) -> impl Future<Output = anyhow::Result<Value>> + Send + 'static + use<> {
        let endpoint = match self {
            Self::Direct(server) => Endpoint::Direct(server.to_owned()),
            Self::Redundant(failover) => Endpoint::Redundant(failover),
        };
        let token = token.map(str::to_owned);
        let path = path.to_owned();
        let method = method.parse();
        async move {
            crate::runtime()?
                .spawn(async move {
                    let request = ApiRequest {
                        token,
                        path,
                        body,
                        method: method?,
                    };
                    match endpoint {
                        Endpoint::Redundant(failover) => {
                            failover.send(client(true)?, &request, retry).await
                        }
                        Endpoint::Direct(server) => {
                            request.send(client(false)?, &server, retry).await
                        }
                    }
                })
                .await?
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_fixture(
        replies: &[(&'static str, &'static str, &'static str)],
    ) -> (&'static str, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap()).leak();
        let replies = replies.to_vec();
        let server = std::thread::spawn(move || {
            for (expected, status, body) in replies {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut socket);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.trim_end(), expected);
                let mut length = 0;
                let mut authorized = false;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line.trim_end().is_empty() {
                        break;
                    }
                    let line = line.trim_end().to_ascii_lowercase();
                    if let Some(len) = line.strip_prefix("content-length: ") {
                        length = len.parse().unwrap();
                    }
                    authorized |= line == "authorization: bearer shared-session";
                }
                assert_eq!(authorized, !expected.contains("/api/health"));
                reader.read_exact(&mut vec![0; length]).unwrap();
                if status == "timeout" {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                let status = if status == "slow success" {
                    std::thread::sleep(Duration::from_millis(300));
                    "200 OK"
                } else {
                    status
                };
                let location = if status.starts_with("302 ") {
                    "Location: http://127.0.0.1:1/elsewhere\r\n"
                } else {
                    ""
                };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (origin, server)
    }

    #[test]
    fn failover_detects_a_block_page_and_reuses_the_reserve_for_writes() {
        let (primary, blocked) = api_fixture(&[("GET /api/health HTTP/1.1", "200 OK", "blocked")]);
        let (reserve, healthy) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "200 OK", r#"{"id":"room"}"#),
            (
                "GET /api/rooms HTTP/1.1",
                "401 Unauthorized",
                r#"{"error":"Sign in"}"#,
            ),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        let endpoint = Endpoint::Redundant(route);
        crate::runtime().unwrap().block_on(async {
            let room = endpoint
                .request(
                    Some("shared-session"),
                    "/rooms",
                    Some(serde_json::json!({})),
                    "POST",
                )
                .await
                .unwrap();
            assert_eq!(room["id"], "room");
            let denied = endpoint
                .request(Some("shared-session"), "/rooms", None, "GET")
                .await
                .unwrap_err();
            assert_eq!(denied.downcast_ref::<RequestError>().unwrap().status, 401);
            assert_eq!(route.origin(), reserve);
        });
        blocked.join().unwrap();
        healthy.join().unwrap();
    }

    #[test]
    fn gateway_errors_keep_healthy_primary_and_do_not_replay_writes() {
        let (primary, server) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "504 Gateway Timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "502 Bad Gateway", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, "http://127.0.0.1:1")));
        let endpoint = Endpoint::Redundant(route);
        crate::runtime().unwrap().block_on(async {
            let error = endpoint
                .request(
                    Some("shared-session"),
                    "/rooms",
                    Some(serde_json::json!({})),
                    "POST",
                )
                .await
                .unwrap_err();
            assert_eq!(error.downcast_ref::<RequestError>().unwrap().status, 504);
            assert_eq!(route.origin(), primary);
            let error = endpoint
                .request(Some("shared-session"), "/rooms", None, "GET")
                .await
                .unwrap_err();
            assert_eq!(error.downcast_ref::<RequestError>().unwrap().status, 502);
            assert!(route.selection.lock().await.route == Some(Route::Primary));
        });
        server.join().unwrap();
    }

    #[test]
    fn repeated_gateway_errors_can_move_the_next_write_to_the_reserve() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "504 Gateway Timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "504 Gateway Timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
        ]);
        let (reserve, backup) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "200 OK", r#"{"id":"room"}"#),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        let endpoint = Endpoint::Redundant(route);
        crate::runtime().unwrap().block_on(async {
            let post = || {
                endpoint.request(
                    Some("shared-session"),
                    "/rooms",
                    Some(serde_json::json!({})),
                    "POST",
                )
            };
            assert_eq!(
                post()
                    .await
                    .unwrap_err()
                    .downcast_ref::<RequestError>()
                    .unwrap()
                    .status,
                504
            );
            assert_eq!(route.origin(), primary);
            assert_eq!(
                post()
                    .await
                    .unwrap_err()
                    .downcast_ref::<RequestError>()
                    .unwrap()
                    .status,
                504
            );
            assert_eq!(route.origin(), reserve);
            assert_eq!(post().await.unwrap()["id"], "room");
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn interleaved_gateway_errors_keep_separate_failure_histories() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/auth/me HTTP/1.1", "502 Bad Gateway", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "502 Bad Gateway", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/auth/me HTTP/1.1", "502 Bad Gateway", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
        ]);
        let (reserve, backup) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            (
                "GET /api/auth/me HTTP/1.1",
                "200 OK",
                r#"{"via":"reserve"}"#,
            ),
        ]);
        let endpoint = Endpoint::Redundant(Box::leak(Box::new(Failover::new(primary, reserve))));
        crate::runtime().unwrap().block_on(async {
            let get = |path| endpoint.request(Some("shared-session"), path, None, "GET");
            assert!(get("/auth/me").await.is_err());
            assert!(get("/rooms").await.is_err());
            assert_eq!(get("/auth/me").await.unwrap()["via"], "reserve");
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn failover_returns_to_primary_without_holding_up_reserve_requests() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "blocked"),
            ("GET /api/health HTTP/1.1", "slow success", "ok"),
            ("GET /api/rooms HTTP/1.1", "200 OK", r#"{"via":"primary"}"#),
        ]);
        let (reserve, backup) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "200 OK", r#"{"via":"reserve"}"#),
            ("GET /api/rooms HTTP/1.1", "200 OK", r#"{"via":"reserve"}"#),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        let endpoint = Endpoint::Redundant(route);
        crate::runtime().unwrap().block_on(async {
            let get = || endpoint.request(Some("shared-session"), "/rooms", None, "GET");
            assert_eq!(get().await.unwrap()["via"], "reserve");
            route.selection.lock().await.recheck_at = None;
            assert_eq!(get().await.unwrap()["via"], "reserve");
            tokio::time::timeout(Duration::from_secs(3), async {
                while route.origin() == reserve {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(get().await.unwrap()["via"], "primary");
            assert_eq!(route.origin(), primary);
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn failover_preserves_opted_in_admission_retries() {
        let (primary, server) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/join HTTP/1.1", "timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            (
                "POST /api/join HTTP/1.1",
                "200 OK",
                r#"{"token":"call-token"}"#,
            ),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, "http://127.0.0.1:1")));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        let request = ApiRequest {
            token: Some("shared-session".into()),
            path: "/join".into(),
            body: Some(serde_json::json!({"room":"test"})),
            method: reqwest::Method::POST,
        };
        crate::runtime().unwrap().block_on(async {
            let result = route.send(&client, &request, true).await.unwrap();
            assert_eq!(result["token"], "call-token");
        });
        server.join().unwrap();
    }

    #[test]
    fn admission_retries_a_gateway_error_on_the_same_healthy_route() {
        let (primary, server) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/join HTTP/1.1", "502 Bad Gateway", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            (
                "POST /api/join HTTP/1.1",
                "200 OK",
                r#"{"token":"call-token"}"#,
            ),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, "http://127.0.0.1:1")));
        crate::runtime().unwrap().block_on(async {
            let result = Endpoint::Redundant(route)
                .request_with_retry(
                    Some("shared-session"),
                    "/join",
                    Some(serde_json::json!({})),
                    "POST",
                    true,
                )
                .await
                .unwrap();
            assert_eq!(result["token"], "call-token");
        });
        server.join().unwrap();
    }

    #[test]
    fn admission_can_reach_the_server_on_its_third_probe() {
        let offline = ("GET /api/health HTTP/1.1", "503 Unavailable", "");
        let (primary, main) = api_fixture(&[
            offline,
            offline,
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            (
                "POST /api/join HTTP/1.1",
                "200 OK",
                r#"{"token":"call-token"}"#,
            ),
        ]);
        let (reserve, backup) = api_fixture(&[offline, offline]);
        let endpoint = Endpoint::Redundant(Box::leak(Box::new(Failover::new(primary, reserve))));
        crate::runtime().unwrap().block_on(async {
            let result = endpoint
                .request_with_retry(
                    Some("shared-session"),
                    "/join",
                    Some(serde_json::json!({})),
                    "POST",
                    true,
                )
                .await
                .unwrap();
            assert_eq!(result["token"], "call-token");
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn updater_rechecks_both_routes_without_resetting_the_api_route() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "503 Unavailable", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
        ]);
        let (reserve, backup) = api_fixture(&[("GET /api/health HTTP/1.1", "200 OK", "ok")]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        crate::runtime().unwrap().block_on(async {
            assert_eq!(route.reachable_origin(primary).await, reserve);
            assert_eq!(route.origin(), primary);
            assert_eq!(route.reachable_origin(reserve).await, primary);
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn offline_callers_share_the_failed_probe_and_keep_the_last_link_origin() {
        let (primary, main) = api_fixture(&[("GET /api/health HTTP/1.1", "503 Unavailable", "")]);
        let (reserve, backup) = api_fixture(&[("GET /api/health HTTP/1.1", "503 Unavailable", "")]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        route.last_used_reserve.store(true, Relaxed);
        let endpoint = Endpoint::Redundant(route);
        crate::runtime().unwrap().block_on(async {
            let get = || endpoint.request(Some("shared-session"), "/rooms", None, "GET");
            let (a, b) = join(get(), get()).await;
            assert!(transient(&a.unwrap_err()));
            assert!(transient(&b.unwrap_err()));
            assert_eq!(route.origin(), reserve);
            assert!(!route.selection.lock().await.probe_due());
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn repeated_write_timeouts_select_the_reserve_without_replaying_the_write() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "timeout", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
        ]);
        let (reserve, backup) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "200 OK", r#"{"id":"room"}"#),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        let request = ApiRequest {
            token: Some("shared-session".into()),
            path: "/rooms".into(),
            body: Some(serde_json::json!({})),
            method: reqwest::Method::POST,
        };
        crate::runtime().unwrap().block_on(async {
            for expected in [primary, reserve] {
                let error = route.send(&client, &request, false).await.unwrap_err();
                assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_timeout());
                assert_eq!(route.origin(), expected);
            }
            assert_eq!(route.origin(), reserve);
            assert_eq!(
                route.send(&client, &request, false).await.unwrap()["id"],
                "room"
            );
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn unexpected_api_redirects_use_the_other_domain_instead() {
        let (primary, main) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "302 Found", ""),
            ("GET /api/health HTTP/1.1", "200 OK", "blocked"),
        ]);
        let (reserve, backup) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "200 OK", r#"{"via":"reserve"}"#),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
        crate::runtime().unwrap().block_on(async {
            let value = Endpoint::Redundant(route)
                .request(Some("shared-session"), "/rooms", None, "GET")
                .await
                .unwrap();
            assert_eq!(value["via"], "reserve");
        });
        main.join().unwrap();
        backup.join().unwrap();
    }

    #[test]
    fn reserve_reuses_the_production_identity() {
        assert_eq!(
            storage_origin("https://cubiecall.ru/"),
            storage_origin("https://cubiecall.com")
        );
        assert_eq!(
            storage_origin("https://cubiecall.ru.example"),
            "https://cubiecall.ru.example"
        );
    }

    #[test]
    fn polling_recovers_after_a_server_drops_the_connection() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let responder = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut headers = Vec::new();
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                }
                if attempt == 1 {
                    let body = r#"{"pending":true}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            }
        });
        crate::runtime().unwrap().block_on(async {
            let error = request(&server, None, "/auth/desktop/poll", None, "POST")
                .await
                .unwrap_err();
            assert!(transient(&error));
            assert_eq!(
                request(&server, None, "/auth/desktop/poll", None, "POST")
                    .await
                    .unwrap()["pending"],
                true
            );
        });
        responder.join().unwrap();
    }

    #[test]
    fn polling_retries_transport_and_server_errors_only() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(transient(
                &RequestError {
                    status,
                    message: "temporary".into()
                }
                .into()
            ));
        }
        for status in [400, 401, 403, 404, 410] {
            assert!(!transient(
                &RequestError {
                    status,
                    message: "terminal".into()
                }
                .into()
            ));
        }
        assert!(!transient(&anyhow::anyhow!("Malformed response")));
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", socket.local_addr().unwrap());
        drop(socket);
        let error = crate::runtime()
            .unwrap()
            .block_on(request(&server, None, "/auth/desktop/poll", None, "POST"))
            .unwrap_err();
        assert!(transient(&error));
    }

    #[test]
    fn engines_have_separate_runtimes_and_http_reuses_its_client() {
        let (a, _) = crate::Engine::new().unwrap();
        let (b, _) = crate::Engine::new().unwrap();
        assert!(!std::ptr::eq(a.rt(), b.rt()));
        assert!(!std::ptr::eq(a.rt(), crate::runtime().unwrap()));
        assert!(std::ptr::eq(client(false).unwrap(), client(false).unwrap()));
    }

    #[test]
    fn admission_waits_for_permission_and_preserves_an_expired_session_status() {
        use futures_util::FutureExt as _;
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let (ready, permission) = async_channel::bounded::<()>(1);
        crate::runtime().unwrap().block_on(async {
            let permission = async move { permission.recv().await?; anyhow::Ok(()) };
            let request = Endpoint::Direct(server.as_str()).request_with_retry(Some("expired-session"), "/join", Some(serde_json::json!({"room":"test"})), "POST", true);
            let mut joining = std::pin::pin!(crate::after_permission(permission, request));
            assert!(joining.as_mut().now_or_never().is_none());
            assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
            // No admission request exists while the prompt remains unanswered,
            // irrespective of how long the user leaves it open.
            listener.set_nonblocking(false).unwrap();
            let responder = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut bytes = Vec::new(); let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); bytes.push(byte[0]); }
                let headers = String::from_utf8(bytes).unwrap().to_lowercase();
                assert!(headers.starts_with("post /api/join "));
                assert!(headers.contains("authorization: bearer expired-session"));
                let len: usize = headers.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                stream.read_exact(&mut vec![0; len]).unwrap();
                let body = r#"{"error":"Sign in to continue"}"#;
                write!(stream, "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            ready.send(()).await.unwrap();
            let error = joining.await.unwrap_err();
            let error = error.downcast_ref::<RequestError>().unwrap();
            assert_eq!(error.status, 401);
            assert_eq!(error.message, "Sign in to continue");
            responder.join().unwrap();
        });
    }
}
