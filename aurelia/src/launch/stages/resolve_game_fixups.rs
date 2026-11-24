use crate::launch::pipeline::{LaunchError, PipelineContext, PipelineStage};
use async_trait::async_trait;

pub struct ResolveGameFixupsStage;

#[async_trait]
impl PipelineStage for ResolveGameFixupsStage {
    fn name(&self) -> &str {
        "ResolveGameFixups"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let app_id = ctx.app.as_ref().map(|a| a.app_id).unwrap_or(ctx.app_id);
        let mut fixups = crate::launch::fixups::game_fixups(app_id);
        if ctx.proton_path.is_some() {
            add_anticheat_env(ctx, &mut fixups).await;
        }

        if !fixups.is_empty() {
            tracing::info!(
                app_id,
                env_count = fixups.env.len(),
                dll_count = fixups.dll_overrides.len(),
                launch_arg_count = fixups.launch_args.len(),
                reg_op_count = fixups.reg_ops.len(),
                "Applying per-game fixups from registry"
            );
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("app_id".into(), app_id.to_string());
            metadata.insert("env_count".into(), fixups.env.len().to_string());
            metadata.insert(
                "dll_override_count".into(),
                fixups.dll_overrides.len().to_string(),
            );
            metadata.insert("launch_arg_count".into(), fixups.launch_args.len().to_string());
            metadata.insert("reg_op_count".into(), fixups.reg_ops.len().to_string());
            ctx.log_info(
                "game_fixups_resolved",
                format!(
                    "Resolved {} env + {} DLL-override + {} launch-arg + {} registry fixups",
                    fixups.env.len(),
                    fixups.dll_overrides.len(),
                    fixups.launch_args.len(),
                    fixups.reg_ops.len()
                ),
                Some("ResolveGameFixups".into()),
                metadata,
            );
        }

        ctx.game_fixups = fixups;
        Ok(())
    }
}

async fn add_anticheat_env(ctx: &PipelineContext, fixups: &mut crate::launch::fixups::GameFixups) {
    use crate::compat::anticheat;
    let Some(dir) = ctx.app.as_ref().and_then(|a| a.install_path.clone()) else {
        return;
    };
    let kinds = tokio::task::spawn_blocking(move || anticheat::detect(std::path::Path::new(&dir)))
        .await
        .unwrap_or_default();
    if kinds.is_empty() {
        return;
    }
    let installed = anticheat::installed_runtimes().await;
    for kind in &kinds {
        if !installed.contains_key(&kind.runtime_app_id()) {
            tracing::warn!(
                "{} not installed; the game's anti-cheat may refuse to start \
                 (install with `aurelia proton anticheat {}`)",
                kind.runtime_name(),
                kind.cli_name()
            );
        }
    }
    for (key, val) in anticheat::runtime_env(&kinds, &installed) {
        if fixups.env.iter().any(|(k, _)| *k == key) {
            continue;
        }
        tracing::info!("Anti-cheat runtime: {key}={val}");
        fixups.env.push((key, val));
    }
}
