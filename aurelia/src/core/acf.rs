use crate::core::utils::extract_quoted_values;

#[derive(Debug, Default)]
pub struct AppManifest {
    pub app_id: Option<u32>,
    pub install_dir: Option<String>,
    pub name: Option<String>,
    pub last_owner: Option<u64>,
    pub state_flags: Option<u32>,
    pub last_updated: u64,
    pub build_id: u64,
    pub active_branch: String,
    pub installed_depots: Vec<u32>,
}

impl AppManifest {
    pub fn fully_installed(&self) -> bool {
        self.state_flags.is_some_and(|flags| flags & 4 != 0)
    }

    pub fn update_pending(&self) -> bool {
        self.state_flags.is_some_and(|flags| flags & 2 != 0)
    }

    pub fn display_name(&self) -> Option<String> {
        let name = self.name.as_deref()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
}

pub fn parse_app_manifest(raw: &str) -> AppManifest {
    let mut m = AppManifest {
        active_branch: "public".to_string(),
        ..Default::default()
    };
    let mut in_user_config = false;
    let mut branch_set = false;
    let (mut depth, mut depots_depth, mut depots_pending) = (0i32, 0i32, false);

    for line in raw.lines() {
        let trimmed = line.trim();
        let parts = extract_quoted_values(trimmed);

        if parts.len() == 1 && parts[0].eq_ignore_ascii_case("userconfig") {
            in_user_config = true;
            continue;
        }

        if parts.len() == 1 && parts[0].eq_ignore_ascii_case("installeddepots") {
            depots_pending = true;
            continue;
        }
        if trimmed == "{" || trimmed == "}" {
            if trimmed == "{" {
                depth += 1;
                if depots_pending {
                    depots_depth = depth;
                    depots_pending = false;
                }
            } else {
                if depots_depth == depth {
                    depots_depth = 0;
                }
                depth -= 1;
            }
            if trimmed == "}" && in_user_config {
                in_user_config = false;
            }
            continue;
        }
        if depots_depth > 0 && depth == depots_depth && parts.len() == 1 {
            if let Ok(id) = parts[0].parse::<u32>() {
                m.installed_depots.push(id);
            }
            continue;
        }

        if parts.len() < 2 {
            continue;
        }
        let value = &parts[1];

        if in_user_config {
            if !branch_set && parts[0].eq_ignore_ascii_case("betakey") && !value.trim().is_empty() {
                m.active_branch = value.to_string();
                branch_set = true;
            }
            continue;
        }

        match parts[0].to_ascii_lowercase().as_str() {
            "appid" if m.app_id.is_none() => m.app_id = value.parse().ok(),
            "installdir" if m.install_dir.is_none() => m.install_dir = Some(value.to_string()),
            "name" if m.name.is_none() => m.name = Some(value.to_string()),
            "lastowner" if m.last_owner.is_none() => {
                m.last_owner = value.parse().ok().filter(|&id| id != 0)
            }
            "stateflags" if m.state_flags.is_none() => m.state_flags = value.parse().ok(),
            "lastupdated" => m.last_updated = value.parse().unwrap_or(0),
            "buildid" => m.build_id = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    m
}

#[cfg(test)]
mod tests {
    use super::parse_app_manifest;

    #[test]
    fn installed_depots_are_listed_without_nested_keys() {
        let raw = r#""AppState"
{
	"appid"		"500"
	"buildid"		"123"
	"InstalledDepots"
	{
		"501"
		{
			"manifest"		"1"
			"size"		"2"
		}
		"502"
		{
			"manifest"		"3"
		}
	}
	"UserConfig"
	{
		"BetaKey"		"beta"
	}
}"#;
        let m = parse_app_manifest(raw);
        assert_eq!(m.installed_depots, vec![501, 502]);
        assert_eq!(m.active_branch, "beta");
        assert_eq!(m.build_id, 123);
    }
}
