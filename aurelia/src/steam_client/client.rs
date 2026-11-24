use super::*;
use crate::core::error::{ErrorKind, TypedError};
use steam_vent_proto::steammessages_clientserver_friends::CMsgClientPersonaState;
use tokio_stream::StreamExt;

pub type CodeReader =
    std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<Option<String>>> + Send>>;

pub(crate) async fn feed_code_while<F: std::future::Future>(
    work: F,
    read_code: CodeReader,
    mut writer: tokio::io::DuplexStream,
) -> F::Output {
    tokio::pin!(work);
    let feed = async move {
        if let Ok(Some(line)) = read_code.await {
            let _ = writer
                .write_all(format!("{}\n", line.trim()).as_bytes())
                .await;
        }
    };
    tokio::select! {
        out = &mut work => out,
        () = feed => work.await,
    }
}

static PLAYING: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn playing_after_declare(set: &[u32], appid: u32) -> Vec<u32> {
    let mut out = set.to_vec();
    out.push(appid);
    out
}

pub(crate) fn playing_after_clear(set: &[u32], appid: u32) -> Vec<u32> {
    let mut out = set.to_vec();
    if let Some(at) = out.iter().position(|&a| a == appid) {
        out.remove(at);
    }
    out
}

pub(crate) fn declared_games(set: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for &a in set {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

impl SteamClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            connection: None,
            state: LoginState::Connected,
            connected_at: None,
            active_cm: None,
            server_list: None,
            pending_confirmations: Vec::new(),
            restore_error: None,
        })
    }

    pub fn from_shared(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            state: LoginState::Complete,
            connected_at: Some(Instant::now()),
            active_cm: None,
            server_list: None,
            pending_confirmations: Vec::new(),
            restore_error: None,
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.connection.is_some()
    }

    pub fn set_restore_error(&mut self, err: anyhow::Error) {
        self.restore_error = Some(Arc::new(err));
    }

    pub fn restore_error(&self) -> Option<&anyhow::Error> {
        self.restore_error.as_deref()
    }

    pub fn enter_offline_with_account(&mut self) {
        self.state = LoginState::Offline;
        self.connection = None;
    }

    pub fn is_offline(&self) -> bool {
        self.state == LoginState::Offline
    }

    pub fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()
    }

    pub async fn probe_alive(&self) -> Result<()> {
        let connection = self.require_connection()?;
        let mut req = CFamilyGroups_GetFamilyGroupForUser_Request::new();
        req.set_steamid(u64::from(connection.steam_id()));
        let _: CFamilyGroups_GetFamilyGroupForUser_Response =
            tokio::time::timeout(PROBE_TIMEOUT, connection.service_method(req))
                .await
                .context("liveness probe timed out")?
                .context("liveness probe failed")?;
        Ok(())
    }

    pub fn cloud_client(&self) -> Result<crate::library::cloud_sync::CloudClient> {
        let connection = self.require_connection_owned()?;
        Ok(crate::library::cloud_sync::CloudClient::new(connection))
    }

    pub fn steam_id(&self) -> Option<u64> {
        self.connection
            .as_ref()
            .map(|connection| u64::from(connection.steam_id()))
    }

    pub async fn revoke_refresh_token(&mut self, token: &str) -> Result<()> {
        use steam_vent_proto::steammessages_auth_steamclient::{
            CAuthentication_Token_Revoke_Request, CAuthentication_Token_Revoke_Response,
            EAuthTokenRevokeAction,
        };
        let mut request = CAuthentication_Token_Revoke_Request::new();
        request.set_token(token.to_string());
        request.set_revoke_action(EAuthTokenRevokeAction::k_EAuthTokenRevokePermanent);
        if let Some(connection) = &self.connection {
            let _: CAuthentication_Token_Revoke_Response = connection
                .service_method(request)
                .await
                .context("failed calling Authentication.RevokeToken")?;
            return Ok(());
        }
        let server_list = self.resolve_server_list().await?;
        let anon = Connection::anonymous(&server_list)
            .await
            .map_err(|e| anyhow!(e))
            .context("failed to open anonymous Steam connection")?;
        let _: CAuthentication_Token_Revoke_Response = anon
            .service_method(request)
            .await
            .context("failed calling Authentication.RevokeToken")?;
        Ok(())
    }

    pub async fn logout(&mut self) -> Result<()> {
        self.connection = None;
        self.state = LoginState::Connected;
        delete_session().await?;
        Ok(())
    }

    pub async fn get_app_ticket(&self, appid: u32) -> Result<Vec<u8>> {
        let connection = self.require_connection()?;

        let mut request = CMsgClientGetAppOwnershipTicket::new();
        request.set_app_id(appid);

        let response: steam_vent_proto::steammessages_clientserver::CMsgClientGetAppOwnershipTicketResponse =
            connection
                .job(request)
                .await
                .context("failed requesting app ownership ticket")?;

        let ticket = response.ticket().to_vec();
        if ticket.is_empty() {
            bail!("Steam returned an empty app ownership ticket for app {appid}");
        }
        Ok(ticket)
    }

    pub async fn upload_rich_presence(&self, kv: Vec<u8>) -> Result<()> {
        use steam_vent_proto::steammessages_clientserver_2::CMsgClientRichPresenceUpload;
        let connection = self.require_connection()?;
        let mut request = CMsgClientRichPresenceUpload::new();
        request.set_rich_presence_kv(kv);
        connection.send(request).await.map_err(|e| anyhow::anyhow!("failed to upload rich presence: {e}"))
    }

    pub async fn get_user_stats(
        &self,
        appid: u32,
    ) -> Result<steam_vent_proto::steammessages_clientserver_userstats::CMsgClientGetUserStatsResponse> {
        use steam_vent_proto::steammessages_clientserver_userstats::CMsgClientGetUserStats;
        let connection = self.require_connection()?;
        let steam_id = self.steam_id().context("not logged in — stats need an authenticated session")?;
        let mut request = CMsgClientGetUserStats::new();
        request.set_game_id(u64::from(appid));
        request.set_steam_id_for_user(steam_id);
        connection.job(request).await.context("ClientGetUserStats failed")
    }

    pub async fn get_user_stats_of_on(
        connection: &Connection,
        appid: u32,
        steam_id: u64,
    ) -> Result<steam_vent_proto::steammessages_clientserver_userstats::CMsgClientGetUserStatsResponse> {
        use steam_vent_proto::steammessages_clientserver_userstats::CMsgClientGetUserStats;
        let mut request = CMsgClientGetUserStats::new();
        request.set_game_id(u64::from(appid));
        request.set_steam_id_for_user(steam_id);
        connection.job(request).await.context("ClientGetUserStats failed")
    }

    pub async fn store_user_stats_of_on(
        connection: &Connection,
        appid: u32,
        settor: u64,
        settee: u64,
        crc: u32,
        stats: &[(u32, u32)],
    ) -> Result<steam_vent_proto::steammessages_clientserver_userstats::CMsgClientStoreUserStatsResponse> {
        use steam_vent_proto::steammessages_clientserver_userstats::{CMsgClientStoreUserStats2, cmsg_client_store_user_stats2::Stats};
        let mut request = CMsgClientStoreUserStats2::new();
        request.set_game_id(u64::from(appid));
        request.set_settor_steam_id(settor);
        request.set_settee_steam_id(settee);
        request.set_crc_stats(crc);
        request.set_explicit_reset(false);
        for (id, value) in stats {
            let mut s = Stats::new();
            s.set_stat_id(*id);
            s.set_stat_value(*value);
            request.stats.push(s);
        }
        connection.job(request).await.context("ClientStoreUserStats2 failed")
    }

    pub fn user_connection(&self) -> Result<&Connection> {
        self.require_connection()
    }

    pub async fn store_user_stats(
        &self,
        appid: u32,
        crc: u32,
        stats: &[(u32, u32)],
    ) -> Result<steam_vent_proto::steammessages_clientserver_userstats::CMsgClientStoreUserStatsResponse> {
        use steam_vent_proto::steammessages_clientserver_userstats::{CMsgClientStoreUserStats2, cmsg_client_store_user_stats2::Stats};
        let connection = self.require_connection()?;
        let steam_id = self.steam_id().context("not logged in — stats need an authenticated session")?;
        let mut request = CMsgClientStoreUserStats2::new();
        request.set_game_id(u64::from(appid));
        request.set_settor_steam_id(steam_id);
        request.set_settee_steam_id(steam_id);
        request.set_crc_stats(crc);
        request.set_explicit_reset(false);
        for (id, value) in stats {
            let mut s = Stats::new();
            s.set_stat_id(*id);
            s.set_stat_value(*value);
            request.stats.push(s);
        }
        connection.job(request).await.context("ClientStoreUserStats2 failed")
    }

    pub async fn send_auth_list(
        &self,
        list: steam_vent_proto::steammessages_clientserver::CMsgClientAuthList,
    ) -> Result<Vec<u32>> {
        use steam_vent_proto::steammessages_clientserver::CMsgClientAuthListAck;
        let connection = self.require_connection()?;
        let mut acks = connection.on::<CMsgClientAuthListAck>();
        connection
            .send(list)
            .await
            .map_err(|e| anyhow!("failed to send the auth ticket list: {e}"))?;
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = acks.next().await {
                if let Ok(ack) = msg {
                    return Some(ack.ticket_crc.clone());
                }
            }
            None
        })
        .await;
        Ok(got.ok().flatten().unwrap_or_default())
    }

    pub async fn relay_call(&self, req: &crate::bridge::relay::CallRequest, appid: u32) -> Result<Option<(i32, i32, Vec<u8>)>> {
        let connection = self.require_connection()?;
        Self::relay_call_on(connection, req, appid).await
    }

    pub async fn relay_call_on(connection: &Connection, req: &crate::bridge::relay::CallRequest, appid: u32) -> Result<Option<(i32, i32, Vec<u8>)>> {
        let routing = req.routing.then_some(appid);
        if !req.expect_reply {
            connection
                .relay_send(req.emsg, req.protobuf, req.job_name.clone(), req.body.clone(), routing)
                .await
                .map_err(|e| anyhow!("relay send failed: {e}"))?;
            return Ok(None);
        }
        let raw = connection
            .relay_job(req.emsg, req.protobuf, req.job_name.clone(), req.body.clone(), Duration::from_millis(u64::from(req.wait_ms.max(1))), routing)
            .await
            .map_err(|e| anyhow!("relay job failed: {e}"))?;
        Ok(Some((raw.kind.0, raw.header.result.unwrap_or(1), raw.data.to_vec())))
    }

    pub fn relay_subscribe(&self, emsg: i32, queue: Arc<crate::bridge::relay::PushQueue>) -> Result<()> {
        let connection = self.require_connection()?;
        Self::relay_subscribe_on(connection, emsg, queue);
        Ok(())
    }

    pub fn relay_subscribe_on(connection: &Connection, emsg: i32, queue: Arc<crate::bridge::relay::PushQueue>) {
        let mut rx = connection.relay_subscribe(emsg);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(raw) => queue.push(emsg, raw.data.to_vec()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    pub fn relay_notifications(&self, job: &'static str) -> Result<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>> {
        let connection = self.require_connection()?;
        let mut rx = connection.relay_subscribe(146);
        let (tx, out) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(raw) => {
                        if raw.header.target_job_name.as_ref().is_some_and(|name| &**name == job) && tx.send(raw.data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
        Ok(out)
    }

    pub async fn gameserver_connect(&self, app_id: u32, token: Option<&str>) -> Result<Connection> {
        let list = steam_vent::ServerList::discover().await.map_err(|e| anyhow!("could not discover Steam servers: {e}"))?;
        let mut last = None;
        for n in 0..4 {
            let attempt = match token {
                Some(t) if !t.is_empty() => Connection::gameserver_with_token(&list, app_id, t).await,
                _ => Connection::anonymous_gameserver(&list, app_id).await,
            };
            match attempt {
                Ok(conn) => return Ok(conn),
                Err(e) => {
                    tracing::debug!("game server logon attempt {n} failed: {e}");
                    last = Some(e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        Err(anyhow!("game server logon failed: {}", last.map(|e| e.to_string()).unwrap_or_default()))
    }

    pub async fn clear_playing(&self) -> Result<()> {
        PLAYING.lock().expect("playing set lock poisoned").clear();
        self.send_playing(&[]).await
    }

    pub async fn clear_playing_app(&self, appid: u32) -> Result<()> {
        let remaining = {
            let mut set = PLAYING.lock().expect("playing set lock poisoned");
            *set = playing_after_clear(&set, appid);
            declared_games(&set)
        };
        self.send_playing(&remaining).await
    }

    pub async fn redeclare_playing(&self) {
        let current = declared_games(&PLAYING.lock().expect("playing set lock poisoned"));
        if !current.is_empty() {
            if let Err(e) = self.send_playing(&current).await {
                tracing::warn!("could not declare the running games again after the reconnect: {e:#}");
            }
        }
    }

    pub fn playing_apps() -> Vec<u32> {
        declared_games(&PLAYING.lock().expect("playing set lock poisoned"))
    }

    async fn send_playing(&self, apps: &[u32]) -> Result<()> {
        use steam_vent_proto::steammessages_clientserver::{CMsgClientGamesPlayed, cmsg_client_games_played::GamePlayed};
        let connection = self.require_connection()?;
        let mut played = CMsgClientGamesPlayed::new();
        for &app in apps {
            let mut game = GamePlayed::new();
            game.set_game_id(u64::from(app));
            played.games_played.push(game);
        }
        connection.send(played).await.map_err(|e| anyhow!("failed to update the in-game state: {e}"))
    }

    pub fn collect_game_connect_tokens(
        &self,
        pool: Arc<std::sync::Mutex<crate::bridge::ticket::TokenPool>>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        use steam_vent_proto::steammessages_clientserver::CMsgClientGameConnectTokens;
        let connection = self.require_connection()?;
        let mut tokens = connection.on::<CMsgClientGameConnectTokens>();
        Ok(tokio::spawn(async move {
            while let Some(msg) = tokens.next().await {
                if let Ok(msg) = msg {
                    pool.lock().expect("token pool lock poisoned").add(msg.tokens);
                }
            }
        }))
    }

    pub async fn declare_playing(&self, appid: u32) -> Result<()> {
        let (without, all) = {
            let mut set = PLAYING.lock().expect("playing set lock poisoned");
            let without: Vec<u32> = declared_games(&set.iter().copied().filter(|&a| a != appid).collect::<Vec<_>>());
            *set = playing_after_declare(&set, appid);
            (without, declared_games(&set))
        };
        self.send_playing(&without).await.map_err(|e| anyhow!("failed to reset the in-game state: {e}"))?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.send_playing(&all).await.map_err(|e| anyhow!("failed to declare app {appid} as running: {e}"))
    }

    pub async fn request_encrypted_app_ticket(&self, appid: u32) -> Result<EncryptedTicketOutcome> {
        let connection = self.require_connection()?;

        let mut request = CMsgClientRequestEncryptedAppTicket::new();
        request.set_app_id(appid);

        let response: steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse =
            connection
                .job(request)
                .await
                .context("failed requesting encrypted app ticket")?;
        extract_encrypted_ticket(&response, appid)
    }

    pub async fn get_account_data(&self) -> AccountData {
        let Some(connection) = self.connection.as_ref() else {
            return AccountData::default();
        };

        let mut data = AccountData {
            steam_id: u64::from(connection.steam_id()),
            country: connection.ip_country_code().to_string(),
            ..Default::default()
        };

        if let Ok(session) = load_session().await {
            if let Some(name) = session.account_name {
                data.account_name = name;
            }
        }

        if data.account_name.is_empty() {
            data.account_name = "Steam User".to_string();
        }

        data.email = "Hidden".to_string();
        data.email_validated = true;

        data
    }

    pub async fn own_persona_name(&self) -> Option<String> {
        let connection = self.connection.as_ref()?;
        let own_id = u64::from(connection.steam_id());
        let mut persona_stream = connection.on::<CMsgClientPersonaState>();

        if let Err(e) = self.announce_configured_presence().await {
            tracing::debug!("own_persona_name: announce failed: {e}");
        }
        if let Err(e) = self.request_friend_data(&[own_id]).await {
            tracing::debug!("own_persona_name: request failed: {e}");
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
        let mut seen = 0usize;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                tracing::debug!("own_persona_name: timed out after {seen} persona message(s)");
                return None;
            }
            match tokio::time::timeout(remaining, persona_stream.next()).await {
                Ok(Some(Ok(state))) => {
                    seen += 1;
                    for fr in &state.friends {
                        if fr.friendid() == own_id && !fr.player_name().is_empty() {
                            return Some(fr.player_name().to_string());
                        }
                    }
                }
                Ok(Some(Err(_))) => continue,
                _ => {
                    tracing::debug!("own_persona_name: stream ended after {seen} message(s)");
                    return None;
                }
            }
        }
    }

    pub fn pending_confirmations(&self) -> &[ConfirmationPrompt] {
        &self.pending_confirmations
    }

    pub fn is_closed_connection_text(message: &str) -> bool {
        let msg = message.to_ascii_lowercase();
        msg.contains("closed connection")
            || msg.contains("connection closed")
            || msg.contains("connection reset")
            || msg.contains("broken pipe")
    }

    pub async fn connect(&mut self) -> Result<()> {
        tracing::debug!("Connecting to Steam: resolving CM server list ...");
        match self.resolve_server_list().await {
            Ok(server_list) => {
                self.active_cm = Some(server_list.pick());
                self.connected_at = Some(Instant::now());
                self.state = LoginState::Connected;
                Ok(())
            }
            Err(err) => {
                if self.try_enter_offline_mode().await? {
                    tracing::warn!("Steam unavailable; entering offline mode");
                    return Ok(());
                }
                Err(err)
            }
        }
    }

    pub(crate) async fn resolve_server_list(&mut self) -> Result<ServerList> {
        if let Some(existing) = &self.server_list {
            return Ok(existing.clone());
        }

        tracing::debug!("Discovering Steam CM servers ...");
        match ServerList::discover().await {
            Ok(list) => {
                tracing::debug!("Discovered Steam CM server list");
                self.server_list = Some(list.clone());
                Ok(list)
            }
            Err(_) => {
                tracing::debug!("CM discovery failed; falling back to bootstrap endpoints");
                let tcp_servers = get_cm_endpoints().await;
                if tcp_servers.is_empty() {
                    bail!(
                        "failed to discover Steam CM servers and no fallback endpoints were available"
                    )
                }

                let ws_servers = tcp_servers
                    .iter()
                    .map(|entry| format!("{}:{}", entry.ip(), entry.port()))
                    .collect();

                let list = ServerList::new(tcp_servers, ws_servers)
                    .context("failed constructing fallback server list")?;
                self.server_list = Some(list.clone());
                Ok(list)
            }
        }
    }

    pub(crate) async fn try_enter_offline_mode(&mut self) -> Result<bool> {
        let cache_path = library_cache_path()?;
        if cache_path.exists() {
            self.state = LoginState::Offline;
            self.connection = None;
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn open_extra_session(&mut self, logon_id: u32) -> Result<Connection> {
        let persisted = load_session().await?;
        let account_name = persisted.account_name.clone().filter(|v| !v.trim().is_empty()).ok_or_else(|| anyhow!("no persisted account_name found"))?;
        let refresh_token = persisted.refresh_token.clone().filter(|v| !v.expose().trim().is_empty()).ok_or_else(|| anyhow!("no persisted refresh_token found"))?;
        let server_list = self.resolve_server_list().await?;
        super::access_with_retry_as(&server_list, &account_name, refresh_token.expose(), logon_id).await
    }

    pub fn active_cm(&self) -> Option<SocketAddr> {
        self.active_cm
    }

    pub async fn restore_session(&mut self) -> Result<SessionState> {
        let persisted = load_session().await?;
        let account_name = persisted
            .account_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("no persisted account_name found"))?;
        let refresh_token = persisted
            .refresh_token
            .clone()
            .filter(|value| !value.expose().trim().is_empty())
            .ok_or_else(|| anyhow!("no persisted refresh_token found"))?;

        self.connect().await?;
        if self.is_offline() {
            return Err(offline_error("offline mode: using cached library"));
        }
        self.state = LoginState::AwaitingAccessTokenLogon;

        let server_list = self.resolve_server_list().await?;
        let connection =
            access_with_retry(&server_list, &account_name, refresh_token.expose()).await?;

        self.connection = Some(connection);
        let mut session = self
            .session_from_connection(account_name)
            .context("refresh token login succeeded but no token was available for persistence")?;
        session.persona_name = persisted.persona_name.clone();
        session.client_instance_id = persisted.client_instance_id;
        session.web_token = persisted.web_token.clone();
        if session.refresh_token != persisted.refresh_token
            || session.account_name != persisted.account_name
            || session.steam_id != persisted.steam_id
        {
            save_session(&session).await?;
        }
        self.state = LoginState::Complete;
        self.pending_confirmations.clear();
        Ok(session)
    }

    pub async fn login(
        &mut self,
        account_name: String,
        password: crate::core::secret::Secret,
        guard_code: Option<String>,
        interactive_pin: Option<CodeReader>,
    ) -> Result<SessionState> {
        self.connect().await?;
        if self.is_offline() {
            return Err(offline_error("offline mode: using cached library"));
        }

        self.state = LoginState::AwaitingCredentialSession;
        let server_list = self.resolve_server_list().await?;

        self.state = LoginState::AwaitingGuardConfirmation;
        self.state = LoginState::AwaitingPollResult;
        self.state = LoginState::AwaitingAccessTokenLogon;

        async fn login_with<H: AuthConfirmationHandler + Send>(
            server_list: &ServerList,
            account_name: &str,
            password: &str,
            handler: H,
        ) -> std::result::Result<Connection, ConnectionError> {
            Connection::login_with_logon_id(
                server_list,
                account_name,
                password,
                FileGuardDataStore::user_cache(),
                handler,
                &ClientInfo::default(),
                super::logon_id(),
            )
            .await
        }

        let login_result = if let Some(code) = guard_code.filter(|v| !v.trim().is_empty()) {
            let (mut writer, reader) = duplex(64);
            writer
                .write_all(format!("{}\n", code.trim()).as_bytes())
                .await
                .context("failed to prepare guard code input")?;
            drop(writer);

            let handler = (
                UserProvidedAuthConfirmationHandler::new(reader, sink()),
                DeviceConfirmationHandler,
            );

            login_with(&server_list, &account_name, password.expose(), handler).await
        } else if let Some(read_code) = interactive_pin {
            tracing::info!("Login method awaited: Steam Guard code — enter it when prompted below");
            let (writer, reader) = duplex(256);
            let handler = (
                UserProvidedAuthConfirmationHandler::new(reader, tokio::io::stderr()),
                DeviceConfirmationHandler,
            );
            let login = login_with(&server_list, &account_name, password.expose(), handler);
            feed_code_while(login, read_code, writer).await
        } else {
            login_with(
                &server_list,
                &account_name,
                password.expose(),
                DeviceConfirmationHandler,
            )
            .await
        };

        let connection = match login_result {
            Ok(connection) => connection,
            Err(ConnectionError::UnsupportedConfirmationAction(methods)) => {
                self.pending_confirmations = methods.iter().map(map_confirmation).collect();
                bail!("Steam Guard confirmation required")
            }
            Err(other) => return Err(anyhow!(other)).context("steam-vent login flow failed"),
        };

        self.connection = Some(connection);
        let session = self
            .session_from_connection(account_name)
            .context("login succeeded but no token was available for persistence")?;
        save_session(&session).await?;
        self.state = LoginState::Complete;
        self.pending_confirmations.clear();
        Ok(session)
    }

    pub async fn login_qr<F>(&mut self, timeout: Duration, mut on_event: F) -> Result<SessionState>
    where
        F: FnMut(QrEvent),
    {
        use steam_vent_proto::steammessages_auth_steamclient::{
            CAuthentication_BeginAuthSessionViaQR_Request,
            CAuthentication_BeginAuthSessionViaQR_Response,
            CAuthentication_PollAuthSessionStatus_Request,
            CAuthentication_PollAuthSessionStatus_Response, EAuthTokenPlatformType,
        };

        self.connect().await?;
        if self.is_offline() {
            return Err(offline_error("offline mode: cannot start QR login"));
        }
        let server_list = self.resolve_server_list().await?;

        let anon = Connection::anonymous(&server_list)
            .await
            .map_err(|e| anyhow!(e))
            .context("failed to open anonymous Steam connection for QR login")?;

        let deadline = Instant::now() + timeout;
        let timed_out = || {
            anyhow!(TypedError::new(
                ErrorKind::Timeout,
                format!(
                    "QR login timed out after {}s without approval",
                    timeout.as_secs()
                ),
            ))
        };
        let mut last_emitted_url = String::new();
        let mut scanned = false;
        'session: loop {
            if Instant::now() >= deadline {
                return Err(timed_out());
            }

            let mut begin = CAuthentication_BeginAuthSessionViaQR_Request::new();
            begin.set_device_friendly_name("Aurelia CLI".to_string());
            begin.set_platform_type(EAuthTokenPlatformType::k_EAuthTokenPlatformType_SteamClient);
            begin.set_website_id("Client".to_string());

            let begin_resp: CAuthentication_BeginAuthSessionViaQR_Response = anon
                .service_method(begin)
                .await
                .map_err(|e| anyhow!(e))
                .context("Authentication.BeginAuthSessionViaQR failed")?;

            let client_id = begin_resp.client_id();
            let request_id = begin_resp.request_id().to_vec();
            let poll_interval = Duration::from_secs_f32(begin_resp.interval().max(2.0));
            let mut challenge_url = begin_resp.challenge_url().to_string();

            if challenge_url != last_emitted_url {
                last_emitted_url = challenge_url.clone();
                on_event(QrEvent::Challenge(&challenge_url));
            }
            tracing::info!("Login method awaited: QR code — scan it with the Steam Mobile app");

            loop {
                tokio::time::sleep(
                    poll_interval.min(deadline.saturating_duration_since(Instant::now())),
                )
                .await;
                if Instant::now() >= deadline {
                    return Err(timed_out());
                }

                let mut poll = CAuthentication_PollAuthSessionStatus_Request::new();
                poll.set_client_id(client_id);
                poll.set_request_id(request_id.clone());

                let resp: CAuthentication_PollAuthSessionStatus_Response =
                    match tokio::time::timeout(CM_CONNECT_TIMEOUT, anon.service_method(poll)).await
                    {
                        Ok(Ok(resp)) => resp,
                        Ok(Err(steam_vent::NetworkError::ApiError(result))) => {
                            match qr_poll_step(result, scanned) {
                                Some(QrPollStep::Regenerate) => {
                                    tracing::info!(
                                        ?result,
                                        "QR auth session ended unscanned; regenerating code"
                                    );
                                    continue 'session;
                                }
                                Some(QrPollStep::Fail(e)) => return Err(anyhow!(e)),
                                None => {
                                    return Err(anyhow!(steam_vent::NetworkError::ApiError(
                                        result
                                    )))
                                    .context("Authentication.PollAuthSessionStatus failed");
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            return Err(anyhow!(e))
                                .context("Authentication.PollAuthSessionStatus failed");
                        }
                        Err(_) => {
                            tracing::warn!("QR status poll timed out; retrying ...");
                            continue;
                        }
                    };

                if resp.had_remote_interaction() && !scanned {
                    scanned = true;
                    tracing::info!("QR code scanned; awaiting approval in the Steam Mobile app");
                    on_event(QrEvent::Scanned);
                }

                if resp.has_new_challenge_url() && resp.new_challenge_url() != challenge_url {
                    challenge_url = resp.new_challenge_url().to_string();
                    last_emitted_url = challenge_url.clone();
                    on_event(QrEvent::Challenge(&challenge_url));
                }

                let refresh_token = resp.refresh_token();
                if !refresh_token.is_empty() {
                    let account_name = resp.account_name().to_string();
                    tracing::info!(account = %account_name, "QR login approved");

                    let connection =
                        access_with_retry(&server_list, &account_name, refresh_token).await?;
                    let steam_id = Some(u64::from(connection.steam_id()));
                    self.connection = Some(connection);

                    let session = SessionState {
                        account_name: Some(account_name),
                        persona_name: None,
                        steam_id,
                        refresh_token: Some(refresh_token.into()),
                        client_instance_id: None,
                        web_token: None,
                    };
                    save_session(&session).await?;
                    self.state = LoginState::Complete;
                    self.pending_confirmations.clear();
                    return Ok(session);
                }
            }
        }
    }

    pub(crate) fn session_from_connection(&self, account_name: String) -> Option<SessionState> {
        let connection = self.connection.as_ref()?;
        let steam_id = u64::from(connection.steam_id());
        Some(SessionState {
            account_name: Some(account_name),
            persona_name: None,
            steam_id: Some(steam_id),
            refresh_token: Some(connection.refresh_token().token().into()),
            client_instance_id: None,
            web_token: None,
        })
    }
}

fn offline_error(message: &str) -> anyhow::Error {
    anyhow!(TypedError::new(ErrorKind::Offline, message))
}

#[derive(Debug)]
pub(crate) enum QrPollStep {
    Regenerate,
    Fail(TypedError),
}

pub(crate) fn qr_poll_step(result: steam_vent::EResult, scanned: bool) -> Option<QrPollStep> {
    use steam_vent::EResult;
    Some(match result {
        EResult::Expired if scanned => QrPollStep::Fail(TypedError::new(
            ErrorKind::LoginExpired,
            "QR code was scanned but not approved before it expired",
        )),
        EResult::FileNotFound | EResult::Cancelled if scanned => QrPollStep::Fail(TypedError::new(
            ErrorKind::LoginDenied,
            "QR sign-in was declined or cancelled by Steam",
        )),
        EResult::Expired | EResult::FileNotFound | EResult::Cancelled => QrPollStep::Regenerate,
        _ => return None,
    })
}

pub(crate) fn extract_encrypted_ticket(
    response: &steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse,
    appid: u32,
) -> Result<EncryptedTicketOutcome> {
    let eresult = response.eresult();
    if eresult != 1 {
        return Ok(EncryptedTicketOutcome::Refused(eresult));
    }
    let envelope = response
        .encrypted_app_ticket
        .as_ref()
        .context("encrypted ticket response carried no ticket")?;
    if envelope.encrypted_ticket().is_empty() {
        bail!("Steam returned an empty encrypted app ticket for app {appid}");
    }
    let bytes = envelope
        .write_to_bytes()
        .context("failed serializing encrypted app ticket")?;
    Ok(EncryptedTicketOutcome::Issued(bytes))
}

#[cfg(test)]
mod playing_tests {
    use super::{declared_games, playing_after_clear, playing_after_declare};

    #[test]
    fn a_second_game_joins_the_declared_set_without_dropping_the_first() {
        let one = playing_after_declare(&[], 550);
        let two = playing_after_declare(&one, 2357570);
        assert_eq!(declared_games(&two), vec![550, 2357570]);
    }

    #[test]
    fn closing_one_game_keeps_the_other_declared() {
        let two = vec![550, 2357570];
        assert_eq!(declared_games(&playing_after_clear(&two, 550)), vec![2357570]);
        assert!(declared_games(&playing_after_clear(&[2357570], 2357570)).is_empty());
        assert_eq!(playing_after_clear(&two, 999), two, "an app that was not declared changes nothing");
    }

    #[test]
    fn two_bridges_of_the_same_app_hold_it_until_both_end() {
        let both = playing_after_declare(&playing_after_declare(&[], 550), 550);
        assert_eq!(declared_games(&both), vec![550]);
        let after_first = playing_after_clear(&both, 550);
        assert_eq!(declared_games(&after_first), vec![550], "the game remains declared for the second");
        assert!(declared_games(&playing_after_clear(&after_first, 550)).is_empty());
    }
}
