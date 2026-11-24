use super::*;

fn license_list(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut msg = CMsgClientLicenseList::new();
    msg.set_eresult(1);
    for (pkg, time) in entries {
        let mut l = steam_vent_proto::steammessages_clientserver::cmsg_client_license_list::License::new();
        l.set_package_id(*pkg);
        l.set_time_created(*time);
        msg.licenses.push(l);
    }
    msg.write_to_bytes().unwrap()
}

#[test]
fn the_license_list_gives_package_and_time() {
    let got = parse_license_list(&license_list(&[(7, 1_600_000_000), (9, 1_700_000_000)]));
    assert_eq!(got.iter().map(|l| (l.package, l.time)).collect::<Vec<_>>(), vec![(7, 1_600_000_000), (9, 1_700_000_000)]);
    assert!(parse_license_list(b"junk").is_empty());
    assert!(parse_license_list(&[]).is_empty());
}

#[test]
fn the_earliest_purchase_is_the_oldest_package_that_gives_the_app() {
    let licenses = vec![License { package: 1, time: 300, token: 0 }, License { package: 2, time: 100, token: 0 }, License { package: 3, time: 50, token: 0 }];
    let map = HashMap::from([(1, vec![550, 440]), (2, vec![550]), (3, vec![730])]);
    assert_eq!(earliest_purchase(550, &licenses, &map), Some(100));
    assert_eq!(earliest_purchase(440, &licenses, &map), Some(300));
    assert_eq!(earliest_purchase(999, &licenses, &map), None, "app in no known package");
    let zero = vec![License { package: 1, time: 0, token: 0 }];
    assert_eq!(earliest_purchase(550, &zero, &map), None);
}

#[test]
fn an_unreadable_package_buffer_gives_no_apps() {
    assert!(apps_of_package(b"").is_empty());
    assert!(apps_of_package(b"\x00\x01\x02garbage").is_empty());
}
