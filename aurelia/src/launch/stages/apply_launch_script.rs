use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::launch::launch_script;
use crate::launch::pipeline::{LaunchError, LaunchErrorKind, PipelineContext, PipelineStage};

pub struct ApplyLaunchScriptStage;

#[async_trait]
impl PipelineStage for ApplyLaunchScriptStage {
    fn name(&self) -> &str {
        "ApplyLaunchScript"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let script = launch_script::resolve(
            ctx.app_id,
            ctx.launcher_config.as_ref(),
            ctx.launch_script_override.as_deref(),
            ctx.disable_launch_script,
        );

        let Some(script) = script else {
            return Ok(());
        };

        if ctx.command_spec.is_none() {
            return Ok(());
        }

        if !script.exists() {
            return Err(LaunchError::new(
                LaunchErrorKind::Validation,
                format!("Launch script not found: {}", script.display()),
            )
            .with_context("app_id", ctx.app_id.to_string())
            .with_context("launch_script", script.to_string_lossy()));
        }

        let app_id = ctx.app_id;
        let app_name = ctx.app.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        let game_dir = ctx.resolved_install_dir.clone().or_else(|| {
            ctx.app
                .as_ref()
                .and_then(|a| a.install_path.as_ref().map(PathBuf::from))
        });

        let spec = ctx.command_spec.as_mut().expect("command_spec present");
        let old_program = spec.program.clone();
        let old_args = std::mem::take(&mut spec.args);

        spec.env
            .insert("AURELIA_APP_ID".to_string(), app_id.to_string());
        spec.env.insert("AURELIA_APP_NAME".to_string(), app_name);
        if let Some(dir) = &game_dir {
            spec.env.insert(
                "AURELIA_GAME_DIR".to_string(),
                dir.to_string_lossy().to_string(),
            );
        }
        spec.env.insert(
            "AURELIA_LAUNCH_PROGRAM".to_string(),
            old_program.to_string_lossy().to_string(),
        );
        spec.env
            .insert("AURELIA_LAUNCH_ARGS".to_string(), old_args.join(" "));

        let mut new_args = Vec::with_capacity(old_args.len() + 1);
        new_args.push(old_program.to_string_lossy().to_string());
        new_args.extend(old_args);
        spec.args = new_args;
        spec.program = script.clone();

        let mut metadata = HashMap::new();
        metadata.insert(
            "launch_script".to_string(),
            script.to_string_lossy().to_string(),
        );
        metadata.insert(
            "wrapped_program".to_string(),
            old_program.to_string_lossy().to_string(),
        );
        ctx.log_info(
            "launch_script_applied",
            format!("Wrapping launch through script: {}", script.display()),
            Some("ApplyLaunchScript".to_string()),
            metadata,
        );

        Ok(())
    }
}

#[cfg(test)]
#[path = "apply_launch_script_tests.rs"]
mod tests;
