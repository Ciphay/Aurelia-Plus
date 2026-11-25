use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use aurelia::bridge::authlist::{
    ActiveTicket, RESERVED_IFACE, SLOT_CANCEL_TICKET, SLOT_REGISTER_TICKET, build_auth_list, parse_register_payload, registered_crc,
};
use aurelia::bridge::server::CallHandler;
use aurelia::bridge::userstats::{DisplayInfo, SLOT_GET_USER_STATS, SLOT_STORE_USER_STATS, StatDef, encode_display, encode_user_stats, parse_stat_defs, parse_store_payload};
use aurelia::bridge::relay::{PushHub, PushQueue, SLOT_CM_CALL, SLOT_CM_POLL, SLOT_CM_SUBSCRIBE, encode_call_reply, parse_call_payload, parse_poll_payload, parse_subscribe_payload};
use aurelia::bridge::gameserver::{OP_LOGOFF, OP_LOGON, SLOT_GAMESERVER, encode_logon};
use aurelia::bridge::cloudfile::{OP_READ, OP_WRITE, SLOT_CLOUDFILE};
use aurelia::bridge::ugc::{OP_CREATE, OP_DELETE, OP_DOWNLOAD, OP_LIST, OP_OWNS, OP_PROGRESS, OP_SERVER_FOLDER, OP_SET_BRANCH, OP_STATE, OP_SUBMIT, OP_SUBSCRIBE, OP_UNSUBSCRIBE, SLOT_UGC, encode_list, encode_state, parse_request};
use aurelia::bridge::presence::{SLOT_SET_PRESENCE, parse_presence_payload, rich_presence_kv};
use aurelia::bridge::log::bridge_log;
use aurelia::bridge::ticket::{SLOT_CREATE_TICKET, TokenPool, build_auth_ticket, parse_create_payload};
use aurelia::steam_client::SteamClient;
use tokio::sync::Mutex;

use super::common::{authed_client, print_json_line, resolve_steam_language};

const TOKEN_POOL_SIZE: usize = 32;
const CLOUD_TIMEOUT: Duration = Duration::from_secs(120);
const TOKEN_WAIT: Duration = Duration::from_secs(10);

const REFILL_WAIT: Duration = Duration::from_secs(5);

macro_rules! blog {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        bridge_log(&line);
        cli_eprintln!("{line}");
    }};
}

type Pool = Arc<std::sync::Mutex<TokenPool>>;

fn pool_len(pool: &Pool) -> u32 {
    pool.lock().expect("token pool lock poisoned").len() as u32
}

fn random_nonces() -> [u32; 2] {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_err() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        bytes = (nanos as u64).to_le_bytes();
    }
    [u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])]
}

#[derive(Default)]
struct AuthState {
    tickets: Vec<ActiveTicket>,
    seq: u32,
    issued: u32,
    ownership: HashMap<u32, Vec<u8>>,
    stats_defs: Vec<StatDef>,
    stats_crc: u32,
    stats_values: HashMap<u32, u32>,
}

#[derive(Clone)]
struct StatsFetched {
    appid: u32,
    eresult: i32,
    reply: Vec<u8>,
    defs: Vec<StatDef>,
    crc: u32,
    values: HashMap<u32, u32>,
}

type StatsGate = Arc<Mutex<Option<StatsFetched>>>;

async fn fetch_stats(client: &SteamClient, appid: u32) -> Option<StatsFetched> {
    let resp = match client.get_user_stats(appid).await {
        Ok(resp) => resp,
        Err(e) => {
            blog!("bridge: user stats failed: {e:#}");
            return None;
        }
    };
    let defs = parse_stat_defs(resp.schema());
    let crc = resp.crc_stats();
    let values: HashMap<u32, u32> = resp.stats.iter().map(|s| (s.stat_id(), s.stat_value())).collect();
    let blocks: HashMap<u32, Vec<u32>> = resp.achievement_blocks.iter().map(|b| (b.achievement_id(), b.unlock_time.clone())).collect();
    blog!("bridge: user stats for {appid}: eresult={} stats={} schema_defs={} achievement_blocks={}", resp.eresult(), resp.stats.len(), defs.len(), blocks.len());
    let mut reply = encode_user_stats(crc, &defs, &values, &blocks);
    if let Ok(list) = client.fetch_achievements(appid, &resolve_steam_language(None).await).await {
        let declared = aurelia::bridge::userstats::schema_hidden_declared(resp.schema());
        let items: Vec<DisplayInfo> = list
            .into_iter()
            .map(|a| {
                let hidden_declared = declared.as_ref().is_none_or(|set| set.contains(&a.api_name));
                DisplayInfo { api_name: a.api_name, title: a.name, description: a.description, hidden: a.hidden, hidden_declared, percent: a.global_percent }
            })
            .collect();
        reply.extend_from_slice(&encode_display(&items));
    }
    if let Some(id) = client.steam_id() {
        aurelia::bridge::warm::save_stats(id, appid, resp.eresult(), &reply);
    }
    Some(StatsFetched { appid, eresult: resp.eresult(), reply, defs, crc, values })
}

async fn stats_for(client: &SteamClient, gate: &StatsGate, appid: u32) -> Option<StatsFetched> {
    let mut slot = gate.lock().await;
    if let Some(cached) = slot.as_ref().filter(|c| c.appid == appid) {
        return Some(cached.clone());
    }
    let fetched = fetch_stats(client, appid).await?;
    *slot = Some(fetched.clone());
    Some(fetched)
}

async fn push_auth_list(client: &SteamClient, state: &mut AuthState, tokens_left: u32) -> Vec<u32> {
    state.seq += 1;
    let list = build_auth_list(tokens_left, &state.tickets, state.seq);
    match client.send_auth_list(list).await {
        Ok(acked) => acked,
        Err(e) => {
            blog!("bridge: auth list failed: {e:#}");
            Vec::new()
        }
    }
}

fn call_handler(client: Arc<SteamClient>, state: Arc<Mutex<AuthState>>, pool: Pool, started: Instant, app_id: u32, hub: Arc<aurelia::bridge::overlay::OverlayHub>, stats_gate: StatsGate, steam_id: u64, queued: Arc<std::sync::atomic::AtomicUsize>) -> CallHandler {
    let pushes = Arc::new(PushHub::default());
    let clan_chat = Arc::new(aurelia::bridge::clanchat::ClanChat::default());
    let clan_hooked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gs: Arc<Mutex<Option<steam_vent::Connection>>> = Arc::new(Mutex::new(None));
    Arc::new(move |iface, slot, payload| {
        let client = Arc::clone(&client);
        let pushes = Arc::clone(&pushes);
        let gs = Arc::clone(&gs);
        let state = Arc::clone(&state);
        let pool = Arc::clone(&pool);
        let hub = Arc::clone(&hub);
        let stats_gate = Arc::clone(&stats_gate);
        let queued = Arc::clone(&queued);
        let clan_chat = Arc::clone(&clan_chat);
        let clan_hooked = Arc::clone(&clan_hooked);
        Box::pin(async move {
            if iface != RESERVED_IFACE {
                return (-1, Vec::new());
            }
            match slot {
                SLOT_CM_CALL => return relay_call(&client, &gs, &payload, app_id).await,
                SLOT_GAMESERVER => return gameserver_call(&client, &gs, &pushes.queue(0), &payload, app_id).await,
                SLOT_CM_SUBSCRIBE => {
                    let Some((emsg, on_server, process)) = parse_subscribe_payload(&payload) else { return (-2, Vec::new()) };
                    let pushes = pushes.queue(process);
                    if on_server {
                        let guard = gs.lock().await;
                        let Some(conn) = guard.as_ref() else { return (-5, Vec::new()) };
                        if pushes.subscribe(emsg | (1 << 30)) {
                            SteamClient::relay_subscribe_on(conn, emsg, Arc::clone(&pushes));
                        }
                    } else if pushes.subscribe(emsg) {
                        if let Err(e) = client.relay_subscribe(emsg, Arc::clone(&pushes)) {
                            blog!("bridge: relay subscribe {emsg} failed: {e:#}");
                            return (-5, Vec::new());
                        }
                    }
                    return (0, Vec::new());
                }
                SLOT_CM_POLL => return (0, pushes.queue(parse_poll_payload(&payload)).drain(60_000)),
                SLOT_UGC => return ugc_call(&client, &payload).await,
                SLOT_GET_USER_STATS if payload.len() >= 4 => {
                    let appid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let Some(fetched) = stats_for(&client, &stats_gate, appid).await else { return (-5, Vec::new()) };
                    let mut state = state.lock().await;
                    (state.stats_defs, state.stats_crc, state.stats_values) = (fetched.defs.clone(), fetched.crc, fetched.values.clone());
                    return (fetched.eresult, fetched.reply);
                }
                aurelia::bridge::accountinfo::SLOT_ACCOUNT_INFO => return (0, aurelia::bridge::accountinfo::encode(steam_vent::last_account_info().as_ref())),
                aurelia::bridge::webops::SLOT_WEB => return web_call(&client, &payload).await,
                aurelia::bridge::image::SLOT_IMAGE => return image_call(&client, &payload).await,
                aurelia::bridge::otherstats::SLOT_OTHER_STATS => return other_stats_call(&client, &gs, &payload, app_id).await,
                aurelia::bridge::clanchat::SLOT_CLAN => return clan_call(&client, &clan_chat, &clan_hooked, &payload).await,
                aurelia::bridge::appsquery::SLOT_APPS => {
                    let known = aurelia::bridge::appsquery::answer(steam_id, &payload);
                    if known.as_deref().is_some_and(|r| r.iter().any(|b| *b != 0)) {
                        return (0, known.unwrap_or_default());
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(3), client.ensure_purchase_data()).await;
                    return match aurelia::bridge::appsquery::answer(steam_id, &payload) {
                        Some(r) => (0, r),
                        None => (-2, Vec::new()),
                    };
                }
                SLOT_CLOUDFILE => return cloudfile_call(&client, &payload).await,
                aurelia::bridge::overlay::SLOT_OVERLAY => return aurelia::bridge::overlay::overlay_call(&hub, app_id, &payload),
                _ => {}
            }
            let mut state = state.lock().await;
            match slot {
                SLOT_CREATE_TICKET => {
                    let Some(req) = parse_create_payload(&payload) else { return (-2, Vec::new()) };
                    let (appid, handle) = (req.appid, req.handle);
                    let mut token = pool.lock().expect("token pool lock poisoned").take();
                    if token.is_none() {
                        push_auth_list(&client, &mut state, 0).await;
                        let deadline = Instant::now() + REFILL_WAIT;
                        while token.is_none() && Instant::now() < deadline {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            token = pool.lock().expect("token pool lock poisoned").take();
                        }
                    }
                    let Some(token) = token else {
                        blog!("bridge: no game connect token left for a new ticket");
                        return (-3, Vec::new());
                    };
                    if !state.ownership.contains_key(&appid) {
                        match client.get_app_ticket(appid).await {
                            Ok(t) => {
                                state.ownership.insert(appid, t);
                            }
                            Err(e) => {
                                blog!("bridge: no ownership ticket for app {appid}: {e:#}");
                                return (-4, Vec::new());
                            }
                        }
                    }
                    state.issued += 1;
                    let ticket = build_auth_ticket(
                        &token,
                        req.ticket_type,
                        random_nonces(),
                        started.elapsed().as_millis() as u32,
                        state.issued,
                        &state.ownership[&appid],
                    );
                    let crc = registered_crc(&ticket);
                    state.tickets.retain(|t| t.handle != handle);
                    state.tickets.push(ActiveTicket {
                        handle,
                        appid,
                        ticket: ticket.clone(),
                        ticket_type: if req.ticket_type == aurelia::bridge::ticket::TICKET_TYPE_DEFAULT { 0 } else { req.ticket_type },
                        identity: req.identity.clone(),
                    });
                    let acked = push_auth_list(&client, &mut state, pool_len(&pool)).await;
                    blog!(
                        "bridge: created auth ticket #{} crc={crc:08x}, acknowledged={}, tokens left={}",
                        state.issued,
                        acked.contains(&crc),
                        pool_len(&pool)
                    );
                    (if acked.contains(&crc) { 0 } else { 1 }, ticket)
                }
                SLOT_REGISTER_TICKET => {
                    let Some(ticket) = parse_register_payload(&payload) else { return (-2, Vec::new()) };
                    let crc = registered_crc(&ticket.ticket);
                    state.tickets.retain(|t| t.handle != ticket.handle);
                    state.tickets.push(ticket);
                    let acked = push_auth_list(&client, &mut state, pool_len(&pool)).await;
                    blog!("bridge: registered auth ticket crc={crc:08x}, Steam acknowledged {acked:08x?}");
                    (if acked.contains(&crc) { 0 } else { 1 }, Vec::new())
                }
                SLOT_SET_PRESENCE => {
                    let Some(pairs) = parse_presence_payload(&payload) else { return (-2, Vec::new()) };
                    match client.upload_rich_presence(rich_presence_kv(&pairs)).await {
                        Ok(()) => {
                            blog!("bridge: rich presence updated ({} keys)", pairs.len());
                            (0, Vec::new())
                        }
                        Err(e) => {
                            blog!("bridge: rich presence failed: {e:#}");
                            (-5, Vec::new())
                        }
                    }
                }
                SLOT_STORE_USER_STATS => {
                    let Some((_, changes)) = parse_store_payload(&payload) else { return (-2, Vec::new()) };
                    let appid = app_id;
                    for (id, value) in &changes {
                        state.stats_values.insert(*id, *value);
                    }
                    match client.store_user_stats(appid, state.stats_crc, &changes).await {
                        Ok(resp) => {
                            state.stats_crc = resp.crc_stats();
                            *stats_gate.lock().await = None;
                            blog!(
                                "bridge: stored {} stats for {appid}: eresult={} out_of_date={} failed_validation={}",
                                changes.len(),
                                resp.eresult(),
                                resp.stats_out_of_date(),
                                resp.stats_failed_validation.len()
                            );
                            (resp.eresult(), Vec::new())
                        }
                        Err(e) => {
                            blog!("bridge: store stats failed, queued for later: {e:#}");
                            let n = aurelia::bridge::offline::enqueue(steam_id, aurelia::bridge::offline::Op::StoreStats { app: appid, changes });
                            queued.store(n, std::sync::atomic::Ordering::Relaxed);
                            (1, Vec::new())
                        }
                    }
                }
                SLOT_CANCEL_TICKET if payload.len() >= 4 => {
                    let handle = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    state.tickets.retain(|t| t.handle != handle);
                    push_auth_list(&client, &mut state, pool_len(&pool)).await;
                    blog!("bridge: cancelled auth ticket handle={handle}");
                    (0, Vec::new())
                }
                _ => (-1, Vec::new()),
            }
        })
    })
}

async fn service_call(client: &SteamClient, job: &str, body: Vec<u8>) -> Result<(i32, Vec<u8>)> {
    let req = aurelia::bridge::relay::CallRequest { expect_reply: true, protobuf: true, routing: false, gameserver: false, emsg: 151, wait_ms: 15_000, job_name: Some(job.to_string()), body };
    match client.relay_call(&req, 0).await? {
        Some((_, eresult, body)) => Ok((eresult, body)),
        None => anyhow::bail!("no answer from {job}"),
    }
}

async fn clan_call(client: &SteamClient, chat: &Arc<aurelia::bridge::clanchat::ClanChat>, hooked: &Arc<std::sync::atomic::AtomicBool>, payload: &[u8]) -> (i32, Vec<u8>) {
    use aurelia::bridge::clanchat::*;
    use protobuf::Message;
    use steam_vent_proto::steammessages_chat_steamclient::{CChatRoom_IncomingChatMessage_Notification, CChatRoom_JoinChatRoomGroup_Request, CChatRoom_JoinChatRoomGroup_Response, CChatRoom_LeaveChatRoomGroup_Request, CChatRoom_SendChatMessage_Request, CClanChatRooms_GetClanChatRoomInfo_Request, CClanChatRooms_GetClanChatRoomInfo_Response};
    let Some((op, clan, body)) = parse(payload) else { return (-2, Vec::new()) };
    match op {
        OP_JOIN => {
            let mut info = CClanChatRooms_GetClanChatRoomInfo_Request::new();
            info.set_steamid(clan);
            info.set_autocreate(false);
            let summary = match service_call(client, "ClanChatRooms.GetClanChatRoomInfo#1", info.write_to_bytes().unwrap_or_default()).await {
                Ok((1, body)) => CClanChatRooms_GetClanChatRoomInfo_Response::parse_from_bytes(&body).ok().and_then(|r| r.chat_group_summary.into_option()),
                Ok((eresult, _)) => {
                    blog!("bridge: clan chat info for {clan}: eresult {eresult}");
                    return (0, encode_join(if eresult == 15 { ENTER_NOT_ALLOWED } else { ENTER_DOESNT_EXIST }, 0, 0));
                }
                Err(e) => {
                    blog!("bridge: clan chat info for {clan} failed: {e:#}");
                    return (0, encode_join(ENTER_ERROR, 0, 0));
                }
            };
            let Some(summary) = summary.filter(|s| s.chat_group_id() != 0) else { return (0, encode_join(ENTER_DOESNT_EXIST, 0, 0)) };
            let mut join = CChatRoom_JoinChatRoomGroup_Request::new();
            join.set_chat_group_id(summary.chat_group_id());
            let state = match service_call(client, "ChatRoom.JoinChatRoomGroup#1", join.write_to_bytes().unwrap_or_default()).await {
                Ok((1, body)) => CChatRoom_JoinChatRoomGroup_Response::parse_from_bytes(&body).ok(),
                Ok((eresult, _)) => return (0, encode_join(if eresult == 15 { ENTER_NOT_ALLOWED } else { ENTER_ERROR }, 0, 0)),
                Err(e) => {
                    blog!("bridge: clan chat join {clan} failed: {e:#}");
                    return (0, encode_join(ENTER_ERROR, 0, 0));
                }
            };
            let members: Vec<u64> = state.as_ref().map(|s| s.state.members.iter().map(|m| INDIVIDUAL_BASE + u64::from(m.accountid())).collect()).unwrap_or_default();
            let owner = if summary.accountid_owner() != 0 { INDIVIDUAL_BASE + u64::from(summary.accountid_owner()) } else { 0 };
            let room = Room { group_id: summary.chat_group_id(), chat_id: summary.default_chat_id(), members, owner };
            let count = room.members.len().max(summary.active_member_count() as usize);
            chat.join(clan, room);
            if !hooked.swap(true, std::sync::atomic::Ordering::SeqCst) {
                if let Ok(mut rx) = client.relay_notifications("ChatRoomClient.NotifyIncomingChatMessage#1") {
                    let chat = Arc::clone(chat);
                    tokio::spawn(async move {
                        while let Some(body) = rx.recv().await {
                            let Ok(n) = CChatRoom_IncomingChatMessage_Notification::parse_from_bytes(&body) else { continue };
                            let Some(clan) = chat.clan_of_group(n.chat_group_id()) else { continue };
                            let text = if n.message_no_bbcode().is_empty() { n.message() } else { n.message_no_bbcode() };
                            chat.push(Incoming { clan, sender: n.steamid_sender(), time: n.timestamp(), text: text.to_string() });
                        }
                    });
                }
            }
            (0, encode_join(ENTER_SUCCESS, count, owner))
        }
        OP_LEAVE => {
            let Some(room) = chat.leave(clan) else { return (0, 0i32.to_le_bytes().to_vec()) };
            let mut req = CChatRoom_LeaveChatRoomGroup_Request::new();
            req.set_chat_group_id(room.group_id);
            let ok = matches!(service_call(client, "ChatRoom.LeaveChatRoomGroup#1", req.write_to_bytes().unwrap_or_default()).await, Ok((1, _)));
            (0, i32::from(ok).to_le_bytes().to_vec())
        }
        OP_SEND => {
            let Some(room) = chat.room(clan) else { return (0, 0i32.to_le_bytes().to_vec()) };
            let mut req = CChatRoom_SendChatMessage_Request::new();
            req.set_chat_group_id(room.group_id);
            req.set_chat_id(room.chat_id);
            req.set_message(String::from_utf8_lossy(body).into_owned());
            let ok = matches!(service_call(client, "ChatRoom.SendChatMessage#1", req.write_to_bytes().unwrap_or_default()).await, Ok((1, _)));
            (0, i32::from(ok).to_le_bytes().to_vec())
        }
        OP_MEMBERS => (0, encode_members(&chat.room(clan).map(|r| r.members).unwrap_or_default())),
        OP_POLL => (0, encode_messages(&chat.drain())),
        _ => (-2, Vec::new()),
    }
}

async fn other_stats_call(client: &SteamClient, gs: &Mutex<Option<steam_vent::Connection>>, payload: &[u8], app_id: u32) -> (i32, Vec<u8>) {
    use aurelia::bridge::otherstats::{Request, parse};
    let Some(req) = parse(payload) else { return (-2, Vec::new()) };
    let gameserver = match &req {
        Request::Get { gameserver, .. } | Request::Store { gameserver, .. } => *gameserver,
    };
    let guard = gs.lock().await;
    let connection = if gameserver {
        match guard.as_ref() {
            Some(c) => c,
            None => return (-5, Vec::new()),
        }
    } else {
        match client.user_connection() {
            Ok(c) => c,
            Err(_) => return (-5, Vec::new()),
        }
    };
    match req {
        Request::Get { steam_id, .. } => match SteamClient::get_user_stats_of_on(connection, app_id, steam_id).await {
            Ok(resp) => {
                let defs = parse_stat_defs(resp.schema());
                let values: HashMap<u32, u32> = resp.stats.iter().map(|s| (s.stat_id(), s.stat_value())).collect();
                let blocks: HashMap<u32, Vec<u32>> = resp.achievement_blocks.iter().map(|b| (b.achievement_id(), b.unlock_time.clone())).collect();
                if resp.eresult() == 1 { (0, encode_user_stats(resp.crc_stats(), &defs, &values, &blocks)) } else { (-5, Vec::new()) }
            }
            Err(e) => {
                blog!("bridge: stats of {steam_id} failed: {e:#}");
                (-5, Vec::new())
            }
        },
        Request::Store { steam_id, crc, stats, .. } => {
            let settor = if gameserver { u64::from(connection.steam_id()) } else { client.steam_id().unwrap_or(0) };
            match SteamClient::store_user_stats_of_on(connection, app_id, settor, steam_id, crc, &stats).await {
                Ok(resp) => (0, resp.eresult().to_le_bytes().to_vec()),
                Err(e) => {
                    blog!("bridge: store stats of {steam_id} failed: {e:#}");
                    (-5, Vec::new())
                }
            }
        }
    }
}

async fn web_call(client: &SteamClient, payload: &[u8]) -> (i32, Vec<u8>) {
    use aurelia::bridge::webops::{self, OP_MARKET};
    let Some((op, app, id)) = webops::parse(payload) else { return (-2, Vec::new()) };
    let ws = match client.web_session().await {
        Ok(ws) => ws,
        Err(e) => {
            blog!("bridge: web session unavailable: {e:#}");
            return (-5, Vec::new());
        }
    };
    if op == OP_MARKET {
        return match ws.market_eligibility().await {
            Ok(e) => (0, webops::encode_market(e.allowed, e.reason_flags(), e.allowed_at_time, e.steamguard_required_days, e.new_device_cooldown_days)),
            Err(e) => {
                blog!("bridge: market eligibility failed: {e:#}");
                (-5, Vec::new())
            }
        };
    }
    let Some((url, form)) = webops::favorite_request(op, app, id) else { return (-2, Vec::new()) };
    let fields: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    match ws.post_form(url, &fields).await {
        Ok((status, _)) if (200..300).contains(&status) => (0, 1i32.to_le_bytes().to_vec()),
        Ok((status, _)) => {
            blog!("bridge: workshop favorite {id} answered HTTP {status}");
            (0, 2i32.to_le_bytes().to_vec())
        }
        Err(e) => {
            blog!("bridge: workshop favorite {id} failed: {e:#}");
            (0, 2i32.to_le_bytes().to_vec())
        }
    }
}

async fn relay_call(client: &SteamClient, gs: &Mutex<Option<steam_vent::Connection>>, payload: &[u8], app_id: u32) -> (i32, Vec<u8>) {
    let Some(req) = parse_call_payload(payload) else { return (-2, Vec::new()) };
    let outcome = if req.gameserver {
        let guard = gs.lock().await;
        match guard.as_ref() {
            Some(conn) => SteamClient::relay_call_on(conn, &req, app_id).await,
            None => Err(anyhow::anyhow!("game server session is not logged on")),
        }
    } else {
        client.relay_call(&req, app_id).await
    };
    match outcome {
        Ok(Some((emsg, eresult, body))) => {
            if eresult != 1 {
                blog!("bridge: relay {} ({:?}) answered eresult {eresult}", req.emsg, req.job_name);
            }
            (0, encode_call_reply(emsg, eresult, &body))
        }
        Ok(None) => (1, Vec::new()),
        Err(e) => {
            blog!("bridge: relay {} ({:?}) failed: {e:#}", req.emsg, req.job_name);
            (-5, Vec::new())
        }
    }
}

async fn gameserver_call(client: &SteamClient, gs: &Mutex<Option<steam_vent::Connection>>, _pushes: &PushQueue, payload: &[u8], app_id: u32) -> (i32, Vec<u8>) {
    match payload.first().copied() {
        Some(OP_LOGON) => {
            let token = std::str::from_utf8(&payload[1..]).ok().filter(|t| !t.is_empty()).map(str::to_string);
            let mut guard = gs.lock().await;
            if guard.is_none() {
                match client.gameserver_connect(app_id, token.as_deref()).await {
                    Ok(conn) => *guard = Some(conn),
                    Err(e) => {
                        blog!("bridge: game server logon failed: {e:#}");
                        return (-5, Vec::new());
                    }
                }
            }
            let id = guard.as_ref().map(|c| u64::from(c.steam_id())).unwrap_or(0);
            let ip = guard.as_ref().and_then(|c| match c.public_ip() {
                std::net::IpAddr::V4(v4) => Some(v4),
                _ => None,
            });
            blog!("bridge: game server logged on as {id}");
            (0, encode_logon(id, ip))
        }
        Some(OP_LOGOFF) => {
            *gs.lock().await = None;
            (0, Vec::new())
        }
        _ => (-1, Vec::new()),
    }
}

async fn image_call(client: &SteamClient, payload: &[u8]) -> (i32, Vec<u8>) {
    use aurelia::bridge::image::{self, ImageRequest};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static PIXELS: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    static ACHIEVEMENTS: OnceLock<Mutex<HashMap<u32, Vec<aurelia::steam_client::GameAchievement>>>> = OnceLock::new();
    let Some(req) = image::parse_request(payload) else { return (-2, Vec::new()) };
    let url = match req {
        ImageRequest::Avatar { steam_id, size } => match client.user_profile(steam_id).await {
            Ok(profile) => match profile.avatar_url {
                Some(full) => image::avatar_variant(&full, size),
                None => return (-4, Vec::new()),
            },
            Err(e) => {
                blog!("bridge: avatar of {steam_id} unavailable: {e:#}");
                return (-5, Vec::new());
            }
        },
        ImageRequest::AchievementIcon { app_id, achieved, name } => {
            let known = ACHIEVEMENTS.get_or_init(Default::default).lock().ok().and_then(|m| m.get(&app_id).cloned());
            let list = match known {
                Some(list) => list,
                None => match client.fetch_achievements(app_id, "english").await {
                    Ok(list) => {
                        if let Ok(mut m) = ACHIEVEMENTS.get_or_init(Default::default).lock() {
                            m.insert(app_id, list.clone());
                        }
                        list
                    }
                    Err(e) => {
                        blog!("bridge: achievements of {app_id} unavailable for icons: {e:#}");
                        return (-5, Vec::new());
                    }
                },
            };
            match list.iter().find(|a| a.api_name.eq_ignore_ascii_case(&name)) {
                Some(a) => if achieved { a.icon_unlocked.clone() } else { a.icon_locked.clone() },
                None => return (-4, Vec::new()),
            }
        }
    };
    if url.is_empty() {
        return (-4, Vec::new());
    }
    if let Some(hit) = PIXELS.get_or_init(Default::default).lock().ok().and_then(|m| m.get(&url).cloned()) {
        return (0, hit);
    }
    let bytes = match reqwest::Client::new().get(&url).timeout(std::time::Duration::from_secs(15)).send().await.and_then(|r| r.error_for_status()) {
        Ok(r) => match r.bytes().await {
            Ok(b) => b,
            Err(e) => {
                blog!("bridge: image {url} failed: {e}");
                return (-5, Vec::new());
            }
        },
        Err(e) => {
            blog!("bridge: image {url} failed: {e}");
            return (-5, Vec::new());
        }
    };
    let Some((w, h, rgba)) = image::decode_rgba(&bytes) else {
        blog!("bridge: image {url} could not be decoded");
        return (-5, Vec::new());
    };
    let reply = image::encode_reply(w, h, &rgba);
    if let Ok(mut m) = PIXELS.get_or_init(Default::default).lock() {
        m.insert(url, reply.clone());
    }
    (0, reply)
}
async fn cloudfile_call(client: &SteamClient, payload: &[u8]) -> (i32, Vec<u8>) {
    let Some((op, app, name, data)) = aurelia::bridge::cloudfile::parse_request(payload) else { return (-2, Vec::new()) };
    let cloud = match client.cloud_client() {
        Ok(c) => c,
        Err(e) => {
            blog!("bridge: cloud client unavailable: {e:#}");
            return (-5, Vec::new());
        }
    };
    match op {
        OP_READ => match cloud.read_cloud_file(app, &name).await {
            Ok(body) => (0, body.unwrap_or_default()),
            Err(e) => {
                blog!("bridge: cloud read {app}/{name} failed: {e:#}");
                (-5, Vec::new())
            }
        },
        OP_WRITE => match cloud.write_cloud_file(app, &name, data).await {
            Ok(()) => (0, Vec::new()),
            Err(e) => {
                blog!("bridge: cloud write {app}/{name} failed: {e:#}");
                (-5, Vec::new())
            }
        },
        _ => (-1, Vec::new()),
    }
}

async fn ugc_call(client: &Arc<SteamClient>, payload: &[u8]) -> (i32, Vec<u8>) {
    let Some((op, app, id)) = parse_request(payload) else { return (-2, Vec::new()) };
    match op {
        OP_LIST => match client.fetch_subscribed_items(app).await {
            Ok(ids) => (0, encode_list(&ids)),
            Err(e) => {
                blog!("bridge: workshop list for {app} failed: {e:#}");
                (-5, Vec::new())
            }
        },
        OP_STATE => match client.workshop_install_info(app, id).await {
            Ok(Some((dir, size, updated))) => (0, encode_state(true, size, updated, &dir.to_string_lossy())),
            Ok(None) => (0, encode_state(false, 0, 0, "")),
            Err(e) => {
                blog!("bridge: workshop state of {id} failed: {e:#}");
                (-5, Vec::new())
            }
        },
        OP_DOWNLOAD => (0, ugc_download(client, id).await.to_le_bytes().to_vec()),
        OP_SUBSCRIBE | OP_UNSUBSCRIBE => {
            let r = if op == OP_SUBSCRIBE { client.subscribe_published_file(id, app).await } else { client.unsubscribe_published_file(id, app).await };
            if let Err(e) = &r {
                blog!("bridge: workshop (un)subscribe {id} failed: {e:#}");
            }
            (0, (if r.is_ok() { 1i32 } else { 2i32 }).to_le_bytes().to_vec())
        }
        OP_CREATE => match client.workshop_create_item(app, &Default::default()).await {
            Ok((item, legal)) => (0, aurelia::bridge::ugc::encode_created(1, item, legal)),
            Err(e) => {
                blog!("bridge: workshop create item for {app} failed: {e:#}");
                (0, aurelia::bridge::ugc::encode_created(2, 0, false))
            }
        },
        OP_SUBMIT => {
            let Some(spec) = aurelia::bridge::ugc::parse_spec(aurelia::bridge::ugc::request_body(payload)) else { return (-2, Vec::new()) };
            ugc_submit(Arc::clone(client), app, id, spec);
            (0, Vec::new())
        }
        OP_PROGRESS => match aurelia::bridge::ugc::jobs().lock().expect("ugc jobs lock poisoned").get(&id) {
            Some(job) => (0, aurelia::bridge::ugc::encode_job(job)),
            None => (-3, Vec::new()),
        },
        OP_SERVER_FOLDER => {
            let folder = String::from_utf8_lossy(aurelia::bridge::ugc::request_body(payload)).trim().to_string();
            let root = (!folder.is_empty()).then(|| std::path::PathBuf::from(&folder));
            let made = root.as_ref().map(|r| std::fs::create_dir_all(r).is_ok()).unwrap_or(true);
            if made {
                aurelia::steam_client::workshop::set_workshop_root(root);
            }
            (0, (if made { 1i32 } else { 2i32 }).to_le_bytes().to_vec())
        }
        OP_OWNS => {
            let owned = u32::try_from(id).map(|app| client.get_app_ticket(app)).ok();
            let owned = match owned {
                Some(ticket) => ticket.await.is_ok(),
                None => false,
            };
            (0, (owned as i32).to_le_bytes().to_vec())
        }
        OP_SET_BRANCH => {
            let branch = String::from_utf8_lossy(aurelia::bridge::ugc::request_body(payload)).trim().to_string();
            let ok = !branch.is_empty() && client.update_app_branch(app, &branch).await.is_ok();
            if ok {
                aurelia::bridge::warm::global().invalidate_app(client.steam_id().unwrap_or(0), app);
            }
            if !ok {
                blog!("bridge: switching app {app} to branch '{branch}' failed");
            }
            (0, (if ok { 1i32 } else { 2i32 }).to_le_bytes().to_vec())
        }
        OP_DELETE => {
            let ok = client.workshop_delete_item(app, id).await;
            if let Err(e) = &ok {
                blog!("bridge: workshop delete {id} failed: {e:#}");
            }
            (0, (if ok.is_ok() { 1i32 } else { 2i32 }).to_le_bytes().to_vec())
        }
        _ => (-1, Vec::new()),
    }
}

fn ugc_submit(client: Arc<SteamClient>, app: u32, handle: u64, spec: aurelia::bridge::ugc::SubmitSpec) {
    use aurelia::bridge::ugc::{jobs, to_request, update_status, PublishJob};
    let item = aurelia::bridge::ugc::spec_item(&spec).unwrap_or(0);
    jobs().lock().expect("ugc jobs lock poisoned").insert(handle, PublishJob { status: 1, item, ..Default::default() });
    tokio::spawn(async move {
        let request = to_request(app, item, spec);
        let progress = move |stage: aurelia::steam_client::workshop_upload::PublishStage, done: u64, total: u64| {
            let mut all = jobs().lock().expect("ugc jobs lock poisoned");
            if let Some(job) = all.get_mut(&handle) {
                (job.status, job.done, job.total) = (update_status(&stage), done, total);
            }
        };
        let result = client.workshop_publish(&request, &progress).await;
        let mut all = jobs().lock().expect("ugc jobs lock poisoned");
        if let Some(job) = all.get_mut(&handle) {
            match result {
                Ok(r) => (job.result, job.item, job.legal, job.status) = (1, r.published_file_id, r.needs_legal_agreement, 5),
                Err(e) => {
                    blog!("bridge: workshop publish of {item} failed: {e:#}");
                    job.result = 2;
                }
            }
        }
    });
}

async fn ugc_download(client: &SteamClient, id: u64) -> i32 {
    use aurelia::core::models::{DownloadProgressState, DownloadState};
    let items = match client.fetch_published_file_details(&[id]).await {
        Ok(items) if !items.is_empty() => items,
        Ok(_) => return 9,
        Err(e) => {
            blog!("bridge: workshop details of {id} failed: {e:#}");
            return 2;
        }
    };
    let state = Arc::new(std::sync::RwLock::new(DownloadState::default()));
    let mut rx = match client.install_workshop_item(&items[0], state).await {
        Ok(rx) => rx,
        Err(e) => {
            blog!("bridge: workshop install of {id} failed to start: {e:#}");
            return 2;
        }
    };
    while let Some(p) = rx.recv().await {
        match p.state {
            DownloadProgressState::Completed => return 1,
            DownloadProgressState::Failed => return 2,
            _ => {}
        }
    }
    2
}

async fn cloud_sync_result(client: &mut SteamClient, app_id: u32, direction: aurelia::library::cloud_sync::SyncDirection, limit: Duration) -> Option<Vec<String>> {
    use aurelia::library::cloud_sync::ConflictPolicy;
    let run = super::cloud::run_cloud_sync(client, app_id, direction, ConflictPolicy::Detect, None);
    match tokio::time::timeout(limit, run).await {
        Ok(Ok(r)) => {
            blog!(
                "bridge: cloud sync {direction:?} for {app_id}: {} down, {} up, {} conflicts, {} skipped, {} failed (remote {})",
                r.outcome.downloaded.len(),
                r.outcome.uploaded.len(),
                r.outcome.conflicts.len(),
                r.outcome.skipped.len(),
                r.outcome.failed.len(),
                r.remote_root.display()
            );
            Some(r.outcome.downloaded)
        }
        Ok(Err(e)) => {
            blog!("bridge: cloud sync {direction:?} for {app_id} failed: {e:#}");
            None
        }
        Err(_) => {
            blog!("bridge: cloud sync {direction:?} for {app_id} timed out after {limit:?}");
            None
        }
    }
}

const DLC_REFRESH: Duration = Duration::from_secs(60);
const FRIENDS_POLL: Duration = Duration::from_secs(3);

fn spawn_with_io(io: Option<crate::output::OutputCtx>, fut: impl std::future::Future<Output = ()> + Send + 'static) -> tokio::task::JoinHandle<()> {
    match io {
        Some(ctx) => tokio::spawn(crate::output::scoped(ctx, fut)),
        None => tokio::spawn(fut),
    }
}

#[derive(Clone, Default)]
struct SocialState {
    friends: Arc<std::sync::Mutex<Vec<aurelia::bridge::social::FriendInfo>>>,
    requests: Arc<std::sync::Mutex<Vec<aurelia::bridge::social::FriendInfo>>>,
}

fn spawn_social(
    client: &Arc<SteamClient>,
    hub: &Arc<aurelia::bridge::overlay::OverlayHub>,
    io: Option<crate::output::OutputCtx>,
    initial: &[aurelia::steam_client::Friend],
) -> (SocialState, Vec<tokio::task::JoinHandle<()>>) {
    use aurelia::bridge::social::{FriendWatch, RequestWatch, split_friends};
    let (friends, requests) = split_friends(initial);
    let social = SocialState { friends: Arc::new(std::sync::Mutex::new(friends.clone())), requests: Arc::new(std::sync::Mutex::new(requests.clone())) };
    let watcher = {
        let (client, hub, social) = (Arc::clone(client), Arc::clone(hub), social.clone());
        spawn_with_io(io.clone(), async move {
            let mut friend_watch = FriendWatch::new(&friends);
            let mut request_watch = RequestWatch::new(&requests);
            let local = (!crate::daemon::in_daemon()).then(|| Arc::new(std::sync::RwLock::new(aurelia::steam_client::Roster::new())));
            let poll = async {
                loop {
                    tokio::time::sleep(FRIENDS_POLL).await;
                    let list: Vec<aurelia::steam_client::Friend> = match &local {
                        Some(roster) => roster.read().expect("roster lock poisoned").values().cloned().collect(),
                        None => crate::daemon::shared_roster_with_requests().await,
                    };
                    let (friends, requests) = split_friends(&list);
                    hub.set_friend_lobbies(friends.iter().filter(|f| f.lobby != 0).map(|f| aurelia::bridge::overlay::FriendLobby { friend: f.steam_id, lobby: f.lobby, app_id: f.game_appid }).collect());
                    for event in friend_watch.update(&friends).into_iter().chain(request_watch.update(&requests)) {
                        hub.emit(event);
                    }
                    *social.friends.lock().expect("friends lock poisoned") = friends;
                    *social.requests.lock().expect("requests lock poisoned") = requests;
                }
            };
            match &local {
                Some(roster) => {
                    tokio::select! {
                        r = client.run_friends_watcher(Arc::clone(roster)) => blog!("bridge: friends watcher ended: {:?}", r.map(|_| ())),
                        _ = poll => {}
                    }
                }
                None => poll.await,
            }
        })
    };
    let chat = {
        let (client, hub) = (Arc::clone(client), Arc::clone(hub));
        spawn_with_io(io, async move {
            use tokio_stream::StreamExt;
            let chat = match client.chat_client() {
                Ok(chat) => chat,
                Err(e) => return blog!("bridge: chat unavailable: {e:#}"),
            };
            let mut events = chat.listen();
            while let Some(item) = events.next().await {
                if let Some(event) = item.ok().and_then(aurelia::bridge::social::chat_event) {
                    hub.emit(event);
                }
            }
        })
    };
    (social, vec![watcher, chat])
}

async fn answer_command(client: &SteamClient, hub: &aurelia::bridge::overlay::OverlayHub, social: &SocialState, line: &str) -> serde_json::Value {
    match aurelia::bridge::overlay::parse_command(line) {
        Ok(aurelia::bridge::overlay::Command::WebSession) => match client.web_session().await {
            Ok(ws) => aurelia::bridge::overlay::web_session_event(ws.steam_id(), &ws.cookies()),
            Err(e) => aurelia::bridge::overlay::web_session_error(&format!("{e:#}")),
        },
        Ok(aurelia::bridge::overlay::Command::Query { req, query }) => aurelia::bridge::overlay::with_req(run_query(client, social, query).await, &req),
        Ok(cmd) => aurelia::bridge::overlay::apply(hub, cmd),
        Err(_) => aurelia::bridge::overlay::line_error(line),
    }
}

async fn run_query(client: &SteamClient, social: &SocialState, query: aurelia::bridge::overlay::Query) -> serde_json::Value {
    use aurelia::bridge::overlay as o;
    let failed = |e: anyhow::Error| o::query_error("query_failed", &format!("{e:#}"));
    match query {
        o::Query::Friends => o::friends_event(&social.friends.lock().expect("friends lock poisoned")),
        o::Query::Join { steam_id } => {
            let friend = social.friends.lock().expect("friends lock poisoned").iter().find(|f| f.steam_id == steam_id).cloned();
            match friend {
                None => o::query_error("not_friend", "this SteamID is not in the friends list"),
                Some(f) if !f.joinable() => o::query_error("not_joinable", "the friend is not in a match that can be joined"),
                Some(f) => {
                    let installed = aurelia::library::scan_installed_app_info().await.map(|apps| apps.contains_key(&f.game_appid)).unwrap_or(false);
                    aurelia::bridge::social::join_event(&f, installed)
                }
            }
        }
        o::Query::FriendRequests => aurelia::bridge::social::friend_requests_event(&social.requests.lock().expect("requests lock poisoned")),
        o::Query::ChatSend { steam_id, message } => match client.send_chat_message(steam_id, message).await {
            Ok(sent) => o::chat_sent_event(sent.server_timestamp, sent.modified_message.as_deref()),
            Err(e) => failed(e),
        },
        o::Query::ChatHistory { steam_id, count } => match client.recent_chat_messages(steam_id, count).await {
            Ok(messages) => o::chat_history_event(steam_id, &messages),
            Err(e) => failed(e),
        },
        o::Query::Achievements { app_id, lang } => match client.fetch_achievements(app_id, &resolve_steam_language(lang).await).await {
            Ok(list) => o::achievements_event(app_id, &list),
            Err(e) => failed(e),
        },
        o::Query::Profile { steam_id } => match client.user_profile(steam_id).await {
            Ok(p) => o::profile_event(&p),
            Err(e) => failed(e),
        },
        o::Query::Persona { state } => match client.announce_persona(state).await {
            Ok(()) => serde_json::json!({ "event": "result", "ok": true }),
            Err(e) => failed(e),
        },
        o::Query::FriendAdd { steam_id } => match client.add_friend(steam_id).await {
            Ok(added) => o::friend_added_event(added.steam_id, added.persona_name.as_deref()),
            Err(e) => failed(e),
        },
        o::Query::Invite { steam_id, connect } => match client.send_game_invite(steam_id, connect).await {
            Ok(()) => serde_json::json!({ "event": "result", "ok": true }),
            Err(e) => failed(e),
        },
        o::Query::FriendRemove { steam_id } => match client.remove_friend(steam_id).await {
            Ok(()) => serde_json::json!({ "event": "result", "ok": true }),
            Err(e) => failed(e),
        },
    }
}

pub(crate) async fn cmd_bridge_simulate(app_id: u32, interval_ms: u64) -> Result<()> {
    use aurelia::bridge::simulate;
    let (hub, mut events) = aurelia::bridge::overlay::OverlayHub::new();
    print_json_line(&serde_json::json!({ "event": "ready", "simulate": true, "app_id": app_id }));
    simulate::seed_recent(&hub, app_id);
    let io = crate::output::current();
    let printer = spawn_with_io(io.clone(), async move {
        while let Some(event) = events.recv().await {
            print_json_line(&event);
        }
    });
    let steps = {
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            for step in 0..simulate::STEPS {
                if interval_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                }
                simulate::emit_step(&hub, app_id, step);
            }
        })
    };
    while let Ok(Some(line)) = crate::output::read_line_opt().await {
        if line.trim().is_empty() {
            continue;
        }
        print_json_line(&simulate::answer(&hub, &line));
    }
    let _ = steps.await;
    drop(hub);
    let _ = printer.await;
    Ok(())
}

pub(crate) async fn cmd_watch() -> Result<()> {
    let client = authed_client().await?;
    let shared = if crate::daemon::in_daemon() {
        let mut roster = crate::daemon::shared_roster_with_requests().await;
        for _ in 0..4 {
            if !roster.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            roster = crate::daemon::shared_roster_with_requests().await;
        }
        Some(roster)
    } else {
        None
    };
    let friend_list = aurelia::bridge::snapshot::read_friend_list(&client, shared).await;
    let client = Arc::new(client);
    let (hub, mut events) = aurelia::bridge::overlay::OverlayHub::new();
    print_json_line(&serde_json::json!({ "event": "ready", "watch": true, "steam_id": client.steam_id().map(|id| id.to_string()) }));
    let io = crate::output::current();
    let printer = spawn_with_io(io.clone(), async move {
        while let Some(event) = events.recv().await {
            print_json_line(&event);
        }
    });
    let (social, social_tasks) = spawn_social(&client, &hub, io.clone(), &friend_list);
    let commands = {
        let (hub, client, social) = (Arc::clone(&hub), Arc::clone(&client), social.clone());
        spawn_with_io(io, async move {
            while let Ok(Some(line)) = crate::output::read_line_opt().await {
                if line.trim().is_empty() {
                    continue;
                }
                print_json_line(&answer_command(&client, &hub, &social, &line).await);
            }
        })
    };
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let terminated = async {
        #[cfg(unix)]
        term.recv().await;
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminated => {}
    }
    commands.abort();
    for task in &social_tasks {
        task.abort();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    printer.abort();
    Ok(())
}

struct OnlineSession {
    client: SteamClient,
    pool: Pool,
    collector: Option<tokio::task::JoinHandle<()>>,
    cloud_changes: Vec<String>,
    snapshot: aurelia::bridge::snapshot::SnapshotV1,
    friend_list: Vec<aurelia::steam_client::Friend>,
}

fn forced_offline() -> bool {
    static FIRST: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let Some(secs) = std::env::var("AURELIA_BRIDGE_OFFLINE_FOR").ok().and_then(|v| v.parse::<u64>().ok()) else { return false };
    FIRST.get_or_init(Instant::now).elapsed() < Duration::from_secs(secs)
}

async fn open_online(app_id: u32, shortcut: bool, cloud_down: bool, t0: Instant) -> Result<OnlineSession> {
    let mut mark = Instant::now();
    let step = |name: &str, mark: &mut Instant| {
        blog!("bridge: startup {name}: {} ms (total {} ms)", mark.elapsed().as_millis(), t0.elapsed().as_millis());
        *mark = Instant::now();
    };
    if let Some(ms) = std::env::var("AURELIA_BRIDGE_SLOW_OPEN_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        static SLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SLOWED.swap(true, std::sync::atomic::Ordering::AcqRel) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }
    if forced_offline() {
        anyhow::bail!("offline (AURELIA_BRIDGE_OFFLINE_FOR)");
    }
    let mut client = match tokio::time::timeout(aurelia::bridge::offline::SESSION_OPEN, authed_client()).await {
        Ok(client) => client?,
        Err(_) => anyhow::bail!("timed out opening the Steam session"),
    };
    step("session", &mut mark);
    let shared = if crate::daemon::in_daemon() {
        let mut roster = crate::daemon::shared_roster_nowait().await;
        for _ in 0..16 {
            if !roster.is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
            roster = crate::daemon::shared_roster_nowait().await;
        }
        Some(roster)
    } else { None };
    step("roster wait", &mut mark);
    let friend_list = aurelia::bridge::snapshot::read_friend_list(&client, shared).await;
    let friends = aurelia::bridge::snapshot::friend_entries(friend_list.clone());
    step("friend list", &mut mark);
    let pool: Pool = Arc::new(std::sync::Mutex::new(TokenPool::new(TOKEN_POOL_SIZE)));
    if shortcut {
        step("declare + connect tokens (atalho: pulado)", &mut mark);
        let snapshot = aurelia::bridge::snapshot::build_snapshot(&mut client, None, friends).await?;
        step("snapshot", &mut mark);
        return Ok(OnlineSession { client, pool, collector: None, cloud_changes: Vec::new(), snapshot, friend_list });
    }
    let collector = client.collect_game_connect_tokens(Arc::clone(&pool))?;
    let (token_tx, token_rx) = tokio::sync::oneshot::channel::<std::result::Result<Vec<u8>, String>>();
    let declare = {
        let (client, pool) = (client.clone(), Arc::clone(&pool));
        async move {
            let outcome = async {
                let began = Instant::now();
                client.declare_playing(app_id).await.map_err(|e| format!("{e:#}"))?;
                blog!("bridge: startup declare sent: {} ms", began.elapsed().as_millis());
                let deadline = Instant::now() + TOKEN_WAIT;
                while pool_len(&pool) == 0 && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                blog!("bridge: startup first token: {} ms", began.elapsed().as_millis());
                pool.lock().expect("token pool lock poisoned").peek().ok_or_else(|| format!("Steam did not deliver a game connect token for app {app_id}"))
            }
            .await;
            let _ = token_tx.send(outcome);
        }
    };
    let steam_id = client.steam_id();
    let cloud = {
        let mut client = client.clone();
        async move {
            use aurelia::library::cloud_sync::SyncDirection;
            if let Some(id) = steam_id.filter(|id| aurelia::bridge::offline::has_cloud_up(*id, app_id)) {
                if cloud_sync_result(&mut client, app_id, SyncDirection::Up, aurelia::bridge::offline::CLOUD_OPEN).await.is_some() {
                    aurelia::bridge::offline::clear_cloud_up(id, app_id);
                }
                return Vec::new();
            }
            if !cloud_down {
                return Vec::new();
            }
            cloud_sync_result(&mut client, app_id, SyncDirection::Down, aurelia::bridge::offline::CLOUD_OPEN).await.unwrap_or_default()
        }
    };
    let token_future: aurelia::bridge::snapshot::TokenFuture = Box::pin(async move {
        match token_rx.await {
            Ok(Ok(token)) => Ok(token),
            Ok(Err(reason)) => Err(anyhow::anyhow!(reason)),
            Err(_) => Err(anyhow::anyhow!("the connect token request ended without an answer")),
        }
    });
    let snapshot_job = async {
        let began = Instant::now();
        let r = aurelia::bridge::snapshot::build_snapshot(&mut client, Some((app_id, token_future)), friends).await;
        blog!("bridge: startup snapshot built: {} ms", began.elapsed().as_millis());
        r
    };
    let cloud = async {
        let began = Instant::now();
        let r = cloud.await;
        blog!("bridge: startup cloud: {} ms", began.elapsed().as_millis());
        r
    };
    let ((), cloud_changes, snapshot) = tokio::join!(declare, cloud, snapshot_job);
    step("declare + cloud + snapshot (em paralelo)", &mut mark);
    match snapshot {
        Ok(snapshot) => Ok(OnlineSession { client, pool, collector: Some(collector), cloud_changes, snapshot, friend_list }),
        Err(e) => {
            client.clear_playing_app(app_id).await.ok();
            collector.abort();
            Err(e)
        }
    }
}

struct Serve {
    snapshot: aurelia::bridge::server::SharedSnapshot,
    app_id: u32,
    shortcut: bool,
    steam_id: u64,
    hub: Arc<aurelia::bridge::overlay::OverlayHub>,
    io: Option<crate::output::OutputCtx>,
    started: Instant,
    connection: Arc<aurelia::bridge::offline::Connection>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    handler: std::sync::OnceLock<CallHandler>,
    attached: std::sync::OnceLock<Attached>,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    subscriptions: std::sync::Mutex<Vec<Vec<u8>>>,
    opening: std::sync::atomic::AtomicBool,
    ready: tokio::sync::Notify,
}

struct Attached {
    client: Arc<SteamClient>,
    state: Arc<Mutex<AuthState>>,
    pool: Pool,
    collector: Option<tokio::task::JoinHandle<()>>,
    social: SocialState,
}

impl Serve {
    fn refresh_queued(&self) {
        self.queued.store(aurelia::bridge::offline::pending(self.steam_id), std::sync::atomic::Ordering::Relaxed);
    }

    fn track(&self, tasks: impl IntoIterator<Item = tokio::task::JoinHandle<()>>) {
        self.tasks.lock().expect("tasks lock poisoned").extend(tasks);
    }
}

async fn attach_online(serve: &Arc<Serve>, online: OnlineSession, replay_subscriptions: bool) {
    let OnlineSession { client, pool, collector, cloud_changes, snapshot, friend_list } = online;
    if let Ok(bytes) = serde_json::to_vec(&snapshot) {
        if let Ok(mut cell) = serve.snapshot.write() {
            *cell = Arc::new(bytes);
        }
    }
    let client = Arc::new(client);
    let state = Arc::new(Mutex::new(AuthState::default()));
    let stats_gate: StatsGate = Arc::new(Mutex::new(None));
    let dlc_initial: Vec<aurelia::bridge::overlay::DlcState> = snapshot
        .app
        .as_ref()
        .map(|a| a.dlc.iter().map(|d| aurelia::bridge::overlay::DlcState { dlc_id: d.appid, owned: d.owned, installed: d.installed }).collect())
        .unwrap_or_default();
    let (app_id, shortcut) = (serve.app_id, serve.shortcut);
    serve.hub.init_dlc(dlc_initial.clone());
    serve.hub.set_cloud_changes(cloud_changes);
    let dlc_watcher = {
        let (client, hub) = (Arc::clone(&client), Arc::clone(&serve.hub));
        let ids: Vec<u32> = if shortcut { Vec::new() } else { dlc_initial.iter().map(|d| d.dlc_id).collect() };
        spawn_with_io(serve.io.clone(), async move {
            while !ids.is_empty() {
                tokio::time::sleep(DLC_REFRESH).await;
                match client.dlc_states(app_id, &ids).await {
                    Ok(states) => hub.set_dlc(
                        app_id,
                        states.into_iter().map(|d| aurelia::bridge::overlay::DlcState { dlc_id: d.app_id, owned: d.owned, installed: d.installed && !d.disabled }).collect(),
                    ),
                    Err(e) => blog!("bridge: dlc refresh failed: {e:#}"),
                }
            }
        })
    };
    let (social, social_tasks) = spawn_social(&client, &serve.hub, serve.io.clone(), &friend_list);
    let stats_prefetch = {
        let (client, gate) = (Arc::clone(&client), Arc::clone(&stats_gate));
        tokio::spawn(async move {
            if !shortcut {
                let _ = stats_for(&client, &gate, app_id).await;
            }
        })
    };
    let flusher = {
        let (serve, client) = (Arc::clone(serve), Arc::clone(&client));
        tokio::spawn(async move {
            loop {
                flush_queue(&serve, &client).await;
                tokio::time::sleep(FLUSH_EVERY).await;
            }
        })
    };
    let state_for_handler = Arc::clone(&state);
    let handler = call_handler(Arc::clone(&client), state_for_handler, Arc::clone(&pool), serve.started, app_id, Arc::clone(&serve.hub), Arc::clone(&stats_gate), serve.steam_id, Arc::clone(&serve.queued));
    if replay_subscriptions {
        let payloads: Vec<Vec<u8>> = std::mem::take(&mut *serve.subscriptions.lock().expect("subscriptions lock poisoned"));
        for payload in payloads {
            let _ = handler(RESERVED_IFACE, SLOT_CM_SUBSCRIBE, payload).await;
        }
    }
    let _ = serve.attached.set(Attached { client, state, pool, collector, social });
    let _ = serve.handler.set(handler);
    serve.track(social_tasks.into_iter().chain([dlc_watcher, stats_prefetch, flusher]));
    serve.connection.set(true);
    serve.opening.store(false, std::sync::atomic::Ordering::SeqCst);
    serve.ready.notify_waiters();
    serve.hub.emit(aurelia::bridge::overlay::connection_event(true, serve.queued.load(std::sync::atomic::Ordering::Relaxed)));
}

const FLUSH_EVERY: Duration = Duration::from_secs(30);

async fn flush_queue(serve: &Serve, client: &Arc<SteamClient>) {
    use aurelia::bridge::offline::{self, Op};
    use aurelia::library::cloud_sync::SyncDirection;
    let ops = offline::snapshot(serve.steam_id);
    if ops.is_empty() {
        serve.refresh_queued();
        return;
    }
    blog!("bridge: sending {} queued operation(s) written without a connection", ops.len());
    for op in ops {
        let ok = match &op {
            Op::StoreStats { app, changes } => match fetch_stats(client, *app).await {
                Some(fetched) => match client.store_user_stats(*app, fetched.crc, changes).await {
                    Ok(resp) => {
                        blog!("bridge: queued stats for {app}: eresult={}", resp.eresult());
                        resp.eresult() == 1
                    }
                    Err(e) => {
                        blog!("bridge: queued stats for {app} failed: {e:#}");
                        false
                    }
                },
                None => false,
            },
            Op::CloudUp { app } => cloud_sync_result(&mut (**client).clone(), *app, SyncDirection::Up, CLOUD_TIMEOUT).await.is_some(),
        };
        if ok {
            offline::ack(serve.steam_id, &op);
        }
    }
    let left = offline::pending(serve.steam_id);
    serve.queued.store(left, std::sync::atomic::Ordering::Relaxed);
    blog!("bridge: queue flushed, {left} left");
}

fn offline_call(serve: &Arc<Serve>, iface: u16, slot: u16, payload: Vec<u8>) -> (i32, Vec<u8>) {
    use aurelia::bridge::offline::{self, Op};
    if iface != RESERVED_IFACE {
        return (-1, Vec::new());
    }
    match slot {
        SLOT_STORE_USER_STATS => {
            let Some((_, changes)) = parse_store_payload(&payload) else { return (-2, Vec::new()) };
            let queued = offline::enqueue(serve.steam_id, Op::StoreStats { app: serve.app_id, changes });
            serve.queued.store(queued, std::sync::atomic::Ordering::Relaxed);
            (1, Vec::new())
        }
        SLOT_GET_USER_STATS => match aurelia::bridge::warm::load_stats(serve.steam_id, serve.app_id) {
            Some((eresult, reply)) => (eresult, reply),
            None => (-5, Vec::new()),
        },
        SLOT_CM_SUBSCRIBE => {
            serve.subscriptions.lock().expect("subscriptions lock poisoned").push(payload);
            (0, Vec::new())
        }
        SLOT_CM_POLL => (0, Vec::new()),
        SLOT_REGISTER_TICKET | SLOT_CANCEL_TICKET | SLOT_SET_PRESENCE => (0, Vec::new()),
        SLOT_CREATE_TICKET => (-3, Vec::new()),
        aurelia::bridge::accountinfo::SLOT_ACCOUNT_INFO => (0, aurelia::bridge::accountinfo::encode(None)),
        aurelia::bridge::appsquery::SLOT_APPS => match aurelia::bridge::appsquery::answer(serve.steam_id, &payload) {
            Some(r) => (0, r),
            None => (-2, Vec::new()),
        },
        aurelia::bridge::overlay::SLOT_OVERLAY => aurelia::bridge::overlay::overlay_call(&serve.hub, serve.app_id, &payload),
        _ => (-5, Vec::new()),
    }
}

const CALL_WAIT_FOR_CONNECTION: Duration = Duration::from_secs(25);

fn needs_connection(iface: u16, slot: u16) -> bool {
    iface == RESERVED_IFACE
        && !matches!(
            slot,
            aurelia::bridge::offline::SLOT_CONNECTION | SLOT_CM_POLL | SLOT_REGISTER_TICKET | SLOT_CANCEL_TICKET | aurelia::bridge::overlay::SLOT_OVERLAY | aurelia::bridge::appsquery::SLOT_APPS | aurelia::bridge::accountinfo::SLOT_ACCOUNT_INFO
        )
}

fn switch_handler(serve: Arc<Serve>) -> CallHandler {
    Arc::new(move |iface, slot, payload| {
        let serve = Arc::clone(&serve);
        Box::pin(async move {
            if iface == RESERVED_IFACE && slot == aurelia::bridge::offline::SLOT_CONNECTION {
                return (0, vec![u8::from(serve.connection.online())]);
            }
            if serve.handler.get().is_none() && serve.opening.load(std::sync::atomic::Ordering::SeqCst) && needs_connection(iface, slot) {
                let ready = serve.ready.notified();
                tokio::pin!(ready);
                ready.as_mut().enable();
                if serve.handler.get().is_none() && serve.opening.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = tokio::time::timeout(CALL_WAIT_FOR_CONNECTION, ready).await;
                }
            }
            match serve.handler.get() {
                Some(handler) => handler(iface, slot, payload).await,
                None => offline_call(&serve, iface, slot, payload),
            }
        })
    })
}

fn answer_command_offline(hub: &aurelia::bridge::overlay::OverlayHub, line: &str) -> serde_json::Value {
    match aurelia::bridge::overlay::parse_command(line) {
        Ok(aurelia::bridge::overlay::Command::WebSession) => aurelia::bridge::overlay::web_session_error("offline: the Steam connection is not up yet"),
        Ok(aurelia::bridge::overlay::Command::Query { req, query: _ }) => aurelia::bridge::overlay::with_req(aurelia::bridge::overlay::query_error("offline", "the Steam connection is not up yet"), &req),
        Ok(cmd) => aurelia::bridge::overlay::apply(hub, cmd),
        Err(_) => aurelia::bridge::overlay::line_error(line),
    }
}

async fn reconnect(serve: Arc<Serve>, inflight: Option<tokio::task::JoinHandle<Result<OnlineSession>>>) {
    let mut attempt = 0u32;
    if let Some(task) = inflight {
        match task.await {
            Ok(Ok(online)) => {
                blog!("bridge: the connection to Steam finished opening in the background");
                attach_online(&serve, online, false).await;
                return;
            }
            Ok(Err(e)) => blog!("bridge: the background opening failed: {e:#}"),
            Err(e) => blog!("bridge: the background opening ended abnormally: {e}"),
        }
        serve.opening.store(false, std::sync::atomic::Ordering::SeqCst);
        serve.ready.notify_waiters();
    }
    loop {
        tokio::time::sleep(aurelia::bridge::offline::retry_delay(attempt)).await;
        attempt += 1;
        match open_online(serve.app_id, serve.shortcut, false, Instant::now()).await {
            Ok(online) => {
                blog!("bridge: connection to Steam is back after {attempt} attempt(s)");
                attach_online(&serve, online, true).await;
                return;
            }
            Err(e) => blog!("bridge: still offline (attempt {attempt}): {e:#}"),
        }
    }
}

async fn manual_game_snapshot() -> (u64, aurelia::bridge::snapshot::SnapshotV1) {
    use aurelia::bridge::snapshot::{Account, SnapshotV1, SCHEMA};
    let saved = aurelia::core::config::load_session().await.unwrap_or_default();
    let account = match (saved.steam_id, saved.refresh_token.is_some()) {
        (Some(id), true) => Account { steamid: id, persona: saved.persona_name.clone().unwrap_or_default(), country: String::new() },
        _ => Account { steamid: 0, persona: String::new(), country: String::new() },
    };
    let id = account.steamid;
    (id, SnapshotV1 { schema: SCHEMA, account, apps: Vec::new(), ticket: None, app: None, friends: Vec::new(), online: false })
}

const ONLINE_BUDGET: Duration = Duration::from_millis(1500);

async fn local_snapshot(app_id: u32, shortcut: bool) -> Option<(u64, aurelia::bridge::snapshot::SnapshotV1)> {
    let saved = aurelia::bridge::warm::last_account_with_snapshot(app_id).and_then(|id| aurelia::bridge::warm::load_snapshot(id, app_id).map(|(s, _)| (id, s)));
    match saved {
        None if shortcut => Some(manual_game_snapshot().await),
        None => aurelia::bridge::snapshot::offline_snapshot(app_id).await,
        other => other,
    }
}

pub(crate) async fn cmd_bridge_serve(app_id: u32) -> Result<()> {
    let t0 = Instant::now();
    let started = Instant::now();
    let shortcut = crate::commands::shortcut::find(app_id).is_some();
    let mut task = tokio::spawn(open_online(app_id, shortcut, true, t0));
    let mut inflight = None;
    let mut local: Option<(u64, aurelia::bridge::snapshot::SnapshotV1)> = None;
    let joined = match tokio::time::timeout(ONLINE_BUDGET, &mut task).await {
        Ok(joined) => Some(joined),
        Err(_) => {
            local = local_snapshot(app_id, shortcut).await;
            if local.is_some() {
                blog!("bridge: Steam did not answer within {} ms; opening from the local snapshot while the connection finishes", ONLINE_BUDGET.as_millis());
                inflight = Some(task);
                None
            } else {
                Some((&mut task).await)
            }
        }
    };
    let (online, offline_reason) = match joined {
        Some(Ok(Ok(online))) => (Some(online), None),
        Some(Ok(Err(e))) => {
            blog!("bridge: opening online failed, trying the cached state: {e:#}");
            (None, Some(e))
        }
        Some(Err(e)) => {
            blog!("bridge: opening online ended abnormally: {e}");
            (None, Some(anyhow::anyhow!("opening the Steam session ended abnormally")))
        }
        None => (None, Some(anyhow::anyhow!("Steam did not answer in time"))),
    };
    let (snapshot, steam_id, opened_offline) = match &online {
        Some(o) => (o.snapshot.clone(), o.client.steam_id().unwrap_or(0), false),
        None => {
            let cached = match local.take() {
                Some(l) => Some(l),
                None => local_snapshot(app_id, shortcut).await,
            };
            let Some((id, mut snapshot)) = cached else {
                let e = offline_reason.unwrap_or_else(|| anyhow::anyhow!("could not open the Steam session"));
                return Err(e.context("no saved state to open this game without a connection; open it once online"));
            };
            snapshot.online = false;
            (snapshot, id, true)
        }
    };
    if let Some(o) = &online {
        let (id, copy) = (steam_id, o.snapshot.clone());
        tokio::task::spawn_blocking(move || aurelia::bridge::warm::save_snapshot(id, app_id, &copy));
    }
    blog!("bridge: startup total: {} ms{}", t0.elapsed().as_millis(), if opened_offline { " (offline, cached snapshot)" } else { "" });
    let (listener, token) = aurelia::bridge::server::bind().await?;
    let port = listener.local_addr()?.port();
    print_json_line(&serde_json::json!({ "port": port, "token": hex::encode(token) }));
    let snapshot_cell: aurelia::bridge::server::SharedSnapshot = Arc::new(std::sync::RwLock::new(Arc::new(serde_json::to_vec(&snapshot)?)));
    let (overlay_hub, mut overlay_events) = aurelia::bridge::overlay::OverlayHub::new();
    let io = crate::output::current();
    let connection = Arc::new(aurelia::bridge::offline::Connection::new(false));
    let queued = Arc::new(std::sync::atomic::AtomicUsize::new(aurelia::bridge::offline::pending(steam_id)));
    let serve = Arc::new(Serve {
        snapshot: Arc::clone(&snapshot_cell),
        app_id,
        shortcut,
        steam_id,
        hub: Arc::clone(&overlay_hub),
        io: io.clone(),
        started,
        connection: Arc::clone(&connection),
        queued: Arc::clone(&queued),
        handler: std::sync::OnceLock::new(),
        attached: std::sync::OnceLock::new(),
        tasks: std::sync::Mutex::new(Vec::new()),
        subscriptions: std::sync::Mutex::new(Vec::new()),
        opening: std::sync::atomic::AtomicBool::new(inflight.is_some()),
        ready: tokio::sync::Notify::new(),
    });
    let _registration = aurelia::bridge::registry::register(
        aurelia::bridge::registry::Session {
            app: app_id,
            steam_id,
            since: aurelia::bridge::registry::now_secs(),
            startup_ms: t0.elapsed().as_millis() as u64,
            opened_offline,
            online: !opened_offline,
            queued: 0,
            port,
        },
        Arc::clone(&connection),
        Arc::clone(&queued),
    );
    let printer = {
        let serve = Arc::clone(&serve);
        spawn_with_io(io.clone(), async move {
            let mut known: Vec<aurelia::steam_client::GameAchievement> = Vec::new();
            let lang = resolve_steam_language(None).await;
            while let Some(mut event) = overlay_events.recv().await {
                if event["event"] == "achievement_unlocked" {
                    let api_name = event["api_name"].as_str().unwrap_or_default().to_string();
                    if let Some(attached) = serve.attached.get() {
                        if !known.iter().any(|a| a.api_name == api_name) {
                            match attached.client.fetch_achievements(app_id, &lang).await {
                                Ok(list) => known = list,
                                Err(e) => blog!("bridge: achievement details unavailable: {e:#}"),
                            }
                        }
                    }
                    let found = known.iter().find(|a| a.api_name == api_name);
                    event = aurelia::bridge::overlay::unlocked_event(event, found);
                }
                print_json_line(&event);
            }
        })
    };
    match aurelia::core::config::data_dir() {
        Ok(dir) => overlay_hub.load_recent(dir.join("recent_players.json")),
        Err(e) => blog!("bridge: recent players will not be saved: {e:#}"),
    }
    if let Some(path) = aurelia::bridge::social::steam_coplay_path(steam_id).filter(|_| steam_id != 0) {
        overlay_hub.use_steam_coplay(path);
    }
    let stop = Arc::new(tokio::sync::Notify::new());
    let commands = {
        let (serve, stop) = (Arc::clone(&serve), Arc::clone(&stop));
        spawn_with_io(io, async move {
            while let Ok(Some(line)) = crate::output::read_line_opt().await {
                if line.trim().is_empty() {
                    continue;
                }
                let stopping = matches!(aurelia::bridge::overlay::parse_command(&line), Ok(aurelia::bridge::overlay::Command::Stop));
                let answer = match serve.attached.get() {
                    Some(a) => answer_command(&a.client, &serve.hub, &a.social, &line).await,
                    None => answer_command_offline(&serve.hub, &line),
                };
                print_json_line(&answer);
                if stopping {
                    stop.notify_one();
                }
            }
        })
    };
    match online {
        Some(online) => attach_online(&serve, online, false).await,
        None => {
            serve.refresh_queued();
            overlay_hub.emit(aurelia::bridge::overlay::connection_event(false, queued.load(std::sync::atomic::Ordering::Relaxed)));
            let task = tokio::spawn(reconnect(Arc::clone(&serve), inflight.take()));
            serve.track([task]);
        }
    }
    let handler = switch_handler(Arc::clone(&serve));
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let terminated = async {
        #[cfg(unix)]
        term.recv().await;
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    };
    let served = tokio::select! {
        r = aurelia::bridge::server::run_with_shared(listener, token, snapshot_cell, handler) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = terminated => Ok(()),
        _ = stop.notified() => Ok(()),
    };
    overlay_hub.close_all("game_exit");
    commands.abort();
    let _ = commands.await;
    let tasks: Vec<_> = std::mem::take(&mut *serve.tasks.lock().expect("tasks lock poisoned"));
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    printer.abort();
    let _ = printer.await;
    match serve.attached.get() {
        Some(a) => {
            if !shortcut {
                let up = cloud_sync_result(&mut (*a.client).clone(), app_id, aurelia::library::cloud_sync::SyncDirection::Up, CLOUD_TIMEOUT).await;
                if up.is_none() {
                    aurelia::bridge::offline::enqueue(steam_id, aurelia::bridge::offline::Op::CloudUp { app: app_id });
                }
            }
            if let Some(collector) = &a.collector {
                collector.abort();
            }
            a.state.lock().await.tickets.clear();
            push_auth_list(&a.client, &mut *a.state.lock().await, pool_len(&a.pool)).await;
            if !shortcut {
                a.client.clear_playing_app(app_id).await.ok();
            }
        }
        None => {
            if !shortcut {
                aurelia::bridge::offline::enqueue(steam_id, aurelia::bridge::offline::Op::CloudUp { app: app_id });
            }
        }
    }
    served?;
    Ok(())
}

pub(crate) async fn cmd_bridge_prewarm(app_id: u32, json: bool) -> Result<()> {
    let started = Instant::now();
    let client = authed_client().await?;
    let fetched = aurelia::bridge::snapshot::prewarm_app_now(&client, app_id).await;
    let ms = started.elapsed().as_millis() as u64;
    if json {
        print_json_line(&serde_json::json!({ "event": "prewarmed", "app_id": app_id, "fetched": fetched, "ms": ms }));
    } else {
        cli_println!("app {app_id}: {} ({ms} ms)", if fetched { "prepared" } else { "already recent" });
    }
    Ok(())
}

pub(crate) fn cmd_bridge_status(json: bool) -> Result<()> {
    let sessions = aurelia::bridge::registry::sessions();
    let declared = SteamClient::playing_apps();
    let accounts = aurelia::bridge::warm::accounts();
    let log = aurelia::core::config::config_dir().ok().map(|d| d.join("logs").join("bridge.log"));
    let log_bytes = log.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
    let managed = crate::daemon::systemd_managed();
    if json {
        print_json_line(&serde_json::json!({
            "pid": std::process::id(),
            "in_daemon": crate::daemon::in_daemon(),
            "managed_by_systemd": managed,
            "sessions": sessions,
            "declared_apps": declared,
            "accounts": accounts,
            "log": { "path": log, "bytes": log_bytes },
        }));
        return Ok(());
    }
    cli_println!("daemon pid {}{}", std::process::id(), if managed { " (systemd --user)" } else { "" });
    if sessions.is_empty() {
        cli_println!("no game attached");
    }
    for s in &sessions {
        cli_println!(
            "app {}: {} since {}s ago, ready in {} ms{}, port {}, queued {}",
            s.app,
            if s.online { "online" } else { "OFFLINE" },
            aurelia::bridge::registry::now_secs().saturating_sub(s.since),
            s.startup_ms,
            if s.opened_offline { " (opened offline)" } else { "" },
            s.port,
            s.queued
        );
    }
    cli_println!("declared to Steam: {}", if declared.is_empty() { "none".to_string() } else { declared.iter().map(u32::to_string).collect::<Vec<_>>().join(", ") });
    for a in &accounts {
        cli_println!("account {}: {} names, {} app infos, {} offline snapshots, queue {}", a.steam_id, a.names, a.app_info.len(), a.snapshots.len(), a.queued);
        for (app, age) in &a.snapshots {
            cli_println!("  snapshot app {app}: {age}s old");
        }
    }
    cli_println!("log: {} bytes", log_bytes);
    Ok(())
}

pub(crate) async fn cmd_web_session() -> Result<()> {
    let client = authed_client().await?;
    let ws = client.web_session().await?;
    print_json_line(&aurelia::bridge::overlay::web_session_event(ws.steam_id(), &ws.cookies()));
    Ok(())
}
