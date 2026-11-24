use super::*;

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(v) = rx.try_recv() {
        out.push(v);
    }
    out
}

#[test]
fn the_steps_emit_every_kind_of_event_a_launcher_handles() {
    let (hub, mut rx) = OverlayHub::new();
    for step in 0..STEPS {
        emit_step(&hub, 480, step);
    }
    let kinds: Vec<String> = drain(&mut rx).iter().map(|e| e["event"].as_str().unwrap().to_string()).collect();
    for want in ["overlay_request", "microtxn_request", "friend_status", "chat_typing", "chat_message", "chat_invite", "achievement_unlocked", "dlc_changed"] {
        assert!(kinds.iter().any(|k| k == want), "faltou {want} em {kinds:?}");
    }
}

#[test]
fn the_sample_purchase_has_the_fields_of_the_real_one() {
    let (hub, mut rx) = OverlayHub::new();
    emit_step(&hub, 480, 2);
    let ev = &drain(&mut rx)[0];
    for field in ["id", "app_id", "order_id", "trans_id", "url", "window", "currency", "total", "total_minor", "items"] {
        assert!(ev.get(field).is_some(), "faltou {field}");
    }
}

#[test]
fn the_sample_purchase_can_be_answered_like_a_real_one() {
    let (hub, mut rx) = OverlayHub::new();
    emit_step(&hub, 480, 2);
    let id = drain(&mut rx)[0]["id"].as_str().unwrap().to_string();
    let line = format!(r#"{{"cmd":"steam_url","id":"{id}","url":"steam://close/authorize"}}"#);
    assert_eq!(answer(&hub, &line), json!({"event": "result", "ok": true}));
}

#[test]
fn queries_answer_with_the_real_event_shapes_and_keep_req() {
    let (hub, _rx) = OverlayHub::new();
    let cases = [
        (r#"{"cmd":"friends","req":"1"}"#, "friends"),
        (r#"{"cmd":"chat_history","steam_id":"5","req":"1"}"#, "chat_history"),
        (r#"{"cmd":"achievements","app_id":480,"req":"1"}"#, "achievements"),
        (r#"{"cmd":"profile","steam_id":"5","req":"1"}"#, "profile"),
        (r#"{"cmd":"chat_send","steam_id":"5","message":"oi","req":"1"}"#, "result"),
        (r#"{"cmd":"invite","steam_id":"5","req":"1"}"#, "result"),
        (r#"{"cmd":"web_session","req":"1"}"#, "web_session"),
    ];
    for (line, event) in cases {
        let reply = answer(&hub, line);
        assert_eq!(reply["event"], event, "{line}");
    }
    assert_eq!(answer(&hub, r#"{"cmd":"friends","req":"7"}"#)["req"], "7");
}

#[test]
fn invalid_lines_get_the_same_error_as_the_serve() {
    let (hub, _rx) = OverlayHub::new();
    let reply = answer(&hub, r#"{"cmd":"nothing","req":"z"}"#);
    assert_eq!((reply["error"].as_str(), reply["req"].as_str()), (Some("invalid_command"), Some("z")));
}

#[test]
fn players_recent_starts_with_an_example_player() {
    let (hub, _rx) = OverlayHub::new();
    seed_recent(&hub, 480);
    let reply = answer(&hub, r#"{"cmd":"players_recent"}"#);
    assert_eq!(reply["players"].as_array().unwrap().len(), 1);
}
