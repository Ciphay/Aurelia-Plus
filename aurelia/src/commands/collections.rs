use crate::commands::common::*;

use anyhow::Result;
use aurelia::library::collections::{self, CollectionsStore};

pub(crate) fn collection_kind(c: &collections::Collection) -> &'static str {
    if c.is_builtin() {
        "built-in"
    } else if c.is_dynamic() {
        "dynamic"
    } else {
        "static"
    }
}

pub(crate) async fn cmd_collections_list(json: bool) -> Result<()> {
    let store = CollectionsStore::load()?;
    let visible: Vec<&collections::Collection> =
        store.collections.iter().filter(|c| !c.deleted).collect();

    if json {
        let items: Vec<serde_json::Value> = visible
            .iter()
            .map(|c| {
                let members = c.added.iter().filter(|a| !c.removed.contains(a)).count();
                serde_json::json!({
                    "id": c.id,
                    "name": c.name,
                    "kind": collection_kind(c),
                    "dynamic": c.is_dynamic(),
                    "count": members,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "namespace_version": store.namespace_version,
            "collections": items,
        }));
        return Ok(());
    }

    if visible.is_empty() {
        cli_println!("No collections. Create one with `aurelia collections create <name>`,");
        cli_println!("or fetch your Steam collections with `aurelia collections pull`.");
        return Ok(());
    }

    cli_println!("{:<14}  {:<9}  {:>7}  NAME", "ID", "KIND", "GAMES");
    for c in &visible {
        let members = c.added.iter().filter(|a| !c.removed.contains(a)).count();
        let count = if c.is_dynamic() {
            "-".to_string()
        } else {
            members.to_string()
        };
        cli_println!(
            "{:<14}  {:<9}  {:>7}  {}",
            c.id,
            collection_kind(c),
            count,
            c.name
        );
    }
    cli_println!("\n{} collection(s).", visible.len());
    Ok(())
}

pub(crate) async fn cmd_collections_show(name: String, json: bool) -> Result<()> {
    let store = CollectionsStore::load()?;
    let c = store.resolve(&name)?;
    let members: Vec<u32> = if c.is_dynamic() {
        Vec::new()
    } else {
        c.added
            .iter()
            .copied()
            .filter(|a| !c.removed.contains(a))
            .collect()
    };

    if json {
        print_json(&serde_json::json!({
            "id": c.id,
            "name": c.name,
            "kind": collection_kind(c),
            "dynamic": c.is_dynamic(),
            "app_ids": members,
        }));
        return Ok(());
    }

    cli_println!("Collection : {}", c.name);
    cli_println!("Id         : {}", c.id);
    cli_println!("Kind       : {}", collection_kind(c));
    if c.is_dynamic() {
        cli_println!(
            "\nThis is a dynamic (filter-based) collection; Steam computes its members, \
             so they can't be listed offline."
        );
        return Ok(());
    }
    if members.is_empty() {
        cli_println!("\n(no games)");
    } else {
        cli_println!("\nGames ({}):", members.len());
        for app_id in members {
            cli_println!("  {app_id}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_collections_create(name: String, json: bool) -> Result<()> {
    let (id, _) = CollectionsStore::edit(|store| store.create(&name))?;
    if json {
        print_json(&serde_json::json!({ "status": "created", "id": id, "name": name }));
    } else {
        cli_println!("Created collection '{name}' ({id}).");
        cli_println!("Add games with `aurelia collections add \"{name}\" <appid> ...`, then");
        cli_println!("`aurelia collections push` to upload it to your Steam account.");
    }
    Ok(())
}

pub(crate) async fn cmd_collections_delete(name: String, json: bool) -> Result<()> {
    CollectionsStore::edit(|store| store.delete(&name))?;
    if json {
        print_json(&serde_json::json!({ "status": "deleted", "name": name }));
    } else {
        cli_println!("Marked collection '{name}' for deletion.");
        cli_println!("Run `aurelia collections push` to apply it to your Steam account.");
    }
    Ok(())
}

pub(crate) async fn cmd_collections_rename(
    name: String,
    new_name: String,
    json: bool,
) -> Result<()> {
    CollectionsStore::edit(|store| store.rename(&name, &new_name))?;
    if json {
        print_json(&serde_json::json!({ "status": "renamed", "from": name, "to": new_name }));
    } else {
        cli_println!("Renamed '{name}' to '{new_name}'.");
    }
    Ok(())
}

pub(crate) async fn cmd_collections_add(name: String, app_ids: Vec<u32>, json: bool) -> Result<()> {
    CollectionsStore::edit(|store| store.add(&name, &app_ids))?;
    if json {
        print_json(&serde_json::json!({ "status": "added", "name": name, "app_ids": app_ids }));
    } else {
        cli_println!("Added {} game(s) to '{name}'.", app_ids.len());
    }
    Ok(())
}

pub(crate) async fn cmd_collections_remove(
    name: String,
    app_ids: Vec<u32>,
    json: bool,
) -> Result<()> {
    CollectionsStore::edit(|store| store.remove(&name, &app_ids))?;
    if json {
        print_json(&serde_json::json!({ "status": "removed", "name": name, "app_ids": app_ids }));
    } else {
        cli_println!("Removed {} game(s) from '{name}'.", app_ids.len());
    }
    Ok(())
}

pub(crate) async fn cmd_collections_pull(json: bool) -> Result<()> {
    let client = authed_client().await?;
    let store = collections::pull(&client).await?;
    let count = store.collections.iter().filter(|c| !c.deleted).count();
    if json {
        print_json(&serde_json::json!({
            "status": "pulled",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Pulled collections from Steam. {count} collection(s) locally (version {}).",
            store.namespace_version
        );
    }
    Ok(())
}

pub(crate) async fn cmd_collections_push(yes: bool, force: bool, json: bool) -> Result<()> {
    let local = CollectionsStore::load()?;
    local.check_push_allowed(force)?;
    confirm_cloud_write(
        "upload",
        local.collections.len(),
        local.pending_deletions(),
        yes,
        json,
    )?;
    let client = authed_client().await?;
    let store = collections::push(&client, force).await?;
    let count = store.collections.len();
    if json {
        print_json(&serde_json::json!({
            "status": "pushed",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Pushed collections to Steam. Now at version {} ({count} collection(s)).",
            store.namespace_version
        );
    }
    Ok(())
}

pub(crate) async fn cmd_collections_sync(yes: bool, json: bool) -> Result<()> {
    let local = CollectionsStore::load()?;
    confirm_cloud_write(
        "sync",
        local.collections.len(),
        local.pending_deletions(),
        yes,
        json,
    )?;
    let client = authed_client().await?;
    let store = collections::sync(&client).await?;
    let count = store.collections.len();
    if json {
        print_json(&serde_json::json!({
            "status": "synced",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Synced collections with Steam. Now at version {} ({count} collection(s)).",
            store.namespace_version
        );
    }
    Ok(())
}
