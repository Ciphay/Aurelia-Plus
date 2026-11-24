use super::Result;
use super::raw::RawConnection;
use crate::auth::{
    AuthConfirmationHandler, ClientInfo, GuardDataStore, RefreshToken, RefreshTokenError,
    begin_password_auth, perform_confirmation,
};
use crate::connection::ConnectionImpl;
use crate::message::{ServiceMethodMessage, ServiceMethodResponseMessage};
use crate::net::RawNetMessage;
use crate::session::{anonymous, login};
use crate::{Connection, ConnectionError, EResult, LoginError, NetworkError, ServerList};
use bytes::BytesMut;
use futures_util::Stream;
use futures_util::{FutureExt, Sink};
use std::future::Future;
use steam_vent_core::{
    NetMessageHeader, ReadonlyConnection, ReceivableMessage, ServiceMethodRequest,
    ServiceNotification,
};
use steam_vent_proto_steam::enums_clientserver::EMsg;
use steamid_ng::{AccountType, SteamID};
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error};

pub struct UnAuthenticatedConnection(RawConnection);

impl UnAuthenticatedConnection {
    pub async fn from_sender_receiver<
        Sender: Sink<BytesMut, Error = NetworkError> + Send + 'static,
        Receiver: Stream<Item = Result<BytesMut>> + Send + 'static,
    >(
        sender: Sender,
        receiver: Receiver,
    ) -> Result<Self, ConnectionError> {
        Ok(UnAuthenticatedConnection(
            RawConnection::from_sender_receiver(sender, receiver).await?,
        ))
    }

    pub async fn connect(server_list: &ServerList) -> Result<Self, ConnectionError> {
        Ok(UnAuthenticatedConnection(
            RawConnection::connect(server_list).await?,
        ))
    }

    pub async fn anonymous(self) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;
        let session = anonymous(&raw, AccountType::AnonUser).await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        let connection = Connection::new(raw, session.auth);

        Ok(connection)
    }

    pub async fn anonymous_server(self) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;
        let session = anonymous(&raw, AccountType::AnonGameServer).await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        let connection = Connection::new(raw, session.auth);

        Ok(connection)
    }

    pub async fn anonymous_gameserver(self, app_id: u32) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;
        let session = crate::session::anonymous_gameserver(&raw, app_id).await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        Ok(Connection::new(raw, session.auth.with_app_id(app_id)))
    }

    pub async fn gameserver_with_token(self, app_id: u32, token: &str) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;
        let session = crate::session::gameserver_with_token(&raw, app_id, token).await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        Ok(Connection::new(raw, session.auth.with_app_id(app_id)))
    }

    pub async fn login<H: AuthConfirmationHandler, G: GuardDataStore>(
        self,
        account: &str,
        password: &str,
        guard_data_store: G,
        confirmation_handler: H,
        client_info: &ClientInfo,
    ) -> Result<Connection, ConnectionError> {
        self.login_with_logon_id(
            account,
            password,
            guard_data_store,
            confirmation_handler,
            client_info,
            0,
        )
        .await
    }

    pub async fn login_with_logon_id<H: AuthConfirmationHandler, G: GuardDataStore>(
        self,
        account: &str,
        password: &str,
        mut guard_data_store: G,
        mut confirmation_handler: H,
        client_info: &ClientInfo,
        logon_id: u32,
    ) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;
        let guard_data = guard_data_store.load(account).await.unwrap_or_else(|e| {
            error!(error = ?e, "failed to retrieve guard data");
            None
        });
        if guard_data.is_some() {
            debug!(account, "found stored guard data");
        }
        let begin = begin_password_auth(
            &mut raw,
            account,
            password,
            guard_data.as_deref(),
            client_info,
        )
        .await?;
        let steam_id = SteamID::from_steam64(begin.steam_id()).map_err(LoginError::from)?;

        let allowed_confirmations = begin.allowed_confirmations();

        let tokens = loop {
            match perform_confirmation(
                &raw,
                &mut confirmation_handler,
                &begin,
                &allowed_confirmations,
            )
            .await
            {
                None
                | Some(Err(ConnectionError::Network(NetworkError::ApiError(
                    EResult::TwoFactorCodeMismatch,
                )))) => continue,
                Some(result) => break result?,
            }
        };

        if let Some(guard_data) = tokens.new_guard_data
            && let Err(e) = guard_data_store.store(account, guard_data).await
        {
            error!(error = ?e, "failed to store guard data");
        } else {
            debug!("no guard data received");
        }

        let session = login(
            &mut raw,
            Some(account),
            steam_id,
            tokens.refresh_token.as_ref(),
            logon_id,
        )
        .await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        let connection = Connection::new(raw, session.auth);

        Ok(connection)
    }

    pub async fn login_with_refresh_token(
        self,
        token: &RefreshToken,
    ) -> Result<Connection, ConnectionError> {
        self.login_with_refresh_token_and_logon_id(token, 0).await
    }

    pub async fn login_with_refresh_token_and_logon_id(
        self,
        token: &RefreshToken,
        logon_id: u32,
    ) -> Result<Connection, ConnectionError> {
        let mut raw = self.0;

        if token.expired() {
            return Err(RefreshTokenError::Expired.into());
        }

        let session = login(&mut raw, None, token.subject, token.token(), logon_id).await?;
        raw.session = session.session;
        raw.setup_heartbeat(session.auth.steam_id);
        Ok(Connection::new(raw, session.auth))
    }
}

impl ReadonlyConnection for UnAuthenticatedConnection {
    type Error = NetworkError;

    fn on_notification<T: ServiceNotification>(&self) -> impl Stream<Item = Result<T>> + 'static {
        BroadcastStream::new(self.0.filter.on_notification(T::NOTIFICATION_NAME))
            .filter_map(|res| res.ok())
            .map(|raw| raw.into_notification())
    }

    fn one_with_header<T: ReceivableMessage + 'static>(
        &self,
    ) -> impl Future<Output = Result<(NetMessageHeader, T)>> + 'static {
        let fut = self.0.filter.one_kind(T::KIND);
        async move {
            let raw = fut.await.map_err(|_| NetworkError::EOF)?;
            raw.into_header_and_message()
        }
    }

    fn one<T: ReceivableMessage + 'static>(&self) -> impl Future<Output = Result<T>> + 'static {
        self.one_with_header::<T>()
            .map(|res| res.map(|(_, msg)| msg))
    }

    fn on_with_header<T: ReceivableMessage + 'static>(
        &self,
    ) -> impl Stream<Item = Result<(NetMessageHeader, T)>> + 'static {
        BroadcastStream::new(self.0.filter.on_kind(T::KIND)).map(|raw| {
            let raw = raw.map_err(|_| NetworkError::EOF)?;
            raw.into_header_and_message()
        })
    }

    fn on<T: ReceivableMessage + 'static>(&self) -> impl Stream<Item = Result<T>> + 'static {
        self.on_with_header::<T>()
            .map(|res| res.map(|(_, msg)| msg))
    }
}

pub(crate) async fn service_method_un_authenticated<Msg: ServiceMethodRequest>(
    connection: &RawConnection,
    msg: Msg,
) -> Result<Msg::Response> {
    let header = connection.generate_header(true);
    let recv = connection.filter.on_job_id(header.source_job_id);
    let msg = RawNetMessage::from_message_with_kind(
        header,
        ServiceMethodMessage(msg),
        EMsg::k_EMsgServiceMethodCallFromClientNonAuthed,
        true,
    )?;
    connection.sender.send_raw(msg).await?;
    let message = timeout(connection.timeout, recv)
        .await
        .map_err(|_| NetworkError::Timeout)?
        .map_err(|_| NetworkError::Timeout)?
        .into_message::<ServiceMethodResponseMessage>()?;
    message.into_response::<Msg>()
}
