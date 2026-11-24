use crate::daemon;
use crate::output;

use crate::commands::common::*;

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use aurelia::core::config::{experimental_enabled, load_session, save_session};
use aurelia::core::error::{ErrorKind, Reported, TypedError, classify};
use aurelia::core::models::QrEvent;
use aurelia::steam_client::SteamClient;

pub(crate) async fn require_experimental(what: &str) -> Result<()> {
    if experimental_enabled().await {
        return Ok(());
    }
    bail!(
        "`{what}` is an experimental feature and is disabled by default. It proves \
         identity / enables web-only commands but cannot create a full session for \
         library, install, or launch — use `aurelia login` or `aurelia login --qr` for \
         those. To enable it, run `aurelia config experimental true` (or set \
         AURELIA_EXPERIMENTAL=1)."
    )
}

async fn secure_session(client: &SteamClient) {
    use aurelia::core::config::{load_launcher_config, save_launcher_config};

    let persona = client.own_persona_name().await;
    let result = async {
        let mut config = load_launcher_config().await?;
        if !config.encrypt_session {
            config.encrypt_session = true;
            save_launcher_config(&config).await?;
        }
        let mut session = load_session().await?;
        if persona.is_some() {
            session.persona_name = persona;
        }
        save_session(&session).await
    }
    .await;
    if let Err(e) = result {
        tracing::warn!("could not encrypt session.json: {e:#}");
    }
}

enum StoredSession {
    None,
    Unknown,
    Account(String),
}

async fn peek_stored_session() -> StoredSession {
    use aurelia::core::models::SessionState;
    use aurelia::core::session_crypto::EncryptedSession;

    let Ok(path) = aurelia::core::config::session_path() else {
        return StoredSession::None;
    };
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return StoredSession::None;
    };
    let session: SessionState = if let Some(envelope) = EncryptedSession::from_json(&raw) {
        return match envelope.display_name.filter(|n| !n.trim().is_empty()) {
            Some(name) => StoredSession::Account(name),
            None => StoredSession::Unknown,
        };
    } else {
        match serde_json::from_str(&raw) {
            Ok(session) => session,
            Err(_) => return StoredSession::None,
        }
    };
    let name = session
        .persona_name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| session.account_name.filter(|n| !n.trim().is_empty()));
    if let Some(name) = name {
        return StoredSession::Account(name);
    }
    if session.refresh_token.is_some() || session.web_token.is_some() {
        return StoredSession::Unknown;
    }
    StoredSession::None
}

async fn warn_if_already_logged_in() {
    match peek_stored_session().await {
        StoredSession::Account(name) => cli_eprintln!(
            "Warning: already logged in as {name} — logging in again replaces that session."
        ),
        StoredSession::Unknown => {
            cli_eprintln!("Warning: an existing session is active — logging in again replaces it.")
        }
        StoredSession::None => {}
    }
}

pub(crate) async fn cmd_login(args: crate::cli::LoginArgs, json: bool) -> Result<()> {
    let crate::cli::LoginArgs {
        username,
        password,
        guard,
        qr,
        timeout,
        code,
        openid,
        web_token,
        health,
        reconnect,
    } = args;
    if health {
        return cmd_login_health(json).await;
    }
    if reconnect {
        return cmd_login_reconnect(json).await;
    }
    if qr {
        return cmd_login_qr(timeout, json).await;
    }
    if openid {
        require_experimental("login --openid").await?;
        return cmd_login_openid(json).await;
    }
    if let Some(pasted) = web_token {
        require_experimental("login --web-token").await?;
        return cmd_login_web_token(pasted, json).await;
    }

    if json {
        return cmd_login_json(username, password, guard).await;
    }

    let username = match username {
        Some(u) => u,
        None => {
            warn_if_already_logged_in().await;
            prompt_line("Steam username: ")?
        }
    };
    if username.is_empty() {
        bail!("no Steam username given");
    }
    let account = username.clone();
    let password = match password.or_else(|| std::env::var("AURELIA_PASSWORD").ok()) {
        Some(p) => p,
        None => {
            rpassword::prompt_password("Steam password: ").context("failed reading password")?
        }
    };

    let mut client = SteamClient::new()?;
    let attempt = client
        .login(
            username.clone(),
            password.clone().into(),
            guard.clone(),
            code.then(|| {
                Box::pin(crate::output::read_line_opt()) as aurelia::steam_client::CodeReader
            }),
        )
        .await;

    match attempt {
        Ok(_) => {
            secure_session(&client).await;
            report_login_success(&account, json);
            Ok(())
        }
        Err(err) => {
            let needs_code = guard.is_none()
                && client.pending_confirmations().iter().any(|p| {
                    use aurelia::core::models::SteamGuardReq::{DeviceCode, EmailCode};
                    matches!(p.requirement, EmailCode { .. } | DeviceCode)
                });

            if needs_code {
                tracing::info!("Login method awaited: Steam Guard code");
                let code = prompt_line("Steam Guard code: ")?;
                client
                    .login(username, password.clone().into(), Some(code), None)
                    .await
                    .context("login failed after providing Steam Guard code")?;
                secure_session(&client).await;
                report_login_success(&account, json);
                Ok(())
            } else if client.pending_confirmations().iter().any(|p| {
                matches!(
                    p.requirement,
                    aurelia::core::models::SteamGuardReq::DeviceConfirmation
                )
            }) {
                tracing::info!("Login method awaited: Steam Mobile app approval");
                bail!("approve this login in the Steam Mobile app, then run `aurelia login` again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

pub(crate) async fn cmd_login_qr(timeout: Option<u64>, json: bool) -> Result<()> {
    let timeout = Duration::from_secs(timeout.unwrap_or(QR_DEFAULT_TIMEOUT_SECS));
    let mut client = SteamClient::new()?;
    if !json {
        let session = client
            .login_qr(timeout, render_login_qr)
            .await
            .context("QR login failed")?;
        secure_session(&client).await;
        report_login_success(&session.account_name.unwrap_or_default(), false);
        return Ok(());
    }

    let outcome = race_control(
        client.login_qr(timeout, emit_qr_challenge_json),
        watch_control(output::read_line_opt),
    )
    .await;
    match outcome {
        Ok(session) => {
            secure_session(&client).await;
            let account = session.account_name.clone().unwrap_or_default();
            report_login_success(&account, true);
            print_result_line(qr_result_ok(&account, session.steam_id));
            Ok(())
        }
        Err(err) => {
            let err = err.context("QR login failed");
            print_result_line(qr_result_err(&err));
            Err(anyhow!(Reported(err)))
        }
    }
}

const QR_DEFAULT_TIMEOUT_SECS: u64 = 600;

async fn race_control<T>(
    work: impl std::future::Future<Output = Result<T>>,
    control: impl std::future::Future<Output = ErrorKind>,
) -> Result<T> {
    tokio::select! {
        r = work => r,
        kind = control => Err(anyhow!(TypedError::new(kind, match kind {
            ErrorKind::Cancelled => "QR login cancelled by the driver",
            _ => "stdin closed; QR login abandoned",
        }))),
    }
}

async fn watch_control<F, Fut>(mut next_line: F) -> ErrorKind
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Option<String>>>,
{
    loop {
        match next_line().await {
            Ok(Some(line)) if is_cancel_line(&line) => return ErrorKind::Cancelled,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return ErrorKind::InputClosed,
        }
    }
}

fn is_cancel_line(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("cancel")
}

fn qr_result_ok(account: &str, steam_id: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "event": "result",
        "ok": true,
        "type": "approved",
        "account": account,
        "steam_id": steam_id,
    })
}

fn qr_result_err(err: &anyhow::Error) -> serde_json::Value {
    let kind = classify(err).kind;
    let mut value = serde_json::json!({
        "event": "result",
        "ok": false,
        "type": kind,
        "error": format!("{err:#}"),
    });
    if let Some(hint) = kind.hint() {
        value["hint"] = hint.into();
    }
    value
}

pub(crate) async fn cmd_login_openid(json: bool) -> Result<()> {
    let steam_id = aurelia::web::openid::verify_identity_via_browser(|url| {
        if json {
            eprint_json_line(&serde_json::json!({ "event": "openid_challenge", "url": url }));
        } else {
            cli_eprintln!("\nComplete the sign-in on the official Steam page:\n  {url}\n");
            if aurelia::web::openid::open_in_browser(url) {
                cli_eprintln!("(opened in your default browser)");
            } else {
                cli_eprintln!("(could not open a browser automatically — open the link yourself)");
            }
        }
    })
    .await
    .context("browser (OpenID) sign-in failed")?;

    let matches_session = load_session()
        .await
        .ok()
        .and_then(|s| s.steam_id)
        .map(|stored| stored == steam_id);

    if json {
        print_json(&serde_json::json!({
            "openid_verified": true,
            "steam_id": steam_id,
            "matches_stored_session": matches_session,
            "logged_in": false,
        }));
    } else {
        cli_println!("Steam identity verified on the official sign-in page.");
        cli_println!("SteamID64 : {steam_id}");
        match matches_session {
            Some(true) => cli_println!("Session   : matches the stored session's account"),
            Some(false) => cli_println!(
                "Session   : WARNING — this is a different account than the stored session"
            ),
            None => {}
        }
        cli_eprintln!(
            "\nNote: Steam's OpenID sign-in proves who you are, but Valve issues no client\n\
             session token over it. Commands that need a session still require\n\
             `aurelia login` or `aurelia login --qr`.\n\
             Your browser is now signed in, though — run `aurelia login --web-token` to\n\
             also enable the web commands (inventory, wallet, market listings)."
        );
    }
    Ok(())
}

pub(crate) async fn cmd_login_web_token(pasted: Option<String>, json: bool) -> Result<()> {
    let env_token = || {
        std::env::var("AURELIA_WEB_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
    };
    let raw = match pasted.filter(|v| !v.trim().is_empty()).or_else(env_token) {
        Some(v) => v,
        None if json => {
            eprint_json_line(&serde_json::json!({
                "event": "web_token_required",
                "url": aurelia::web::web_token::CLIENTJSTOKEN_URL,
            }));
            read_stdin_line().await?
        }
        None => {
            cli_eprintln!(
                "\nIn the browser where you are signed in to Steam, open:\n  {}\n",
                aurelia::web::web_token::CLIENTJSTOKEN_URL
            );
            prompt_line("Paste the entire JSON line from that page: ")?
        }
    };

    let mut session = load_session().await.unwrap_or_default();
    let info = aurelia::web::web_token::parse_web_token(&raw, session.steam_id)?;
    let now = aurelia::web::web_token::now_unix();
    if info.expires_at.is_some_and(|exp| exp <= now) {
        bail!(
            "this web token already expired — reload the clientjstoken page in a signed-in \
             browser and paste the fresh JSON"
        );
    }

    if let Some(stored) = session.steam_id
        && stored != info.steam_id
    {
        bail!(
            "this web token belongs to SteamID {} but the stored session is for {stored} — \
             run `aurelia logout` first if you really want to switch accounts",
            info.steam_id
        );
    }
    session.web_token = Some(info.token.clone());
    if session.steam_id.is_none() {
        session.steam_id = Some(info.steam_id);
    }
    if session.account_name.is_none() {
        session.account_name = info.account_name.clone();
    }
    let full_session = session.refresh_token.is_some();
    save_session(&session).await?;

    if json {
        print_json(&serde_json::json!({
            "web_token_saved": true,
            "steam_id": info.steam_id,
            "account": info.account_name,
            "expires_at": info.expires_at,
            "logged_in": full_session,
        }));
    } else {
        cli_println!("Web token saved.");
        if let Some(account) = &info.account_name {
            cli_println!("Account   : {account}");
        }
        cli_println!("SteamID64 : {}", info.steam_id);
        match info.expires_at {
            Some(exp) => cli_println!(
                "Expires   : {} UTC",
                crate::commands::common::format_unix_timestamp(exp)
            ),
            None => cli_println!("Expires   : unknown (typically ~24h — Steam decides)"),
        }
        if !full_session {
            cli_eprintln!(
                "\nThis enables the web commands (inventory, wallet, market listings) only.\n\
                 Library/install/launch still need a full `aurelia login` or `aurelia login --qr`.\n\
                 When it expires, reload the clientjstoken page and re-run `aurelia login --web-token`."
            );
        }
    }
    Ok(())
}

pub(crate) const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) async fn login_with_timeout(
    client: &mut SteamClient,
    username: &str,
    password: &str,
    guard: Option<String>,
) -> Result<aurelia::core::models::SessionState> {
    match tokio::time::timeout(
        LOGIN_TIMEOUT,
        client.login(username.to_string(), password.into(), guard, None),
    )
    .await
    {
        Ok(login_result) => login_result,
        Err(_) => bail!(
            "login timed out after {}s waiting for a Steam Guard code or Steam Mobile app approval",
            LOGIN_TIMEOUT.as_secs()
        ),
    }
}

pub(crate) async fn cmd_login_json(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
) -> Result<()> {
    let account = username.clone().unwrap_or_default();
    match cmd_login_json_inner(username, password, guard).await {
        Ok(()) => {
            eprint_json_line(&qr_result_ok(&account, None));
            Ok(())
        }
        Err(err) => {
            eprint_json_line(&qr_result_err(&err));
            Err(err)
        }
    }
}

async fn cmd_login_json_inner(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
) -> Result<()> {
    let username = username.context("--json login requires a username (-u/--username)")?;
    let password = password
        .or_else(|| std::env::var("AURELIA_PASSWORD").ok())
        .context("--json login requires a password (-p/--password or AURELIA_PASSWORD)")?;
    let account = username.clone();

    let mut client = SteamClient::new()?;

    eprint_json_line(&serde_json::json!({
        "event": "awaiting_confirmation",
        "message": "Signing in — if prompted, approve this login in your Steam Mobile app."
    }));

    match login_with_timeout(&mut client, &username, &password, guard.clone()).await {
        Ok(_) => {
            secure_session(&client).await;
            report_login_success(&account, true);
            Ok(())
        }
        Err(err) => {
            use aurelia::core::models::SteamGuardReq::{DeviceCode, DeviceConfirmation, EmailCode};

            let code_kind = guard
                .is_none()
                .then(|| {
                    client
                        .pending_confirmations()
                        .iter()
                        .find_map(|p| match p.requirement {
                            EmailCode { .. } => Some("email"),
                            DeviceCode => Some("device"),
                            _ => None,
                        })
                })
                .flatten();

            if let Some(kind) = code_kind {
                eprint_json_line(&serde_json::json!({ "event": "guard_required", "type": kind }));
                let code = read_stdin_line()
                    .await
                    .context("failed reading Steam Guard code from stdin")?;
                login_with_timeout(&mut client, &username, &password, Some(code))
                    .await
                    .context("login failed after providing Steam Guard code")?;
                secure_session(&client).await;
                report_login_success(&account, true);
                Ok(())
            } else if client
                .pending_confirmations()
                .iter()
                .any(|p| matches!(p.requirement, DeviceConfirmation))
            {
                eprint_json_line(
                    &serde_json::json!({ "event": "guard_required", "type": "device_confirmation" }),
                );
                bail!("approve this login in the Steam Mobile app, then run login again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

pub(crate) fn emit_qr_challenge_json(event: QrEvent) {
    let line = match event {
        QrEvent::Challenge(url) => serde_json::json!({ "event": "qr_challenge", "url": url }),
        QrEvent::Scanned => serde_json::json!({ "event": "qr_scanned" }),
    };
    eprint_json_line(&line);
}

pub(crate) async fn read_stdin_line() -> Result<String> {
    output::read_line()
        .await
        .context("failed reading stdin")
        .map(|s| s.trim().to_string())
}

pub(crate) fn render_login_qr(event: QrEvent) {
    let url = match event {
        QrEvent::Challenge(url) => url,
        QrEvent::Scanned => {
            cli_eprintln!("\nQR code scanned — approve the sign-in in your Steam Mobile app.");
            return;
        }
    };
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            cli_eprintln!("\nScan this QR code with the Steam Mobile app:\n{rendered}");
        }
        Err(e) => cli_eprintln!("\n(could not render QR code: {e})"),
    }
    cli_eprintln!("Or open this link in the Steam Mobile app:\n  {url}\n");
}

pub(crate) fn report_login_success(account: &str, json: bool) {
    if json {
        print_json_line(&serde_json::json!({ "logged_in": true, "account": account }));
    } else {
        cli_println!("Login successful.");
    }
}

const REVOKE_TIMEOUT: Duration = Duration::from_secs(20);

enum Revocation {
    Revoked,
    NoToken,
    Failed(String),
}

async fn revoke(token: &str) -> Revocation {
    let attempt = async {
        let mut client = restored_client().await?;
        client.revoke_refresh_token(token).await
    };
    match tokio::time::timeout(REVOKE_TIMEOUT, attempt).await {
        Ok(Ok(())) => Revocation::Revoked,
        Ok(Err(e)) => Revocation::Failed(format!("{e:#}")),
        Err(_) => Revocation::Failed("timed out".to_string()),
    }
}

pub(crate) async fn cmd_session_plain(json: bool) -> Result<()> {
    let session = load_session()
        .await
        .context("could not read the stored session — run this where the OS keyring is unlocked, or run `aurelia login`")?;
    aurelia::core::config::save_session_plaintext(&session).await?;
    if json {
        print_json(&serde_json::json!({ "plaintext": true }));
    } else {
        cli_println!("The session is now stored as owner-only plaintext; no OS keyring is needed to read it.");
    }
    Ok(())
}

pub(crate) async fn cmd_logout(json: bool) -> Result<()> {
    let revocation = match load_session().await {
        Ok(session) => match session.refresh_token {
            Some(token) => revoke(token.expose()).await,
            None => Revocation::NoToken,
        },
        Err(e) => Revocation::Failed(format!("{e:#}")),
    };
    aurelia::core::config::delete_session().await?;
    let token = match &revocation {
        Revocation::Revoked => "revoked",
        Revocation::NoToken => "none",
        Revocation::Failed(_) => "not_revoked",
    };
    if json {
        let mut out = serde_json::json!({ "logged_out": true, "token": token });
        if let Revocation::Failed(reason) = &revocation {
            out["token_error"] = serde_json::json!(reason);
        }
        print_json(&out);
    } else {
        cli_println!("Logged out.");
        if let Revocation::Failed(reason) = &revocation {
            cli_eprintln!("warning: Steam did not revoke the refresh token: {reason}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_login_health(json: bool) -> Result<()> {
    let via_daemon = daemon::in_daemon();
    let status = if via_daemon {
        daemon::session_status().await
    } else {
        let client = restored_client().await?;
        let session = load_session().await.ok();
        daemon::SessionStatus {
            authenticated: client.is_authenticated(),
            account: session.as_ref().and_then(|s| s.account_name.clone()),
            steam_id: client
                .steam_id()
                .or(session.as_ref().and_then(|s| s.steam_id)),
            web_token: session
                .as_ref()
                .is_some_and(|s| s.web_token.as_ref().is_some_and(|t| !t.is_empty())),
        }
    };
    report_session_status(&status, via_daemon, json);
    Ok(())
}

pub(crate) async fn cmd_login_reconnect(json: bool) -> Result<()> {
    if !daemon::in_daemon() {
        bail!(
            "--reconnect needs the session daemon, but this command is running standalone \
             (AURELIA_NO_DAEMON is set, or the daemon is unreachable). Start `aurelia daemon` first."
        );
    }
    let status = daemon::force_reconnect().await;
    report_session_status(&status, true, json);
    Ok(())
}

pub(crate) fn report_session_status(status: &daemon::SessionStatus, via_daemon: bool, json: bool) {
    if json {
        print_json(&serde_json::json!({
            "logged_in": status.authenticated,
            "account": status.account,
            "steam_id": status.steam_id,
            "web_token": status.web_token,
            "daemon": via_daemon,
        }));
    } else {
        cli_println!(
            "Session : {}",
            if status.authenticated {
                "authenticated"
            } else {
                "not logged in"
            }
        );
        if let Some(account) = &status.account {
            cli_println!("Account : {account}");
        }
        if let Some(steam_id) = status.steam_id {
            cli_println!("SteamID : {steam_id}");
        }
        if status.web_token {
            cli_println!("Web     : web token stored (inventory/wallet/market available)");
        }
        cli_println!(
            "Daemon  : {}",
            if via_daemon {
                "yes (shared session)"
            } else {
                "no (standalone)"
            }
        );
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
