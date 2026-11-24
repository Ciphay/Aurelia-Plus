use aurelia::core::utils::build_dll_overrides;

#[test]
fn test_build_dll_overrides_baseline() {
    let overrides = build_dll_overrides(false, false, false, false, false, None, false, false);

    assert!(overrides.contains("vstdlib_s=n"));
    assert!(overrides.contains("steamclient=n"));

    assert!(!overrides.contains("d3d9=n,b"));
    assert!(!overrides.contains("d3d11=n,b"));
    assert!(!overrides.contains("dxgi=n,b"));
    assert!(!overrides.contains("d3d12=n,b"));

    assert!(!overrides.contains("GameOverlayRenderer=n"));
}

#[test]
fn test_build_dll_overrides_dxvk_active() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, false, false);

    assert!(overrides.contains("d3d9=n,b"));
    assert!(overrides.contains("d3d11=n,b"));
    assert!(overrides.contains("dxgi=n,b"));

    assert!(overrides.contains("GameOverlayRenderer=n"));
}

#[test]
fn test_build_dll_overrides_vkd3d_active() {
    let overrides = build_dll_overrides(false, true, false, true, false, None, false, false);

    assert!(overrides.contains("d3d12=n,b"));

    assert!(!overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_local_dll_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let d3d11_path = tmp.path().join("d3d11.dll");
    std::fs::write(&d3d11_path, "fake dll").unwrap();

    let overrides = build_dll_overrides(
        true,
        false,
        false,
        true,
        false,
        Some(tmp.path()),
        false,
        false,
    );

    assert!(!overrides.contains("d3d11=n,b"));
    assert!(overrides.contains("d3d9=n,b"));
}

#[test]
fn test_build_dll_overrides_steam_enabled_omits_steam_overrides() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, false, true);

    assert!(!overrides.contains("steamclient=n"));
    assert!(!overrides.contains("steam_api=n"));
    assert!(!overrides.contains("lsteamclient="));
    assert!(!overrides.contains("vstdlib_s=n"));

    assert!(overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_strict_dxvk() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, true, false);

    assert!(overrides.contains("d3d9=n"));
    assert!(overrides.contains("d3d11=n"));
    assert!(overrides.contains("dxgi=n"));
    assert!(overrides.contains("d3d8=n"));
    assert!(overrides.contains("d3d10core=n"));

    assert!(!overrides.contains("d3d9=n,b"));
    assert!(!overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_strict_dxvk_ignores_local() {
    let tmp = tempfile::tempdir().unwrap();
    let d3d11_path = tmp.path().join("d3d11.dll");
    std::fs::write(&d3d11_path, "fake dll").unwrap();

    let overrides = build_dll_overrides(
        true,
        false,
        false,
        true,
        false,
        Some(tmp.path()),
        true,
        false,
    );

    assert!(overrides.contains("d3d11=n"));
}
