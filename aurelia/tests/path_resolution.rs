use aurelia::core::config::LauncherConfig;
use aurelia::core::models::LibraryGame;
use aurelia::steam_client::{LaunchInfo, LaunchTarget, SteamClient, sanitize_install_dir};
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_batman_path_resolution() {
    let tmp = tempdir().unwrap();
    let library_root = tmp.path().to_path_buf();
    let steamapps = library_root.join("steamapps");
    let common = steamapps.join("common");
    fs::create_dir_all(&common).unwrap();

    let app_id = 209000;
    let game_name = "Batman™: Arkham Origins";
    let installdir = "Batman Arkham Origins";

    let game_dir = common.join(installdir);
    fs::create_dir_all(&game_dir).unwrap();
    let exe_path = game_dir.join("SinglePlayer/Binaries/Win32/BatmanOrigins.exe");
    fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
    fs::write(&exe_path, "fake exe").unwrap();

    let manifest_path = steamapps.join(format!("appmanifest_{}.acf", app_id));
    let manifest_content = format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\"{}\"\n\t\"name\"\t\"{}\"\n\t\"installdir\"\t\"{}\"\n}}",
        app_id, game_name, installdir
    );
    fs::write(&manifest_path, manifest_content).unwrap();

    let client = SteamClient::new().unwrap();
    let mut app = LibraryGame {
        app_id,
        name: game_name.to_string(),
        install_path: Some(game_dir.to_string_lossy().to_string()),
        is_installed: true,
        playtime_forever_minutes: Some(0),
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

    let launch_info = LaunchInfo {
        app_id,
        id: "0".to_string(),
        description: "Play".to_string(),
        executable: "SinglePlayer\\Binaries\\Win32\\BatmanOrigins.exe".to_string(),
        arguments: "".to_string(),
        workingdir: None,
        target: LaunchTarget::NativeLinux,
    };

    let launcher_config = LauncherConfig {
        steam_library_path: library_root.to_string_lossy().to_string(),
        ..Default::default()
    };

    app.install_path = Some(common.join("WRONG_PATH").to_string_lossy().to_string());

    let result: anyhow::Result<std::process::Child> = client
        .internal_legacy_launch_adhoc(&app, &launch_info, None, &launcher_config, None)
        .await;

    if let Err(e) = &result {
        println!("Result error: {:?}", e);
    }

    assert!(result.is_ok() || !result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn test_colon_preservation_on_linux() {
    let app_id = 123;
    let game_name = "Game: With Colon";

    let client = SteamClient::new().unwrap();
    let (resolved_name, pics_dir) = client.resolve_install_game_info(app_id).await;
    assert!(resolved_name.contains("App 123"));
    assert!(pics_dir.is_none());

    let result_dir = sanitize_install_dir(game_name);

    #[cfg(not(target_os = "windows"))]
    assert_eq!(result_dir, "Game: With Colon");

    #[cfg(target_os = "windows")]
    assert_eq!(result_dir, "Game_ With Colon");
}
