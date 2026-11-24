use super::*;
use super::workshop_build::{chunk_id_hex, BuiltChunk, DEFAULT_CHUNK_SIZE};
use sha1::{Digest, Sha1};
use steam_vent_proto::steammessages_clientserver_ucm::{
    CMsgClientUCMDeletePublishedFile, CMsgClientUCMDeletePublishedFileResponse, CMsgClientUCMPublishFile, CMsgClientUCMPublishFileResponse, CMsgClientUCMUpdatePublishedFile,
    CMsgClientUCMUpdatePublishedFileResponse,
};
use steam_vent_proto::steammessages_clientserver_ucm::cmsg_client_ucmupdate_published_file::{AdditionalPreview, KeyValueTag};
use steam_vent_proto::steammessages_cloud_steamclient::{CCloud_BeginHTTPUpload_Request, CCloud_BeginHTTPUpload_Response, CCloud_CommitHTTPUpload_Request, CCloud_CommitHTTPUpload_Response};
use steam_vent_proto::steammessages_depotbuilder_steamclient::{
    CContentBuilder_FinishDepotUpload_Request, CContentBuilder_FinishDepotUpload_Response, CContentBuilder_GetMissingDepotChunks_Request, CContentBuilder_GetMissingDepotChunks_Response,
    CContentBuilder_InitDepotBuild_Request, CContentBuilder_InitDepotBuild_Response, CContentBuilder_StartDepotUpload_Request, CContentBuilder_StartDepotUpload_Response,
};

pub const VISIBILITY_PUBLIC: i32 = 0;
pub const VISIBILITY_FRIENDS_ONLY: i32 = 1;
pub const VISIBILITY_PRIVATE: i32 = 2;
pub const VISIBILITY_UNLISTED: i32 = 3;

const UPLOAD_PARALLELISM: usize = 4;
const MAX_MISSING_ROUNDS: usize = 6;

#[derive(Debug, Clone, Default)]
pub struct ItemFields {
    pub title: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<i32>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<String>,
    pub language: Option<i32>,
    pub kv_tags: Vec<(String, String)>,
    pub removed_keys: Vec<String>,
    pub change_note: Option<String>,
    pub preview_file: Option<PathBuf>,
    pub previews: Vec<PreviewSpec>,
    pub previews_to_remove: Vec<i32>,
    pub game_branch_min: Option<String>,
    pub game_branch_max: Option<String>,
    pub descriptors_add: Vec<i32>,
    pub descriptors_remove: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewSpec {
    File { kind: u32, path: PathBuf, replaces: Option<i32> },
    Video { id: String, replaces: Option<i32> },
}

#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub app_id: u32,
    pub item_id: Option<u64>,
    pub content_dir: Option<PathBuf>,
    pub fields: ItemFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishStage {
    Creating,
    Preparing,
    Building,
    UploadingManifest,
    UploadingChunks(u32, u32),
    Finishing,
    Updating,
    Done,
}

pub type PublishProgress<'a> = &'a (dyn Fn(PublishStage, u64, u64) + Send + Sync);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishResult {
    pub published_file_id: u64,
    pub manifest_id: u64,
    pub needs_legal_agreement: bool,
}

fn eresult_error(what: &str, eresult: i32) -> anyhow::Error {
    anyhow!("{what} failed (EResult {eresult}){}", match eresult {
        2 => ": generic failure",
        8 => ": invalid parameter",
        9 => ": invalid state",
        15 => ": access denied (the account may not own the app or may not be allowed to publish)",
        16 => ": timeout",
        25 => ": limit exceeded",
        _ => "",
    })
}

fn unknown_string(message: &impl steam_vent_proto::protobuf::Message, field: u32) -> Option<String> {
    use steam_vent_proto::protobuf::UnknownValueRef;
    message.special_fields().unknown_fields().iter().find_map(|(n, v)| match v {
        UnknownValueRef::LengthDelimited(bytes) if n == field => std::str::from_utf8(bytes).ok().map(str::to_string).filter(|s| !s.is_empty()),
        _ => None,
    })
}

pub fn upload_hosts(init: &CContentBuilder_InitDepotBuild_Response) -> Option<(String, String)> {
    if let Ok(host) = std::env::var("AURELIA_WS_UPLOAD_HOST") {
        return Some((host.clone(), host));
    }
    let named = Some(init.url_host().to_string()).filter(|h| !h.is_empty());
    let manifest = unknown_string(init, 15).or_else(|| unknown_string(init, 16)).or_else(|| named.clone())?;
    let chunks = unknown_string(init, 16).or_else(|| unknown_string(init, 15)).or(named)?;
    Some((manifest, chunks))
}

fn manifest_body(built: &[u8]) -> Vec<u8> {
    if let Ok(path) = std::env::var("AURELIA_WS_MANIFEST_DUMP") {
        let _ = std::fs::write(path, built);
    }
    if let Some(bytes) = std::env::var("AURELIA_WS_MANIFEST_FILE").ok().and_then(|path| std::fs::read(path).ok()) {
        return bytes;
    }
    super::workshop_build::zip_compress(built).unwrap_or_else(|_| built.to_vec())
}

fn unknown_varint(message: &impl steam_vent_proto::protobuf::Message, field: u32) -> Option<u64> {
    use steam_vent_proto::protobuf::UnknownValueRef;
    message.special_fields().unknown_fields().iter().find_map(|(n, v)| match v {
        UnknownValueRef::Varint(value) if n == field => Some(value),
        _ => None,
    })
}

fn base_url(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    if host.starts_with("http://") || host.starts_with("https://") { host.to_string() } else { format!("https://{host}") }
}

pub fn chunk_upload_request(host: &str, app_id: u32, depot_id: u32, handle: u64, chunk: &BuiltChunk) -> (String, Vec<(&'static str, String)>) {
    (
        format!("{}/chunkupload/{}", base_url(host), chunk_id_hex(&chunk.sha)),
        vec![
            ("appid", app_id.to_string()),
            ("depotid", depot_id.to_string()),
            ("buildid", handle.to_string()),
            ("compressed_crc", chunk.payload_crc.to_string()),
            ("compressed_size", chunk.payload.len().to_string()),
        ],
    )
}

pub fn manifest_upload_request(host: &str, app_id: u32, handle: u64) -> (String, Vec<(&'static str, String)>) {
    (
        format!("{}/manifestupload", base_url(host)),
        vec![("appid", app_id.to_string()), ("buildid", handle.to_string()), ("can_poll_status", "1".to_string())],
    )
}

pub fn needs_split(fields: &ItemFields) -> bool {
    (!fields.previews.is_empty() || !fields.previews_to_remove.is_empty()) && (fields.game_branch_min.is_some() || fields.game_branch_max.is_some())
}

fn update_message(app_id: u32, item_id: u64, manifest_id: Option<u64>, fields: &ItemFields, preview_name: Option<&str>, uploaded: &[(String, String)]) -> CMsgClientUCMUpdatePublishedFile {
    let mut update = CMsgClientUCMUpdatePublishedFile::new();
    update.set_app_id(app_id);
    update.set_published_file_id(item_id);
    if let Some(manifest) = manifest_id {
        update.set_update_content_manifest(true);
        update.set_content_manifest(manifest);
    }
    if let Some(title) = &fields.title {
        update.set_update_title(true);
        update.set_title(title.clone());
    }
    if let Some(description) = &fields.description {
        update.set_update_description(true);
        update.set_description(description.clone());
    }
    if let Some(visibility) = fields.visibility {
        update.set_update_visibility(true);
        update.set_visibility(visibility);
    }
    if let Some(tags) = &fields.tags {
        update.set_update_tags(true);
        update.tags = tags.clone();
    }
    if let Some(metadata) = &fields.metadata {
        update.set_update_metadata(true);
        update.set_metadata(metadata.clone());
    }
    if let Some(language) = fields.language {
        update.set_language(language);
    }
    if let Some(note) = &fields.change_note {
        update.set_change_description(note.clone());
    }
    update.removed_kvtags = fields.removed_keys.clone();
    for (key, value) in &fields.kv_tags {
        let mut tag = KeyValueTag::new();
        tag.set_key(key.clone());
        tag.set_value(value.clone());
        update.kvtags.push(tag);
    }
    let mut files = uploaded.iter();
    for spec in &fields.previews {
        let mut extra = AdditionalPreview::new();
        match spec {
            PreviewSpec::File { kind, replaces, .. } => {
                let (internal, original) = files.next().cloned().unwrap_or_default();
                extra.set_internal_file_name(internal);
                extra.set_original_file_name(original);
                extra.set_preview_type(*kind);
                extra.set_update_index(replaces.unwrap_or(-1));
            }
            PreviewSpec::Video { id, replaces } => {
                extra.set_videoid(id.clone());
                extra.set_preview_type(1);
                extra.set_update_index(replaces.unwrap_or(-1));
            }
        }
        update.previews.push(extra);
    }
    update.previews_to_remove = fields.previews_to_remove.clone();
    if let Some(min) = &fields.game_branch_min {
        update.set_game_branch_min(min.clone());
    }
    if let Some(max) = &fields.game_branch_max {
        update.set_game_branch_max(max.clone());
    }
    update.content_descriptors_to_add = fields.descriptors_add.clone();
    update.content_descriptors_to_remove = fields.descriptors_remove.clone();
    if let Some(name) = preview_name {
        update.set_update_preview_file(true);
        update.set_preview_file_name(name.to_string());
    }
    update.set_clear_in_progress(true);
    update
}

pub fn preview_name(path: &Path) -> String {
    let base = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect()
}

impl SteamClient {
    pub async fn workshop_create_item(&self, app_id: u32, fields: &ItemFields) -> Result<(u64, bool)> {
        let connection = self.require_connection()?;
        let mut create = CMsgClientUCMPublishFile::new();
        create.set_app_id(app_id);
        create.set_consumer_app_id(app_id);
        create.set_workshop_file(true);
        create.set_file_type(0);
        create.set_in_progress(true);
        create.set_title(fields.title.clone().unwrap_or_default());
        create.set_description(fields.description.clone().unwrap_or_default());
        create.set_visibility(fields.visibility.unwrap_or(VISIBILITY_PRIVATE));
        create.tags = fields.tags.clone().unwrap_or_default();
        let response: CMsgClientUCMPublishFileResponse = connection.job(create).await.context("failed creating the Workshop item")?;
        if response.eresult() != 1 {
            return Err(eresult_error("creating the Workshop item", response.eresult()));
        }
        Ok((response.published_file_id(), response.needs_workshop_legal_agreement_acceptance()))
    }

    pub async fn workshop_upload_content(&self, app_id: u32, item_id: u64, dir: &Path, progress: PublishProgress<'_>) -> Result<u64> {
        let connection = self.require_connection()?;
        let depot_id = app_id;
        progress(PublishStage::Preparing, 0, 0);
        let mut init = CContentBuilder_InitDepotBuild_Request::new();
        init.set_appid(app_id);
        init.set_depotid(depot_id);
        init.set_workshop_itemid(item_id);
        if let Ok(v) = std::env::var("AURELIA_WS_LOCAL_CS") {
            init.set_for_local_cs(v == "1");
        }
        if let Some(d) = std::env::var("AURELIA_WS_DEPOT").ok().and_then(|d| d.parse::<u32>().ok()) {
            init.set_depotid(d);
        }
        let init: CContentBuilder_InitDepotBuild_Response = connection.service_method(init).await.context("ContentBuilder.InitDepotBuild failed")?;
        let key: [u8; 32] = init.aes_key().try_into().map_err(|_| anyhow!("Steam sent a depot key of {} bytes (expected 32)", init.aes_key().len()))?;
        let hosts = upload_hosts(&init);
        if let Ok(path) = std::env::var("AURELIA_WS_DUMP") {
            let raw = steam_vent_proto::protobuf::Message::write_to_bytes(&init).unwrap_or_default();
            let mut text = format!("InitDepotBuild bruto ({} bytes): {}\n", raw.len(), hex::encode(&raw[..raw.len().min(200)]));
            for (n, v) in init.special_fields.unknown_fields().iter() {
                text.push_str(&format!("  campo desconhecido {n}: {v:?}\n"));
            }
            let _ = std::fs::write(path, text);
        }
        let Some((manifest_host, chunk_host)) = hosts else {
            bail!(
                "Steam did not name an upload host (InitDepotBuild: baseline {}, chunk size {}, aes key {} bytes, rsa key {} bytes)",
                init.baseline_manifestid(), init.chunk_size(), init.aes_key().len(), init.rsa_key().len()
            );
        };
        let chunk_size = if init.chunk_size() == 0 { DEFAULT_CHUNK_SIZE } else { init.chunk_size() as usize };

        progress(PublishStage::Building, 0, 0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        let dir = dir.to_path_buf();
        let rsa_key = init.rsa_key().to_vec();
        let method = std::env::var("AURELIA_WS_METHOD").ok().and_then(|m| m.parse().ok()).unwrap_or_else(|| unknown_varint(&init, 12).unwrap_or(7) as u32);
        let compression = super::workshop_build::Compression::from_method(method as u32);
        let built = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let mut built = super::workshop_build::build_depot_with(&dir, depot_id, key, chunk_size, now, compression)?;
            if !rsa_key.is_empty() {
                built.manifest = super::workshop_build::sign_manifest(&built.manifest, &rsa_key)?;
            }
            Ok(built)
        })
        .await
        .context("build task failed")??;

        let mut start = CContentBuilder_StartDepotUpload_Request::new();
        start.set_appid(app_id);
        start.set_depotid(depot_id);
        start.set_workshop_itemid(item_id);
        start.set_baseline_manifestid(init.baseline_manifestid());
        let manifest_bytes = manifest_body(&built.manifest);
        start.set_manifest_size(std::env::var("AURELIA_WS_DECLARED_SIZE").ok().and_then(|n| n.parse().ok()).unwrap_or(manifest_bytes.len() as u32));
        let start: CContentBuilder_StartDepotUpload_Response = connection.service_method(start).await.context("ContentBuilder.StartDepotUpload failed")?;
        let handle = start.depot_build_handle();

        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).http1_only().user_agent("Valve/Steam HTTP Client 1.0").build().context("building the HTTP client")?;
        progress(PublishStage::UploadingManifest, 0, manifest_bytes.len() as u64);
        let (url, query) = manifest_upload_request(&manifest_host, app_id, handle);
        let reply = http
            .post(&url)
            .query(&query)
            .header("Content-Type", "application/octet-stream")
            .body(manifest_bytes.clone())
            .send()
            .await
            .with_context(|| format!("manifest upload to {url} failed"))?;
        let status = reply.status();
        let headers = format!("{:?}", reply.headers());
        if !status.is_success() {
            let body = reply.text().await.unwrap_or_default();
            bail!("manifest upload to {url} answered HTTP {status} ({headers}): {}", body.chars().take(200).collect::<String>());
        }
        if let Ok(path) = std::env::var("AURELIA_WS_DUMP") {
            let body = reply.text().await.unwrap_or_default();
            let _ = std::fs::OpenOptions::new().append(true).create(true).open(path).and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "manifestupload: HTTP {status}\n  headers: {headers}\n  body ({} bytes): {body}", body.len())
            });
        }

        let by_sha: std::collections::HashMap<Vec<u8>, &BuiltChunk> = built.chunks.iter().map(|c| (c.sha.to_vec(), c)).collect();
        let mut sent_bytes = 0u64;
        let mut sent_chunks = 0u32;
        for _ in 0..MAX_MISSING_ROUNDS {
            let mut ask = CContentBuilder_GetMissingDepotChunks_Request::new();
            ask.set_appid(app_id);
            ask.set_depot_build_handle(handle);
            let missing: CContentBuilder_GetMissingDepotChunks_Response = connection.service_method(ask).await.context("ContentBuilder.GetMissingDepotChunks failed")?;
            if missing.missing_chunks.is_empty() {
                break;
            }
            let total = missing.total_missing_chunks().max(missing.missing_chunks.len() as u32) + sent_chunks;
            let wanted: Vec<&BuiltChunk> = missing
                .missing_chunks
                .iter()
                .map(|m| by_sha.get(m.sha()).copied().ok_or_else(|| anyhow!("Steam asked for a chunk that is not part of the build ({})", hex::encode(m.sha()))))
                .collect::<Result<_>>()?;
            let upload_total: u64 = wanted.iter().map(|c| c.payload.len() as u64).sum::<u64>() + sent_bytes;
            for batch in wanted.chunks(UPLOAD_PARALLELISM) {
                let mut sends = tokio::task::JoinSet::new();
                for chunk in batch {
                    let (url, query) = chunk_upload_request(&chunk_host, app_id, depot_id, handle, chunk);
                    let (http, payload) = (http.clone(), chunk.payload.clone());
                    sends.spawn(async move {
                        let len = payload.len();
                        let reply = http.put(&url).query(&query).header("Content-Type", "application/octet-stream").body(payload).send().await.with_context(|| format!("chunk upload to {url} failed"))?;
                        let status = reply.status();
                        if !status.is_success() {
                            let body = reply.text().await.unwrap_or_default();
                            bail!("chunk upload to {url} answered HTTP {status}: {}", body.chars().take(200).collect::<String>());
                        }
                        Ok::<usize, anyhow::Error>(len)
                    });
                }
                while let Some(done) = sends.join_next().await {
                    sent_bytes += done.context("chunk upload task failed")?? as u64;
                    sent_chunks += 1;
                }
                progress(PublishStage::UploadingChunks(sent_chunks, total), sent_bytes, upload_total);
            }
        }

        progress(PublishStage::Finishing, sent_bytes, sent_bytes);
        let mut finish = CContentBuilder_FinishDepotUpload_Request::new();
        finish.set_appid(app_id);
        finish.set_depot_build_handle(handle);
        let finish: CContentBuilder_FinishDepotUpload_Response = connection.service_method(finish).await.context("ContentBuilder.FinishDepotUpload failed")?;
        if finish.manifestid() == 0 {
            bail!("Steam finished the upload without naming a manifest");
        }
        Ok(finish.manifestid())
    }

    pub async fn workshop_upload_preview(&self, app_id: u32, file: &Path) -> Result<String> {
        let connection = self.require_connection()?;
        let data = std::fs::read(file).with_context(|| format!("reading the preview {}", file.display()))?;
        if data.is_empty() {
            bail!("the preview {} is empty", file.display());
        }
        let name = preview_name(file);
        let sha = hex::encode(Sha1::digest(&data));
        let mut begin = CCloud_BeginHTTPUpload_Request::new();
        begin.set_appid(app_id);
        begin.set_file_size(u32::try_from(data.len()).context("the preview is larger than 4 GiB")?);
        begin.set_filename(name.clone());
        begin.set_file_sha(sha.clone());
        begin.set_is_public(true);
        begin.platforms_to_sync.push("all".to_string());
        let begin: CCloud_BeginHTTPUpload_Response = connection.service_method(begin).await.context("Cloud.BeginHTTPUpload failed")?;
        let scheme = if begin.use_https() { "https" } else { "http" };
        let url = format!("{scheme}://{}{}", begin.url_host(), begin.url_path());
        let mut put = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build()?.put(&url);
        for header in &begin.request_headers {
            put = put.header(header.name(), header.value());
        }
        let sent = put.body(data).send().await;
        let ok = matches!(&sent, Ok(r) if r.status().is_success());
        let mut commit = CCloud_CommitHTTPUpload_Request::new();
        commit.set_transfer_succeeded(ok);
        commit.set_appid(app_id);
        commit.set_file_sha(sha);
        commit.set_filename(name.clone());
        let commit: CCloud_CommitHTTPUpload_Response = connection.service_method(commit).await.context("Cloud.CommitHTTPUpload failed")?;
        match sent {
            Err(e) => return Err(anyhow!("preview upload to {url} failed: {e}")),
            Ok(r) if !r.status().is_success() => bail!("preview upload to {url} answered HTTP {}", r.status()),
            Ok(_) => {}
        }
        if !commit.file_committed() {
            bail!("Steam did not commit the preview {name}");
        }
        Ok(name)
    }

    pub async fn workshop_update_item(&self, app_id: u32, item_id: u64, manifest_id: Option<u64>, fields: &ItemFields, preview: Option<&str>, uploaded: &[(String, String)]) -> Result<bool> {
        let mut legal = false;
        if needs_split(fields) {
            let mut first = fields.clone();
            first.game_branch_min = None;
            first.game_branch_max = None;
            legal |= self.send_update(update_message(app_id, item_id, manifest_id, &first, preview, uploaded)).await?;
            let versions = ItemFields { game_branch_min: fields.game_branch_min.clone(), game_branch_max: fields.game_branch_max.clone(), ..ItemFields::default() };
            legal |= self.send_update(update_message(app_id, item_id, None, &versions, None, &[])).await?;
        } else {
            legal |= self.send_update(update_message(app_id, item_id, manifest_id, fields, preview, uploaded)).await?;
        }
        Ok(legal)
    }

    async fn send_update(&self, message: CMsgClientUCMUpdatePublishedFile) -> Result<bool> {
        let connection = self.require_connection()?;
        let response: CMsgClientUCMUpdatePublishedFileResponse = connection.job(message).await.context("failed updating the Workshop item")?;
        if response.eresult() != 1 {
            return Err(eresult_error("updating the Workshop item", response.eresult()));
        }
        Ok(response.needs_workshop_legal_agreement_acceptance())
    }

    pub async fn workshop_publish(&self, request: &PublishRequest, progress: PublishProgress<'_>) -> Result<PublishResult> {
        let (item_id, mut legal) = match request.item_id {
            Some(id) => (id, false),
            None => {
                progress(PublishStage::Creating, 0, 0);
                self.workshop_create_item(request.app_id, &request.fields).await?
            }
        };
        let manifest = match &request.content_dir {
            Some(dir) => Some(
                self.workshop_upload_content(request.app_id, item_id, dir, progress)
                    .await
                    .with_context(|| format!("the item {item_id} was created and is still in progress (delete it with `aurelia workshop delete {} {item_id}`)", request.app_id))?,
            ),
            None => None,
        };
        let preview = match &request.fields.preview_file {
            Some(file) => Some(self.workshop_upload_preview(request.app_id, file).await?),
            None => None,
        };
        let mut uploaded = Vec::new();
        for spec in &request.fields.previews {
            if let PreviewSpec::File { path, .. } = spec {
                let internal = self.workshop_upload_preview(request.app_id, path).await?;
                uploaded.push((internal, preview_name(path)));
            }
        }
        progress(PublishStage::Updating, 0, 0);
        legal |= self.workshop_update_item(request.app_id, item_id, manifest, &request.fields, preview.as_deref(), &uploaded).await?;
        progress(PublishStage::Done, 0, 0);
        Ok(PublishResult { published_file_id: item_id, manifest_id: manifest.unwrap_or(0), needs_legal_agreement: legal })
    }

    pub async fn debug_fetch_manifest(&self, app_id: u32, depot_id: u32, manifest_id: u64) -> Result<Vec<u8>> {
        use steam_vent_proto::steammessages_contentsystem_steamclient::{CContentServerDirectory_GetManifestRequestCode_Request, CContentServerDirectory_GetManifestRequestCode_Response};
        let connection = self.require_connection()?;
        let mut ask = CContentServerDirectory_GetManifestRequestCode_Request::new();
        ask.set_app_id(app_id);
        ask.set_depot_id(depot_id);
        ask.set_manifest_id(manifest_id);
        let code: CContentServerDirectory_GetManifestRequestCode_Response = connection.service_method(ask).await.context("GetManifestRequestCode failed")?;
        let url = format!("https://steampipe.akamaized.net/depot/{depot_id}/manifest/{manifest_id}/5/{}", code.manifest_request_code());
        let reply = reqwest::Client::new().get(&url).send().await.with_context(|| format!("GET {url}"))?;
        if !reply.status().is_success() {
            bail!("GET {url} answered HTTP {}", reply.status());
        }
        Ok(reply.bytes().await?.to_vec())
    }

    pub async fn workshop_my_items(&self, app_id: u32) -> Result<Vec<(u64, String, i32)>> {
        use steam_vent_proto::steammessages_publishedfile_steamclient::{CPublishedFile_GetUserFiles_Request, CPublishedFile_GetUserFiles_Response};
        let connection = self.require_connection()?;
        let steam_id = self.steam_id().context("not logged in")?;
        let mut out = Vec::new();
        for page in 1..=20u32 {
            let mut ask = CPublishedFile_GetUserFiles_Request::new();
            ask.set_steamid(steam_id);
            ask.set_appid(app_id);
            ask.set_page(page);
            ask.set_numperpage(100);
            ask.set_type("myfiles".to_string());
            ask.set_return_vote_data(false);
            let reply: CPublishedFile_GetUserFiles_Response = connection.service_method(ask).await.context("PublishedFile.GetUserFiles failed")?;
            let got = reply.publishedfiledetails.len();
            out.extend(reply.publishedfiledetails.iter().map(|d| (d.publishedfileid(), d.title().to_string(), d.visibility() as i32)));
            if got < 100 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn workshop_delete_item(&self, app_id: u32, item_id: u64) -> Result<()> {
        let connection = self.require_connection()?;
        let mut request = CMsgClientUCMDeletePublishedFile::new();
        request.set_app_id(app_id);
        request.set_published_file_id(item_id);
        let response: CMsgClientUCMDeletePublishedFileResponse = connection.job(request).await.context("failed deleting the Workshop item")?;
        if response.eresult() != 1 {
            return Err(eresult_error("deleting the Workshop item", response.eresult()));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "workshop_upload_tests.rs"]
mod tests;
