use anyhow::{Context, bail};
use axum::http::HeaderMap;
use std::net::IpAddr;
use url::Url;

#[derive(Clone)]
pub struct Reserve {
    pub origin: String,
    pub signaling: String,
    pub proxy: IpAddr,
}

impl Reserve {
    fn parse(origin: &str, proxy: &str) -> anyhow::Result<Self> {
        let origin = public_origin("CUBIE_RESERVE_URL", origin, false)?;
        Ok(Self {
            signaling: origin.replacen("https://", "wss://", 1),
            origin,
            proxy: proxy
                .parse()
                .context("CUBIE_RESERVE_PROXY_IP must be an IP address")?,
        })
    }
}

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub livekit_public_url: String,
    pub livekit_api_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    pub database_url: String,
    pub public_url: String,
    pub legacy_url: Option<String>,
    pub reserve: Option<Reserve>,
    pub resend_api_key: Option<String>,
    pub mail_from: String,
    pub bootstrap_email: Option<String>,
    /// Explicit local-only email sink. Never enabled by a release deployment.
    pub mail_dir: Option<std::path::PathBuf>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let public_url = public_origin(
            "CUBIE_PUBLIC_URL",
            &env("CUBIE_PUBLIC_URL").unwrap_or_else(|| "http://127.0.0.1:5173".into()),
            true,
        )?;
        let legacy_url = env("CUBIE_LEGACY_URL")
            .map(|url| public_origin("CUBIE_LEGACY_URL", &url, false))
            .transpose()?;
        let reserve = match (env("CUBIE_RESERVE_URL"), env("CUBIE_RESERVE_PROXY_IP")) {
            (None, None) => None,
            (Some(url), Some(proxy)) => Some(Reserve::parse(&url, &proxy)?),
            _ => bail!("Set CUBIE_RESERVE_URL and CUBIE_RESERVE_PROXY_IP together"),
        };
        if let Some(reserve) = &reserve
            && (reserve.origin == public_url || legacy_url.as_deref() == Some(&reserve.origin))
        {
            bail!("CUBIE_RESERVE_URL must identify a distinct origin");
        }
        let url = Url::parse(&public_url)?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let cfg = Self {
            bind: env("CUBIE_BIND").unwrap_or_else(|| "127.0.0.1:8080".into()),
            livekit_public_url: env("LIVEKIT_PUBLIC_URL").unwrap_or_else(|| {
                if local {
                    "ws://127.0.0.1:7880".into()
                } else {
                    public_url.replacen("https://", "wss://", 1)
                }
            }),
            livekit_api_url: env("LIVEKIT_API_URL")
                .unwrap_or_else(|| "http://127.0.0.1:7880".into()),
            livekit_api_key: env("LIVEKIT_API_KEY").context("LIVEKIT_API_KEY is not set")?,
            livekit_api_secret: env("LIVEKIT_API_SECRET")
                .context("LIVEKIT_API_SECRET is not set")?,
            database_url: env("CUBIE_DATABASE_URL").unwrap_or_else(|| {
                // Keep an existing development database together with its WAL.
                let data = std::path::Path::new("dev/data");
                if !data.join("cubie.db").exists() && data.join("quipcall.db").exists() {
                    "sqlite://dev/data/quipcall.db".into()
                } else {
                    "sqlite://dev/data/cubie.db".into()
                }
            }),
            public_url,
            legacy_url,
            reserve,
            resend_api_key: env("RESEND_API_KEY"),
            mail_from: env("CUBIE_MAIL_FROM").unwrap_or_else(|| {
                let host = if local {
                    "example.test"
                } else {
                    url.host_str().unwrap_or("example.test")
                };
                format!("Cubie <login@{host}>")
            }),
            bootstrap_email: env("CUBIE_BOOTSTRAP_EMAIL").map(|s| s.trim().to_lowercase()),
            mail_dir: env("CUBIE_MAIL_DIR").map(Into::into),
        };
        if cfg.livekit_api_secret.len() < 32 {
            bail!("LIVEKIT_API_SECRET must be at least 32 characters");
        }
        if cfg.mail_dir.is_some() && !local {
            bail!("CUBIE_MAIL_DIR is only allowed on localhost");
        }
        if cfg.mail_dir.is_none() && cfg.resend_api_key.is_none() {
            bail!("set RESEND_API_KEY, or CUBIE_MAIL_DIR for local development");
        }
        Ok(cfg)
    }
    pub fn secure_cookie(&self) -> bool {
        self.public_url.starts_with("https://")
    }
    pub fn trusts_origin(&self, origin: &str) -> bool {
        origin == self.public_url
            || self.legacy_url.as_deref() == Some(origin)
            || self
                .reserve
                .as_ref()
                .is_some_and(|reserve| reserve.origin == origin)
    }

    // Origin nginx overwrites this with the immediate peer, including before
    // the reserve vhost changes. X-Real-IP is separate and drives rate limits.
    fn via_reserve(&self, headers: &HeaderMap) -> Option<&Reserve> {
        let peer = headers
            .get("x-forwarded-for")?
            .to_str()
            .ok()?
            .parse::<IpAddr>()
            .ok()?;
        self.reserve
            .as_ref()
            .filter(|reserve| reserve.proxy == peer)
    }

    pub fn link_origin(&self, headers: &HeaderMap) -> &str {
        self.via_reserve(headers)
            .map_or(&self.public_url, |reserve| &reserve.origin)
    }

    pub fn signaling_url(&self, headers: &HeaderMap) -> &str {
        self.via_reserve(headers)
            .map_or(&self.livekit_public_url, |reserve| &reserve.signaling)
    }
}

fn public_origin(key: &str, value: &str, allow_local: bool) -> anyhow::Result<String> {
    let url = Url::parse(value).with_context(|| format!("Invalid {key}"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(allow_local && url.scheme() == "http" && local) {
        bail!("{key} must use HTTPS (except the local primary)");
    }
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("{key} must be an origin without a path or credentials");
    }
    Ok(url.origin().ascii_serialization())
}
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
