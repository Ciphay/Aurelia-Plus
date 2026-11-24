use super::*;

fn with_config<R>(f: impl FnOnce() -> R) -> R {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("AURELIA_CONFIG_DIR", dir.path()) };
    let out = f();
    unsafe { std::env::remove_var("AURELIA_CONFIG_DIR") };
    out
}

fn net(at: u64) -> AppNet {
    AppNet { dlc: vec![DlcEntry { appid: 1, owned: true, installed: false, name: String::new() }], cloud_quota: 7, betas: Vec::new(), at }
}

#[test]
fn names_come_back_until_they_expire_and_survive_a_restart() {
    with_config(|| {
        let warm = Warm { inner: Mutex::new(Inner::default()) };
        warm.put_names(42, &HashMap::from([(1, "Ana".to_string())]));
        assert_eq!(warm.names(42, &[1, 2]), HashMap::from([(1, "Ana".to_string())]));
        let again = Warm { inner: Mutex::new(Inner::default()) };
        assert_eq!(again.names(42, &[1]).get(&1).map(String::as_str), Some("Ana"));
        assert!(again.names(43, &[1]).is_empty());
        again.with(42, |i| {
            i.disk.names.insert(1, (now_secs() - NAMES_TTL_SECS - 1, "Velho".into()));
        });
        assert!(again.names(42, &[1]).is_empty());
    });
}

#[test]
fn the_network_part_of_an_app_expires_and_can_be_invalidated() {
    with_config(|| {
        let warm = Warm { inner: Mutex::new(Inner::default()) };
        warm.put_app_net(1, 550, net(now_secs()));
        assert_eq!(warm.app_net(1, 550).map(|n| n.cloud_quota), Some(7));
        warm.invalidate_app(1, 550);
        assert!(warm.app_net(1, 550).is_none());
        warm.put_app_net(1, 550, net(now_secs() - APP_NET_TTL_SECS - 1));
        assert!(warm.app_net(1, 550).is_none(), "vencida");
    });
}

#[test]
fn a_corrupt_or_foreign_cache_file_is_ignored() {
    with_config(|| {
        let dir = dir_for(5).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cache.json"), b"{ not json").unwrap();
        let warm = Warm { inner: Mutex::new(Inner::default()) };
        assert!(warm.app_net(5, 1).is_none());
        std::fs::write(dir.join("cache.json"), br#"{"format":999,"names":{},"apps":{}}"#).unwrap();
        let other = Warm { inner: Mutex::new(Inner::default()) };
        assert!(other.names(5, &[1]).is_empty());
    });
}

#[test]
fn the_last_snapshot_of_an_app_is_kept_per_account() {
    with_config(|| {
        let snap = super::super::snapshot::assemble(
            super::super::snapshot::Account { steamid: 9, persona: "P".into(), country: "BR".into() },
            &[],
            &Default::default(),
            None,
        );
        assert!(load_snapshot(9, 550).is_none());
        save_snapshot(9, 550, &snap);
        let (back, at) = load_snapshot(9, 550).expect("snapshot salvo");
        assert_eq!(back.account.persona, "P");
        assert!(at > 0);
        assert_eq!(last_account_with_snapshot(550), Some(9));
        assert_eq!(last_account_with_snapshot(551), None);
    });
}

#[test]
fn an_expired_app_still_opens_the_game_until_it_is_too_old() {
    with_config(|| {
        let warm = Warm { inner: Mutex::new(Inner::default()) };
        let now = now_secs();
        warm.put_app_net(1, 550, net(now));
        assert_eq!(warm.app_net_stale_ok(1, 550).map(|(_, fresh)| fresh), Some(true));
        warm.put_app_net(1, 550, net(now - APP_NET_TTL_SECS - 1));
        assert_eq!(warm.app_net_stale_ok(1, 550).map(|(n, fresh)| (n.cloud_quota, fresh)), Some((7, false)), "vencida, mas serve");
        assert!(warm.app_net(1, 550).is_none(), "a leitura estrita continua tratando como vencida");
        warm.put_app_net(1, 550, net(now - APP_NET_STALE_MAX_SECS - 1));
        assert!(warm.app_net_stale_ok(1, 550).is_none(), "too old is discarded");
        assert!(warm.app_net_stale_ok(1, 999).is_none(), "nothing stored");
    });
}

#[test]
fn only_one_background_refresh_runs_per_app_and_it_is_released() {
    let first = begin_refresh(9_001, 77_001).expect("first");
    assert!(begin_refresh(9_001, 77_001).is_none(), "already refreshing");
    assert!(begin_refresh(9_001, 77_002).is_some(), "another app is independent");
    drop(first);
    assert!(begin_refresh(9_001, 77_001).is_some(), "released after the guard drops");
}

#[test]
fn the_purchase_time_is_the_oldest_license_among_the_packages_that_give_the_app() {
    with_config(|| {
        let warm = Warm { inner: Mutex::new(Inner::default()) };
        assert_eq!(warm.packages_missing(1, &[10, 11]), vec![10, 11]);
        warm.put_licenses(1, vec![(10, 300), (11, 100), (12, 0)], HashMap::from([(10, vec![550, 440]), (11, vec![550]), (12, vec![550])]));
        assert_eq!(warm.purchase_time(1, 550), Some(100), "the oldest, ignoring the license without a date");
        assert_eq!(warm.purchase_time(1, 440), Some(300));
        assert_eq!(warm.purchase_time(1, 999), None);
        assert!(warm.packages_missing(1, &[10, 11, 13]).contains(&13));
        let again = Warm { inner: Mutex::new(Inner::default()) };
        assert_eq!(again.purchase_time(1, 550), Some(100));
        assert_eq!(again.purchase_time(2, 550), None, "other account");
    });
}
