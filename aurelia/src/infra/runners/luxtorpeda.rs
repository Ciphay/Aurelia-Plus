use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::anyhow;

use crate::infra::runners::{CommandSpec, LaunchContext, Runner};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};

pub struct LuxtorpedaRunner;

#[async_trait::async_trait]
impl Runner for LuxtorpedaRunner {
    fn name(&self) -> &str {
        "Luxtorpeda"
    }

    async fn prepare_prefix(&self, ctx: &LaunchContext) -> Result<(), LaunchError> {
        crate::compat::luxtorpeda::ensure_installed(custom_path(ctx).as_deref())
            .await
            .map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    "failed to install the luxtorpeda plugin",
                )
                .with_source(e)
            })?;

        let compat = compat_data_path(ctx);
        std::fs::create_dir_all(&compat).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Permission,
                format!("failed creating {}", compat.display()),
            )
            .with_source(anyhow!(e))
        })?;
        Ok(())
    }

    async fn build_env(&self, ctx: &LaunchContext) -> Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        let app_id = ctx.app.app_id.to_string();

        env.insert("SteamAppId".to_string(), app_id.clone());
        env.insert("SteamGameId".to_string(), app_id.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id);
        env.insert(
            "STEAM_COMPAT_DATA_PATH".to_string(),
            compat_data_path(ctx).to_string_lossy().to_string(),
        );

        crate::infra::runners::insert_fake_steam_trap(&mut env)?;

        for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
            if let Ok(val) = std::env::var(var) {
                env.insert(var.to_string(), val);
            }
        }

        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                env.insert(key.clone(), val.clone());
            }
        }

        Ok(env)
    }

    async fn build_command(&self, ctx: &LaunchContext) -> Result<CommandSpec, LaunchError> {
        let entry = crate::compat::luxtorpeda::ensure_installed(custom_path(ctx).as_deref())
            .await
            .map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    "failed to resolve the luxtorpeda entry point",
                )
                .with_source(e)
            })?;

        let (_install_dir, executable, game_working_dir) = ctx.game_paths()?;

        let mut args = vec!["run".to_string(), executable.to_string_lossy().to_string()];
        args.extend(
            ctx.launch_info
                .arguments
                .split_whitespace()
                .map(ToString::to_string),
        );
        if let Some(config) = &ctx.user_config {
            args.extend(
                config
                    .launch_options
                    .split_whitespace()
                    .map(ToString::to_string),
            );
        }

        Ok(CommandSpec {
            program: entry,
            args,
            cwd: Some(game_working_dir),
            env: self.build_env(ctx).await?,
        })
    }

    fn launch(&self, spec: &CommandSpec) -> Result<std::process::Child, LaunchError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(&spec.env);
        cmd.stdout(crate::infra::logging::relay::child_stdio());
        cmd.stderr(crate::infra::logging::relay::child_stdio());

        tracing::debug!(
            program = ?cmd.get_program(),
            args = ?cmd.get_args().collect::<Vec<_>>(),
            "luxtorpeda launch",
        );

        cmd.spawn()
            .map(crate::infra::logging::relay::attach)
            .map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Process,
                    "failed to spawn luxtorpeda process",
                )
                .with_source(anyhow!(e))
            })
    }
}

fn custom_path(ctx: &LaunchContext) -> Option<PathBuf> {
    ctx.launcher_config
        .luxtorpeda_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

fn compat_data_path(ctx: &LaunchContext) -> PathBuf {
    crate::core::utils::game_compat_data_dir(
        &ctx.launcher_config,
        ctx.app.app_id,
        ctx.app.install_path.as_deref().map(std::path::Path::new),
    )
}
