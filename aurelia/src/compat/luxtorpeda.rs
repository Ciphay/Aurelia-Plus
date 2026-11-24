use crate::compat::plugin::{self, InstalledPlugin, PluginSpec};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

static SPEC: PluginSpec = PluginSpec {
    id: "luxtorpeda",
    display: "luxtorpeda",
    host: "Codeberg",
    release_api: "https://codeberg.org/api/v1/repos/luxtorpeda/luxtorpeda/releases/latest",
    user_agent: "aurelia-luxtorpeda-plugin",
    root_marker: |p| p.join("toolmanifest.vdf").exists(),
    entry_point,
    archive_marker_missing: "luxtorpeda archive did not contain a toolmanifest.vdf",
};

fn find_tool_root(base: &Path) -> Option<PathBuf> {
    plugin::find_root(&SPEC, base)
}

pub fn entry_point(root: &Path) -> PathBuf {
    let fallback = root.join("luxtorpeda");
    let Ok(manifest) = std::fs::read_to_string(root.join("toolmanifest.vdf")) else {
        return fallback;
    };
    parse_commandline(&manifest)
        .map(|rel| root.join(rel.trim_start_matches('/')))
        .unwrap_or(fallback)
}

fn parse_commandline(manifest: &str) -> Option<String> {
    let idx = manifest.find("\"commandline\"")?;
    let rest = &manifest[idx + "\"commandline\"".len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    let value = &rest[start..end];
    value.split_whitespace().next().map(str::to_string)
}

pub fn installed(custom: Option<&Path>) -> Option<InstalledPlugin> {
    if let Some(custom) = custom {
        let root = find_tool_root(custom)?;
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
        let root = find_tool_root(custom).with_context(|| {
            format!(
                "configured luxtorpeda_path '{}' does not contain a toolmanifest.vdf",
                custom.display()
            )
        })?;
        return Ok(entry_point(&root));
    }
    plugin::ensure_managed(&SPEC).await
}

pub fn uninstall() -> Result<bool> {
    plugin::uninstall(&SPEC)
}

#[cfg(test)]
#[path = "luxtorpeda_tests.rs"]
mod tests;
