
pub const SLOT_IMAGE: u16 = 20;

const KIND_AVATAR: u8 = 1;
const KIND_ACHIEVEMENT_ICON: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRequest {
    Avatar { steam_id: u64, size: u8 },
    AchievementIcon { app_id: u32, achieved: bool, name: String },
}

pub fn parse_request(p: &[u8]) -> Option<ImageRequest> {
    match *p.first()? {
        KIND_AVATAR if p.len() >= 10 => Some(ImageRequest::Avatar { steam_id: u64::from_le_bytes(p[1..9].try_into().ok()?), size: p[9].min(2) }),
        KIND_ACHIEVEMENT_ICON if p.len() > 6 => Some(ImageRequest::AchievementIcon {
            app_id: u32::from_le_bytes(p[1..5].try_into().ok()?),
            achieved: p[5] != 0,
            name: String::from_utf8_lossy(&p[6..]).into_owned(),
        }),
        _ => None,
    }
}

pub fn avatar_variant(full_url: &str, size: u8) -> String {
    let base = full_url.strip_suffix("_full.jpg").unwrap_or(full_url.trim_end_matches(".jpg"));
    match size {
        0 => format!("{base}.jpg"),
        1 => format!("{base}_medium.jpg"),
        _ => format!("{base}_full.jpg"),
    }
}

pub fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    let pixels = decoder.decode().ok()?;
    let (w, h) = decoder.dimensions()?;
    (pixels.len() == w * h * 4).then_some((w as u32, h as u32, pixels))
}

pub fn encode_reply(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + rgba.len());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(rgba);
    out
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;