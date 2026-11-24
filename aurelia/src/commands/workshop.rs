use crate::cli::*;
use crate::commands::common::*;

use anyhow::{Context, Result, bail};
use aurelia::core::models::DownloadState;
use aurelia::steam_client::SteamClient;
use std::sync::{Arc, RwLock};

pub(crate) fn workshop_kind_label(kind: aurelia::core::models::WorkshopItemKind) -> &'static str {
    match kind {
        aurelia::core::models::WorkshopItemKind::Collection => "collection",
        aurelia::core::models::WorkshopItemKind::Item => "item",
    }
}

pub(crate) async fn cmd_workshop_browse(
    app_id: u32,
    search: Option<String>,
    sort: WorkshopSort,
    count: u32,
    cursor: String,
    tags: Vec<String>,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let page = client
        .query_workshop_files(
            app_id,
            search.as_deref(),
            sort.query_type(),
            &cursor,
            count,
            &tags,
        )
        .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "total": page.total,
            "next_cursor": page.next_cursor,
            "items": page.items,
        }));
        return Ok(());
    }

    if page.items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>10}  TITLE", "ID", "SIZE");
    for item in &page.items {
        let size = if item.file_size > 0 {
            human_bytes(item.file_size)
        } else {
            "-".to_string()
        };
        cli_println!("{:>12}  {:>10}  {}", item.id, size, item.title);
    }
    cli_println!(
        "\nShowing {} of {} result(s).",
        page.items.len(),
        page.total
    );
    if !page.next_cursor.is_empty() && page.next_cursor != cursor {
        cli_println!("Next page: --cursor \"{}\"", page.next_cursor);
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_info(ids: Vec<u64>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        print_json(&items);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    for item in &items {
        cli_println!("ID      : {}", item.id);
        cli_println!("Title   : {}", item.title);
        cli_println!("App     : {}", item.app_id);
        cli_println!("Type    : {}", workshop_kind_label(item.kind));
        if item.kind == aurelia::core::models::WorkshopItemKind::Collection {
            cli_println!("Items   : {}", item.children.len());
        } else {
            cli_println!("Size    : {}", human_bytes(item.file_size));
            cli_println!("Manifest: {}", item.hcontent_file);
        }
        cli_println!(
            "Updated : {}",
            format_unix_timestamp(item.time_updated.max(0) as u64)
        );
        cli_println!();
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let ids = client.fetch_subscribed_items(app_id).await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        print_json(&items);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>12}  TITLE", "ID", "SIZE");
    for item in &items {
        cli_println!(
            "{:>12}  {:>12}  {}",
            item.id,
            human_bytes(item.file_size),
            item.title
        );
    }
    cli_println!("\n{} subscribed item(s).", items.len());
    Ok(())
}

pub(crate) async fn workshop_resolve_ids(
    client: &SteamClient,
    ids: Vec<u64>,
    no_recurse: bool,
) -> Result<Vec<u64>> {
    if no_recurse {
        Ok(ids)
    } else {
        client.expand_collections(&ids).await
    }
}

async fn resolve_workshop_items(
    ids: Vec<u64>,
    no_recurse: bool,
) -> Result<(SteamClient, Vec<aurelia::core::models::WorkshopItem>)> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;
    Ok((client, items))
}

async fn for_each_workshop_item<'a, F, Fut>(
    client: &'a SteamClient,
    items: &'a [aurelia::core::models::WorkshopItem],
    past_tense: &str,
    fail_verb: &str,
    json: bool,
    op: F,
) -> Result<Vec<u64>>
where
    F: Fn(&'a SteamClient, u64, u32) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut acted = Vec::new();
    for item in items {
        op(client, item.id, item.app_id)
            .await
            .with_context(|| format!("failed to {fail_verb} Workshop item {}", item.id))?;
        acted.push(item.id);
        if !json {
            cli_println!("{past_tense} Workshop item {} ({}).", item.id, item.title);
        }
    }
    Ok(acted)
}

async fn install_one_workshop_item(
    client: &SteamClient,
    item: &aurelia::core::models::WorkshopItem,
    json: bool,
) -> Result<()> {
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .install_workshop_item(item, state)
        .await
        .with_context(|| format!("failed to start install for Workshop item {}", item.id))?;
    drive_progress(rx, json).await
}

pub(crate) async fn cmd_workshop_install(
    ids: Vec<u64>,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let (client, items) = resolve_workshop_items(ids, no_recurse).await?;
    if items.is_empty() {
        bail!("no installable Workshop items resolved");
    }

    for item in &items {
        if !json {
            cli_println!("Installing Workshop item {} ({}) ...", item.id, item.title);
        }
        install_one_workshop_item(&client, item, json).await?;
        if json {
            print_result_line(serde_json::json!({
                "id": item.id,
                "app_id": item.app_id,
                "status": "installed",
            }));
        }
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_uninstall(
    ids: Vec<u64>,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let (client, items) = resolve_workshop_items(ids, no_recurse).await?;

    let removed = for_each_workshop_item(
        &client,
        &items,
        "Uninstalled",
        "uninstall",
        json,
        |c, id, app| c.uninstall_workshop_item(id, app),
    )
    .await?;
    if json {
        print_json(&serde_json::json!({ "uninstalled": removed }));
    } else if removed.is_empty() {
        cli_println!("Nothing to uninstall.");
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_subscribe(
    ids: Vec<u64>,
    install: bool,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let (client, items) = resolve_workshop_items(ids, no_recurse).await?;

    let subscribed = for_each_workshop_item(
        &client,
        &items,
        "Subscribed to",
        "subscribe to",
        json,
        |c, id, app| c.subscribe_published_file(id, app),
    )
    .await?;

    if install {
        for item in &items {
            if !json {
                cli_println!("Installing Workshop item {} ...", item.id);
            }
            install_one_workshop_item(&client, item, json).await?;
        }
    }

    if json {
        print_json(&serde_json::json!({ "subscribed": subscribed, "installed": install }));
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_unsubscribe(
    ids: Vec<u64>,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let (client, items) = resolve_workshop_items(ids, no_recurse).await?;

    let unsubscribed = for_each_workshop_item(
        &client,
        &items,
        "Unsubscribed from",
        "unsubscribe from",
        json,
        |c, id, app| c.unsubscribe_published_file(id, app),
    )
    .await?;
    if json {
        print_json(&serde_json::json!({ "unsubscribed": unsubscribed }));
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_status(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let installed = client.read_installed_workshop(app_id).await?;
    let subscribed = client
        .fetch_subscribed_items(app_id)
        .await
        .unwrap_or_default();

    let mut all_ids: Vec<u64> = installed.iter().map(|i| i.id).collect();
    for &s in &subscribed {
        if !all_ids.contains(&s) {
            all_ids.push(s);
        }
    }
    let details = client
        .fetch_published_file_details(&all_ids)
        .await
        .unwrap_or_default();

    let detail_by_id: std::collections::HashMap<u64, &aurelia::core::models::WorkshopItem> =
        details.iter().map(|d| (d.id, d)).collect();
    let installed_by_id: std::collections::HashMap<u64, &_> =
        installed.iter().map(|i| (i.id, i)).collect();

    struct StatusRow {
        id: u64,
        title: String,
        installed: bool,
        subscribed: bool,
        update: bool,
    }
    let rows: Vec<StatusRow> = all_ids
        .iter()
        .map(|&id| {
            let inst = installed_by_id.get(&id).copied();
            let current_manifest = detail_by_id.get(&id).map(|d| d.hcontent_file);
            let update = match (inst, current_manifest) {
                (Some(i), Some(cur)) => cur != 0 && cur != i.manifest_id,
                _ => false,
            };
            StatusRow {
                id,
                title: detail_by_id
                    .get(&id)
                    .map(|d| d.title.clone())
                    .unwrap_or_default(),
                installed: inst.is_some(),
                subscribed: subscribed.contains(&id),
                update,
            }
        })
        .collect();

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "installed": r.installed,
                    "subscribed": r.subscribed,
                    "update_available": r.update,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "items": arr }));
        return Ok(());
    }

    if rows.is_empty() {
        cli_println!("No installed or subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!(
        "{:>12}  {:<9}  {:<10}  {:<7}  TITLE",
        "ID",
        "INSTALLED",
        "SUBSCRIBED",
        "UPDATE"
    );
    for r in &rows {
        cli_println!(
            "{:>12}  {:<9}  {:<10}  {:<7}  {}",
            r.id,
            if r.installed { "yes" } else { "no" },
            if r.subscribed { "yes" } else { "no" },
            if r.update { "yes" } else { "-" },
            r.title,
        );
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_rate(id: u64, up: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.vote_workshop_item(id, up).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "vote": if up { "up" } else { "down" },
            "status": "rated",
        }));
    } else {
        cli_println!(
            "Rated Workshop item {id} {}.",
            if up { "thumbs-up" } else { "thumbs-down" }
        );
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_comments_read(
    id: u64,
    start: i32,
    count: i32,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let comments = client.workshop_comments(id, start, count).await?;

    if json {
        print_json(&serde_json::json!({ "id": id, "comments": comments }));
        return Ok(());
    }
    if comments.is_empty() {
        cli_println!("No comments on Workshop item {id}.");
        return Ok(());
    }
    for c in &comments {
        cli_println!(
            "[{}] {} (+{})",
            format_unix_timestamp(c.timestamp.max(0) as u64),
            c.author,
            c.upvotes
        );
        cli_println!("  {}", c.text);
    }
    cli_println!("\n{} comment(s).", comments.len());
    Ok(())
}

pub(crate) async fn cmd_workshop_comment(id: u64, text: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let comment_id = client.post_workshop_comment(id, &text).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "comment_id": comment_id,
            "status": "posted",
        }));
    } else {
        cli_println!("Posted comment {comment_id} to Workshop item {id}.");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_workshop_publish(
    app_id: u32,
    content: Option<std::path::PathBuf>,
    item: Option<u64>,
    title: Option<String>,
    description: Option<String>,
    visibility: Option<VisibilityArg>,
    tags: Vec<String>,
    change_note: Option<String>,
    preview: Option<std::path::PathBuf>,
    json: bool,
) -> Result<()> {
    use aurelia::steam_client::workshop_upload::{ItemFields, PublishRequest, PublishStage};
    if item.is_none() && content.is_none() {
        bail!("a new item needs --content (the folder to publish)");
    }
    if item.is_none() && title.as_deref().unwrap_or("").is_empty() {
        bail!("a new item needs --title");
    }
    let client = authed_client().await?;
    let fields = ItemFields {
        title,
        description,
        visibility: visibility.map(VisibilityArg::code).or(if item.is_none() { Some(2) } else { None }),
        tags: if tags.is_empty() { None } else { Some(tags) },
        change_note,
        preview_file: preview,
        ..Default::default()
    };
    let request = PublishRequest { app_id, item_id: item, content_dir: content, fields };
    let progress = move |stage: PublishStage, done: u64, total: u64| {
        if json {
            return;
        }
        match stage {
            PublishStage::Creating => cli_eprintln!("Creating the item..."),
            PublishStage::Preparing => cli_eprintln!("Asking Steam for an upload slot..."),
            PublishStage::Building => cli_eprintln!("Building the content (chunks, compression, encryption)..."),
            PublishStage::UploadingManifest => cli_eprintln!("Uploading the manifest ({total} bytes)..."),
            PublishStage::UploadingChunks(n, of) => cli_eprintln!("Uploaded {n}/{of} chunks ({done}/{total} bytes)"),
            PublishStage::Finishing => cli_eprintln!("Finishing the upload..."),
            PublishStage::Updating => cli_eprintln!("Updating the item..."),
            PublishStage::Done => {}
        }
    };
    let result = client.workshop_publish(&request, &progress).await?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "published_file_id": result.published_file_id,
            "manifest_id": result.manifest_id,
            "needs_legal_agreement": result.needs_legal_agreement,
            "status": if item.is_some() { "updated" } else { "published" },
        }));
    } else {
        cli_println!("{} Workshop item {} (manifest {}).", if item.is_some() { "Updated" } else { "Published" }, result.published_file_id, result.manifest_id);
        if result.needs_legal_agreement {
            cli_println!("Steam wants you to accept the Workshop legal agreement before the item can be public: https://steamcommunity.com/sharedfiles/workshoplegalagreement");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_delete(app_id: u32, id: u64, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.workshop_delete_item(app_id, id).await?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "published_file_id": id, "status": "deleted" }));
    } else {
        cli_println!("Deleted Workshop item {id}.");
    }
    Ok(())
}

pub(crate) async fn cmd_workshop_debug_manifest(app_id: u32, depot_id: u32, manifest_id: u64, out: std::path::PathBuf) -> Result<()> {
    let client = authed_client().await?;
    let bytes = client.debug_fetch_manifest(app_id, depot_id, manifest_id).await?;
    std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
    cli_println!("{} bytes in {}", bytes.len(), out.display());
    Ok(())
}

pub(crate) async fn cmd_workshop_mine(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let items = client.workshop_my_items(app_id).await?;
    if json {
        let list: Vec<_> = items.iter().map(|(id, title, vis)| serde_json::json!({ "id": id, "title": title, "visibility": vis })).collect();
        print_json(&list);
    } else if items.is_empty() {
        cli_println!("No published items for app {app_id}.");
    } else {
        for (id, title, vis) in items {
            cli_println!("{id}  visibility {vis}  {title}");
        }
    }
    Ok(())
}
