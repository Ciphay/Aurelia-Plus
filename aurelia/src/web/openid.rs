use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashSet;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const STEAM_OPENID_ENDPOINT: &str = "https://steamcommunity.com/openid/login";

const NONCE_PARAM: &str = "aurelia_nonce";

const BROWSER_WAIT: Duration = Duration::from_secs(300);

const MIN_INDIVIDUAL_STEAMID: u64 = 76561197960265728;

pub async fn verify_identity_via_browser<F>(mut on_url: F) -> Result<u64>
where
    F: FnMut(&str),
{
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("failed to bind the localhost callback listener")?;
    let port = listener
        .local_addr()
        .context("failed to read the callback listener address")?
        .port();

    let nonce = format!("{:032x}", rand::random::<u128>());
    let return_to = format!("http://127.0.0.1:{port}/callback?{NONCE_PARAM}={nonce}");
    let realm = format!("http://127.0.0.1:{port}");
    let auth_url = build_auth_url(&return_to, &realm)?;
    on_url(auth_url.as_str());
    tracing::info!("Login method awaited: sign-in on the official Steam page in the browser");

    let http = crate::core::net::steam_web_client()?;

    tokio::time::timeout(BROWSER_WAIT, async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .context("callback listener failed")?;
            let head = read_request_head(&mut stream).await.unwrap_or_default();
            let Some(target) = request_target(&head) else {
                respond(&mut stream, "400 Bad Request", FAILURE_PAGE).await;
                continue;
            };
            if target != "/callback" && !target.starts_with("/callback?") {
                respond(&mut stream, "404 Not Found", FAILURE_PAGE).await;
                continue;
            }

            let params = parse_query_pairs(target)?;
            match validate_assertion(&params, &return_to) {
                Ok(steam_id) => match check_authentication(&http, &params).await {
                    Ok(()) => {
                        respond(&mut stream, "200 OK", SUCCESS_PAGE).await;
                        return Ok(steam_id);
                    }
                    Err(err) => {
                        respond(&mut stream, "403 Forbidden", FAILURE_PAGE).await;
                        return Err(err);
                    }
                },
                Err(err) => {
                    respond(&mut stream, "400 Bad Request", FAILURE_PAGE).await;
                    return Err(err).context("the Steam OpenID assertion failed validation");
                }
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow!(
            "browser sign-in timed out after {}s without a completed Steam sign-in",
            BROWSER_WAIT.as_secs()
        )
    })?
}

pub fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(url).spawn();
    spawned.is_ok()
}

fn build_auth_url(return_to: &str, realm: &str) -> Result<reqwest::Url> {
    let mut url =
        reqwest::Url::parse(STEAM_OPENID_ENDPOINT).context("invalid Steam OpenID endpoint")?;
    url.query_pairs_mut()
        .append_pair("openid.ns", "http://specs.openid.net/auth/2.0")
        .append_pair("openid.mode", "checkid_setup")
        .append_pair("openid.return_to", return_to)
        .append_pair("openid.realm", realm)
        .append_pair(
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        )
        .append_pair(
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        );
    Ok(url)
}

pub(crate) fn validate_assertion(
    params: &[(String, String)],
    expected_return_to: &str,
) -> Result<u64> {
    let get = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    match get("openid.mode") {
        Some("id_res") => {}
        Some("cancel") => bail!("the sign-in was cancelled on the Steam page"),
        other => bail!("unexpected openid.mode {other:?} in the callback"),
    }
    if get("openid.op_endpoint") != Some(STEAM_OPENID_ENDPOINT) {
        bail!("the assertion does not name the official Steam OpenID endpoint");
    }
    if get("openid.return_to") != Some(expected_return_to) {
        bail!("openid.return_to does not match this login attempt (possible replay)");
    }

    let signed: HashSet<&str> = get("openid.signed").unwrap_or("").split(',').collect();
    for field in [
        "claimed_id",
        "identity",
        "return_to",
        "response_nonce",
        "assoc_handle",
        "op_endpoint",
    ] {
        if !signed.contains(field) {
            bail!("Steam's signature does not cover the {field} field");
        }
    }

    let claimed = get("openid.claimed_id").context("missing openid.claimed_id")?;
    if get("openid.identity") != Some(claimed) {
        bail!("openid.identity and openid.claimed_id disagree");
    }
    steam_id_from_claimed_id(claimed)
}

pub(crate) fn steam_id_from_claimed_id(claimed: &str) -> Result<u64> {
    let id = claimed
        .strip_prefix("https://steamcommunity.com/openid/id/")
        .or_else(|| claimed.strip_prefix("http://steamcommunity.com/openid/id/"))
        .context("claimed_id is not a steamcommunity.com identity URL")?;
    if id.len() != 17 || !id.bytes().all(|b| b.is_ascii_digit()) {
        bail!("claimed_id does not contain a well-formed SteamID64");
    }
    let steam_id: u64 = id.parse().context("claimed_id SteamID64 did not parse")?;
    if steam_id < MIN_INDIVIDUAL_STEAMID {
        bail!("claimed_id is not an individual-account SteamID64");
    }
    Ok(steam_id)
}

async fn check_authentication(http: &reqwest::Client, params: &[(String, String)]) -> Result<()> {
    let mut encoder =
        reqwest::Url::parse(STEAM_OPENID_ENDPOINT).context("invalid Steam OpenID endpoint")?;
    {
        let mut pairs = encoder.query_pairs_mut();
        pairs.clear();
        for (k, v) in params.iter().filter(|(k, _)| k.starts_with("openid.")) {
            if k == "openid.mode" {
                pairs.append_pair(k, "check_authentication");
            } else {
                pairs.append_pair(k, v);
            }
        }
    }
    let form_body = encoder.query().unwrap_or_default().to_string();

    let body = http
        .post(STEAM_OPENID_ENDPOINT)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body)
        .send()
        .await
        .context("could not reach Steam to verify the sign-in assertion")?
        .error_for_status()
        .context("Steam rejected the verification request")?
        .text()
        .await
        .context("failed reading Steam's verification response")?;

    if !verification_says_valid(&body) {
        bail!("Steam did not confirm the sign-in assertion (is_valid != true)");
    }
    Ok(())
}

pub(crate) fn verification_says_valid(body: &str) -> bool {
    body.lines().any(|line| line.trim() == "is_valid:true")
}

pub(crate) fn parse_query_pairs(target: &str) -> Result<Vec<(String, String)>> {
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .context("the callback request URL did not parse")?;
    Ok(url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect())
}

async fn read_request_head(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .context("callback read failed")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn request_target(head: &str) -> Option<&str> {
    let mut parts = head.lines().next()?.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    parts.next()
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

const SUCCESS_PAGE: &str = "<!DOCTYPE html><html><head><title>Aurelia</title></head><body style=\"font-family:sans-serif;text-align:center;margin-top:4em\"><h2>Steam sign-in verified</h2><p>You can close this tab and return to the terminal.</p><p style=\"max-width:36em;margin:2em auto;color:#555\">Optional: to also enable Aurelia's web features (inventory, wallet, market listings), open <a href=\"https://steamcommunity.com/chat/clientjstoken\">steamcommunity.com/chat/clientjstoken</a> in this browser, copy the JSON shown, and run <code>aurelia login --web-token</code>.</p></body></html>";

const FAILURE_PAGE: &str = "<!DOCTYPE html><html><head><title>Aurelia</title></head><body style=\"font-family:sans-serif;text-align:center;margin-top:4em\"><h2>Steam sign-in not verified</h2><p>Return to the terminal for details.</p></body></html>";

#[cfg(test)]
#[path = "openid_tests.rs"]
mod tests;
