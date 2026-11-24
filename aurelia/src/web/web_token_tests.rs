use super::*;

const STEAM_ID: &str = "76561198056839548";
const EXP: u64 = 4102444800;

fn b64url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = [
            ALPHABET[(n >> 18) as usize & 63],
            ALPHABET[(n >> 12) as usize & 63],
            ALPHABET[(n >> 6) as usize & 63],
            ALPHABET[n as usize & 63],
        ];
        let keep = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        out.push_str(std::str::from_utf8(&chars[..keep]).unwrap());
    }
    out
}

fn fake_jwt(sub: &str, exp: u64) -> String {
    let header = b64url_encode(br#"{"typ":"JWT","alg":"EdDSA"}"#);
    let payload = b64url_encode(
        serde_json::json!({
            "iss": "steam",
            "sub": sub,
            "aud": ["web:community"],
            "exp": exp,
            "iat": 1752600000u64,
        })
        .to_string()
        .as_bytes(),
    );
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

fn clientjstoken_json(steam_id: &str, token: &str, logged_in: bool) -> String {
    serde_json::json!({
        "logged_in": logged_in,
        "steamid": steam_id,
        "account_name": "gabelogannewell",
        "token": token,
    })
    .to_string()
}

#[test]
fn parses_a_full_clientjstoken_paste() {
    let token = fake_jwt(STEAM_ID, EXP);
    let info = parse_web_token(&clientjstoken_json(STEAM_ID, &token, true), None).unwrap();
    assert_eq!(info.steam_id, STEAM_ID.parse::<u64>().unwrap());
    assert_eq!(info.account_name.as_deref(), Some("gabelogannewell"));
    assert_eq!(info.token.expose(), token);
    assert_eq!(info.expires_at, Some(EXP));
}

#[test]
fn parses_a_bare_token_paste() {
    let token = fake_jwt(STEAM_ID, EXP);
    for paste in [token.clone(), format!("\"{token}\"")] {
        let info = parse_web_token(&paste, None).unwrap();
        assert_eq!(info.steam_id, STEAM_ID.parse::<u64>().unwrap());
        assert_eq!(info.account_name, None);
        assert_eq!(info.expires_at, Some(EXP));
    }
}

#[test]
fn parses_an_opaque_token_json_paste() {
    let raw = format!(
        r#"{{"logged_in":true,"steamid":"{STEAM_ID}","accountid":96573820,"account_name":"ali_kid","token":"v+xYapFDu1AAAAAAAAAAAAAAAAAAAAAAAwAf8yFETJ\/NcYZuSG9mmAbr"}}"#
    );
    let info = parse_web_token(&raw, None).unwrap();
    assert_eq!(info.steam_id, STEAM_ID.parse::<u64>().unwrap());
    assert_eq!(info.account_name.as_deref(), Some("ali_kid"));
    assert_eq!(
        info.token.expose(),
        "v+xYapFDu1AAAAAAAAAAAAAAAAAAAAAAAwAf8yFETJ/NcYZuSG9mmAbr"
    );
    assert_eq!(info.expires_at, None);
}

#[test]
fn bare_opaque_token_binds_to_the_fallback_identity_or_is_rejected() {
    const OPAQUE: &str = r"v+xYapFDu1AAAAAAAAAAAAAAAAAAAAAAAwAf8yFETJ\/NcYZuSG9mmAbr";
    let err = parse_web_token(OPAQUE, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("paste the entire clientjstoken JSON")
    );
    let id: u64 = STEAM_ID.parse().unwrap();
    let info = parse_web_token(OPAQUE, Some(id)).unwrap();
    assert_eq!(info.steam_id, id);
    assert_eq!(
        info.token.expose(),
        "v+xYapFDu1AAAAAAAAAAAAAAAAAAAAAAAwAf8yFETJ/NcYZuSG9mmAbr"
    );
    assert_eq!(info.expires_at, None);
    let jwt = fake_jwt(STEAM_ID, EXP);
    let info = parse_web_token(&jwt, Some(76561198000000001)).unwrap();
    assert_eq!(info.steam_id, id);
}

#[test]
fn parses_a_steam_login_secure_cookie_paste() {
    let token = fake_jwt(STEAM_ID, EXP);
    for sep in ["||", "%7C%7C"] {
        let info = parse_web_token(&format!("{STEAM_ID}{sep}{token}"), None).unwrap();
        assert_eq!(info.steam_id, STEAM_ID.parse::<u64>().unwrap());
        assert_eq!(info.token.expose(), token);
        assert_eq!(info.expires_at, Some(EXP));
    }
    let err = parse_web_token(&format!("76561198000000001||{token}"), None).unwrap_err();
    assert!(err.to_string().contains("belongs to"));
    let info = parse_web_token(&format!("{STEAM_ID}||opaquetokenvalue"), None).unwrap();
    assert_eq!(info.steam_id, STEAM_ID.parse::<u64>().unwrap());
    assert_eq!(info.expires_at, None);
}

#[test]
fn rejects_a_signed_out_browser() {
    let token = fake_jwt(STEAM_ID, EXP);
    let err = parse_web_token(&clientjstoken_json(STEAM_ID, &token, false), None).unwrap_err();
    assert!(err.to_string().contains("not signed in"));
}

#[test]
fn rejects_a_steamid_token_mismatch() {
    let token = fake_jwt("76561198000000001", EXP);
    let err = parse_web_token(&clientjstoken_json(STEAM_ID, &token, true), None).unwrap_err();
    assert!(err.to_string().contains("belongs to"));
}

#[test]
fn rejects_garbage_pastes() {
    for paste in ["", "   ", "{\"logged_in\":true}"] {
        assert!(parse_web_token(paste, None).is_err(), "{paste:?}");
    }
}

#[test]
fn stored_token_expiry_is_enforced_with_leeway() {
    let token = fake_jwt(STEAM_ID, 1_000_000);
    assert!(check_stored_expiry(&token, 999_000, 60).is_ok());
    assert!(check_stored_expiry(&token, 999_970, 60).is_err());
    assert!(check_stored_expiry(&token, 1_000_001, 60).is_err());
    assert!(check_stored_expiry("v+xYopaque", u64::MAX - 61, 60).is_ok());
}

#[test]
fn b64url_roundtrips() {
    for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\x00\xfe"] {
        assert_eq!(b64url_decode(&b64url_encode(data)).unwrap(), data);
    }
    assert_eq!(b64url_decode("Zm8=").unwrap(), b"fo");
    assert!(b64url_decode("Zm+8").is_err());
}

#[test]
fn jwt_claims_requires_three_segments() {
    assert!(jwt_claims("onlyonepart").is_err());
    assert!(jwt_claims("two.parts").is_err());
    assert!(jwt_claims(&fake_jwt(STEAM_ID, EXP)).is_ok());
}
