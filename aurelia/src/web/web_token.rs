use anyhow::{Context, Result, bail};

pub const CLIENTJSTOKEN_URL: &str = "https://steamcommunity.com/chat/clientjstoken";

#[derive(Debug, Clone, PartialEq)]
pub struct WebTokenInfo {
    pub steam_id: u64,
    pub account_name: Option<String>,
    pub token: crate::core::secret::Secret,
    pub expires_at: Option<u64>,
}

pub fn parse_web_token(raw: &str, fallback_steam_id: Option<u64>) -> Result<WebTokenInfo> {
    let raw = raw.trim().trim_matches('"').trim();
    if raw.is_empty() {
        bail!("nothing was pasted");
    }

    let (declared_steam_id, account_name, token) = if raw.starts_with('{') {
        let json: serde_json::Value =
            serde_json::from_str(raw).context("the pasted text is not valid JSON")?;
        if json.get("logged_in").and_then(|v| v.as_bool()) != Some(true) {
            bail!(
                "the browser session is not signed in (logged_in is not true) — sign in on \
                 steamcommunity.com first, then reload the clientjstoken page"
            );
        }
        let steam_id: u64 = json
            .get("steamid")
            .and_then(|v| v.as_str())
            .context("the JSON has no steamid field")?
            .parse()
            .context("the JSON steamid did not parse")?;
        let account = json
            .get("account_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let token = json
            .get("token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .context("the JSON has no token value")?
            .to_string();
        (Some(steam_id), account, token)
    } else if let Some((id_part, token_part)) =
        raw.split_once("||").or_else(|| raw.split_once("%7C%7C"))
    {
        let steam_id: u64 = id_part
            .trim()
            .parse()
            .context("the part before '||' is not a SteamID64")?;
        (Some(steam_id), None, unescape_json_slashes(token_part))
    } else {
        (None, None, unescape_json_slashes(raw))
    };

    let (subject, expires_at) = match jwt_claims(&token) {
        Ok(claims) => {
            let subject: u64 = claims
                .get("sub")
                .and_then(|v| v.as_str())
                .context("the token has no sub (SteamID) claim")?
                .parse()
                .context("the token's sub claim is not a SteamID64")?;
            if let Some(declared) = declared_steam_id
                && declared != subject
            {
                bail!(
                    "the paste names SteamID {declared} but its token belongs to {subject} — \
                     re-fetch the clientjstoken page and paste it unmodified"
                );
            }
            (subject, claims.get("exp").and_then(|v| v.as_u64()))
        }
        Err(_) => {
            let Some(declared) = declared_steam_id.or(fallback_steam_id) else {
                bail!(
                    "this token is not in JWT format and carries no account identity (and no \
                     stored session provides one) — paste the entire clientjstoken JSON line \
                     instead"
                );
            };
            (declared, None)
        }
    };

    Ok(WebTokenInfo {
        steam_id: subject,
        account_name,
        token: token.into(),
        expires_at,
    })
}

pub fn check_stored_expiry(token: &str, now_unix: u64, leeway_secs: u64) -> Result<()> {
    let Ok(claims) = jwt_claims(token) else {
        return Ok(());
    };
    if let Some(exp) = claims.get("exp").and_then(|v| v.as_u64())
        && exp <= now_unix.saturating_add(leeway_secs)
    {
        bail!(
            "the stored web token expired {} — sign in on steamcommunity.com again and re-run \
             `aurelia login --web-token`",
            crate::steam_client::unix_to_ymd(exp as i64)
        );
    }
    Ok(())
}

fn unescape_json_slashes(s: &str) -> String {
    s.trim().replace("\\/", "/")
}

pub use crate::core::utils::now_unix;

pub(crate) fn jwt_claims(token: &str) -> Result<serde_json::Value> {
    let payload = token
        .split('.')
        .nth(1)
        .filter(|s| !s.is_empty())
        .context("the token is not a JWT (expected three dot-separated segments)")?;
    let bytes = b64url_decode(payload).context("the token payload is not base64url")?;
    serde_json::from_slice(&bytes).context("the token payload is not JSON")
}

pub(crate) fn b64url_decode(input: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in input.as_bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => bail!("invalid base64url byte 0x{byte:02x}"),
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "web_token_tests.rs"]
mod tests;
