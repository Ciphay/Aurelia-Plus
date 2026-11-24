pub mod cli;
pub mod event_log;
pub mod relay;
pub mod session;
pub mod wine_capture;

pub use cli::*;
pub use event_log::*;
pub use session::*;
pub use wine_capture::*;

use std::collections::HashMap;

pub const SENSITIVE_KEY_FRAGMENTS: &[&str] = &[
    "STEAM_TOKEN",
    "STEAM_PASSWORD",
    "TOKEN",
    "PASSWORD",
    "REFRESH_TOKEN",
    "SESSION_TOKEN",
    "SECRET",
];

pub fn redact_sensitive(mut map: HashMap<String, String>) -> HashMap<String, String> {
    for (key, value) in map.iter_mut() {
        let upper_key = key.to_uppercase();
        if SENSITIVE_KEY_FRAGMENTS
            .iter()
            .any(|frag| upper_key.contains(frag))
        {
            *value = "[REDACTED]".to_string();
        }
    }
    map
}

#[cfg(test)]
mod tests;
