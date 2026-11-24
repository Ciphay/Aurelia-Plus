pub const SLOT_UGC: u16 = 10;

pub const OP_LIST: u8 = 1;
pub const OP_STATE: u8 = 2;
pub const OP_DOWNLOAD: u8 = 3;
pub const OP_SUBSCRIBE: u8 = 4;
pub const OP_UNSUBSCRIBE: u8 = 5;
pub const OP_CREATE: u8 = 6;
pub const OP_SUBMIT: u8 = 7;
pub const OP_PROGRESS: u8 = 8;
pub const OP_DELETE: u8 = 9;
pub const OP_SERVER_FOLDER: u8 = 10;
pub const OP_SET_BRANCH: u8 = 11;
pub const OP_OWNS: u8 = 12;

pub fn parse_request(p: &[u8]) -> Option<(u8, u32, u64)> {
    if p.len() < 13 {
        return None;
    }
    Some((p[0], u32::from_le_bytes(p[1..5].try_into().ok()?), u64::from_le_bytes(p[5..13].try_into().ok()?)))
}

pub fn encode_list(ids: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 8 * ids.len());
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

pub fn encode_state(installed: bool, size: u64, time_updated: u32, folder: &str) -> Vec<u8> {
    let path = folder.as_bytes();
    let mut out = Vec::with_capacity(15 + path.len());
    out.push(installed as u8);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&time_updated.to_le_bytes());
    out.extend_from_slice(&(path.len().min(u16::MAX as usize) as u16).to_le_bytes());
    out.extend_from_slice(&path[..path.len().min(u16::MAX as usize)]);
    out
}

pub fn request_body(p: &[u8]) -> &[u8] {
    p.get(13..).unwrap_or(&[])
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct SubmitSpec {
    pub title: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<i32>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<String>,
    pub language: Option<String>,
    pub content: Option<String>,
    pub preview: Option<String>,
    pub change_note: Option<String>,
    #[serde(default)]
    pub kv_tags: Vec<(String, String)>,
    #[serde(default)]
    pub removed_keys: Vec<String>,
    pub item: Option<u64>,
    #[serde(default)]
    pub previews: Vec<PreviewJson>,
    #[serde(default)]
    pub previews_to_remove: Vec<i32>,
    pub game_branch_min: Option<String>,
    pub game_branch_max: Option<String>,
    #[serde(default)]
    pub descriptors_add: Vec<i32>,
    #[serde(default)]
    pub descriptors_remove: Vec<i32>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct PreviewJson {
    pub file: Option<String>,
    pub video: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: u32,
    pub replaces: Option<i32>,
}

pub fn language_code(name: &str) -> Option<i32> {
    const NAMES: [&str; 29] = [
        "english", "german", "french", "italian", "korean", "spanish", "schinese", "tchinese", "russian", "thai", "japanese", "portuguese", "polish", "danish", "dutch", "finnish",
        "norwegian", "swedish", "hungarian", "czech", "romanian", "turkish", "arabic", "brazilian", "bulgarian", "greek", "ukrainian", "latam", "vietnamese",
    ];
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as i32)
}

pub fn spec_item(spec: &SubmitSpec) -> Option<u64> {
    spec.item.filter(|&i| i != 0)
}

pub fn parse_spec(body: &[u8]) -> Option<SubmitSpec> {
    serde_json::from_slice(body).ok()
}

pub fn to_request(app_id: u32, item_id: u64, spec: SubmitSpec) -> crate::steam_client::workshop_upload::PublishRequest {
    use crate::steam_client::workshop_upload::{ItemFields, PublishRequest};
    PublishRequest {
        app_id,
        item_id: Some(item_id),
        content_dir: spec.content.filter(|c| !c.is_empty()).map(std::path::PathBuf::from),
        fields: ItemFields {
            title: spec.title,
            description: spec.description,
            visibility: spec.visibility,
            tags: spec.tags,
            metadata: spec.metadata,
            language: spec.language.as_deref().and_then(language_code),
            kv_tags: spec.kv_tags,
            removed_keys: spec.removed_keys,
            change_note: spec.change_note,
            preview_file: spec.preview.filter(|p| !p.is_empty()).map(std::path::PathBuf::from),
            previews: spec
                .previews
                .into_iter()
                .filter_map(|p| match (p.file, p.video) {
                    (Some(file), _) if !file.is_empty() => Some(crate::steam_client::workshop_upload::PreviewSpec::File { kind: p.kind, path: file.into(), replaces: p.replaces }),
                    (_, Some(id)) if !id.is_empty() => Some(crate::steam_client::workshop_upload::PreviewSpec::Video { id, replaces: p.replaces }),
                    _ => None,
                })
                .collect(),
            previews_to_remove: spec.previews_to_remove,
            game_branch_min: spec.game_branch_min,
            game_branch_max: spec.game_branch_max,
            descriptors_add: spec.descriptors_add,
            descriptors_remove: spec.descriptors_remove,
        },
    }
}

pub fn update_status(stage: &crate::steam_client::workshop_upload::PublishStage) -> u8 {
    use crate::steam_client::workshop_upload::PublishStage as S;
    match stage {
        S::Creating | S::Preparing => 1,
        S::Building => 2,
        S::UploadingManifest | S::UploadingChunks(..) => 3,
        S::Finishing | S::Updating | S::Done => 5,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishJob {
    pub status: u8,
    pub done: u64,
    pub total: u64,
    pub result: i32,
    pub item: u64,
    pub legal: bool,
}

pub fn encode_job(job: &PublishJob) -> Vec<u8> {
    let mut out = Vec::with_capacity(30);
    out.push(job.status);
    out.extend_from_slice(&job.done.to_le_bytes());
    out.extend_from_slice(&job.total.to_le_bytes());
    out.extend_from_slice(&job.result.to_le_bytes());
    out.extend_from_slice(&job.item.to_le_bytes());
    out.push(job.legal as u8);
    out
}

pub fn jobs() -> &'static std::sync::Mutex<std::collections::HashMap<u64, PublishJob>> {
    static JOBS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, PublishJob>>> = std::sync::OnceLock::new();
    JOBS.get_or_init(Default::default)
}

pub fn encode_created(result: i32, item: u64, legal: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(13);
    out.extend_from_slice(&result.to_le_bytes());
    out.extend_from_slice(&item.to_le_bytes());
    out.push(legal as u8);
    out
}

#[cfg(test)]
#[path = "ugc_tests.rs"]
mod tests;
