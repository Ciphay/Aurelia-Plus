use std::collections::HashMap;

pub const SLOT_GET_USER_STATS: u16 = 4;
pub const SLOT_STORE_USER_STATS: u16 = 5;

pub const KIND_ACHIEVEMENTS: u32 = 4;
pub const KIND_GROUP_ACHIEVEMENTS: u32 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct StatDef {
    pub id: u32,
    pub kind: u32,
    pub name: String,
    pub default: u32,
    pub bits: Vec<(u32, String)>,
}

fn stat_kind_from_str(s: &str) -> Option<u32> {
    if let Ok(n) = s.trim().parse::<u32>() {
        return Some(n);
    }
    match s.trim().to_ascii_uppercase().replace('_', "").as_str() {
        "INT" => Some(1),
        "FLOAT" => Some(2),
        "AVGRATE" => Some(3),
        "ACHIEVEMENTS" => Some(KIND_ACHIEVEMENTS),
        "GROUPACHIEVEMENTS" => Some(KIND_GROUP_ACHIEVEMENTS),
        _ => None,
    }
}

fn is_achievement_kind(kind: u32) -> bool {
    kind == KIND_ACHIEVEMENTS || kind == KIND_GROUP_ACHIEVEMENTS
}

pub fn parse_stat_defs(schema: &[u8]) -> Vec<StatDef> {
    if schema.is_empty() {
        return Vec::new();
    }
    let bytes = schema.to_vec();
    std::thread::Builder::new()
        .name("stats-schema-parse".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || parse_stat_defs_inner(&bytes))
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or_default()
}

pub fn schema_hidden_declared(schema: &[u8]) -> Option<std::collections::HashSet<String>> {
    if schema.is_empty() {
        return None;
    }
    let bytes = schema.to_vec();
    std::thread::Builder::new()
        .name("stats-schema-hidden".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut out = std::collections::HashSet::new();
            let Ok(vdf) = steam_vdf_parser::parse_binary(&bytes) else { return None };
            let Some(root) = vdf.as_obj() else { return None };
            let stats = root.get("stats").and_then(|v| v.as_obj()).or_else(|| root.values().next().and_then(|v| v.as_obj()).and_then(|o| o.get("stats")).and_then(|v| v.as_obj()));
            for (_, stat) in stats.into_iter().flat_map(|s| s.iter()) {
                let Some(bits) = stat.as_obj().and_then(|o| o.get("bits")).and_then(|v| v.as_obj()) else { continue };
                for (_, bit) in bits.iter() {
                    let Some(bit) = bit.as_obj() else { continue };
                    let declared = bit.get("display").and_then(|d| d.as_obj()).is_some_and(|d| d.get("hidden").is_some());
                    if let (true, Some(name)) = (declared, bit.get("name").and_then(|v| v.as_str())) {
                        out.insert(name.to_string());
                    }
                }
            }
            Some(out)
        })
        .ok()
        .and_then(|h| h.join().ok())
        .flatten()
}

fn parse_stat_defs_inner(schema: &[u8]) -> Vec<StatDef> {
    let Ok(vdf) = steam_vdf_parser::parse_binary(schema) else { return Vec::new() };
    let Some(root) = vdf.as_obj() else { return Vec::new() };
    let stats = root.get("stats").and_then(|v| v.as_obj()).or_else(|| {
        root.values().next().and_then(|v| v.as_obj()).and_then(|o| o.get("stats")).and_then(|v| v.as_obj())
    });
    let Some(stats) = stats else { return Vec::new() };
    let mut out = Vec::new();
    for (key, val) in stats.iter() {
        let (Ok(id), Some(obj)) = (key.parse::<u32>(), val.as_obj()) else { continue };
        let kind = obj.get("type").and_then(|v| v.as_i32().map(|n| n as u32).or_else(|| v.as_str().and_then(stat_kind_from_str))).unwrap_or(1);
        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let default = match obj.get("default") {
            Some(v) if v.is_float() => v.as_float().unwrap_or(0.0).to_bits(),
            Some(v) => v.as_i32().map(|n| n as u32).or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok().map(|n| n as u32))).unwrap_or(0),
            None => 0,
        };
        let mut bits = Vec::new();
        if let Some(b) = obj.get("bits").and_then(|v| v.as_obj()) {
            for (bit_key, bit_val) in b.iter() {
                let Ok(bit) = bit_key.parse::<u32>() else { continue };
                if let Some(n) = bit_val.as_obj().and_then(|o| o.get("name")).and_then(|v| v.as_str()) {
                    bits.push((bit, n.to_string()));
                }
            }
            bits.sort();
        }
        out.push(StatDef { id, kind, name, default, bits });
    }
    out.sort_by_key(|d| d.id);
    out
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_name(out: &mut Vec<u8>, name: &str) {
    let b = &name.as_bytes()[..name.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(b.len() as u16).to_le_bytes());
    out.extend_from_slice(b);
}

pub fn encode_user_stats(crc: u32, defs: &[StatDef], values: &HashMap<u32, u32>, unlock_blocks: &HashMap<u32, Vec<u32>>) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, crc);
    let plain: Vec<&StatDef> = defs.iter().filter(|d| !is_achievement_kind(d.kind)).collect();
    put32(&mut out, plain.len() as u32);
    for d in plain {
        put32(&mut out, d.id);
        put32(&mut out, d.kind);
        put32(&mut out, values.get(&d.id).copied().unwrap_or(d.default));
        put32(&mut out, d.default);
        put_name(&mut out, &d.name);
    }
    let achievements: Vec<(&StatDef, &(u32, String))> = defs.iter().filter(|d| is_achievement_kind(d.kind)).flat_map(|d| d.bits.iter().map(move |b| (d, b))).collect();
    put32(&mut out, achievements.len() as u32);
    for (d, (bit, name)) in achievements {
        put32(&mut out, d.id);
        put32(&mut out, *bit);
        put32(&mut out, unlock_blocks.get(&d.id).and_then(|t| t.get(*bit as usize)).copied().unwrap_or(0));
        put_name(&mut out, name);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub api_name: String,
    pub title: String,
    pub description: String,
    pub hidden: bool,
    pub hidden_declared: bool,
    pub percent: f32,
}

pub fn encode_display(items: &[DisplayInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, items.len() as u32);
    for d in items {
        put_name(&mut out, &d.api_name);
        put_name(&mut out, &d.title);
        put_name(&mut out, &d.description);
        out.push(d.hidden as u8 | (u8::from(!d.hidden_declared) << 1));
        out.extend_from_slice(&d.percent.to_le_bytes());
    }
    out
}

pub fn parse_store_payload(p: &[u8]) -> Option<(u32, Vec<(u32, u32)>)> {
    let rd = |o: usize| p.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (crc, n) = (rd(0)?, rd(4)? as usize);
    if p.len() != 8 + n * 8 {
        return None;
    }
    Some((crc, (0..n).map(|i| (rd(8 + i * 8).unwrap(), rd(12 + i * 8).unwrap())).collect()))
}

#[cfg(test)]
#[path = "userstats_tests.rs"]
mod tests;
