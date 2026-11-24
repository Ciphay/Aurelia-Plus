use super::*;

fn config_with_library(library: &std::path::Path) -> crate::core::config::LauncherConfig {
    let mut config = crate::core::config::LauncherConfig::default();
    config.steam_library_path = library.to_string_lossy().into_owned();
    config.use_shared_compat_data = false;
    config
}

#[test]
fn proton_prefix_wins_once_proton_has_set_it_up() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path();
    let proton_prefix = library.join("steamapps/compatdata/1903340/pfx");
    std::fs::create_dir_all(proton_prefix.join("drive_c/users/steamuser")).unwrap();

    let config = config_with_library(library);
    let store = crate::core::models::UserConfigStore::new();

    assert_eq!(
        game_save_prefix(&config, 1903340, &store, None),
        proton_prefix
    );
    assert_ne!(
        steam_wineprefix_for_game(&config, 1903340, &store, None),
        proton_prefix
    );
}

#[test]
fn bare_compatdata_dir_is_not_enough() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path();
    std::fs::create_dir_all(library.join("steamapps/compatdata/1903340")).unwrap();

    let config = config_with_library(library);
    let store = crate::core::models::UserConfigStore::new();

    assert_eq!(
        game_save_prefix(&config, 1903340, &store, None),
        steam_wineprefix_for_game(&config, 1903340, &store, None),
    );
}

#[test]
fn proton_compat_prefix_matches_steam_compat_data_path() {
    let tmp = tempfile::tempdir().unwrap();
    let config = config_with_library(tmp.path());
    assert_eq!(
        proton_compat_prefix(&config, 42, None),
        tmp.path()
            .join("steamapps")
            .join("compatdata")
            .join("42")
            .join("pfx"),
    );
}
