use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ProtonSource {
    Valve { app_id: u32 },
    Github { url: String, ext: String },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProtonPackage {
    pub name: String,
    pub label: String,
    pub size: u64,
    #[serde(skip)]
    pub source: ProtonSource,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledProton {
    pub name: String,
    pub location: &'static str,
    pub path: PathBuf,
}

const VALVE_PROTONS: &[(&str, u32)] = &[
    ("Proton - Experimental", 1493710),
    ("Proton 9.0", 2805730),
    ("Proton 8.0", 2348590),
    ("Proton 7.0", 1887720),
    ("Proton 6.3", 1580130),
    ("Proton 5.13", 1420170),
];

struct GithubSource {
    repo: &'static str,
    label: &'static str,
    ext: &'static str,
    microarch: bool,
}

const GE_SOURCES: &[GithubSource] = &[
    GithubSource {
        repo: "GloriousEggroll/proton-ge-custom",
        label: "Proton-GE",
        ext: ".tar.gz",
        microarch: false,
    },
    GithubSource {
        repo: "GloriousEggroll/wine-ge-custom",
        label: "Wine-GE",
        ext: ".tar.xz",
        microarch: false,
    },
    GithubSource {
        repo: "CachyOS/proton-cachyos",
        label: "Proton-CachyOS",
        ext: ".tar.xz",
        microarch: true,
    },
];

const GE_RELEASES_PER_REPO: usize = 8;

pub(crate) const UNIFIED_LIB_SUBDIRS: &[&str] = &[
    "lib/wine",
    "lib64/wine",
    "files/lib/wine",
    "files/lib64/wine",
    "dist/lib/wine",
    "dist/lib64/wine",
    "files/lib",
    "files/lib64",
];

pub(crate) const ARCH_SUBDIRS: &[&str] =
    &["x86_64-windows", "i386-windows", "x86_64-unix", "i386-unix"];

pub(crate) const COMPONENT_FAMILIES: &[&str] = &["dxvk", "vkd3d", "vkd3d-proton", "nvapi"];

pub(crate) fn wine_component_dirs(family: &str) -> Vec<String> {
    UNIFIED_LIB_SUBDIRS
        .iter()
        .filter(|l| l.ends_with("wine"))
        .map(|l| format!("{l}/{family}"))
        .collect()
}

pub(crate) fn component_dll_subdirs(family: &str) -> Vec<String> {
    let mut out = Vec::new();
    for lib in UNIFIED_LIB_SUBDIRS.iter().filter(|l| l.ends_with("wine")) {
        for arch in ARCH_SUBDIRS {
            out.push(format!("{lib}/{family}/{arch}"));
        }
        out.push(format!("{lib}/{family}"));
    }
    out
}

pub(crate) fn host_cachyos_microarch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            return "x86_64_v3";
        }
    }
    "x86_64"
}

pub fn compat_tools_dir() -> Result<PathBuf> {
    if let Some(steam) = crate::core::config::detect_steam_path() {
        return Ok(steam.join("compatibilitytools.d"));
    }
    let home = crate::core::config::home_dir()?;
    Ok(home.join(".local/share/Steam/compatibilitytools.d"))
}

pub fn list_installed(steam_library_path: &Path) -> Vec<InstalledProton> {
    let tools = compat_tools_dir().ok();
    let libraries = crate::core::utils::runner_library_roots(
        steam_library_path,
        &crate::library::steam_root_candidates(),
    );
    let mut out: Vec<InstalledProton> = Vec::new();
    for (i, lib) in libraries.iter().enumerate() {
        let tools = if i == 0 { tools.as_deref() } else { None };
        out.extend(list_installed_in(&lib.join("steamapps/common"), tools));
    }
    let mut seen_paths = std::collections::HashSet::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(&p.path).unwrap_or_else(|_| p.path.clone());
        seen_paths.insert(key)
    });
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    out.dedup_by(|a, b| a.name == b.name);
    out
}

pub fn list_installed_in(common: &Path, tools: Option<&Path>) -> Vec<InstalledProton> {
    let mut out = Vec::new();

    if let Ok(entries) = std::fs::read_dir(common) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir()
                && name.to_ascii_lowercase().contains("proton")
                && path.join("proton").is_file()
            {
                out.push(InstalledProton {
                    name,
                    location: "steam",
                    path,
                });
            }
        }
    }

    if let Some(Ok(entries)) = tools.map(std::fs::read_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && is_custom_tool_dir(&path) {
                out.push(InstalledProton {
                    name: entry.file_name().to_string_lossy().to_string(),
                    location: "custom",
                    path,
                });
            }
        }
    }

    let mut seen_paths = std::collections::HashSet::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(&p.path).unwrap_or_else(|_| p.path.clone());
        seen_paths.insert(key)
    });

    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn is_custom_tool_dir(path: &Path) -> bool {
    path.join("proton").is_file()
        || path.join("compatibilitytool.vdf").is_file()
        || path.join("bin/wine").is_file()
}

fn valve_package(name: &str, app_id: u32) -> ProtonPackage {
    ProtonPackage {
        name: name.to_string(),
        label: "Valve".to_string(),
        size: 0,
        source: ProtonSource::Valve { app_id },
    }
}

pub async fn list_available() -> Result<Vec<ProtonPackage>> {
    let mut out: Vec<ProtonPackage> = VALVE_PROTONS
        .iter()
        .map(|(name, app_id)| valve_package(name, *app_id))
        .collect();

    for src in GE_SOURCES {
        match fetch_github_releases(src, GE_RELEASES_PER_REPO).await {
            Ok(mut pkgs) => out.append(&mut pkgs),
            Err(e) => tracing::warn!("could not list {} releases: {e:#}", src.label),
        }
    }
    Ok(out)
}

pub async fn resolve_package(name: &str) -> Result<ProtonPackage> {
    let needle = normalize_name(name);
    if let Some((vname, app_id)) = VALVE_PROTONS
        .iter()
        .find(|(n, _)| normalize_name(n) == needle)
    {
        return Ok(valve_package(vname, *app_id));
    }

    for src in GE_SOURCES {
        if let Some(pkg) = fetch_github_release_by_tag(src, name).await? {
            return Ok(pkg);
        }
    }

    bail!(
        "no Proton/Wine package named '{name}' (try `aurelia proton list` to see available names)"
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct GhRelease {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GhAsset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    #[serde(default)]
    pub(crate) size: u64,
}

pub(crate) fn github_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().user_agent("aurelia-proton-manager");
    if let Some(token) = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()) {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = format!("Bearer {token}").parse() {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        builder = builder.default_headers(headers);
    }
    builder
        .build()
        .context("failed to build the GitHub HTTP client")
}

fn choose_asset(assets: Vec<GhAsset>, ext: &str, microarch: Option<&str>) -> Option<GhAsset> {
    choose_asset_for_host(assets, ext, microarch, cfg!(target_arch = "aarch64"))
}

fn is_arm64_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("aarch64") || lower.contains("arm64")
}

fn choose_asset_for_host(
    assets: Vec<GhAsset>,
    ext: &str,
    microarch: Option<&str>,
    host_is_arm64: bool,
) -> Option<GhAsset> {
    let mut matching: Vec<GhAsset> = assets
        .into_iter()
        .filter(|a| a.name.ends_with(ext))
        .collect();

    if host_is_arm64 {
        if let Some(i) = matching.iter().position(|a| is_arm64_asset(&a.name)) {
            return Some(matching.swap_remove(i));
        }
    } else {
        matching.retain(|a| !is_arm64_asset(&a.name));
    }

    if let Some(arch) = microarch {
        if arch == "x86_64_v3" {
            if let Some(i) = matching.iter().position(|a| a.name.contains("x86_64_v3")) {
                return Some(matching.swap_remove(i));
            }
        }
        if let Some(i) = matching.iter().position(|a| {
            a.name.contains("x86_64")
                && !a.name.contains("x86_64_v3")
                && !a.name.contains("x86_64_v4")
        }) {
            return Some(matching.swap_remove(i));
        }
    }

    matching.into_iter().next()
}

fn release_to_package(src: &GithubSource, rel: GhRelease) -> Option<ProtonPackage> {
    let microarch = src.microarch.then(host_cachyos_microarch);
    let asset = choose_asset(rel.assets, src.ext, microarch)?;
    let label = if src.microarch && asset.name.contains("x86_64_v3") {
        format!("{} (x86_64_v3 — AVX2 optimized)", src.label)
    } else {
        src.label.to_string()
    };
    Some(ProtonPackage {
        name: rel.tag_name,
        label,
        size: asset.size,
        source: ProtonSource::Github {
            url: asset.browser_download_url,
            ext: src.ext.to_string(),
        },
    })
}

async fn fetch_github_releases(src: &GithubSource, per_page: usize) -> Result<Vec<ProtonPackage>> {
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page={per_page}",
        src.repo
    );
    let client = github_client()?;
    let releases: Vec<GhRelease> = crate::core::net::send_with_retry(&client, client.get(&url))
        .await
        .with_context(|| format!("failed requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("GitHub returned an error for {}", src.repo))?
        .json()
        .await
        .context("failed parsing GitHub releases JSON")?;

    Ok(releases
        .into_iter()
        .filter_map(|rel| release_to_package(src, rel))
        .collect())
}

async fn fetch_github_release_by_tag(
    src: &GithubSource,
    tag: &str,
) -> Result<Option<ProtonPackage>> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/tags/{tag}",
        src.repo
    );
    let client = github_client()?;
    let resp = crate::core::net::send_with_retry(&client, client.get(&url))
        .await
        .with_context(|| format!("failed requesting {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let rel: GhRelease = resp
        .error_for_status()
        .with_context(|| format!("GitHub returned an error for {}/{tag}", src.repo))?
        .json()
        .await
        .context("failed parsing GitHub release JSON")?;
    Ok(release_to_package(src, rel))
}

pub async fn install_github_package(
    pkg: &ProtonPackage,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let (url, ext) = match &pkg.source {
        ProtonSource::Github { url, ext } => (url, ext),
        ProtonSource::Valve { .. } => {
            bail!(
                "'{}' is an official Valve Proton — install it via Steam, not the GE downloader",
                pkg.name
            )
        }
    };

    let dest_dir = compat_tools_dir()?;
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("failed creating {}", dest_dir.display()))?;

    let tmp = dest_dir.join(format!(".{}.download{}", pkg.name, ext));
    download_to(url, &tmp, on_progress)
        .await
        .with_context(|| format!("failed downloading {}", pkg.name))?;

    if !crate::core::utils::is_safe_dir_name(&pkg.name) {
        bail!("invalid runtime name {:?}", pkg.name);
    }
    let staging = dest_dir.join(format!(".{}.extract", pkg.name));
    let _ = std::fs::remove_dir_all(&staging);
    let result = extract_tarball(&tmp, ext, &staging)
        .with_context(|| format!("failed extracting {}", pkg.name));
    let _ = std::fs::remove_file(&tmp);
    let target = dest_dir.join(&pkg.name);
    let result = result.and_then(|()| place_extracted(&staging, &target));
    let _ = std::fs::remove_dir_all(&staging);
    result?;

    Ok(target)
}

pub(crate) fn place_extracted(staging: &Path, target: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = std::fs::read_dir(staging)
        .with_context(|| format!("failed reading {}", staging.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    let source = match entries.as_slice() {
        [only] if only.is_dir() => only.clone(),
        _ => staging.to_path_buf(),
    };
    if target.exists() {
        std::fs::remove_dir_all(target)
            .with_context(|| format!("failed replacing {}", target.display()))?;
    }
    std::fs::rename(&source, target)
        .with_context(|| format!("failed moving runtime into {}", target.display()))
}

pub(crate) async fn download_to(
    url: &str,
    dest: &Path,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let client = github_client()?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed requesting {url}"))?
        .error_for_status()
        .context("download request failed")?;

    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("failed creating {}", dest.display()))?;

    let mut downloaded = 0u64;
    on_progress(0, total);
    while let Some(chunk) = resp
        .chunk()
        .await
        .context("failed reading download stream")?
    {
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed writing {}", dest.display()))?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().await.ok();
    Ok(())
}

pub(crate) fn extract_tarball(archive: &Path, ext: &str, dest_parent: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_parent)
        .with_context(|| format!("failed creating {}", dest_parent.display()))?;
    let file = std::fs::File::open(archive)
        .with_context(|| format!("failed opening {}", archive.display()))?;
    match ext {
        ".tar.gz" => unpack_guarded(flate2::read::GzDecoder::new(file), dest_parent),
        ".tar.xz" => unpack_guarded(xz2::read::XzDecoder::new(file), dest_parent),
        ".tar" => unpack_guarded(file, dest_parent),
        other => bail!("unsupported runtime archive type '{other}'"),
    }
}

fn unpack_guarded<R: std::io::Read>(reader: R, dest_parent: &Path) -> Result<()> {
    use std::path::Component;
    let mut archive = tar::Archive::new(reader);
    for entry in archive
        .entries()
        .context("reading runtime archive entries")?
    {
        let mut entry = entry.context("reading runtime archive entry")?;
        let path = entry
            .path()
            .context("decoding archive entry path")?
            .into_owned();
        if path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!(
                "refusing unsafe path '{}' in runtime archive",
                path.display()
            );
        }
        entry
            .unpack_in(dest_parent)
            .with_context(|| format!("extracting '{}'", path.display()))?;
    }
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    if !crate::core::utils::is_safe_dir_name(name) {
        bail!("invalid runtime name {name:?}");
    }
    let base = compat_tools_dir()?;
    let dir = base.join(name);
    if !dir.exists() {
        bail!(
            "'{name}' is not an installed custom runtime in {} \
             (official Valve Proton is removed via Steam)",
            base.display()
        );
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("failed removing {}", dir.display()))?;
    Ok(())
}

fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn valve_app_id(name: &str) -> Option<u32> {
    let needle = normalize_name(name);
    VALVE_PROTONS
        .iter()
        .find(|(n, _)| normalize_name(n) == needle)
        .map(|(_, id)| *id)
}

#[cfg(test)]
#[path = "proton_tests.rs"]
mod tests;
