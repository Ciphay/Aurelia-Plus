use crate::daemon;
use crate::output;

use crate::commands::common::*;

use anyhow::Result;

const HOST_STEAM_NOTE: &str = "Steam is running here, so Aurelia stays unannounced to keep it \
signed in to Friends & Chat. Names, status and incoming messages may be missing.";

pub(crate) async fn cmd_friends(json: bool) -> Result<()> {
    let friends = if daemon::in_daemon() {
        daemon::shared_roster().await
    } else {
        let client = authed_client().await?;
        let mut all = client
            .collect_friends(std::time::Duration::from_secs(3))
            .await?;
        all.retain(|f| f.relationship == 3);
        all
    };

    if json {
        print_json(&friends);
        return Ok(());
    }
    if aurelia::steam_client::host_steam_owns_presence() {
        cli_eprintln!("{HOST_STEAM_NOTE}");
    }

    if friends.is_empty() {
        cli_println!(
            "No friends to show yet. (If you just started a session, the roster fills a moment \
             after connecting — try again.)"
        );
        return Ok(());
    }

    cli_println!("{:<20}  {:<17}  {:<12}  GAME", "STATUS", "STEAMID", "NAME");
    for f in &friends {
        let name = f.persona_name.as_deref().unwrap_or("?");
        let game = match (&f.game_name, f.game_app_id) {
            (Some(g), _) => g.clone(),
            (None, Some(id)) => format!("app {id}"),
            (None, None) => String::new(),
        };
        cli_println!(
            "{:<20}  {:<17}  {:<12}  {}",
            persona_state_label(f.persona_state),
            f.steam_id,
            name,
            game
        );
    }
    cli_println!("\n{} friend(s).", friends.len());
    Ok(())
}

pub(crate) async fn cmd_user(user: String, json: bool) -> Result<()> {
    let client = match authed_client().await {
        Ok(client) => client,
        Err(e) => {
            if json && user.eq_ignore_ascii_case("me") {
                if let Some(me) = offline_account(&e).await {
                    print_json(&me);
                    return Ok(());
                }
            }
            return Err(e);
        }
    };
    let steam_id = resolve_user_ident(&client, &user).await?;
    let profile = client.user_profile(steam_id).await?;
    if json {
        print_json(&profile);
        return Ok(());
    }
    cli_println!(
        "{}  ({})",
        profile
            .persona_name
            .as_deref()
            .unwrap_or("(no persona name)"),
        profile.steam_id
    );
    cli_println!("Profile  : {}", profile.profile_url);
    if let Some(url) = &profile.avatar_url {
        cli_println!("Avatar   : {url}");
    }
    let visibility = match profile.visibility {
        3 => "public",
        2 => "friends-only",
        1 => "private",
        _ => "unknown",
    };
    let limited = if profile.is_limited {
        ", limited account"
    } else {
        ""
    };
    cli_println!("Privacy  : {visibility}{limited}");
    if let Some(name) = &profile.real_name {
        cli_println!("Name     : {name}");
    }
    if let Some(place) = &profile.location {
        cli_println!("Location : {place}");
    }
    if let Some(t) = profile.created {
        cli_println!(
            "Created  : {}",
            aurelia::steam_client::unix_to_ymd(t as i64)
        );
    }
    cli_println!("Status   : {}", persona_state_label(profile.persona_state));
    match (&profile.game_name, profile.game_app_id) {
        (Some(g), _) => cli_println!("Playing  : {g}"),
        (None, Some(id)) => cli_println!("Playing  : app {id}"),
        _ => {}
    }
    if let Some(t) = profile.last_logoff {
        cli_println!(
            "Last seen: {}",
            aurelia::steam_client::unix_to_ymd(t as i64)
        );
    }
    if let Some(h) = &profile.headline {
        cli_println!("Headline : {h}");
    }
    if let Some(s) = &profile.summary {
        cli_println!("\n{}", aurelia::web::store::strip_html(s));
    }
    if let Some(t) = profile.ban_expires {
        cli_println!(
            "\nCommunity ban until {}",
            aurelia::steam_client::unix_to_ymd(t as i64)
        );
    }
    Ok(())
}

pub(crate) async fn cmd_friends_search(query: String, json: bool) -> Result<()> {
    cmd_user(query, json).await
}

pub(crate) async fn cmd_friends_add(query: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let steam_id = resolve_user_ident(&client, &query).await?;
    let added = client.add_friend(steam_id).await?;
    let name = added.persona_name;

    if json {
        print_json(&serde_json::json!({
            "steam_id": added.steam_id,
            "persona_name": name,
            "status": "request_sent",
        }));
    } else {
        match &name {
            Some(n) => cli_println!("Friend request sent to {n} ({}).", added.steam_id),
            None => cli_println!("Friend request sent to {}.", added.steam_id),
        }
    }
    Ok(())
}

pub(crate) async fn cmd_friends_remove(steamid: u64, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.remove_friend(steamid).await?;
    if json {
        print_json(&serde_json::json!({ "steam_id": steamid, "status": "removed" }));
    } else {
        cli_println!("Removed {steamid} from your friends.");
    }
    Ok(())
}

pub(crate) async fn cmd_chat_send(steamid: u64, message: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let sent = client.send_chat_message(steamid, message).await?;
    if json {
        print_json(&serde_json::json!({
            "steamid": steamid,
            "sent": true,
            "server_timestamp": sent.server_timestamp,
            "modified_message": sent.modified_message,
        }));
    } else {
        cli_println!("Message sent to {steamid}.");
    }
    Ok(())
}

pub(crate) async fn cmd_chat_invite(
    steamid: u64,
    app_id: u32,
    connect: Option<String>,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    client.invite_to_game(steamid, app_id, connect).await?;
    if json {
        print_json(&serde_json::json!({ "steamid": steamid, "app_id": app_id, "sent": true }));
    } else {
        cli_println!("Game invite for app {app_id} sent to {steamid}.");
    }
    Ok(())
}

pub(crate) async fn cmd_chat_history(steamid: u64, count: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let messages = client.recent_chat_messages(steamid, count).await?;
    if json {
        print_json(&messages);
        return Ok(());
    }
    if messages.is_empty() {
        cli_println!("No messages with {steamid}.");
        return Ok(());
    }
    for m in &messages {
        let who = if m.from_self { "me" } else { "them" };
        cli_println!("[{}] {:>4}: {}", m.timestamp, who, m.message);
    }
    Ok(())
}

pub(crate) fn render_chat_event(target: u64, event: steam_vent_chat::ChatEvent, json: bool) {
    use steam_vent_chat::ChatEvent;
    let source = match &event {
        ChatEvent::Message(e) | ChatEvent::EchoMessage(e) | ChatEvent::GameInvite(e) => {
            u64::from(e.source)
        }
        ChatEvent::Typing(e) => u64::from(e.source),
        ChatEvent::Other(e) => u64::from(e.source),
        _ => return,
    };
    if source != target {
        return;
    }
    match event {
        ChatEvent::Message(e) => {
            let text = e.message_no_bbcode.unwrap_or(e.message);
            if json {
                print_json_line(&serde_json::json!({
                    "event": "message", "from": target, "text": text,
                    "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("them: {text}");
            }
        }
        ChatEvent::EchoMessage(e) => {
            let text = e.message_no_bbcode.unwrap_or(e.message);
            if json {
                print_json_line(&serde_json::json!({
                    "event": "echo", "to": target, "text": text,
                    "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("me (sent elsewhere): {text}");
            }
        }
        ChatEvent::Typing(e) => {
            if json {
                print_json_line(&serde_json::json!({
                    "event": "typing", "from": target, "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("* {target} is typing...");
            }
        }
        ChatEvent::GameInvite(e) => {
            if json {
                print_json_line(&serde_json::json!({
                    "event": "game_invite", "from": target,
                    "text": e.message_no_bbcode.unwrap_or(e.message),
                    "timestamp": e.server_timestamp,
                }));
            } else {
                cli_println!("* {target} sent a game invite");
            }
        }
        ChatEvent::Other(e) if json => {
            print_json_line(&serde_json::json!({
                "event": "other", "with": target, "entry_type": e.entry_type,
                "local_echo": e.local_echo, "text": e.message,
                "timestamp": e.server_timestamp,
            }));
        }
        _ => {}
    }
}

pub(crate) async fn cmd_chat_open(steamid: u64, json: bool) -> Result<()> {
    use tokio_stream::StreamExt;

    let client = authed_client().await?;
    if let Err(e) = client.announce_configured_presence().await {
        tracing::warn!("could not announce presence: {e:#}");
    }

    if !json && aurelia::steam_client::host_steam_owns_presence() {
        cli_eprintln!("{HOST_STEAM_NOTE}");
    }

    let chat = client.chat_client()?;
    let mut events = chat.listen();

    if json {
        print_json_line(&serde_json::json!({ "event": "ready", "with": steamid }));
    } else {
        cli_println!(
            "Chat with {steamid}. Type a message and press Enter to send; Ctrl-D (Ctrl-Z on \
             Windows) to quit."
        );
    }

    loop {
        tokio::select! {
            incoming = events.next() => {
                match incoming {
                    Some(Ok(event)) => render_chat_event(steamid, event, json),
                    Some(Err(_)) => continue,
                    None => break,
                }
            }
            line = output::read_line_opt() => {
                match line {
                    Ok(Some(text)) => {
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        if let Err(e) = client.send_chat_message(steamid, text.to_string()).await {
                            if json {
                                print_json_line(&serde_json::json!({
                                    "event": "error", "message": format!("{e:#}"),
                                }));
                            } else {
                                cli_eprintln!("send failed: {e:#}");
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }

    if json {
        print_json_line(&serde_json::json!({ "event": "closed", "with": steamid }));
    }
    Ok(())
}

pub(crate) async fn cmd_debug_burst(emsg: Option<i32>) -> Result<()> {
    let _client = authed_client().await?;
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let burst = aurelia::steam_client::logon_burst();
    if burst.is_empty() {
        cli_println!("(nothing stored: this process did not perform the logon, or the buffer was already drained; run `aurelia kill` and try again)");
    }
    for (kind, proto, body) in &burst {
        match emsg {
            Some(want) if want == *kind => cli_println!("{kind} protobuf={proto} {} bytes: {}", body.len(), hex::encode(body)),
            Some(_) => {}
            None => cli_println!("{kind} protobuf={proto} {} bytes", body.len()),
        }
    }
    Ok(())
}

pub(crate) async fn cmd_debug_cert(app_id: u32, key: Option<String>) -> Result<()> {
    use ed25519_dalek::SigningKey;
    use protobuf::Message;
    use steam_vent_proto::steamnetworkingsockets_messages_certs::{cmsg_steam_datagram_certificate::EKeyType, CMsgSteamDatagramCertificate};
    use steam_vent_proto::steammessages_clientserver::{CMsgClientNetworkingCertReply, CMsgClientNetworkingCertRequest};

    let client = authed_client().await?;
    let public: [u8; 32] = match &key {
        Some(h) => hex::decode(h.trim())?.try_into().map_err(|_| anyhow::anyhow!("--key precisa de 32 bytes (64 hex)"))?,
        None => {
            let secret: [u8; 32] = rand::random();
            SigningKey::from_bytes(&secret).verifying_key().to_bytes()
        }
    };
    cli_println!("public key: {}", hex::encode(public));

    let mut wrapped = CMsgSteamDatagramCertificate::new();
    wrapped.set_key_type(EKeyType::ED25519);
    wrapped.set_key_data(public.to_vec());
    let variants: [(&str, Vec<u8>); 2] = [("embrulhada no certificado", wrapped.write_to_bytes()?), ("32 bytes crus", public.to_vec())];
    for (label, key_data) in variants {
        if key.is_some() && label != "32 bytes crus" {
            continue;
        }
        let mut req = CMsgClientNetworkingCertRequest::new();
        req.set_key_data(key_data);
        req.set_app_id(app_id);
        let call = aurelia::bridge::relay::CallRequest {
            expect_reply: true,
            protobuf: true,
            routing: false,
            gameserver: false,
            emsg: 5621,
            wait_ms: 15_000,
            job_name: None,
            body: req.write_to_bytes()?,
        };
        cli_println!("--- request ({label}), app {app_id}");
        match client.relay_call(&call, app_id).await {
            Ok(Some((kind, result, data))) => {
                cli_println!("response: emsg={kind} eresult={result} {} bytes", data.len());
                if let Ok(reply) = CMsgClientNetworkingCertReply::parse_from_bytes(&data) {
                    cli_println!("ca_key_id={} ca_signature={} bytes cert={} bytes", reply.ca_key_id(), reply.ca_signature().len(), reply.cert().len());
                    cli_println!("HEX cert={} sig={}", hex::encode(reply.cert()), hex::encode(reply.ca_signature()));
                    {
                        use base64::Engine;
                        let mut signed = steam_vent_proto::steamnetworkingsockets_messages_certs::CMsgSteamDatagramCertificateSigned::new();
                        signed.set_cert(reply.cert().to_vec());
                        signed.set_ca_key_id(reply.ca_key_id());
                        signed.set_ca_signature(reply.ca_signature().to_vec());
                        cli_println!("SIGNED_B64 {}", base64::engine::general_purpose::STANDARD.encode(signed.write_to_bytes()?));
                    }
                    if let Ok(cert) = CMsgSteamDatagramCertificate::parse_from_bytes(reply.cert()) {
                        cli_println!("certificado: {cert:?}");
                    }
                }
            }
            Ok(None) => cli_println!("no response expected"),
            Err(e) => cli_println!("error: {e:#}"),
        }
    }
    Ok(())
}

pub(crate) async fn cmd_debug_servers(app_id: u32, filter: String) -> Result<()> {
    fn varint(b: &[u8], i: &mut usize) -> u64 {
        let (mut r, mut s) = (0u64, 0);
        while *i < b.len() {
            let c = b[*i];
            *i += 1;
            r |= u64::from(c & 0x7f) << s;
            s += 7;
            if c < 0x80 {
                break;
            }
        }
        r
    }
    fn fields(b: &[u8]) -> Vec<(u64, u64, u64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let k = varint(b, &mut i);
            let (f, wt) = (k >> 3, k & 7);
            match wt {
                0 => out.push((f, wt, varint(b, &mut i), Vec::new())),
                1 if i + 8 <= b.len() => { out.push((f, wt, u64::from_le_bytes(b[i..i + 8].try_into().unwrap()), Vec::new())); i += 8; }
                5 if i + 4 <= b.len() => { out.push((f, wt, u64::from(u32::from_le_bytes(b[i..i + 4].try_into().unwrap())), Vec::new())); i += 4; }
                2 => { let l = varint(b, &mut i) as usize; let e = (i + l).min(b.len()); out.push((f, wt, 0, b[i..e].to_vec())); i = e; }
                _ => break,
            }
        }
        out
    }
    let client = authed_client().await?;
    let mut body = Vec::new();
    let put_varint = |body: &mut Vec<u8>, f: u64, mut v: u64| {
        body.push((f << 3) as u8);
        loop { let c = (v & 0x7f) as u8; v >>= 7; if v == 0 { body.push(c); break; } body.push(c | 0x80); }
    };
    put_varint(&mut body, 1, u64::from(app_id));
    put_varint(&mut body, 2, 0);
    put_varint(&mut body, 3, 255);
    body.push(4 << 3 | 2);
    body.push(filter.len() as u8);
    body.extend_from_slice(filter.as_bytes());
    put_varint(&mut body, 5, 512);
    let req = aurelia::bridge::relay::CallRequest { expect_reply: true, protobuf: true, routing: false, gameserver: false, emsg: 6403, wait_ms: 20_000, job_name: None, body };
    let Some((_, eresult, reply)) = client.relay_call(&req, app_id).await? else { anyhow::bail!("no response") };
    let top = fields(&reply);
    let table: Vec<String> = top.iter().filter(|x| x.0 == 4).map(|x| String::from_utf8_lossy(&x.3).into_owned()).collect();
    cli_println!("eresult {eresult}, {} servidores", top.iter().filter(|x| x.0 == 1).count());
    for (_, _, _, srv) in top.iter().filter(|x| x.0 == 1) {
        let (mut ip, mut port, mut players, mut maxp, mut flags, mut name) = (0u32, 0u64, 0u64, 0u64, 0u64, String::new());
        for (f, _, v, b) in fields(srv) {
            match f {
                1 => ip = v as u32,
                4 => { for (g, wt, w, _) in fields(&b) { if g == 1 && wt == 5 { ip = w as u32; } } }
                9 => port = v,
                2 if port == 0 => port = v,
                8 => players = v,
                13 => maxp = v,
                11 => flags = v,
                20 => name = String::from_utf8_lossy(&b).into_owned(),
                21 => name = table.get((v as usize).wrapping_sub(1)).cloned().unwrap_or_default(),
                _ => {}
            }
        }
        cli_println!("{}:{port} {players}/{maxp} flags={flags:#x} {name}", std::net::Ipv4Addr::from(ip));
    }
    Ok(())
}

pub(crate) async fn cmd_debug_listen(app_id: u32, secs: u64, extra: Vec<i32>) -> Result<()> {
    use protobuf::Message;
    use steam_vent_proto::steamnetworkingsockets_messages::CMsgSteamNetworkingP2PRendezvous;
    use steam_vent_proto::steammessages_clientserver::{CMsgClientP2PConnectionFailInfo, CMsgClientP2PConnectionInfo};

    let client = authed_client().await?;
    client.declare_playing(app_id).await?;
    let q = std::sync::Arc::new(aurelia::bridge::relay::PushQueue::default());
    for emsg in [5434, 5435].into_iter().chain(extra.iter().copied()) {
        client.relay_subscribe(emsg, q.clone())?;
    }
    cli_println!("escutando 5434/5435{} por {secs} s (jogando {app_id})", extra.iter().map(|e| format!("/{e}")).collect::<String>());
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let got = q.drain(1 << 20);
        let mut off = 0;
        while off + 8 <= got.len() {
            let emsg = i32::from_le_bytes(got[off..off + 4].try_into().unwrap());
            let len = u32::from_le_bytes(got[off + 4..off + 8].try_into().unwrap()) as usize;
            let b = &got[off + 8..(off + 8 + len).min(got.len())];
            off += 8 + len;
            if extra.contains(&emsg) {
                cli_println!("{emsg} bruto ({} bytes) {}", b.len(), hex::encode(b));
                continue;
            }
            if emsg == 5435 {
                match CMsgClientP2PConnectionFailInfo::parse_from_bytes(b) {
                    Ok(m) => cli_println!("5435 {m:?}"),
                    Err(_) => cli_println!("5435 bruto {}", hex::encode(b)),
                }
                continue;
            }
            let Ok(m) = CMsgClientP2PConnectionInfo::parse_from_bytes(b) else { cli_println!("5434 bruto {}", hex::encode(b)); continue };
            let mut head = m.clone();
            head.clear_rendezvous();
            cli_println!("5434 {head:?}");
            if m.has_rendezvous() {
                match CMsgSteamNetworkingP2PRendezvous::parse_from_bytes(m.rendezvous()) {
                    Ok(r) => cli_println!("  rendezvous {r:?}"),
                    Err(_) => cli_println!("  rendezvous bruto {}", hex::encode(m.rendezvous())),
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn cmd_debug_signal(app_id: u32, sender: String, playing: String, dest: Option<u64>) -> Result<()> {
    use protobuf::Message;
    use steam_vent::ConnectionTrait;
    use steam_vent_proto::steamnetworkingsockets_messages::{cmsg_steam_networking_p2prendezvous::ConnectionClosed, CMsgSteamNetworkingP2PRendezvous};
    use steam_vent_proto::steammessages_clientserver::{CMsgClientGamesPlayed, CMsgClientP2PConnectionFailInfo, CMsgClientP2PConnectionInfo, cmsg_client_games_played::GamePlayed};

    let mut client = authed_client().await?;
    let me = client.steam_id().ok_or_else(|| anyhow::anyhow!("no steam id"))?;
    let identity = format!("steamid:{me}");
    let second = client.open_extra_session(0x5EC0_0001).await?;
    let (play1, play2) = match playing.as_str() { "both" => (true, true), "1" => (true, false), "2" => (false, true), _ => (false, false) };
    if play1 {
        client.declare_playing(app_id).await?;
    }
    if play2 {
        second.send(CMsgClientGamesPlayed::new()).await.map_err(|e| anyhow::anyhow!("session 2: {e}"))?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let mut played = CMsgClientGamesPlayed::new();
        let mut game = GamePlayed::new();
        game.set_game_id(u64::from(app_id));
        played.games_played.push(game);
        second.send(played).await.map_err(|e| anyhow::anyhow!("session 2: {e}"))?;
    }
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let q1 = std::sync::Arc::new(aurelia::bridge::relay::PushQueue::default());
    let q2 = std::sync::Arc::new(aurelia::bridge::relay::PushQueue::default());
    for emsg in [5434, 5435] {
        client.relay_subscribe(emsg, q1.clone())?;
        aurelia::steam_client::SteamClient::relay_subscribe_on(&second, emsg, q2.clone());
    }

    let dest_id = dest.unwrap_or(me);
    let mut closed = ConnectionClosed::new();
    closed.set_debug("aurelia routing test".to_string());
    closed.set_reason_code(1000);
    let mut rendezvous = CMsgSteamNetworkingP2PRendezvous::new();
    rendezvous.set_from_identity(identity.clone());
    rendezvous.set_to_identity(format!("steamid:{dest_id}"));
    rendezvous.set_from_connection_id(0x1234_5678);
    rendezvous.set_to_connection_id(0x1234_5679);
    rendezvous.connection_closed = protobuf::MessageField::some(closed);
    let mut info = CMsgClientP2PConnectionInfo::new();
    info.set_steam_id_dest(dest_id);
    info.set_steam_id_src(me);
    info.set_app_id(app_id);
    info.set_rendezvous(rendezvous.write_to_bytes()?);
    let body = info.write_to_bytes()?;
    cli_println!("config: sender=session {sender} playing={playing} dest={dest_id}{}", if dest_id == me { " (the account itself)" } else { "" });
    if sender == "2" {
        second.relay_send(5434, true, None, body, None).await.map_err(|e| anyhow::anyhow!("envio: {e}"))?;
    } else {
        client.relay_call(&aurelia::bridge::relay::CallRequest { expect_reply: false, protobuf: true, routing: false, gameserver: false, emsg: 5434, wait_ms: 1000, job_name: None, body }, app_id).await?;
    }
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    for (name, q) in [("1", &q1), ("2", &q2)] {
        let got = q.drain(1 << 20);
        let mut off = 0;
        let mut n = 0;
        while off + 8 <= got.len() {
            let emsg = i32::from_le_bytes(got[off..off + 4].try_into().unwrap());
            let len = u32::from_le_bytes(got[off + 4..off + 8].try_into().unwrap()) as usize;
            let b = &got[off + 8..(off + 8 + len).min(got.len())];
            n += 1;
            if emsg == 5435 {
                if let Ok(m) = CMsgClientP2PConnectionFailInfo::parse_from_bytes(b) {
                    cli_println!("RESULT session {name}: 5435 failure error={} close_reason={} msg={:?} src={} dest={}", m.ep2p_session_error(), m.close_reason(), m.close_message(), m.steam_id_src(), m.steam_id_dest());
                }
            } else if let Ok(m) = CMsgClientP2PConnectionInfo::parse_from_bytes(b) {
                cli_println!("RESULT session {name}: 5434 src={} dest={} app={} legacy_conn={} rendezvous={}B", m.steam_id_src(), m.steam_id_dest(), m.app_id(), m.legacy_connection_id_src(), m.rendezvous().len());
            }
            off += 8 + len;
        }
        if n == 0 {
            cli_println!("RESULT session {name}: nothing");
        }
    }
    Ok(())
}
