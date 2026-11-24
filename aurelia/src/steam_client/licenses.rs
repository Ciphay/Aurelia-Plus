use std::collections::HashMap;

use anyhow::{Context, Result};
use protobuf::Message;
use steam_vent::{Connection, ConnectionTrait};
use steam_vent_proto::steammessages_clientserver::CMsgClientLicenseList;
use steam_vent_proto::steammessages_clientserver_appinfo::{CMsgClientPICSProductInfoRequest, CMsgClientPICSProductInfoResponse, cmsg_client_picsproduct_info_request};

pub const EMSG_LICENSE_LIST: i32 = 780;
const PACKAGES_PER_REQUEST: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct License {
    pub package: u32,
    pub time: u32,
    pub token: u64,
}

pub fn parse_license_list(body: &[u8]) -> Vec<License> {
    let Ok(msg) = CMsgClientLicenseList::parse_from_bytes(body) else { return Vec::new() };
    msg.licenses.iter().map(|l| License { package: l.package_id(), time: l.time_created(), token: l.access_token() }).collect()
}

pub fn licenses_from_logon() -> Vec<License> {
    super::friends::logon_burst().into_iter().filter(|(kind, _, _)| *kind == EMSG_LICENSE_LIST).flat_map(|(_, _, body)| parse_license_list(&body)).collect()
}

pub fn apps_of_package(buffer: &[u8]) -> Vec<u32> {
    let Ok(vdf) = super::find_vdf_in_pics(buffer) else { return Vec::new() };
    let root = vdf.value();
    let candidates: Vec<&steam_vdf_parser::Value> = std::iter::once(root).chain(root.as_obj().into_iter().flat_map(|o| o.values())).collect();
    for node in candidates {
        if let Some(apps) = node.get_obj(&["appids"]) {
            return apps.iter().filter_map(|(_, v)| v.as_i32().map(|n| n as u32).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).collect();
        }
    }
    Vec::new()
}

pub async fn fetch_package_apps(connection: &Connection, licenses: &[License]) -> Result<HashMap<u32, Vec<u32>>> {
    let mut out = HashMap::new();
    for chunk in licenses.chunks(PACKAGES_PER_REQUEST) {
        let mut request = CMsgClientPICSProductInfoRequest::new();
        for l in chunk {
            request.packages.push(cmsg_client_picsproduct_info_request::PackageInfo { packageid: Some(l.package), access_token: (l.token != 0).then_some(l.token), ..Default::default() });
        }
        let response: CMsgClientPICSProductInfoResponse = connection.job(request).await.context("failed requesting PICS package info")?;
        for p in response.packages {
            out.insert(p.packageid(), apps_of_package(p.buffer()));
        }
    }
    Ok(out)
}

pub fn app_name_of(buffer: &[u8]) -> Option<String> {
    let vdf = super::find_vdf_in_pics(buffer).ok()?;
    let section = super::pics_app_section(vdf.value());
    section.get_str(&["common", "name"]).map(str::to_string)
}

impl super::SteamClient {
    pub async fn app_names(&self, ids: &[u32]) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        let Ok(connection) = self.require_connection() else { return out };
        for chunk in ids.chunks(100) {
            let mut request = CMsgClientPICSProductInfoRequest::new();
            for id in chunk {
                request.apps.push(cmsg_client_picsproduct_info_request::AppInfo { appid: Some(*id), ..Default::default() });
            }
            let Ok(response) = connection.job::<_, CMsgClientPICSProductInfoResponse>(request).await else { continue };
            for app in response.apps {
                if let Some(name) = app_name_of(app.buffer()) {
                    out.insert(app.appid(), name);
                }
            }
        }
        out
    }

    pub async fn ensure_purchase_data(&self) -> bool {
        let Some(steam_id) = self.steam_id() else { return false };
        let licenses = licenses_from_logon();
        if licenses.is_empty() {
            return false;
        }
        let warm = crate::bridge::warm::global();
        let wanted: Vec<u32> = licenses.iter().map(|l| l.package).collect();
        let missing = warm.packages_missing(steam_id, &wanted);
        let mut fetched = HashMap::new();
        if !missing.is_empty() {
            let pending: Vec<License> = licenses.iter().copied().filter(|l| missing.contains(&l.package)).collect();
            let Ok(connection) = self.require_connection() else { return false };
            match fetch_package_apps(connection, &pending).await {
                Ok(map) => fetched = map,
                Err(e) => {
                    tracing::debug!("package info unavailable: {e:#}");
                    return false;
                }
            }
        }
        let changed = !fetched.is_empty();
        warm.put_licenses(steam_id, licenses.iter().map(|l| (l.package, l.time)).collect(), fetched);
        changed
    }
}

pub fn earliest_purchase(app: u32, licenses: &[License], package_apps: &HashMap<u32, Vec<u32>>) -> Option<u32> {
    licenses.iter().filter(|l| l.time > 0 && package_apps.get(&l.package).is_some_and(|apps| apps.contains(&app))).map(|l| l.time).min()
}

#[cfg(test)]
#[path = "licenses_tests.rs"]
mod tests;
