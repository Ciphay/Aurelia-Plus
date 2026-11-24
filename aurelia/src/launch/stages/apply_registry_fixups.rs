use async_trait::async_trait;
use std::path::{Path, PathBuf};
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError};

pub struct ApplyRegistryFixupsStage;

#[async_trait]
impl PipelineStage for ApplyRegistryFixupsStage {
    fn name(&self) -> &str { "ApplyRegistryFixups" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if ctx.game_fixups.reg_ops.is_empty() {
            return Ok(());
        }
        let Some(prefix) = ctx.command_spec.as_ref().and_then(|s| s.env.get("WINEPREFIX")).cloned() else {
            tracing::warn!("Registry fixups present but no WINEPREFIX was resolved; skipping them");
            return Ok(());
        };

        let library_root = ctx.launcher_config.as_ref()
            .map(|c| PathBuf::from(&c.steam_library_path))
            .unwrap_or_default();
        let proton = ctx.launcher_config.as_ref()
            .and_then(|c| c.game_configs.get(&ctx.app_id))
            .and_then(|c| c.forced_proton_version.clone())
            .or_else(|| ctx.proton_path.clone().filter(|p| !p.is_empty()))
            .unwrap_or_else(|| "wine".to_string());
        let active_runner = crate::core::utils::resolve_runner(&proton, &library_root);
        let wine = resolve_prefix_wine(&active_runner);

        let reg_ops = ctx.game_fixups.reg_ops.clone();
        for op in &reg_ops {
            let args = op.to_reg_add_args();
            #[cfg(unix)]
            {
                match std::process::Command::new(&wine)
                    .args(&args)
                    .env("WINEPREFIX", &prefix)
                    .env("WINEDEBUG", "-all")
                    .output()
                {
                    Ok(out) if out.status.success() => {
                        tracing::info!(reg_op = ?op, "Applied registry fixup");
                    }
                    Ok(out) => warn_fixup(
                        ctx,
                        op,
                        format!(
                            "reg.exe exited with {:?}: {}",
                            out.status.code(),
                            String::from_utf8_lossy(&out.stderr).trim()
                        ),
                    ),
                    Err(e) => warn_fixup(
                        ctx,
                        op,
                        format!("failed to spawn {}: {e}", wine.display()),
                    ),
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (&args, &wine, &prefix);
                tracing::debug!(reg_op = ?op, "Skipping registry fixup (non-unix host)");
            }
        }

        Ok(())
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn resolve_prefix_wine(runner: &Path) -> PathBuf {
    if let Some(wine) = crate::core::utils::proton_bundled_bare_wine(runner) {
        return wine;
    }
    for rel in ["bin/wine64", "bin/wine"] {
        let candidate = runner.join(rel);
        if candidate.exists() {
            return candidate;
        }
    }
    if runner.is_file() {
        return runner.to_path_buf();
    }
    PathBuf::from("wine")
}

#[cfg_attr(not(unix), allow(dead_code))]
fn warn_fixup(ctx: &PipelineContext, op: &crate::launch::fixups::RegOp, detail: String) {
    tracing::warn!(reg_op = ?op, %detail, "Registry fixup failed; launch continues");
    if let Some(logger) = &ctx.logger {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("reg_op".to_string(), format!("{:?}", op));
        metadata.insert("detail".to_string(), detail);
        let _ = logger.log(
            crate::infra::logging::LogLevel::Warn,
            "fixup_registry_warning",
            "Registry fixup failed; launch continues".to_string(),
            Some("ApplyRegistryFixups".to_string()),
            metadata,
        );
    }
}
