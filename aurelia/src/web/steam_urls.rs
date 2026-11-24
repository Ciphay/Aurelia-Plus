const CDN_APPS_BASE: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
const STORE_APP_BASE: &str = "https://store.steampowered.com/app";

pub fn store_url(app_id: u32) -> String {
    format!("{STORE_APP_BASE}/{app_id}")
}

pub fn header_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/header.jpg")
}

pub fn capsule_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/library_600x900.jpg")
}

pub fn small_capsule_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/capsule_231x87.jpg")
}

pub fn hero_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/library_hero.jpg")
}

pub fn logo_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/logo.png")
}

#[cfg(test)]
#[path = "steam_urls_tests.rs"]
mod tests;
