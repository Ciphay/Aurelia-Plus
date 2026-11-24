mod filter;
pub(crate) mod raw;
pub(crate) mod unauthenticated;

use crate::GameCoordinator;
use crate::auth::{AuthConfirmationHandler, ClientInfo, GuardDataStore, RefreshToken};
use crate::message::{ServiceMethodMessage, ServiceMethodResponseMessage};
use crate::net::{NetworkError, RawNetMessage};
use crate::serverlist::ServerList;
use crate::session::{ConnectionError, RawSession, SessionAuthenticationDetails};
use async_stream::try_stream;
pub(crate) use filter::MessageFilter;
use futures_util::{FutureExt, Sink, SinkExt};
use raw::RawConnection;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
pub use steam_vent_core::{ConnectionTrait, ReadonlyConnection};
use steam_vent_core::{
    EncodableMessage, JobId, NetMessageHeader, RawSteamId, ReceivableMessage, SendableMessage,
    ServiceMethodRequest, ServiceNotification,
};
use steam_vent_proto_common::{GCHandshake, JobMultiple, MsgKindEnum};
use steamid_ng::SteamID;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tracing::instrument;
pub use unauthenticated::UnAuthenticatedConnection;

pub(crate) type Result<T, E = NetworkError> = std::result::Result<T, E>;

type TransportWriter = Arc<Mutex<dyn Sink<RawNetMessage, Error = NetworkError> + Unpin + Send>>;

#[derive(Clone)]
pub(crate) struct MessageSender {
    write: TransportWriter,
}

impl MessageSender {
    pub async fn send_raw(&self, raw_message: RawNetMessage) -> Result<()> {
        self.write.lock().await.send(raw_message).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct Connection {
    raw: RawConnection,
    pub(crate) auth: SessionAuthenticationDetails,
}

impl Debug for Connection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

impl Connection {
    pub(self) fn new(raw: RawConnection, auth: SessionAuthenticationDetails) -> Self {
        Self { raw, auth }
    }

    pub async fn anonymous(server_list: &ServerList) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .anonymous()
            .await
    }

    pub async fn anonymous_server(server_list: &ServerList) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .anonymous_server()
            .await
    }

    pub async fn anonymous_gameserver(server_list: &ServerList, app_id: u32) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .anonymous_gameserver(app_id)
            .await
    }

    pub async fn gameserver_with_token(server_list: &ServerList, app_id: u32, token: &str) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .gameserver_with_token(app_id, token)
            .await
    }

    pub async fn login<H: AuthConfirmationHandler, G: GuardDataStore>(
        server_list: &ServerList,
        account: &str,
        password: &str,
        guard_data_store: G,
        confirmation_handler: H,
        client_info: &ClientInfo,
    ) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .login(
                account,
                password,
                guard_data_store,
                confirmation_handler,
                client_info,
            )
            .await
    }

    pub async fn login_with_refresh_token(
        server_list: &ServerList,
        token: &RefreshToken,
    ) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .login_with_refresh_token(token)
            .await
    }

    pub async fn login_with_logon_id<H: AuthConfirmationHandler, G: GuardDataStore>(
        server_list: &ServerList,
        account: &str,
        password: &str,
        guard_data_store: G,
        confirmation_handler: H,
        client_info: &ClientInfo,
        logon_id: u32,
    ) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .login_with_logon_id(
                account,
                password,
                guard_data_store,
                confirmation_handler,
                client_info,
                logon_id,
            )
            .await
    }

    pub async fn login_with_refresh_token_and_logon_id(
        server_list: &ServerList,
        token: &RefreshToken,
        logon_id: u32,
    ) -> Result<Self, ConnectionError> {
        UnAuthenticatedConnection::connect(server_list)
            .await?
            .login_with_refresh_token_and_logon_id(token, logon_id)
            .await
    }

    pub fn refresh_token(&self) -> &RefreshToken {
        &self.auth.refresh_token
    }

    pub fn steam_id(&self) -> SteamID {
        self.auth.steam_id
    }

    pub fn session_id(&self) -> i32 {
        self.raw.session.session_id
    }

    pub fn cell_id(&self) -> u32 {
        self.raw.session.cell_id
    }

    pub fn public_ip(&self) -> IpAddr {
        self.auth.public_ip
    }

    pub fn ip_country_code(&self) -> &str {
        &self.auth.ip_country_code
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.raw.timeout = timeout;
    }

    pub(crate) fn sender(&self) -> &MessageSender {
        &self.raw.sender
    }

    pub fn take_unprocessed(&self) -> Vec<RawNetMessage> {
        self.raw.filter.unprocessed()
    }
}

impl Connection {
    pub async fn game_coordinator<Handshake: GCHandshake>(
        &self,
        handshake: &Handshake,
    ) -> Result<(GameCoordinator, Handshake::Welcome), NetworkError> {
        GameCoordinator::with_handshake(self, handshake).await
    }
}

#[derive(Debug)]
struct RelayBody {
    data: Vec<u8>,
    job_name: Option<String>,
}

impl EncodableMessage for RelayBody {
    fn write_body<W: std::io::Write>(&self, mut writer: W) -> std::result::Result<(), std::io::Error> {
        writer.write_all(&self.data)
    }

    fn encode_size(&self) -> usize {
        self.data.len()
    }

    fn process_header(&self, header: &mut NetMessageHeader) {
        if let Some(name) = &self.job_name {
            header.target_job_name = Some(name.clone().into());
        }
    }
}

fn relay_kind(kind: i32) -> Result<steam_vent_proto_steam::enums_clientserver::EMsg> {
    use steam_vent_proto_common::protobuf::Enum;
    steam_vent_proto_steam::enums_clientserver::EMsg::from_i32(kind).ok_or(NetworkError::InvalidHeader)
}

impl Connection {
    pub async fn relay_job(
        &self,
        kind: i32,
        is_protobuf: bool,
        job_name: Option<String>,
        body: Vec<u8>,
        wait: Duration,
        routing_appid: Option<u32>,
    ) -> Result<RawNetMessage> {
        let mut header = self.generate_header(true);
        header.source_app_id = routing_appid.or(header.source_app_id);
        let recv = self.filter().on_job_id(header.source_job_id);
        self.raw_send_with_kind(header, RelayBody { data: body, job_name }, relay_kind(kind)?, is_protobuf)
            .await?;
        timeout(wait, recv)
            .await
            .map_err(|_| NetworkError::Timeout)?
            .map_err(|_| NetworkError::EOF)
    }

    pub async fn relay_send(&self, kind: i32, is_protobuf: bool, job_name: Option<String>, body: Vec<u8>, routing_appid: Option<u32>) -> Result<()> {
        let mut header = self.generate_header(false);
        header.source_app_id = routing_appid.or(header.source_app_id);
        self.raw_send_with_kind(header, RelayBody { data: body, job_name }, relay_kind(kind)?, is_protobuf)
            .await
    }

    pub fn relay_subscribe(&self, kind: i32) -> tokio::sync::broadcast::Receiver<RawNetMessage> {
        self.filter().on_kind(MsgKindNum(kind))
    }
}

struct MsgKindNum(i32);

impl From<MsgKindNum> for steam_vent_proto_common::MsgKind {
    fn from(v: MsgKindNum) -> Self {
        steam_vent_proto_common::MsgKind(v.0)
    }
}

pub(crate) trait ConnectionImpl: Sync + Debug {
    fn timeout(&self) -> Duration;
    fn filter(&self) -> &MessageFilter;
    fn raw_session(&self) -> &RawSession;
    fn auth_details(&self) -> Option<&SessionAuthenticationDetails>;

    fn generate_header(&self, job: bool) -> NetMessageHeader {
        NetMessageHeader {
            session_id: self.raw_session().session_id,
            source_job_id: if job {
                self.raw_session().job_id.next()
            } else {
                JobId::NONE
            },
            target_job_id: JobId::NONE,
            steam_id: RawSteamId::new(
                self.auth_details()
                    .map(|auth| auth.steam_id.steam64())
                    .unwrap_or_default(),
            ),
            source_app_id: self.auth_details().and_then(|auth| auth.app_id),
            ..NetMessageHeader::default()
        }
    }

    fn one_with_header<T: ReceivableMessage + 'static>(
        &self,
    ) -> impl Future<Output = Result<(NetMessageHeader, T)>> + 'static {
        let fut = self.filter().one_kind(T::KIND);
        async move {
            let raw = fut.await.map_err(|_| NetworkError::EOF)?;
            raw.into_header_and_message()
        }
    }

    fn on_with_header<T: ReceivableMessage + 'static>(
        &self,
    ) -> impl Stream<Item = Result<(NetMessageHeader, T)>> + 'static {
        BroadcastStream::new(self.filter().on_kind(T::KIND)).map(|raw| {
            let raw = raw.map_err(|_| NetworkError::EOF)?;
            raw.into_header_and_message()
        })
    }

    fn raw_send<Msg: SendableMessage>(
        &self,
        header: NetMessageHeader,
        msg: Msg,
    ) -> impl Future<Output = Result<()>> + Send {
        let kind = msg.kind();
        let is_protobuf = msg.is_protobuf();
        self.raw_send_with_kind(header, msg, kind, is_protobuf)
    }

    fn raw_send_with_kind<Msg: EncodableMessage, K: MsgKindEnum>(
        &self,
        header: NetMessageHeader,
        msg: Msg,
        kind: K,
        is_protobuf: bool,
    ) -> impl Future<Output = Result<()>> + Send;
}

impl ConnectionImpl for Connection {
    fn raw_session(&self) -> &RawSession {
        &self.raw.session
    }

    fn auth_details(&self) -> Option<&SessionAuthenticationDetails> {
        Some(&self.auth)
    }

    fn timeout(&self) -> Duration {
        self.raw.timeout()
    }

    fn filter(&self) -> &MessageFilter {
        self.raw.filter()
    }

    async fn raw_send_with_kind<Msg: EncodableMessage, K: MsgKindEnum>(
        &self,
        header: NetMessageHeader,
        msg: Msg,
        kind: K,
        is_protobuf: bool,
    ) -> Result<()> {
        <RawConnection as ConnectionImpl>::raw_send_with_kind(
            &self.raw,
            header,
            msg,
            kind,
            is_protobuf,
        )
        .await
    }
}

macro_rules! impl_connection {
    ($con:path) => {
        impl ConnectionTrait for $con {
            type Error = NetworkError;

            fn on_notification<T: ServiceNotification>(
                &self,
            ) -> impl Stream<Item = Result<T>> + 'static {
                BroadcastStream::new(self.filter().on_notification(T::NOTIFICATION_NAME))
                    .filter_map(|res| res.ok())
                    .map(|raw| raw.into_notification())
            }

            fn one<T: ReceivableMessage + 'static>(
                &self,
            ) -> impl Future<Output = Result<T>> + 'static {
                self.one_with_header::<T>()
                    .map(|res| res.map(|(_, msg)| msg))
            }

            fn on<T: ReceivableMessage + 'static>(
                &self,
            ) -> impl Stream<Item = Result<T>> + 'static {
                self.on_with_header::<T>()
                    .map(|res| res.map(|(_, msg)| msg))
            }

            async fn service_method<Msg: ServiceMethodRequest>(
                &self,
                msg: Msg,
            ) -> Result<Msg::Response> {
                let header = self.generate_header(true);
                let recv = self.filter().on_job_id(header.source_job_id);
                self.raw_send(header, ServiceMethodMessage(msg)).await?;
                let message = timeout(self.timeout(), recv)
                    .await
                    .map_err(|_| NetworkError::Timeout)?
                    .map_err(|_| NetworkError::EOF)?
                    .into_message::<ServiceMethodResponseMessage>()?;
                message.into_response::<Msg>()
            }

            async fn job<Req: SendableMessage, Rsp: ReceivableMessage>(
                &self,
                msg: Req,
            ) -> Result<Rsp> {
                let header = self.generate_header(true);
                let recv = self.filter().on_job_id(header.source_job_id);
                self.raw_send(header, msg).await?;
                timeout(self.timeout(), recv)
                    .await
                    .map_err(|_| NetworkError::Timeout)?
                    .map_err(|_| NetworkError::EOF)?
                    .into_message()
            }

            fn job_multi<Req: SendableMessage, Rsp: ReceivableMessage + JobMultiple>(
                &self,
                msg: Req,
            ) -> impl Stream<Item = Result<Rsp>> + Send {
                try_stream! {
                    let header = self.generate_header(true);
                    let source_job_id = header.source_job_id;
                    let mut recv = self.filter().on_job_id_multi(source_job_id);
                    self.raw_send(header, msg).await?;
                    loop {
                        let msg: Rsp = timeout(self.timeout(), recv.recv())
                            .await
                            .map_err(|_| NetworkError::Timeout)?
                            .ok_or(NetworkError::EOF)?
                            .into_message()?;
                        let completed = msg.completed();
                        yield msg;
                        if completed {
                            break;
                        }
                    }
                    self.filter().complete_job_id_multi(source_job_id);
                }
            }

            #[instrument(skip(msg), fields(kind = ?msg.kind()))]
            fn send<Msg: SendableMessage>(
                &self,
                msg: Msg,
            ) -> impl Future<Output = Result<()>> + Send {
                self.raw_send(self.generate_header(false), msg)
            }
        }
    };
}

impl_connection!(RawConnection);
impl_connection!(Connection);
impl_connection!(GameCoordinator);

#[cfg(test)]
mod send_tests {
    use super::*;
    use crate::auth::{
        AuthConfirmationHandler, ClientInfo, ConsoleAuthConfirmationHandler,
        DeviceConfirmationHandler, FileGuardDataStore,
    };

    fn assert_send<T: Send>(_: &T) {}

    #[allow(dead_code)]
    fn login_futures_are_send(list: &ServerList, token: &RefreshToken) {
        let combined = (
            ConsoleAuthConfirmationHandler::default(),
            DeviceConfirmationHandler,
        );
        let info = ClientInfo::default();
        let fut = Connection::login(
            list,
            "a",
            "b",
            FileGuardDataStore::user_cache(),
            combined,
            &info,
        );
        assert_send(&fut);
        let boxed = DeviceConfirmationHandler.boxed();
        let fut = Connection::login(
            list,
            "a",
            "b",
            FileGuardDataStore::user_cache(),
            boxed,
            &info,
        );
        assert_send(&fut);
        let fut = Connection::login_with_refresh_token_and_logon_id(list, token, 7);
        assert_send(&fut);
    }

    #[test]
    fn login_futures_compile_as_send() {}
}
