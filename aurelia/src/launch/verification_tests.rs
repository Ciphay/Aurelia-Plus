use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tempfile::tempdir;

use crate::infra::logging::{EventLogger, LaunchSession};
use crate::infra::runners::{CommandSpec, LaunchContext, Runner};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind, LaunchPipeline, PipelineContext};

struct MockRunner {
    exit_immediately: bool,
}

fn setup(exit_immediately: bool) -> (LaunchPipeline, PipelineContext, tempfile::TempDir) {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(
        crate::launch::stages::spawn_process::SpawnProcessStage,
    ));

    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let logger = EventLogger::new(&session).unwrap();

    let cmd = if exit_immediately {
        "exit 0"
    } else {
        "sleep 10"
    };
    let mut ctx = PipelineContext::new(123);
    ctx.logger = Some(logger);
    ctx.session = Some(session);
    ctx.runner = Some(Box::new(MockRunner { exit_immediately }));
    ctx.command_spec = Some(CommandSpec {
        program: PathBuf::from("sh"),
        args: vec!["-c".to_string(), cmd.to_string()],
        ..Default::default()
    });

    (pipeline, ctx, tmp)
}

fn summary_content(ctx: &PipelineContext) -> String {
    let summary_path = ctx.session.as_ref().unwrap().summary_path();
    std::fs::read_to_string(summary_path).unwrap()
}

#[async_trait]
impl Runner for MockRunner {
    fn name(&self) -> &str {
        "MockRunner"
    }
    async fn prepare_prefix(&self, _ctx: &LaunchContext) -> Result<(), LaunchError> {
        Ok(())
    }
    async fn build_env(
        &self,
        _ctx: &LaunchContext,
    ) -> Result<HashMap<String, String>, LaunchError> {
        Ok(HashMap::new())
    }
    async fn build_command(&self, _ctx: &LaunchContext) -> Result<CommandSpec, LaunchError> {
        Ok(CommandSpec::default())
    }
    fn launch(&self, _spec: &CommandSpec) -> Result<Child, LaunchError> {
        let cmd = if self.exit_immediately {
            "exit 0"
        } else {
            "sleep 10"
        };
        Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LaunchError::new(LaunchErrorKind::Process, e.to_string()))
    }
}

#[tokio::test]
async fn test_launch_verification_early_exit() {
    let (pipeline, mut ctx, _tmp) = setup(true);

    let _ = pipeline.run(&mut ctx).await;

    assert_eq!(ctx.verification.status, "failed_after_spawn");
    assert!(ctx.verification.process_lifetime_ms.is_some());
    assert_eq!(ctx.verification.exit_code, Some(0));

    let summary = summary_content(&ctx);
    assert!(summary.contains("\"result\": \"Failure\""));
    assert!(summary.contains("\"status\": \"failed_after_spawn\""));
}

#[tokio::test]
async fn test_launch_verification_success() {
    let (pipeline, mut ctx, _tmp) = setup(false);

    let _ = pipeline.run(&mut ctx).await;

    assert_eq!(ctx.verification.status, "verified");
    assert!(ctx.verification.process_lifetime_ms.is_some());

    let summary = summary_content(&ctx);
    assert!(summary.contains("\"result\": \"Success\""));
    assert!(summary.contains("\"status\": \"verified\""));

    if let Some(mut child) = ctx.child.take() {
        let _ = child.kill();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_background_steam_signal_then_exit_within_grace_is_early_exit() {
    use crate::infra::runners::wine_tkg::{ReadinessGrace, reclassify_after_grace};

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 0.2; exit 3")
        .spawn()
        .unwrap();

    assert!(child.try_wait().unwrap().is_none());

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    match reclassify_after_grace(&mut child) {
        ReadinessGrace::ExitedEarly { code } => assert_eq!(code, Some(3)),
        ReadinessGrace::Ready => {
            panic!("process exited within the grace window but was classified as Ready")
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_background_steam_alive_after_grace_is_ready() {
    use crate::infra::runners::wine_tkg::{ReadinessGrace, reclassify_after_grace};

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 10")
        .spawn()
        .unwrap();

    let outcome = reclassify_after_grace(&mut child);
    let _ = child.kill();
    assert_eq!(outcome, ReadinessGrace::Ready);
}
