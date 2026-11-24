#[cfg(test)]
mod tests {
    use crate::core::config::LauncherConfig;
    use crate::core::models::LibraryGame;
    use crate::infra::runners::{CommandSpec, LaunchContext, Runner, WineTkgRunner};
    use crate::steam_client::{LaunchInfo, LaunchTarget};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn mock_context() -> LaunchContext {
        LaunchContext {
            app: LibraryGame {
                app_id: 123,
                name: "Test Game".to_string(),
                playtime_forever_minutes: None,
                is_installed: true,
                install_path: Some("/tmp/game".to_string()),
                local_manifest_ids: Default::default(),
                update_available: false,
                update_queued: false,
                active_branch: "public".to_string(),
                is_owned: true,
                is_family_shared: false,
                online_required: None,
                platform: None,
                from_windows_steam: false,
            },
            launch_info: LaunchInfo {
                app_id: 123,
                id: "0".to_string(),
                description: "Test".to_string(),
                executable: "game.exe".to_string(),
                arguments: "-v".to_string(),
                workingdir: None,
                target: LaunchTarget::WindowsProton,
            },
            launcher_config: LauncherConfig::default(),
            user_config: None,
            proton_path: Some("/tmp/proton".to_string()),
            steam_enabled: false,
            use_umu: false,
            umu_run: None,
            target_architecture: crate::core::models::ExecutableArchitecture::X86_64,
            dll_resolutions: Vec::new(),
            game_fixups: Default::default(),
            verification_ptr: std::ptr::null_mut(),
        }
    }

    #[test]
    fn test_launch_context_construction() {
        let ctx = mock_context();
        assert_eq!(ctx.app.app_id, 123);
    }

    #[test]
    fn test_command_spec_construction() {
        let spec = CommandSpec {
            program: PathBuf::from("/usr/bin/wine"),
            args: vec!["game.exe".to_string()],
            env: HashMap::from([("WINEPREFIX".to_string(), "/tmp/prefix".to_string())]),
            ..Default::default()
        };

        assert_eq!(spec.program, PathBuf::from("/usr/bin/wine"));
        assert_eq!(spec.args[0], "game.exe");
        assert_eq!(spec.env.get("WINEPREFIX").unwrap(), "/tmp/prefix");
    }

    #[tokio::test]
    async fn test_wine_tkg_runner_graphics_policy_autodetect() {
        use crate::core::models::{GraphicsBackendPolicy, UserAppConfig};
        use std::fs;
        use tempfile::tempdir;

        let tmp = tempdir().unwrap();
        let lib = tmp.path().join("library");
        let pfx = tmp.path().join("prefix");
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&pfx).unwrap();

        let runner_path = tmp.path().join("proton_8");
        let dxvk_dll = runner_path.join("files/lib/wine/dxvk/d3d11.dll");
        fs::create_dir_all(dxvk_dll.parent().unwrap()).unwrap();
        fs::write(&dxvk_dll, "fake dll content").unwrap();

        let app = LibraryGame {
            app_id: 123,
            name: "Test".to_string(),
            install_path: Some(tmp.path().to_string_lossy().to_string()),
            is_installed: true,
            playtime_forever_minutes: None,
            active_branch: "public".to_string(),
            update_available: false,
            update_queued: false,
            local_manifest_ids: HashMap::new(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: None,
            from_windows_steam: false,
        };

        let mut config = LauncherConfig::default();
        config.steam_library_path = lib.to_string_lossy().to_string();

        let mut user_config = UserAppConfig::default();
        user_config.graphics_layers.graphics_backend_policy = GraphicsBackendPolicy::Auto;

        let ctx = LaunchContext {
            app,
            launch_info: LaunchInfo {
                app_id: 123,
                id: "0".into(),
                description: "Test".into(),
                executable: "test.exe".into(),
                arguments: "".into(),
                workingdir: None,
                target: LaunchTarget::WindowsProton,
            },
            launcher_config: config,
            user_config: Some(user_config.clone()),
            proton_path: Some(runner_path.to_string_lossy().to_string()),
            steam_enabled: false,
            use_umu: false,
            umu_run: None,
            target_architecture: crate::core::models::ExecutableArchitecture::X86_64,
            dll_resolutions: Vec::new(),
            game_fixups: Default::default(),
            verification_ptr: std::ptr::null_mut(),
        };

        let runner = WineTkgRunner;
        let env = runner.build_env(&ctx).await.unwrap();

        let overrides = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(!overrides.contains("d3d11=n,b"));

        let mut user_config_dxvk = user_config.clone();
        user_config_dxvk.graphics_layers.graphics_backend_policy = GraphicsBackendPolicy::DXVK;
        let mut ctx_dxvk = ctx.clone();
        ctx_dxvk.user_config = Some(user_config_dxvk);
        let env_dxvk = runner.build_env(&ctx_dxvk).await.unwrap();
        let overrides_dxvk = env_dxvk.get("WINEDLLOVERRIDES").unwrap();
        assert!(overrides_dxvk.contains("d3d11=n"));
        assert!(!overrides_dxvk.contains("d3d11=n,b"));
    }

    #[tokio::test]
    async fn test_fixup_fragments_merged_into_build_env() {
        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.game_fixups = crate::launch::fixups::GameFixups {
            env: vec![("PROTON_NO_ESYNC".to_string(), "1".to_string())],
            dll_overrides: vec![("xlive".to_string(), "builtin".to_string())],
            ..Default::default()
        };

        let env = runner.build_env(&ctx).await.unwrap();

        assert_eq!(env.get("PROTON_NO_ESYNC").map(String::as_str), Some("1"));
        let overrides = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(
            overrides.contains("xlive=builtin"),
            "overrides were: {overrides}"
        );
    }

    #[tokio::test]
    async fn test_user_env_wins_over_fixup() {
        use crate::core::models::UserAppConfig;
        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.game_fixups = crate::launch::fixups::GameFixups {
            env: vec![("PROTON_NO_ESYNC".to_string(), "1".to_string())],
            ..Default::default()
        };
        let mut user = UserAppConfig::default();
        user.env_variables
            .insert("PROTON_NO_ESYNC".to_string(), "0".to_string());
        ctx.user_config = Some(user);

        let env = runner.build_env(&ctx).await.unwrap();
        assert_eq!(env.get("PROTON_NO_ESYNC").map(String::as_str), Some("0"));
    }

    fn fake_wine_runner(root: &std::path::Path) -> PathBuf {
        let runner_dir = root.join("wine-tkg");
        std::fs::create_dir_all(runner_dir.join("bin")).unwrap();
        std::fs::write(runner_dir.join("bin/wine"), "").unwrap();
        runner_dir
    }

    #[tokio::test]
    async fn test_fixup_launch_args_threaded_into_build_command() {
        let tmp = tempfile::tempdir().unwrap();
        let runner_dir = fake_wine_runner(tmp.path());

        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.proton_path = Some(runner_dir.to_string_lossy().to_string());
        ctx.game_fixups = crate::launch::fixups::GameFixups {
            launch_args: vec!["-ignoredifferentvideocard".to_string()],
            ..Default::default()
        };

        let spec = runner.build_command(&ctx).await.unwrap();
        assert!(
            spec.args.iter().any(|a| a == "-ignoredifferentvideocard"),
            "args were: {:?}",
            spec.args
        );
    }

    #[tokio::test]
    async fn test_fixup_launch_arg_not_duplicated_when_user_passes_it() {
        use crate::core::models::UserAppConfig;
        let tmp = tempfile::tempdir().unwrap();
        let runner_dir = fake_wine_runner(tmp.path());

        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.proton_path = Some(runner_dir.to_string_lossy().to_string());
        ctx.game_fixups = crate::launch::fixups::GameFixups {
            launch_args: vec!["-ignoredifferentvideocard".to_string()],
            ..Default::default()
        };
        let mut user = UserAppConfig::default();
        user.launch_options = "-ignoredifferentvideocard".to_string();
        ctx.user_config = Some(user);

        let spec = runner.build_command(&ctx).await.unwrap();
        let count = spec.args.iter().filter(|a| *a == "-ignoredifferentvideocard").count();
        assert_eq!(count, 1, "args were: {:?}", spec.args);
    }

    #[tokio::test]
    async fn test_user_dll_overrides_merge_per_dll() {
        use crate::core::models::UserAppConfig;
        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.launcher_config.steam_runtime_policy =
            crate::core::models::SteamRuntimePolicy::Disabled;
        let base = runner.build_env(&ctx).await.unwrap();
        let computed = base.get("WINEDLLOVERRIDES").unwrap().clone();
        assert!(computed.contains("steamclient=n"), "computed: {computed}");

        let mut user = UserAppConfig::default();
        user.env_variables
            .insert("WINEDLLOVERRIDES".to_string(), "dxgi=b".to_string());
        ctx.user_config = Some(user);
        let env = runner.build_env(&ctx).await.unwrap();
        let merged = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(merged.contains("steamclient=n"), "merged: {merged}");
        assert!(merged.contains("lsteamclient="), "merged: {merged}");
        assert_eq!(
            crate::core::utils::dll_override_mode(merged, "dxgi").as_deref(),
            Some("b")
        );
        assert_eq!(
            crate::core::utils::dll_override_entries(merged)
                .iter()
                .filter(|(n, _)| n == "dxgi")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn test_fixup_env_dll_overrides_fill_only() {
        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        ctx.launcher_config.steam_runtime_policy =
            crate::core::models::SteamRuntimePolicy::Disabled;
        ctx.game_fixups = crate::launch::fixups::GameFixups {
            env: vec![(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient=b;xlive=n".to_string(),
            )],
            dll_overrides: vec![],
            ..Default::default()
        };
        let env = runner.build_env(&ctx).await.unwrap();
        let merged = env.get("WINEDLLOVERRIDES").unwrap();
        assert_eq!(
            crate::core::utils::dll_override_mode(merged, "steamclient").as_deref(),
            Some("n"),
            "{merged}"
        );
        assert_eq!(
            crate::core::utils::dll_override_mode(merged, "xlive").as_deref(),
            Some("n"),
            "{merged}"
        );
        assert!(merged.contains("lsteamclient="), "{merged}");
    }

    #[tokio::test]
    async fn test_resolver_entry_replaces_policy_entry() {
        use crate::core::models::{GraphicsBackendPolicy, UserAppConfig};
        use crate::launch::dll_provider_resolver::{DllProvider, DllResolution};
        let runner = WineTkgRunner;
        let mut ctx = mock_context();
        let mut user = UserAppConfig::default();
        user.graphics_layers.graphics_backend_policy = GraphicsBackendPolicy::WineD3D;
        ctx.user_config = Some(user);
        ctx.dll_resolutions = vec![DllResolution {
            name: "d3d11".to_string(),
            chosen_provider: DllProvider::GameLocal,
            chosen_path: None,
            fallback_reason: None,
            candidates: Vec::new(),
        }];
        let env = runner.build_env(&ctx).await.unwrap();
        let overrides = env.get("WINEDLLOVERRIDES").unwrap();
        let d3d11: Vec<_> = crate::core::utils::dll_override_entries(overrides)
            .into_iter()
            .filter(|(n, _)| n == "d3d11")
            .collect();
        assert_eq!(
            d3d11,
            vec![("d3d11".to_string(), "n".to_string())],
            "{overrides}"
        );
        assert!(!overrides.starts_with(';'));
    }

    #[tokio::test]
    async fn test_compat_data_path_follows_game_library() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("A");
        let b = tmp.path().join("B");
        std::fs::create_dir_all(a.join("steamapps")).unwrap();
        std::fs::write(
            a.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                b.display()
            ),
        )
        .unwrap();
        let app_id = 3_999_999_919;
        std::fs::create_dir_all(b.join("steamapps")).unwrap();
        std::fs::write(b.join(format!("steamapps/appmanifest_{app_id}.acf")), "").unwrap();

        let mut ctx = mock_context();
        ctx.app.app_id = app_id;
        ctx.launcher_config.steam_library_path = a.to_string_lossy().into_owned();
        let env = WineTkgRunner.build_env(&ctx).await.unwrap();
        let compat = env.get("STEAM_COMPAT_DATA_PATH").unwrap();
        assert_eq!(
            PathBuf::from(compat),
            b.join(format!("steamapps/compatdata/{app_id}"))
        );
    }

    #[tokio::test]
    async fn test_wine_tkg_runner_stubs() {
        let runner = WineTkgRunner;
        let ctx = mock_context();

        let env = runner.build_env(&ctx).await.unwrap();
        assert_eq!(env.get("SteamAppId").unwrap(), "123");
    }
}
