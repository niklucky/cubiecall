//! Application HTTP independent of call lifetimes, usable from any UI executor.
pub use futures_util::future::join;
pub use reqwest::Url;
use serde_json::Value;
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering::Relaxed},
    },
    time::Duration,
};

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

const UNKNOWN: u8 = 0;
const PRIMARY: u8 = 1;
const RESERVE: u8 = 2;

pub struct Failover {
    primary: &'static str,
    reserve: &'static str,
    selected: AtomicU8,
    probe: tokio::sync::Mutex<()>,
}

impl Failover {
    pub const fn new(primary: &'static str, reserve: &'static str) -> Self {
        Self {
            primary,
            reserve,
            selected: AtomicU8::new(UNKNOWN),
            probe: tokio::sync::Mutex::const_new(()),
        }
    }

    pub fn origin(&self) -> &'static str {
        self.origin_for(self.selected.load(Relaxed))
    }

    fn origin_for(&self, selected: u8) -> &'static str {
        if selected == RESERVE {
            self.reserve
        } else {
            self.primary
        }
    }

    async fn select(&self, client: &reqwest::Client, skip: u8) -> anyhow::Result<u8> {
        // Concurrent operations share one probe, and a working reserve stays
        // selected for the process until it fails.
        let _probe = self.probe.lock().await;
        let selected = self.selected.load(Relaxed);
        if selected != UNKNOWN && selected != skip {
            return Ok(selected);
        }
        for route in [PRIMARY, RESERVE]
            .into_iter()
            .filter(|route| *route != skip)
        {
            let health = client
                .get(format!("{}/api/health", self.origin_for(route)))
                .timeout(Duration::from_secs(3))
                .send()
                .await;
            if let Ok(response) = health
                && response.status() == reqwest::StatusCode::OK
                && response.content_length().is_none_or(|len| len == 2)
                && response
                    .bytes()
                    .await
                    .is_ok_and(|body| body.as_ref() == b"ok")
            {
                self.selected.store(route, Relaxed);
                return Ok(route);
            }
        }
        anyhow::bail!("Can't reach Cubie")
    }

    async fn send(&self, client: &reqwest::Client, request: &ApiRequest) -> anyhow::Result<Value> {
        let first = self.select(client, UNKNOWN).await?;
        let mut route = first;
        loop {
            let result = request.send(client, self.origin_for(route), false).await;
            let Err(error) = &result else { return result };
            let transport = error.downcast_ref::<reqwest::Error>();
            let failed = transport.is_some_and(|e| {
                e.is_connect() || e.is_timeout() || e.is_body() || e.is_request() || e.is_decode()
            }) || error
                .downcast_ref::<RequestError>()
                .is_some_and(|e| matches!(e.status, 502..=504));
            if !failed {
                return result;
            }
            let _ = self
                .selected
                .compare_exchange(route, UNKNOWN, Relaxed, Relaxed);
            // An upstream timeout can happen after a write committed.
            let safe_to_retry =
                matches!(request.method, reqwest::Method::GET | reqwest::Method::HEAD)
                    || transport.is_some_and(reqwest::Error::is_connect);
            if route != first || !safe_to_retry {
                return result;
            }
            let Ok(other) = self.select(client, route).await else {
                return result;
            };
            route = other;
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
                Err(e) if retry && attempt < 3 && (e.is_connect() || e.is_timeout()) => {
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
                            failover.send(client(true)?, &request).await
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
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    fn failover_never_replays_a_write_after_a_gateway_timeout() {
        let (primary, failed) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("POST /api/rooms HTTP/1.1", "504 Gateway Timeout", ""),
        ]);
        let (reserve, healthy) = api_fixture(&[
            ("GET /api/health HTTP/1.1", "200 OK", "ok"),
            ("GET /api/rooms HTTP/1.1", "200 OK", r#"{"rooms":[]}"#),
        ]);
        let route = Box::leak(Box::new(Failover::new(primary, reserve)));
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
            let rooms = endpoint
                .request(Some("shared-session"), "/rooms", None, "GET")
                .await
                .unwrap();
            assert_eq!(rooms["rooms"], serde_json::json!([]));
            assert_eq!(route.origin(), reserve);
        });
        failed.join().unwrap();
        healthy.join().unwrap();
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
