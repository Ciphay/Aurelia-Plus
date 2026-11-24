use crate::steam_urls;

use crate::commands::common::*;

use anyhow::{Context, Result, bail};

enum WalkOutcome<T> {
    NotDir,
    Found(T),
    Exhausted,
    Complete,
}

fn walk_install_dir<T>(
    install_path: &str,
    mut visit: impl FnMut(&std::path::Path) -> Option<T>,
) -> WalkOutcome<T> {
    let root = std::path::Path::new(install_path);
    if !root.is_dir() {
        return WalkOutcome::NotDir;
    }
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut budget = 100_000usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return WalkOutcome::Exhausted;
            }
            budget -= 1;
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
                continue;
            }
            if let Some(found) = visit(&path) {
                return WalkOutcome::Found(found);
            }
        }
    }
    WalkOutcome::Complete
}

pub(crate) fn detect_installed_platform_set(install_path: &str) -> Option<(bool, bool)> {
    let is_elf = |path: &std::path::Path| -> bool {
        use std::io::Read;
        let mut head = [0u8; 4];
        std::fs::File::open(path)
            .ok()
            .and_then(|mut f| f.read_exact(&mut head).ok())
            .is_some_and(|_| head == *b"\x7fELF")
    };
    let mut linux = false;
    let mut windows = false;
    let outcome = walk_install_dir(install_path, |path| {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("exe") => windows = true,
            None | Some("so") | Some("x86_64") | Some("appimage") | Some("bin") => {
                if !linux && is_elf(path) {
                    linux = true;
                }
            }
            _ => {}
        }
        (linux && windows).then_some(())
    });
    match outcome {
        WalkOutcome::NotDir => None,
        _ => Some((linux, windows)),
    }
}

pub(crate) fn detect_installed_platform(install_path: &str) -> Option<String> {
    let outcome = walk_install_dir(install_path, |path| {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            .then(|| "windows".to_string())
    });
    match outcome {
        WalkOutcome::Found(platform) => Some(platform),
        WalkOutcome::Complete => Some("linux".to_string()),
        WalkOutcome::NotDir | WalkOutcome::Exhausted => None,
    }
}

pub(crate) async fn cmd_list(
    installed: bool,
    search: Option<String>,
    collection: Option<String>,
    online: bool,
    check_updates: bool,
    json: bool,
) -> Result<()> {
    let mut client = restored_client().await?;
    let mut games = load_library(&mut client).await;

    let collections_store = aurelia::library::collections::CollectionsStore::load().ok();

    if client.is_authenticated() {
        tracing::info!("Fetching Family Sharing library from Steam ...");
        match client.fetch_family_shared_apps().await {
            Ok(shared) => {
                tracing::info!("Fetched {} Family-Shared apps", shared.len());
                merge_family_shared(&mut games, shared);
            }
            Err(e) => tracing::warn!("could not fetch family shared apps: {e:#}"),
        }
    }

    for sc in crate::commands::shortcut::load() {
        games.retain(|g| g.app_id != sc.app_id);
        games.push(aurelia::core::models::LibraryGame {
            app_id: sc.app_id,
            name: sc.name.clone(),
            playtime_forever_minutes: None,
            is_installed: true,
            install_path: Some(sc.dir().to_string_lossy().into_owned()),
            local_manifest_ids: Default::default(),
            update_available: false,
            update_queued: false,
            active_branch: "public".to_string(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: Some("windows".to_string()),
            from_windows_steam: false,
        });
    }

    if installed {
        games.retain(|g| g.is_installed);
    }
    if let Some(needle) = search.as_deref().map(str::to_ascii_lowercase) {
        games.retain(|g| g.name.to_ascii_lowercase().contains(&needle));
    }
    if let Some(name) = collection.as_deref() {
        let store = collections_store
            .as_ref()
            .context("no collections found — run `aurelia collections pull` first")?;
        let col = store.resolve(name)?;
        if col.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; its membership is computed by \
                 Steam and can't be resolved offline",
                col.name
            );
        }
        let members: std::collections::HashSet<u32> = col.added.iter().copied().collect();
        let removed: std::collections::HashSet<u32> = col.removed.iter().copied().collect();
        games.retain(|g| members.contains(&g.app_id) && !removed.contains(&g.app_id));
    }
    games.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });

    let game_collections = |app_id: u32| -> Vec<String> {
        collections_store
            .as_ref()
            .map(|s| {
                s.collections
                    .iter()
                    .filter(|c| !c.deleted && !c.is_dynamic() && c.contains(app_id))
                    .map(|c| c.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    };

    if online {
        if client.is_authenticated() && !client.is_offline() {
            tracing::info!(
                "Resolving online-required status for {} game(s) ...",
                games.len()
            );
            for g in &mut games {
                match client.fetch_online_required(g.app_id).await {
                    Ok(required) => g.online_required = Some(required),
                    Err(e) => {
                        tracing::warn!(
                            "could not determine online-required for {}: {e:#}",
                            g.app_id
                        );
                    }
                }
            }
        } else {
            tracing::warn!(
                "--online needs an authenticated, online session; ONLINE column will be unknown"
            );
        }
    }

    if check_updates {
        if client.is_authenticated() && !client.is_offline() {
            tracing::info!("Checking {} game(s) for updates ...", games.len());
            if let Err(e) = client.check_for_updates(&mut games).await {
                tracing::warn!("could not check for updates: {e:#}");
            }
        } else {
            tracing::warn!(
                "--check-updates needs an authenticated, online session; update status will be unknown"
            );
        }
    }

    for g in &mut games {
        if g.is_installed {
            if let Some(path) = g.install_path.as_deref() {
                g.platform = detect_installed_platform(path);
            }
        }
    }

    if json {
        let enriched: Vec<serde_json::Value> = games
            .iter()
            .map(|g| {
                let mut v = serde_json::to_value(g).unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "assets".into(),
                        serde_json::json!({
                            "header": steam_urls::header_url(g.app_id),
                            "capsule": steam_urls::capsule_url(g.app_id),
                            "hero": steam_urls::hero_url(g.app_id),
                            "logo": steam_urls::logo_url(g.app_id),
                        }),
                    );
                    obj.insert(
                        "store_url".into(),
                        serde_json::json!(steam_urls::store_url(g.app_id)),
                    );
                    obj.insert(
                        "collections".into(),
                        serde_json::json!(game_collections(g.app_id)),
                    );
                }
                v
            })
            .collect();
        print_json(&enriched);
        return Ok(());
    }

    if games.is_empty() {
        cli_println!("No games match.");
        return Ok(());
    }

    if online {
        cli_println!(
            "{:>9}  {:<10}  {:<13}  {:<7}  {:<20}  NAME",
            "APPID",
            "STATUS",
            "LICENSE",
            "ONLINE",
            "COLLECTIONS"
        );
    } else {
        cli_println!(
            "{:>9}  {:<10}  {:<13}  {:<20}  NAME",
            "APPID",
            "STATUS",
            "LICENSE",
            "COLLECTIONS"
        );
    }
    for g in &games {
        let status = if g.is_installed {
            if g.update_available {
                "update"
            } else {
                "installed"
            }
        } else {
            "-"
        };
        let license = if g.is_owned {
            "owned"
        } else if g.is_family_shared {
            "family-shared"
        } else {
            "unlicensed"
        };
        let mut branch = branch_suffix(g);
        if g.from_windows_steam {
            branch.push_str(" [wine]");
        }
        let cols = game_collections(g.app_id);
        let cols = if cols.is_empty() {
            "-".to_string()
        } else {
            cols.join(", ")
        };
        if online {
            let online_col = match g.online_required {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {:<7}  {:<20}  {}{}",
                g.app_id,
                status,
                license,
                online_col,
                cols,
                g.name,
                branch
            );
        } else {
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {:<20}  {}{}",
                g.app_id,
                status,
                license,
                cols,
                g.name,
                branch
            );
        }
    }

    let shared = games.iter().filter(|g| g.is_family_shared).count();
    if shared > 0 {
        cli_println!(
            "\n{} game(s), {} via Family Sharing (not licensed to this account).",
            games.len(),
            shared
        );
    } else {
        cli_println!("\n{} game(s).", games.len());
    }
    Ok(())
}

pub(crate) async fn cmd_account(json: bool) -> Result<()> {
    let client = match authed_client().await {
        Ok(client) => client,
        Err(e) => {
            if json {
                if let Some(me) = offline_account(&e).await {
                    print_json(&me);
                    return Ok(());
                }
            }
            return Err(e);
        }
    };
    let mut data = client.get_account_data().await;
    data.persona_name = client.own_persona_name().await;
    let daemon_logon_id = crate::daemon::in_daemon().then(aurelia::steam_client::logon_id);

    if json {
        let value = serde_json::json!({
            "daemon_logon_id": daemon_logon_id,
            "steam_id": data.steam_id,
            "account_name": data.account_name,
            "persona_name": data.persona_name,
            "country": data.country,
            "email": data.email,
            "email_validated": data.email_validated,
            "authed_machines": data.authed_machines,
            "flags": data.flags,
            "vac_bans": data.vac_bans,
            "vac_banned_apps": data.vac_banned_apps,
        });
        print_json(&value);
        return Ok(());
    }

    if let Some(id) = daemon_logon_id {
        cli_println!("DaemonLogonID: {id}");
    }
    cli_println!("Account : {}", data.account_name);
    if let Some(persona) = &data.persona_name {
        cli_println!("Persona : {persona}");
    }
    cli_println!("SteamID : {}", data.steam_id);
    cli_println!("Country : {}", data.country);
    cli_println!(
        "Email   : {} ({})",
        data.email,
        if data.email_validated {
            "validated"
        } else {
            "unvalidated"
        }
    );
    cli_println!("Devices : {}", data.authed_machines);
    cli_println!("VAC bans: {}", data.vac_bans);
    Ok(())
}

pub(crate) async fn cmd_libraries(json: bool) -> Result<()> {
    let libraries: Vec<(String, Option<u64>)> = aurelia::library::all_library_roots()
        .await
        .into_iter()
        .filter(|root| root.join("steamapps").is_dir())
        .map(|root| {
            let free = available_space_for(&root);
            (root.to_string_lossy().to_string(), free)
        })
        .collect();

    if json {
        let entries: Vec<serde_json::Value> = libraries
            .iter()
            .map(|(path, free)| serde_json::json!({ "path": path, "free_bytes": free }))
            .collect();
        print_json(&serde_json::json!({ "libraries": entries }));
    } else if libraries.is_empty() {
        cli_println!("No Steam library folders found.");
    } else {
        for (path, free) in &libraries {
            match free {
                Some(bytes) => cli_println!("{path}  ({} free)", human_bytes(*bytes)),
                None => cli_println!("{path}"),
            }
        }
    }
    Ok(())
}
