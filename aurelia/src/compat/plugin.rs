use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledPlugin {
    pub version: String,
    pub root: PathBuf,
    pub entry: PathBuf,
}

pub(crate) struct PluginSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub host: &'static str,
    pub release_api: &'static str,
    pub user_agent: &'static str,
    pub root_marker: fn(&Path) -> bool,
    pub archive_marker_missing: &'static str,
    pub entry_point: fn(&Path) -> PathBuf,
}

struct PluginRelease {
    tag: String,
    url: String,
    ext: String,
}

pub(crate) fn plugin_dir(spec: &PluginSpec) -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?
        .join("plugins")
        .join(spec.id))
}

fn version_stamp(base: &Path) -> PathBuf {
    base.join(".aurelia_version")
}

pub(crate) fn find_root(spec: &PluginSpec, base: &Path) -> Option<PathBuf> {
    if (spec.root_marker)(base) {
        return Some(base.to_path_buf());
    }
    let entries = std::fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && (spec.root_marker)(&p) {
            return Some(p);
        }
    }
    None
}

pub(crate) fn managed_install(spec: &PluginSpec) -> Option<InstalledPlugin> {
    let base = plugin_dir(spec).ok()?;
    let root = find_root(spec, &base)?;
    let version = std::fs::read_to_string(version_stamp(&base))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Some(InstalledPlugin {
        version,
        entry: (spec.entry_point)(&root),
        root,
    })
}

pub(crate) async fn install(
    spec: &PluginSpec,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let root = install_payload(spec, on_progress).await?;
    Ok((spec.entry_point)(&root))
}

pub(crate) async fn ensure_managed(spec: &PluginSpec) -> Result<PathBuf> {
    if let Some(inst) = managed_install(spec) {
        return Ok(inst.entry);
    }
    let mut noop = |_, _| {};
    install(spec, &mut noop).await
}

async fn latest_release(spec: &PluginSpec) -> Result<PluginRelease> {
    let client = if spec.release_api.starts_with("https://api.github.com/") {
        crate::compat::proton::github_client()
    } else {
        reqwest::Client::builder()
            .user_agent(spec.user_agent)
            .build()
            .map_err(anyhow::Error::new)
    }
    .with_context(|| format!("failed to build the {} HTTP client", spec.host))?;

    let release: crate::compat::proton::GhRelease = client
        .get(spec.release_api)
        .send()
        .await
        .with_context(|| format!("failed requesting the {} latest release", spec.display))?
        .error_for_status()
        .with_context(|| {
            format!(
                "{} returned an error for the {} latest release",
                spec.host, spec.display
            )
        })?
        .json()
        .await
        .with_context(|| format!("failed parsing the {} release JSON", spec.display))?;

    let (asset, ext) = pick_archive(&release.assets).with_context(|| {
        format!(
            "no .tar.gz/.tar.xz/.tar asset on {} release '{}'",
            spec.display, release.tag_name
        )
    })?;

    Ok(PluginRelease {
        tag: release.tag_name.clone(),
        url: asset.browser_download_url.clone(),
        ext: ext.to_string(),
    })
}

fn pick_archive(
    assets: &[crate::compat::proton::GhAsset],
) -> Option<(&crate::compat::proton::GhAsset, &'static str)> {
    [".tar.gz", ".tar.xz", ".tar"].into_iter().find_map(|ext| {
        assets
            .iter()
            .find(|a| a.name.ends_with(ext) && !a.name.contains(".sha"))
            .map(|a| (a, ext))
    })
}

pub(crate) async fn install_payload(
    spec: &PluginSpec,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let release = latest_release(spec).await?;
    let base = plugin_dir(spec)?;

    if base.exists() {
        std::fs::remove_dir_all(&base)
            .with_context(|| format!("failed clearing {}", base.display()))?;
    }
    std::fs::create_dir_all(&base)
        .with_context(|| format!("failed creating {}", base.display()))?;

    let tmp = base.join(format!(".download{}", release.ext));
    crate::compat::proton::download_to(&release.url, &tmp, on_progress)
        .await
        .with_context(|| format!("failed downloading {}", spec.display))?;

    let result = crate::compat::proton::extract_tarball(&tmp, &release.ext, &base)
        .with_context(|| format!("failed extracting {}", spec.display));
    let _ = std::fs::remove_file(&tmp);
    result?;

    std::fs::write(version_stamp(&base), &release.tag)
        .with_context(|| format!("failed stamping version in {}", base.display()))?;

    find_root(spec, &base).context(spec.archive_marker_missing)
}

pub(crate) fn uninstall(spec: &PluginSpec) -> Result<bool> {
    let base = plugin_dir(spec)?;
    if !base.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&base)
        .with_context(|| format!("failed removing {}", base.display()))?;
    Ok(true)
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;
