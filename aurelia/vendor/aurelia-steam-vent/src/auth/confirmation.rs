use crate::auth::SteamGuardToken;
use another_steam_totp::generate_auth_code;
use futures_util::FutureExt;
use futures_util::future::{Either, select};
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::future::ready;
use steam_vent_proto_steam::steammessages_auth_steamclient::{
    CAuthentication_AllowedConfirmation, EAuthSessionGuardType,
};
use tokio::io::AsyncBufReadExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Stdin, Stdout, stdin, stdout};
use tokio_stream::Stream;

#[derive(Debug, Clone)]
pub struct ConfirmationMethod(CAuthentication_AllowedConfirmation);

impl From<CAuthentication_AllowedConfirmation> for ConfirmationMethod {
    fn from(value: CAuthentication_AllowedConfirmation) -> Self {
        Self(value)
    }
}

impl ConfirmationMethod {
    pub fn confirmation_type(&self) -> &'static str {
        match self.0.confirmation_type() {
            EAuthSessionGuardType::k_EAuthSessionGuardType_Unknown => "unknown",
            EAuthSessionGuardType::k_EAuthSessionGuardType_None => "none",
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailCode => "email",
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceCode => "device code",
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceConfirmation => {
                "device confirmation"
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailConfirmation => {
                "email confirmation"
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_MachineToken => "machine token",
            EAuthSessionGuardType::k_EAuthSessionGuardType_LegacyMachineAuth => "machine auth",
        }
    }

    pub fn confirmation_details(&self) -> &str {
        self.0.associated_message()
    }

    pub fn action_required(&self) -> bool {
        self.0.confirmation_type() != EAuthSessionGuardType::k_EAuthSessionGuardType_None
    }

    pub fn class(&self) -> ConfirmationMethodClass {
        match self.0.confirmation_type() {
            EAuthSessionGuardType::k_EAuthSessionGuardType_Unknown => ConfirmationMethodClass::None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_None => ConfirmationMethodClass::None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailCode => {
                ConfirmationMethodClass::Code
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceCode => {
                ConfirmationMethodClass::Code
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceConfirmation => {
                ConfirmationMethodClass::Confirmation
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailConfirmation => {
                ConfirmationMethodClass::Confirmation
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_MachineToken => {
                ConfirmationMethodClass::Stored
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_LegacyMachineAuth => {
                ConfirmationMethodClass::Stored
            }
        }
    }

    pub fn token_type(&self) -> Option<GuardTokenType> {
        match self.0.confirmation_type() {
            EAuthSessionGuardType::k_EAuthSessionGuardType_Unknown => None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_None => None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailCode => Some(GuardTokenType::Email),
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceCode => {
                Some(GuardTokenType::Device)
            }
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceConfirmation => None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_EmailConfirmation => None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_MachineToken => None,
            EAuthSessionGuardType::k_EAuthSessionGuardType_LegacyMachineAuth => None,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ConfirmationMethodClass {
    Code,
    Confirmation,
    Stored,
    None,
}

#[non_exhaustive]
#[derive(Debug)]
pub enum ConfirmationAction {
    GuardToken(SteamGuardToken, GuardTokenType),
    None,
    Abort,
}

#[derive(Debug)]
pub enum GuardTokenType {
    Email,
    Device,
}

impl From<GuardTokenType> for EAuthSessionGuardType {
    fn from(value: GuardTokenType) -> Self {
        match value {
            GuardTokenType::Device => EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceCode,
            GuardTokenType::Email => EAuthSessionGuardType::k_EAuthSessionGuardType_EmailCode,
        }
    }
}

pub trait AuthConfirmationHandler {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this>;

    fn boxed(self) -> Box<dyn AuthConfirmationHandler + Send>
    where
        Self: Sized + Send + 'static,
    {
        Box::new(self)
    }
}

impl<Handler: AuthConfirmationHandler> AuthConfirmationHandler for &mut Handler {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        (**self).handle_confirmation(allowed_confirmations)
    }
}

pub type ConsoleAuthConfirmationHandler = UserProvidedAuthConfirmationHandler<Stdin, Stdout>;

pub struct UserProvidedAuthConfirmationHandler<Read, Write> {
    input: BufReader<Read>,
    output: Write,
}

impl Default for ConsoleAuthConfirmationHandler {
    fn default() -> Self {
        ConsoleAuthConfirmationHandler {
            input: BufReader::new(stdin()),
            output: stdout(),
        }
    }
}

impl<Read, Write> UserProvidedAuthConfirmationHandler<Read, Write>
where
    Read: AsyncRead + Unpin + Send + Sync,
    Write: AsyncWrite + Unpin + Send + Sync,
{
    pub fn new(input: Read, output: Write) -> Self {
        UserProvidedAuthConfirmationHandler {
            input: BufReader::new(input),
            output,
        }
    }
}

impl<Read, Write> AuthConfirmationHandler for UserProvidedAuthConfirmationHandler<Read, Write>
where
    Read: AsyncRead + Unpin + Send + Sync,
    Write: AsyncWrite + Unpin + Send + Sync,
{
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        for method in allowed_confirmations {
            if let Some(token_type) = method.token_type() {
                let msg = format!(
                    "{}: {}",
                    method.confirmation_type(),
                    method.confirmation_details()
                );

                return Box::new(async move {
                    self.output.write_all(msg.as_bytes()).await.ok();
                    self.output.flush().await.ok();
                    let mut buff = String::with_capacity(16);
                    self.input.read_line(&mut buff).await.ok();
                    buff.truncate(buff.trim().len());
                    if buff.is_empty() {
                        Some(ConfirmationAction::Abort)
                    } else {
                        let token = SteamGuardToken(buff);
                        Some(ConfirmationAction::GuardToken(token, token_type))
                    }
                });
            }
        }
        Box::new(async { None })
    }
}

pub struct SharedSecretAuthConfirmationHandler {
    shared_secret: String,
}

impl SharedSecretAuthConfirmationHandler {
    pub fn new(shared_secret: &str) -> Self {
        SharedSecretAuthConfirmationHandler {
            shared_secret: shared_secret.into(),
        }
    }
}

impl AuthConfirmationHandler for SharedSecretAuthConfirmationHandler {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        for method in allowed_confirmations {
            if let Some(token_type) = method.token_type() {
                return Box::new(async move {
                    let auth_code = generate_auth_code(&self.shared_secret, None)
                        .expect("Could not generate auth code given shared secret.");
                    let token = SteamGuardToken(auth_code);
                    Some(ConfirmationAction::GuardToken(token, token_type))
                });
            }
        }
        Box::new(async { None })
    }
}

#[derive(Default)]
pub struct DeviceConfirmationHandler;

impl AuthConfirmationHandler for DeviceConfirmationHandler {
    fn handle_confirmation(
        &mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + '_> {
        for method in allowed_confirmations {
            if method.class() == ConfirmationMethodClass::Confirmation {
                return Box::new(async move { Some(ConfirmationAction::None) });
            }
        }
        Box::new(async { None })
    }
}

impl<Left, Right> AuthConfirmationHandler for (Left, Right)
where
    Left: AuthConfirmationHandler + Send + Sync,
    Right: AuthConfirmationHandler + Send + Sync,
{
    fn handle_confirmation(
        &mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + '_> {
        let left = Box::into_pin(self.0.handle_confirmation(allowed_confirmations));
        let right = Box::into_pin(self.1.handle_confirmation(allowed_confirmations));
        Box::new(async move {
            match select(left, right).await {
                Either::Left((left_result, right_fut)) => match left_result {
                    None | Some(ConfirmationAction::None) => right_fut.await,
                    _ => left_result,
                },
                Either::Right((right_result, left_fut)) => match right_result {
                    None | Some(ConfirmationAction::None) => left_fut.await,
                    _ => right_result,
                },
            }
        })
    }
}

impl AuthConfirmationHandler for Box<dyn AuthConfirmationHandler> {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        self.as_mut().handle_confirmation(allowed_confirmations)
    }
}

impl AuthConfirmationHandler for Box<dyn AuthConfirmationHandler + Send> {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        self.as_mut().handle_confirmation(allowed_confirmations)
    }
}

fn first_some<T, S: Stream<Item = Option<T>> + Unpin>(
    stream: S,
) -> impl Future<Output = Option<T>> {
    stream
        .filter_map(ready)
        .into_future()
        .map(|(first, _rest)| first)
}

fn iter_handler<'a, Handler: AuthConfirmationHandler + 'a, I: Iterator<Item = &'a mut Handler>>(
    iter: I,
    allowed_confirmations: &[ConfirmationMethod],
) -> FuturesUnordered<impl Future<Output = Option<ConfirmationAction>> + Unpin + use<'a, Handler, I>>
{
    iter.map(|handler: &mut Handler| {
        Box::into_pin(handler.handle_confirmation(allowed_confirmations))
    })
    .collect::<FuturesUnordered<_>>()
}

impl<Handler: AuthConfirmationHandler> AuthConfirmationHandler for &mut [Handler] {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        let stream = iter_handler(self.iter_mut(), allowed_confirmations);
        Box::new(first_some(stream))
    }
}

impl<const N: usize, Handler: AuthConfirmationHandler> AuthConfirmationHandler for [Handler; N] {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        let stream = iter_handler(self.iter_mut(), allowed_confirmations);
        Box::new(first_some(stream))
    }
}

impl<Handler: AuthConfirmationHandler> AuthConfirmationHandler for Vec<Handler> {
    fn handle_confirmation<'this>(
        &'this mut self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Box<dyn Future<Output = Option<ConfirmationAction>> + Send + 'this> {
        let stream = iter_handler(self.iter_mut(), allowed_confirmations);
        Box::new(first_some(stream))
    }
}
