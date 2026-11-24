use super::*;

#[test]
fn request_round_trip_and_short_input() {
    let mut p = vec![OP_DOWNLOAD];
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&123456789012u64.to_le_bytes());
    assert_eq!(parse_request(&p), Some((OP_DOWNLOAD, 480, 123456789012)));
    assert_eq!(parse_request(&p[..12]), None);
}

#[test]
fn list_and_state_layouts() {
    assert_eq!(encode_list(&[1, 2]), vec![2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    let s = encode_state(true, 5, 7, "/a/b");
    assert_eq!(s, vec![1, 5, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 4, 0, b'/', b'a', b'/', b'b']);
}

#[test]
fn the_submit_spec_keeps_only_what_the_game_set() {
    let spec = parse_spec(br#"{"title":"Meu mod","visibility":2,"tags":["a"],"content":"/tmp/x","language":"brazilian","kv_tags":[["k","v"]]}"#).unwrap();
    let req = to_request(480, 42, spec);
    assert_eq!((req.app_id, req.item_id), (480, Some(42)));
    assert_eq!(req.content_dir.as_deref(), Some(std::path::Path::new("/tmp/x")));
    assert_eq!((req.fields.title.as_deref(), req.fields.visibility, req.fields.language), (Some("Meu mod"), Some(2), Some(23)));
    assert_eq!(req.fields.description, None, "what the game did not provide is left untouched");
    assert_eq!(req.fields.kv_tags, vec![("k".to_string(), "v".to_string())]);
    assert!(parse_spec(b"not json").is_none());
}

#[test]
fn an_empty_content_folder_means_a_metadata_only_update() {
    let req = to_request(1, 2, parse_spec(br#"{"content":""}"#).unwrap());
    assert!(req.content_dir.is_none());
}

#[test]
fn languages_map_to_elanguage_and_unknown_names_do_not() {
    assert_eq!((language_code("english"), language_code("German"), language_code("schinese"), language_code("vietnamese")), (Some(0), Some(1), Some(6), Some(28)));
    assert_eq!(language_code("klingon"), None);
}

#[test]
fn progress_and_created_replies_have_the_documented_layout() {
    let job = PublishJob { status: 3, done: 10, total: 20, result: 1, item: 99, legal: true };
    let bytes = encode_job(&job);
    assert_eq!(bytes.len(), 30);
    assert_eq!((bytes[0], u64::from_le_bytes(bytes[1..9].try_into().unwrap()), i32::from_le_bytes(bytes[17..21].try_into().unwrap()), bytes[29]), (3, 10, 1, 1));
    assert_eq!(encode_created(1, 7, false), [1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn the_body_is_whatever_follows_the_fixed_header() {
    let mut req = vec![7u8; 13];
    req.extend_from_slice(b"{}");
    assert_eq!(request_body(&req), b"{}");
    assert!(request_body(&req[..10]).is_empty());
}

#[test]
fn stages_map_to_the_statuses_the_game_can_read() {
    use crate::steam_client::workshop_upload::PublishStage as S;
    assert_eq!([update_status(&S::Preparing), update_status(&S::Building), update_status(&S::UploadingChunks(1, 2)), update_status(&S::Updating)], [1, 2, 3, 5]);
}

#[test]
fn the_submit_spec_carries_extra_previews_versions_and_descriptors() {
    use crate::steam_client::workshop_upload::PreviewSpec;
    let spec = parse_spec(br#"{"item":9,"previews":[{"file":"/x/a.png","type":0},{"video":"abc"},{"file":"/x/b.png","type":3,"replaces":2},{}],"previews_to_remove":[1],"game_branch_min":"1.0","descriptors_add":[5]}"#).unwrap();
    let f = to_request(480, 9, spec).fields;
    assert_eq!(
        f.previews,
        vec![
            PreviewSpec::File { kind: 0, path: "/x/a.png".into(), replaces: None },
            PreviewSpec::Video { id: "abc".into(), replaces: None },
            PreviewSpec::File { kind: 3, path: "/x/b.png".into(), replaces: Some(2) },
        ],
        "an empty preview is ignored"
    );
    assert_eq!((f.previews_to_remove, f.game_branch_min.as_deref(), f.game_branch_max, f.descriptors_add), (vec![1], Some("1.0"), None, vec![5]));
}
