use std::future::Future;

tokio::task_local! {
    static ENV: Vec<(String, String)>;
}

pub fn is_forwardable_env(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_SESSION_DESKTOP",
        "XDG_CURRENT_DESKTOP",
        "XDG_SEAT",
        "DBUS_SESSION_BUS_ADDRESS",
        "QT_QPA_PLATFORM_THEME",
        "ENABLE_GAMESCOPE_WSI",
        "vk_xwayland_wait_ready",
    ];
    const PREFIXES: &[&str] = &[
        "GAMESCOPE_",
        "SDL_",
        "PROTON_",
        "DXVK_",
        "VKD3D_",
        "FSR4_",
        "MANGOHUD",
        "MESA_",
        "RADV_",
        "STEAM_",
    ];
    if key.starts_with("STEAM_COMPAT") {
        return false;
    }
    EXACT.contains(&key) || PREFIXES.iter().any(|p| key.starts_with(p))
}

pub fn collect() -> Vec<(String, String)> {
    std::env::vars().filter(|(k, _)| is_forwardable_env(k)).collect()
}

pub async fn scoped<F: Future>(env: Vec<(String, String)>, fut: F) -> F::Output {
    let env = env.into_iter().filter(|(k, _)| is_forwardable_env(k)).collect();
    ENV.scope(env, fut).await
}

pub fn get() -> Vec<(String, String)> {
    ENV.try_with(|e| e.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_display_and_graphics_variables_pass() {
        for k in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "GAMESCOPE_WAYLAND_DISPLAY", "PROTON_FSR4_UPGRADE", "DXVK_HDR"] {
            assert!(is_forwardable_env(k), "{k}");
        }
        for k in ["PATH", "HOME", "LD_PRELOAD", "LD_LIBRARY_PATH", "STEAM_COMPAT_DATA_PATH", "WINEPREFIX", "AURELIA_PASSWORD"] {
            assert!(!is_forwardable_env(k), "{k}");
        }
    }

    #[tokio::test]
    async fn scope_filters_and_is_empty_outside() {
        assert!(get().is_empty());
        let seen = scoped(vec![("DISPLAY".into(), ":1".into()), ("PATH".into(), "/evil".into())], async { get() }).await;
        assert_eq!(seen, vec![("DISPLAY".to_string(), ":1".to_string())]);
    }
}
