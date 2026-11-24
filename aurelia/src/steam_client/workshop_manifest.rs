use crate::core::utils::extract_quoted_values;
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledWorkshopItem {
    pub published_file_id: u64,
    pub manifest_id: u64,
    pub size: u64,
    pub time_updated: i64,
}

pub(crate) fn workshop_manifest_path(library_root: &Path, app_id: u32) -> PathBuf {
    library_root
        .join("steamapps")
        .join("workshop")
        .join(format!("appworkshop_{app_id}.acf"))
}

pub(crate) fn workshop_content_dir(
    library_root: &Path,
    app_id: u32,
    published_file_id: u64,
) -> PathBuf {
    library_root
        .join("steamapps")
        .join("workshop")
        .join("content")
        .join(app_id.to_string())
        .join(published_file_id.to_string())
}

pub(crate) fn read_workshop_manifest(path: &Path) -> Result<Vec<InstalledWorkshopItem>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed reading workshop manifest {}", path.display()))?;
    Ok(parse_installed_items(&raw))
}

pub(crate) fn upsert_installed_item(
    path: &Path,
    app_id: u32,
    item: InstalledWorkshopItem,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating directory {}", parent.display()))?;
    }

    let mut items = read_workshop_manifest(path)?;

    if let Some(existing) = items
        .iter_mut()
        .find(|i| i.published_file_id == item.published_file_id)
    {
        *existing = item;
    } else {
        items.push(item);
    }

    write_workshop_manifest(path, app_id, &items)
}

pub(crate) fn remove_installed_item(
    path: &Path,
    app_id: u32,
    published_file_id: u64,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut items = read_workshop_manifest(path)?;
    let before = items.len();
    items.retain(|i| i.published_file_id != published_file_id);

    if items.len() == before {
        return Ok(());
    }

    write_workshop_manifest(path, app_id, &items)
}

fn write_workshop_manifest(
    path: &Path,
    app_id: u32,
    items: &[InstalledWorkshopItem],
) -> Result<()> {
    let size_on_disk: u64 = items.iter().map(|i| i.size).sum();

    let mut content = format!(
        "\"AppWorkshop\"\n{{\n\
         \t\"appid\"\t\t\"{app_id}\"\n\
         \t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\
         \t\"NeedsUpdate\"\t\t\"0\"\n\
         \t\"NeedsDownload\"\t\t\"0\"\n"
    );

    content.push_str("\t\"WorkshopItemsInstalled\"\n\t{\n");
    for item in items {
        write_item_block(&mut content, item.published_file_id, |buf| {
            let _ = write!(buf, "\t\t\t\"size\"\t\t\"{}\"\n", item.size);
            let _ = write!(buf, "\t\t\t\"timeupdated\"\t\t\"{}\"\n", item.time_updated);
            let _ = write!(buf, "\t\t\t\"manifest\"\t\t\"{}\"\n", item.manifest_id);
        });
    }
    content.push_str("\t}\n");

    content.push_str("\t\"WorkshopItemDetails\"\n\t{\n");
    for item in items {
        write_item_block(&mut content, item.published_file_id, |buf| {
            let _ = write!(buf, "\t\t\t\"manifest\"\t\t\"{}\"\n", item.manifest_id);
            let _ = write!(buf, "\t\t\t\"timeupdated\"\t\t\"{}\"\n", item.time_updated);
            let _ = write!(buf, "\t\t\t\"timetouched\"\t\t\"{}\"\n", item.time_updated);
        });
    }
    content.push_str("\t}\n}\n");

    std::fs::write(path, content)
        .with_context(|| format!("failed writing workshop manifest {}", path.display()))?;
    Ok(())
}

fn write_item_block(buf: &mut String, published_file_id: u64, fields: impl FnOnce(&mut String)) {
    let _ = write!(buf, "\t\t\"{published_file_id}\"\n\t\t{{\n");
    fields(buf);
    buf.push_str("\t\t}\n");
}

fn parse_installed_items(raw: &str) -> Vec<InstalledWorkshopItem> {
    #[derive(Debug, PartialEq)]
    enum Section {
        Other,
        ItemsInstalled,
        ItemDetails,
    }

    let mut items_installed: std::collections::HashMap<u64, ItemInstalled> =
        std::collections::HashMap::new();
    let mut items_details: std::collections::HashMap<u64, ItemDetails> =
        std::collections::HashMap::new();

    let mut section = Section::Other;
    let mut depth: u32 = 0;
    let mut current_id: Option<u64> = None;

    for line in raw.lines() {
        let trimmed = line.trim();

        if depth == 0 {
            let quoted = extract_quoted_values(trimmed);
            if quoted.len() == 1 {
                match quoted[0].as_str() {
                    "WorkshopItemsInstalled" => {
                        section = Section::ItemsInstalled;
                        continue;
                    }
                    "WorkshopItemDetails" => {
                        section = Section::ItemDetails;
                        continue;
                    }
                    _ => {}
                }
            }
        }

        if section == Section::Other {
            continue;
        }

        if trimmed == "{" {
            depth += 1;
            continue;
        }

        if trimmed == "}" {
            if depth == 0 {
                section = Section::Other;
                current_id = None;
                continue;
            }
            depth -= 1;
            if depth == 0 {
                section = Section::Other;
                current_id = None;
            } else if depth == 1 {
                current_id = None;
            }
            continue;
        }

        let quoted = extract_quoted_values(trimmed);
        if depth == 1 && quoted.len() == 1 {
            if let Ok(id) = quoted[0].parse::<u64>() {
                current_id = Some(id);
            }
            continue;
        }

        if depth == 2 {
            let Some(id) = current_id else { continue };
            if quoted.len() >= 2 {
                let key = quoted[0].as_str();
                let val = &quoted[1];
                match section {
                    Section::ItemsInstalled => {
                        items_installed.entry(id).or_default().apply_field(key, val);
                    }
                    Section::ItemDetails => {
                        items_details.entry(id).or_default().apply_field(key, val);
                    }
                    Section::Other => {}
                }
            }
        }
    }

    let mut all_ids: Vec<u64> = items_installed.keys().copied().collect();
    all_ids.extend(
        items_details
            .keys()
            .filter(|id| !items_installed.contains_key(id)),
    );
    all_ids.sort_unstable();

    all_ids
        .into_iter()
        .map(|id| {
            let inst = items_installed.get(&id);
            let det = items_details.get(&id);
            InstalledWorkshopItem {
                published_file_id: id,
                manifest_id: inst
                    .map(|i| i.manifest_id)
                    .or_else(|| det.map(|d| d.manifest_id))
                    .unwrap_or(0),
                size: inst.map_or(0, |i| i.size),
                time_updated: inst
                    .map(|i| i.time_updated)
                    .or_else(|| det.map(|d| d.time_updated))
                    .unwrap_or(0),
            }
        })
        .collect()
}

#[derive(Default)]
struct ItemInstalled {
    size: u64,
    time_updated: i64,
    manifest_id: u64,
}

impl ItemInstalled {
    fn apply_field(&mut self, key: &str, val: &str) {
        match key {
            "size" => self.size = val.parse().unwrap_or(self.size),
            "timeupdated" => self.time_updated = val.parse().unwrap_or(self.time_updated),
            "manifest" => self.manifest_id = val.parse().unwrap_or(self.manifest_id),
            _ => {}
        }
    }
}

#[derive(Default)]
struct ItemDetails {
    manifest_id: u64,
    time_updated: i64,
}

impl ItemDetails {
    fn apply_field(&mut self, key: &str, val: &str) {
        match key {
            "manifest" => self.manifest_id = val.parse().unwrap_or(self.manifest_id),
            "timeupdated" => self.time_updated = val.parse().unwrap_or(self.time_updated),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "workshop_manifest_tests.rs"]
mod tests;
