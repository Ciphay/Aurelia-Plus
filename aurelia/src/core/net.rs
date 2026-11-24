use crate::core::config::ProxyConfig;
use crate::core::error::{ErrorKind, TypedError};
use std::time::Duration;

const PROXY_VARS: [&str; 3] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"];

fn env_proxy_already_set() -> bool {
    PROXY_VARS.iter().any(|name| {
        std::env::var_os(name).is_some() || std::env::var_os(name.to_ascii_lowercase()).is_some()
    })
}

pub fn install_proxy_env(config: &ProxyConfig) {
    let Some(url) = config
        .url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return;
    };

    if !env_proxy_already_set() {
        for name in PROXY_VARS {
            unsafe { std::env::set_var(name, url) };
        }
    }

    if let Some(no_proxy) = config
        .no_proxy
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if std::env::var_os("NO_PROXY").is_none() && std::env::var_os("no_proxy").is_none() {
            unsafe { std::env::set_var("NO_PROXY", no_proxy) };
        }
    }
}

pub fn validate_proxy_url(url: &str) -> anyhow::Result<()> {
    reqwest::Proxy::all(url)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("invalid proxy URL `{url}`: {err}"))
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;

const HTTP_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF_BASE: Duration = Duration::from_secs(1);
const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

enum Verdict {
    Done,
    RateLimited(Option<Duration>),
    Transient,
}

impl Verdict {
    fn of(resp: &reqwest::Response) -> Self {
        let status = resp.status().as_u16();
        let headers = resp.headers();
        match status {
            429 => Self::RateLimited(retry_after(headers)),
            403 if header_str(headers, "x-ratelimit-remaining") == Some("0") => {
                Self::RateLimited(ratelimit_reset(headers))
            }
            502..=504 => Self::Transient,
            _ => Self::Done,
        }
    }
}

fn header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    header_str(headers, "retry-after")
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn ratelimit_reset(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let reset = header_str(headers, "x-ratelimit-reset")?
        .parse::<u64>()
        .ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(Duration::from_secs(reset.saturating_sub(now).max(1)))
}

fn backoff(attempt: u32) -> Duration {
    let jitter_ms = u64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            % 250,
    );
    RETRY_BACKOFF_BASE * 2u32.saturating_pow(attempt.saturating_sub(1))
        + Duration::from_millis(jitter_ms)
}

fn rate_limited(
    host: &str,
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
) -> TypedError {
    TypedError::new(
        ErrorKind::RateLimited,
        format!(
            "{host} is rate-limiting requests (HTTP {}); wait before retrying",
            status.as_u16()
        ),
    )
    .with_retry_after(retry_after)
}

pub async fn send_with_retry(
    client: &reqwest::Client,
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    use anyhow::Context;
    let request = request.build().context("failed to build HTTP request")?;
    let host = request.url().host_str().unwrap_or("?").to_string();
    for attempt in 1..=HTTP_ATTEMPTS {
        let last = attempt == HTTP_ATTEMPTS;
        let Some(req) = request.try_clone() else {
            return client
                .execute(request)
                .await
                .with_context(|| format!("request to {host} failed"));
        };
        match client.execute(req).await {
            Ok(resp) => match Verdict::of(&resp) {
                Verdict::Done => return Ok(resp),
                Verdict::RateLimited(retry) => match retry {
                    Some(wait) if wait <= RETRY_AFTER_CAP && !last => {
                        tracing::warn!("{host} rate-limited; retrying in {}s", wait.as_secs());
                        tokio::time::sleep(wait).await;
                    }
                    _ => return Err(rate_limited(&host, resp.status(), retry).into()),
                },
                Verdict::Transient => {
                    if last {
                        return Ok(resp);
                    }
                    let wait = backoff(attempt);
                    tracing::warn!(
                        "{host} returned HTTP {}; retrying in {:.1}s",
                        resp.status().as_u16(),
                        wait.as_secs_f32()
                    );
                    tokio::time::sleep(wait).await;
                }
            },
            Err(e) if !last && (e.is_timeout() || e.is_connect()) => {
                let wait = backoff(attempt);
                tracing::warn!(
                    "request to {host} failed ({e}); retrying in {:.1}s",
                    wait.as_secs_f32()
                );
                tokio::time::sleep(wait).await;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("request to {host} failed")));
            }
        }
    }
    unreachable!("loop returns on the last attempt")
}

pub fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    use anyhow::Context;
    reqwest::Client::builder()
        .user_agent("aurelia")
        .timeout(timeout)
        .build()
        .context("failed to build HTTP client")
}

pub fn steam_web_client() -> anyhow::Result<reqwest::Client> {
    http_client(Duration::from_secs(20))
}
