use super::*;

#[test]
fn parses_set_cookie_name_and_value() {
    assert_eq!(
        parse_set_cookie("webTradeEligibility=%7B%22allowed%22%3A1%7D; path=/; secure"),
        Some((
            "webTradeEligibility".to_string(),
            "%7B%22allowed%22%3A1%7D".to_string()
        ))
    );
    assert_eq!(parse_set_cookie("noequals; path=/"), None);
    assert_eq!(parse_set_cookie("=x"), None);
}

#[test]
fn merge_adds_new_and_keeps_auth_cookies() {
    let existing = "steamLoginSecure=1%7C%7Ctok; sessionid=abc; steamCountry=DE";
    let new = vec![
        ("webTradeEligibility".to_string(), "e1".to_string()),
        ("sessionid".to_string(), "evil".to_string()),
        ("steamCountry".to_string(), "US".to_string()),
    ];
    let merged = merge_cookies(existing, &new);
    assert_eq!(
        merged,
        "steamLoginSecure=1%7C%7Ctok; sessionid=abc; steamCountry=US; webTradeEligibility=e1"
    );

    let again = merge_cookies(&merged, &[("webTradeEligibility".into(), "e2".into())]);
    assert_eq!(again.matches("webTradeEligibility").count(), 1);
    assert!(again.ends_with("webTradeEligibility=e2"));
}

#[test]
fn ineligible_error_is_typed_access_denied() {
    let err = ineligible_error();
    let typed = err.downcast_ref::<TypedError>().unwrap();
    assert_eq!(typed.kind, ErrorKind::AccessDenied);
}

#[test]
fn cookies_lists_the_session_cookies_as_name_value_pairs() {
    let session = WebSession::new(76561198000000001, "tok", Some("BR")).unwrap();
    let cookies = session.cookies();
    assert_eq!(cookies[0], ("steamLoginSecure".to_string(), "76561198000000001%7C%7Ctok".to_string()));
    assert_eq!(cookies[1].0, "sessionid");
    assert_eq!(cookies[1].1, session.sessionid());
    assert_eq!(cookies[2], ("steamCountry".to_string(), "BR".to_string()));
    assert_eq!(cookies.len(), 3);
}

#[test]
fn cookies_omit_the_country_when_unknown() {
    let session = WebSession::new(76561198000000001, "tok", None).unwrap();
    assert_eq!(session.cookies().len(), 2);
}

#[test]
fn the_trade_eligibility_cookie_is_read_and_mapped_to_a_reason() {
    let ok = "%7B%22allowed%22%3A1%2C%22allowed_at_time%22%3A0%2C%22steamguard_required_days%22%3A15%2C%22new_device_cooldown_days%22%3A7%2C%22time_checked%22%3A1795000000%7D";
    let e = MarketEligibility::from_cookie(ok).unwrap();
    assert!(e.allowed);
    assert_eq!((e.steamguard_required_days, e.new_device_cooldown_days), (15, 7));
    assert_eq!(e.reason_flags(), 0);
    let guard = "%7B%22allowed%22%3A0%2C%22allowed_at_time%22%3A1795600000%2C%22steamguard_required_days%22%3A15%2C%22new_device_cooldown_days%22%3A0%7D";
    let g = MarketEligibility::from_cookie(guard).unwrap();
    assert!(!g.allowed);
    assert_eq!((g.allowed_at_time, g.reason_flags()), (1_795_600_000, 128));
    let device = "%7B%22allowed%22%3A0%2C%22allowed_at_time%22%3A1795600000%2C%22steamguard_required_days%22%3A0%2C%22new_device_cooldown_days%22%3A7%7D";
    assert_eq!(MarketEligibility::from_cookie(device).unwrap().reason_flags(), 2048);
    let other = "%7B%22allowed%22%3A0%7D";
    assert_eq!(MarketEligibility::from_cookie(other).unwrap().reason_flags(), 1);
    assert!(MarketEligibility::from_cookie("junk").is_none());
    assert!(MarketEligibility::from_cookie("%7B%22allowed%22").is_none());
}

#[test]
fn form_bodies_escape_everything_but_unreserved_characters() {
    assert_eq!(form_urlencode(&[("id", "123"), ("appid", "550")]), "id=123&appid=550");
    assert_eq!(form_urlencode(&[("a b", "x&y=z/é")]), "a%20b=x%26y%3Dz%2F%C3%A9");
    assert_eq!(form_urlencode(&[]), "");
}
