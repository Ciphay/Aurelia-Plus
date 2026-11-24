#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegOp {
    Dword { path: &'static str, key: &'static str, value: u32 },
    String { path: &'static str, key: &'static str, value: &'static str },
}

impl RegOp {
    pub fn to_reg_add_args(&self) -> Vec<String> {
        let (path, key, reg_type, value) = match self {
            RegOp::Dword { path, key, value } => (*path, *key, "REG_DWORD", value.to_string()),
            RegOp::String { path, key, value } => (*path, *key, "REG_SZ", (*value).to_string()),
        };
        vec![
            "reg.exe".to_string(),
            "add".to_string(),
            path.to_string(),
            "/v".to_string(),
            key.to_string(),
            "/t".to_string(),
            reg_type.to_string(),
            "/d".to_string(),
            value,
            "/f".to_string(),
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameFixups {
    pub env: Vec<(String, String)>,
    pub dll_overrides: Vec<(String, String)>,
    pub launch_args: Vec<String>,
    pub reg_ops: Vec<RegOp>,
}

impl GameFixups {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty()
            && self.dll_overrides.is_empty()
            && self.launch_args.is_empty()
            && self.reg_ops.is_empty()
    }
}

struct FixupEntry {
    app_id: u32,
    env: &'static [(&'static str, &'static str)],
    dll_overrides: &'static [(&'static str, &'static str)],
    launch_args: &'static [&'static str],
    reg_ops: &'static [RegOp],
}

const FIXUPS: &[FixupEntry] = &[
    FixupEntry {
        app_id: 211420,
        env: &[("PROTON_NO_ESYNC", "1")],
        dll_overrides: &[],
        launch_args: &[],
        reg_ops: &[],
    },
    FixupEntry {
        app_id: 22370,
        env: &[],
        dll_overrides: &[("xlive", "builtin")],
        launch_args: &[],
        reg_ops: &[],
    },
    FixupEntry {
        app_id: 271590,
        env: &[
            ("WINE_LARGE_ADDRESS_AWARE", "1"),
            ("STAGING_SHARED_MEMORY", "1"),
        ],
        dll_overrides: &[("dinput8", "n,b")],
        launch_args: &["-ignoredifferentvideocard"],
        reg_ops: &[],
    },
];

pub fn game_fixups(app_id: u32) -> GameFixups {
    match FIXUPS.iter().find(|e| e.app_id == app_id) {
        Some(entry) => GameFixups {
            env: entry
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            dll_overrides: entry
                .dll_overrides
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            launch_args: entry.launch_args.iter().map(ToString::to_string).collect(),
            reg_ops: entry.reg_ops.to_vec(),
        },
        None => GameFixups::default(),
    }
}

#[cfg(test)]
#[path = "fixups_tests.rs"]
mod tests;
