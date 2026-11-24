use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::steam_client::{RemoteNamespace, SteamClient};

const KEY_PREFIX: &str = "user-collections.";

const BUILTIN_IDS: &[&str] = &["favorite", "hidden"];

fn builtin_default_name(id: &str) -> Option<&'static str> {
    match id {
        "favorite" => Some("Favorites"),
        "hidden" => Some("Hidden"),
        _ => None,
    }
}

static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

type PushSnapshot = Vec<(String, Vec<(u32, bool)>)>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub added: Vec<u32>,
    #[serde(default)]
    pub removed: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_spec: Option<Value>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<u32>,
}

impl Collection {
    pub fn is_dynamic(&self) -> bool {
        self.filter_spec.is_some()
    }

    pub fn is_builtin(&self) -> bool {
        BUILTIN_IDS.contains(&self.id.as_str())
    }

    pub fn contains(&self, app_id: u32) -> bool {
        self.added.contains(&app_id) && !self.removed.contains(&app_id)
    }

    pub fn to_entry(&self) -> (String, Option<String>) {
        let key = format!("{KEY_PREFIX}{}", self.id);
        if self.deleted {
            return (key, None);
        }
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), Value::from(self.id.clone()));
        obj.insert("name".into(), Value::from(self.name.clone()));
        obj.insert("added".into(), Value::from(self.added.clone()));
        obj.insert("removed".into(), Value::from(self.removed.clone()));
        if let Some(filter) = &self.filter_spec {
            obj.insert("filterSpec".into(), filter.clone());
        }
        let value = Value::Object(obj).to_string();
        (key, Some(value))
    }

    pub fn from_entry(key: &str, value_json: &str) -> Result<Collection> {
        let v: Value = serde_json::from_str(value_json)
            .with_context(|| format!("collection entry {key} has invalid JSON value"))?;
        let key_id = key.strip_prefix(KEY_PREFIX).unwrap_or(key).to_string();
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .unwrap_or(key_id);
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(Collection {
            id,
            name,
            added: parse_app_ids(v.get("added")),
            removed: parse_app_ids(v.get("removed")),
            filter_spec: v.get("filterSpec").cloned(),
            deleted: false,
            pending: Vec::new(),
        })
    }
}

fn parse_app_ids(value: Option<&Value>) -> Vec<u32> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u32))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollectionsStore {
    #[serde(default)]
    pub namespace_version: u64,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub synced: bool,
}

pub const STORE_FORMAT: u32 = 1;

fn membership(added: &[u32], removed: &[u32], app: u32) -> Option<bool> {
    if removed.contains(&app) {
        Some(false)
    } else if added.contains(&app) {
        Some(true)
    } else {
        None
    }
}

fn store_path() -> Result<std::path::PathBuf> {
    Ok(crate::core::config::config_dir()?.join("collections.json"))
}

impl CollectionsStore {
    pub fn load() -> Result<CollectionsStore> {
        let path = store_path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("failed parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CollectionsStore {
                format: STORE_FORMAT,
                ..CollectionsStore::default()
            }),
            Err(e) => Err(e).with_context(|| format!("failed reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = store_path()?;
        let text = serde_json::to_string_pretty(self).context("failed serializing collections")?;
        crate::core::atomic::write_atomic(&path, text.as_bytes())
    }

    pub fn edit<T>(f: impl FnOnce(&mut CollectionsStore) -> Result<T>) -> Result<(T, Self)> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Self::load()?;
        let out = f(&mut store)?;
        store.save()?;
        Ok((out, store))
    }

    pub fn resolve(&self, name_or_id: &str) -> Result<&Collection> {
        let idx = self.resolve_index(name_or_id)?;
        Ok(&self.collections[idx])
    }

    fn resolve_index(&self, name_or_id: &str) -> Result<usize> {
        if let Some(i) = self
            .collections
            .iter()
            .position(|c| !c.deleted && c.id == name_or_id)
        {
            return Ok(i);
        }
        let wanted = name_or_id.to_ascii_lowercase();
        let matches: Vec<usize> = self
            .collections
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.deleted && c.name.to_ascii_lowercase() == wanted)
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [] => bail!("no collection named or with id '{name_or_id}'"),
            [i] => Ok(*i),
            _ => bail!(
                "'{name_or_id}' is ambiguous: {} collections share that name — use the id instead",
                matches.len()
            ),
        }
    }

    fn resolve_or_create_builtin(&mut self, name_or_id: &str) -> Result<usize> {
        match self.resolve_index(name_or_id) {
            Ok(i) => Ok(i),
            Err(e) => {
                let Some(name) = builtin_default_name(name_or_id) else {
                    return Err(e);
                };
                self.collections.push(Collection {
                    id: name_or_id.to_string(),
                    name: name.to_string(),
                    added: Vec::new(),
                    removed: Vec::new(),
                    filter_spec: None,
                    deleted: false,
                    pending: Vec::new(),
                });
                Ok(self.collections.len() - 1)
            }
        }
    }

    pub fn create(&mut self, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("collection name must not be empty");
        }
        let id = format!("uc-{:08x}", rand::random::<u32>());
        self.collections.push(Collection {
            id: id.clone(),
            name: name.to_string(),
            added: Vec::new(),
            removed: Vec::new(),
            filter_spec: None,
            deleted: false,
            pending: Vec::new(),
        });
        Ok(id)
    }

    pub fn delete(&mut self, name_or_id: &str) -> Result<()> {
        let idx = self.resolve_index(name_or_id)?;
        let c = &self.collections[idx];
        if c.is_builtin() {
            bail!(
                "'{}' is a built-in collection and can't be deleted — remove its games instead",
                c.name
            );
        }
        if c.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; edit it in the Steam client",
                c.name
            );
        }
        self.collections[idx].deleted = true;
        Ok(())
    }

    pub fn rename(&mut self, name_or_id: &str, new_name: &str) -> Result<()> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            bail!("new collection name must not be empty");
        }
        let idx = self.resolve_index(name_or_id)?;
        self.collections[idx].name = new_name.to_string();
        Ok(())
    }

    pub fn add(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        self.set_membership(name_or_id, app_ids, true)
    }

    pub fn remove(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        self.set_membership(name_or_id, app_ids, false)
    }

    fn set_membership(&mut self, name_or_id: &str, app_ids: &[u32], member: bool) -> Result<()> {
        let idx = self.resolve_or_create_builtin(name_or_id)?;
        self.ensure_static(idx)?;
        let c = &mut self.collections[idx];
        for &app in app_ids {
            let (keep, drop) = if member {
                (&mut c.added, &mut c.removed)
            } else {
                (&mut c.removed, &mut c.added)
            };
            drop.retain(|&x| x != app);
            if !keep.contains(&app) {
                keep.push(app);
            }
            if !c.pending.contains(&app) {
                c.pending.push(app);
            }
        }
        Ok(())
    }

    fn ensure_static(&self, idx: usize) -> Result<()> {
        let c = &self.collections[idx];
        if c.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; its membership is computed by \
                 Steam and can't be edited here — manage it in the Steam client",
                c.name
            );
        }
        Ok(())
    }

    pub fn has_remote_baseline(&self) -> bool {
        self.synced || self.namespace_version > 0
    }

    pub fn pending_deletions(&self) -> usize {
        self.collections.iter().filter(|c| c.deleted).count()
    }

    pub fn check_push_allowed(&self, force: bool) -> Result<()> {
        if force || self.has_remote_baseline() {
            return Ok(());
        }
        bail!(
            "refusing to push: local collections were never pulled from Steam. \
             Run `aurelia collections pull` first, or pass `--force` to upload anyway."
        );
    }

    fn apply_remote(&mut self, remote: RemoteNamespace) {
        for (key, value) in remote.entries {
            let id = key.strip_prefix(KEY_PREFIX).unwrap_or(&key).to_string();
            match value {
                None => self.collections.retain(|c| c.id != id),
                Some(value_json) => {
                    let incoming = match Collection::from_entry(&key, &value_json) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!("skipping unparseable collection {key}: {e:#}");
                            continue;
                        }
                    };
                    let legacy = self.format < STORE_FORMAT;
                    match self.collections.iter_mut().find(|c| c.id == incoming.id) {
                        Some(local) => {
                            if legacy {
                                seed_pending(local, &incoming);
                            }
                            let pending = &local.pending;
                            let merge = |remote: &[u32], local: &[u32]| -> Vec<u32> {
                                let mut out: Vec<u32> = remote
                                    .iter()
                                    .copied()
                                    .filter(|a| !pending.contains(a))
                                    .collect();
                                out.extend(local.iter().copied().filter(|a| pending.contains(a)));
                                out
                            };
                            let added = merge(&incoming.added, &local.added);
                            let removed = merge(&incoming.removed, &local.removed);
                            local.added = added;
                            local.removed = removed;
                            local.name = incoming.name;
                            local.filter_spec = incoming.filter_spec;
                        }
                        None => self.collections.push(incoming),
                    }
                }
            }
        }
        self.namespace_version = remote.version;
        self.format = STORE_FORMAT;
        self.synced = true;
    }

    fn push_snapshot(&self) -> (Vec<(String, Option<String>)>, PushSnapshot) {
        let entries = self.collections.iter().map(Collection::to_entry).collect();
        let snapshot = self
            .collections
            .iter()
            .map(|c| {
                let state = c.pending.iter().map(|&a| (a, c.contains(a))).collect();
                (c.id.clone(), state)
            })
            .collect();
        (entries, snapshot)
    }

    fn finish_push(&mut self, snapshot: &PushSnapshot, deleted: &[String], new_version: u64) {
        for c in &mut self.collections {
            let Some((_, sent)) = snapshot.iter().find(|(id, _)| *id == c.id) else {
                continue;
            };
            let current: Vec<(u32, bool)> = c.pending.iter().map(|&a| (a, c.contains(a))).collect();
            c.pending = current
                .into_iter()
                .filter(|entry| !sent.contains(entry))
                .map(|(a, _)| a)
                .collect();
        }
        self.collections
            .retain(|c| !(c.deleted && deleted.contains(&c.id)));
        self.namespace_version = new_version;
        self.format = STORE_FORMAT;
        self.synced = true;
    }
}

fn is_version_conflict(e: &anyhow::Error) -> bool {
    let text = format!("{e:#}").to_ascii_lowercase();
    text.contains("conflict") || text.contains("version") || text.contains("out of date")
}

fn upload_error(e: anyhow::Error) -> anyhow::Error {
    if is_version_conflict(&e) {
        anyhow::anyhow!(
            "Steam has newer collections; run `aurelia collections pull` first, then push again. ({e:#})"
        )
    } else {
        e.context("failed uploading collections to Steam")
    }
}

fn seed_pending(local: &mut Collection, remote: &Collection) {
    let explicit: Vec<u32> = local.added.iter().chain(&local.removed).copied().collect();
    for app in explicit {
        let mine = membership(&local.added, &local.removed, app);
        let theirs = membership(&remote.added, &remote.removed, app);
        if mine != theirs && !local.pending.contains(&app) {
            local.pending.push(app);
        }
    }
}

pub async fn pull(client: &SteamClient) -> Result<CollectionsStore> {
    let remote = client
        .download_collections()
        .await
        .context("failed downloading collections from Steam")?;
    let ((), store) = CollectionsStore::edit(|store| {
        store.apply_remote(remote);
        Ok(())
    })?;
    Ok(store)
}

pub async fn push(client: &SteamClient, force: bool) -> Result<CollectionsStore> {
    let store = {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        CollectionsStore::load()?
    };
    store.check_push_allowed(force)?;
    let (entries, snapshot) = store.push_snapshot();
    let deleted: Vec<String> = store
        .collections
        .iter()
        .filter(|c| c.deleted)
        .map(|c| c.id.clone())
        .collect();

    let new_version = client
        .upload_collections(store.namespace_version, entries)
        .await
        .map_err(upload_error)?;

    let ((), store) = CollectionsStore::edit(|store| {
        store.finish_push(&snapshot, &deleted, new_version);
        Ok(())
    })?;
    Ok(store)
}

pub async fn sync(client: &SteamClient) -> Result<CollectionsStore> {
    pull(client).await?;
    push(client, false).await
}

#[cfg(test)]
#[path = "collections_tests.rs"]
mod tests;
