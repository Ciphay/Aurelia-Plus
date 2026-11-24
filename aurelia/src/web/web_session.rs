use crate::core::error::{ErrorKind, TypedError};
use anyhow::{Context, Result, bail};

pub const ECON_IMAGE_BASE: &str = "https://community.cloudflare.steamstatic.com/economy/image/";

#[derive(Clone)]
pub struct WebSession {
    http: reqwest::Client,
    steam_id: u64,
    sessionid: String,
    cookie: std::sync::Arc<std::sync::Mutex<crate::core::secret::Secret>>,
}

impl WebSession {
    pub fn new(steam_id: u64, web_access_token: &str, country: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("aurelia")
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build HTTP client")?;

        let login_secure = format!("{steam_id}%7C%7C{web_access_token}");
        let sessionid = random_sessionid();
        let mut cookie = format!("steamLoginSecure={login_secure}; sessionid={sessionid}");
        if let Some(cc) = country.filter(|c| !c.is_empty()) {
            cookie.push_str("; steamCountry=");
            cookie.push_str(cc);
        }

        Ok(Self {
            http,
            steam_id,
            sessionid,
            cookie: std::sync::Arc::new(std::sync::Mutex::new(cookie.into())),
        })
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub fn cookies(&self) -> Vec<(String, String)> {
        let header = self.cookie_header();
        header
            .expose()
            .split("; ")
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    pub fn sessionid(&self) -> &str {
        &self.sessionid
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        let mut resp = self.send_get(url).await?;
        if let Some(check) = eligibility_redirect(&resp) {
            self.pass_eligibility_check(&check).await?;
            resp = self.send_get(url).await?;
        }
        read_checked(resp).await
    }

    pub async fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Result<(u16, String)> {
        let cookie = self.cookie_header();
        let mut form: Vec<(&str, &str)> = fields.to_vec();
        form.push(("sessionid", self.sessionid.as_str()));
        let request = self
            .http
            .post(url)
            .header(reqwest::header::COOKIE, cookie.expose())
            .header(reqwest::header::REFERER, "https://steamcommunity.com/")
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form_urlencode(&form));
        let resp = crate::core::net::send_with_retry(&self.http, request).await?;
        if redirect_location(&resp).is_some_and(|l| l.contains("/login")) {
            bail!("the Steam web session was rejected (redirected to login). Run `aurelia login --reconnect`.");
        }
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    }

    pub async fn market_eligibility(&self) -> Result<MarketEligibility> {
        let resp = self.send_get("https://steamcommunity.com/market/eligibilitycheck/?goto=%2Fmarket%2F").await?;
        if redirect_location(&resp).is_some_and(|l| l.contains("/login")) {
            bail!("the Steam web session was rejected (redirected to login). Run `aurelia login --reconnect`.");
        }
        let cookies: Vec<(String, String)> = resp.headers().get_all(reqwest::header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).filter_map(parse_set_cookie).collect();
        let raw = cookies.iter().find(|(name, _)| name == "webTradeEligibility").map(|(_, value)| value.clone());
        raw.as_deref().and_then(MarketEligibility::from_cookie).ok_or_else(|| ineligible_error())
    }

    async fn send_get(&self, url: &str) -> Result<reqwest::Response> {
        let cookie = self.cookie_header();
        let request = self
            .http
            .get(url)
            .header(reqwest::header::COOKIE, cookie.expose());
        crate::core::net::send_with_retry(&self.http, request).await
    }

    fn cookie_header(&self) -> crate::core::secret::Secret {
        let guard = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        guard.expose().into()
    }

    async fn pass_eligibility_check(&self, check_url: &str) -> Result<()> {
        tracing::debug!("following Steam market eligibility check");
        let resp = self.send_get(check_url).await?;
        if redirect_location(&resp).is_some_and(|l| l.contains("/login")) {
            bail!(
                "the Steam web session was rejected (redirected to login). Run \
                 `aurelia login --reconnect`."
            );
        }
        let set: Vec<(String, String)> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(parse_set_cookie)
            .collect();
        if set.is_empty() {
            return Err(ineligible_error());
        }
        let mut guard = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        let merged = merge_cookies(guard.expose(), &set);
        *guard = merged.into();
        Ok(())
    }
}

fn form_urlencode(fields: &[(&str, &str)]) -> String {
    fn enc(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in fields.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        enc(k, &mut out);
        out.push('=');
        enc(v, &mut out);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketEligibility {
    pub allowed: bool,
    pub allowed_at_time: u32,
    pub steamguard_required_days: i32,
    pub new_device_cooldown_days: i32,
}

impl MarketEligibility {
    pub fn from_cookie(raw: &str) -> Option<Self> {
        let mut bytes = Vec::with_capacity(raw.len());
        let b = raw.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            } else {
                bytes.push(b[i]);
                i += 1;
            }
        }
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let int = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        Some(Self { allowed: int("allowed") != 0, allowed_at_time: int("allowed_at_time").max(0) as u32, steamguard_required_days: int("steamguard_required_days") as i32, new_device_cooldown_days: int("new_device_cooldown_days") as i32 })
    }

    pub fn reason_flags(&self) -> u32 {
        if self.allowed {
            0
        } else if self.new_device_cooldown_days > 0 && self.allowed_at_time > 0 && self.steamguard_required_days == 0 {
            2048
        } else if self.steamguard_required_days > 0 {
            128
        } else {
            1
        }
    }
}

fn redirect_location(resp: &reqwest::Response) -> Option<String> {
    if !resp.status().is_redirection() {
        return None;
    }
    resp.headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn eligibility_redirect(resp: &reqwest::Response) -> Option<String> {
    let location = redirect_location(resp)?;
    if !location.contains("eligibilitycheck") {
        return None;
    }
    resp.url().join(&location).ok().map(|u| u.to_string())
}

fn ineligible_error() -> anyhow::Error {
    TypedError::new(
        ErrorKind::AccessDenied,
        "this account cannot use the Steam Community Market yet. Steam requires the \
         Steam Guard Mobile Authenticator enabled for 15+ days (and no recent new-device \
         holds) before market and wallet features become available.",
    )
    .into()
}

fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

fn merge_cookies(existing: &str, new: &[(String, String)]) -> String {
    const KEEP: &[&str] = &["steamLoginSecure", "sessionid"];
    let mut pairs: Vec<(String, String)> = existing
        .split(';')
        .filter_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            Some((n.to_string(), v.to_string()))
        })
        .collect();
    for (name, value) in new {
        if KEEP.contains(&name.as_str()) {
            continue;
        }
        match pairs.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value.clone(),
            None => pairs.push((name.clone(), value.clone())),
        }
    }
    pairs
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn read_checked(resp: reqwest::Response) -> Result<String> {
    let status = resp.status();
    if status.as_u16() == 429 {
        return Err(TypedError::new(
            ErrorKind::RateLimited,
            "Steam is rate-limiting market requests (HTTP 429). Wait a few minutes before \
             trying again — repeated requests during a block extend it.",
        )
        .into());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(TypedError::new(
            ErrorKind::AuthRequired,
            format!(
                "the Steam web session was rejected (HTTP {}). Run `aurelia login --reconnect`, \
                 or `aurelia login` if you are not signed in.",
                status.as_u16()
            ),
        )
        .into());
    }
    if status.is_redirection() {
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(unknown)");
        if location.contains("eligibilitycheck") {
            return Err(ineligible_error());
        }
        if location.contains("/login") {
            bail!(
                "the Steam web session was rejected (redirected to login). Run \
                 `aurelia login --reconnect`."
            );
        }
        bail!(
            "the request was redirected unexpectedly (HTTP {} → {location}).",
            status.as_u16()
        );
    }
    resp.text()
        .await
        .context("failed reading the response body")
}

fn random_sessionid() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..24)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap())
        .collect()
}

#[cfg(test)]
#[path = "web_session_tests.rs"]
mod tests;
