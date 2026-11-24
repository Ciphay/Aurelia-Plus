use crate::compat::plugin::{self, InstalledPlugin, PluginSpec};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const ENTRY_NAME: &str = "umu-run";

static SPEC: PluginSpec = PluginSpec {
    id: "umu",
    display: "umu-launcher",
    host: "GitHub",
    release_api: "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest",
    user_agent: "aurelia-umu-plugin",
    root_marker: |p| p.join(ENTRY_NAME).is_file(),
    entry_point,
    archive_marker_missing: "umu-launcher archive did not contain a `umu-run` executable",
};

fn find_entry_root(base: &Path) -> Option<PathBuf> {
    plugin::find_root(&SPEC, base)
}

pub fn entry_point(root: &Path) -> PathBuf {
    root.join(ENTRY_NAME)
}

pub fn installed(custom: Option<&Path>) -> Option<InstalledPlugin> {
    if let Some(custom) = custom {
        if custom.is_file() && custom.file_name().and_then(|n| n.to_str()) == Some(ENTRY_NAME) {
            let root = custom
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| custom.to_path_buf());
            return Some(InstalledPlugin {
                version: "custom".to_string(),
                entry: custom.to_path_buf(),
                root,
            });
        }
        let root = find_entry_root(custom)?;
        return Some(InstalledPlugin {
            version: "custom".to_string(),
            entry: entry_point(&root),
            root,
        });
    }
    plugin::managed_install(&SPEC)
}

pub async fn install(on_progress: &mut (dyn FnMut(u64, u64) + Send)) -> Result<PathBuf> {
    plugin::install(&SPEC, on_progress).await
}

pub async fn ensure_installed(custom: Option<&Path>) -> Result<PathBuf> {
    if let Some(custom) = custom {
        return installed(Some(custom))
            .map(|inst| inst.entry)
            .with_context(|| {
                format!(
                    "configured umu_path '{}' does not contain a `umu-run` executable",
                    custom.display()
                )
            });
    }
    plugin::ensure_managed(&SPEC).await
}

pub fn uninstall() -> Result<bool> {
    plugin::uninstall(&SPEC)
}

#[cfg(test)]
#[path = "umu_tests.rs"]
mod tests;
