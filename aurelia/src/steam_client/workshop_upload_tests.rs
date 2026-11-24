use super::*;
use super::super::workshop_build::BuiltChunk;

fn chunk() -> BuiltChunk {
    let mut sha = [0u8; 20];
    sha[0] = 0xAB;
    BuiltChunk { sha, payload: vec![0u8; 77], payload_crc: 12345 }
}

#[test]
fn a_chunk_upload_names_the_chunk_and_carries_the_build_handle_in_the_query() {
    let (url, query) = chunk_upload_request("cache1.example.test", 480, 480, 9_000_000_000_000, &chunk());
    assert_eq!(url, "https://cache1.example.test/chunkupload/ab00000000000000000000000000000000000000");
    assert_eq!(
        query,
        vec![("appid", "480".to_string()), ("depotid", "480".to_string()), ("buildid", "9000000000000".to_string()), ("compressed_crc", "12345".to_string()), ("compressed_size", "77".to_string())]
    );
}

#[test]
fn a_manifest_upload_goes_to_manifestupload_and_a_host_with_a_scheme_is_kept() {
    let (url, query) = manifest_upload_request("https://upload.example.test/", 480, 7);
    assert_eq!(url, "https://upload.example.test/manifestupload");
    assert_eq!(query, vec![("appid", "480".to_string()), ("buildid", "7".to_string()), ("can_poll_status", "1".to_string())]);
    assert_eq!(manifest_upload_request("http://127.0.0.1:8080", 1, 2).0, "http://127.0.0.1:8080/manifestupload");
}

#[test]
fn an_update_only_touches_what_was_given_and_closes_the_in_progress_state() {
    let fields = ItemFields { title: Some("Meu mod".into()), visibility: Some(VISIBILITY_PRIVATE), tags: Some(vec!["a".into(), "b".into()]), change_note: Some("v2".into()), ..Default::default() };
    let m = update_message(480, 42, Some(777), &fields, None, &[]);
    assert_eq!((m.app_id(), m.published_file_id()), (480, 42));
    assert!(m.update_content_manifest() && m.content_manifest() == 777);
    assert!(m.update_title() && m.title() == "Meu mod");
    assert!(m.update_visibility() && m.visibility() == 2);
    assert!(m.update_tags() && m.tags == ["a", "b"]);
    assert!(!m.update_description() && !m.update_metadata(), "what was not given is left untouched");
    assert_eq!(m.change_description(), "v2");
    assert!(m.clear_in_progress(), "the first update takes the item out of the in-progress state");
}

#[test]
fn a_metadata_only_update_does_not_touch_the_content() {
    let m = update_message(480, 42, None, &ItemFields { description: Some("d".into()), ..Default::default() }, None, &[]);
    assert!(!m.update_content_manifest());
    assert!(m.update_description());
}

#[test]
fn key_value_tags_are_added_and_removed() {
    let fields = ItemFields { kv_tags: vec![("modo".into(), "coop".into())], removed_keys: vec!["velho".into()], ..Default::default() };
    let m = update_message(1, 2, None, &fields, None, &[]);
    assert_eq!(m.removed_kvtags, ["velho"]);
    assert_eq!((m.kvtags[0].key(), m.kvtags[0].value()), ("modo", "coop"));
}

#[test]
fn eresult_errors_say_what_happened() {
    assert!(eresult_error("publicar", 15).to_string().contains("access denied"));
    assert!(eresult_error("publicar", 99).to_string().contains("EResult 99"));
}

#[test]
fn a_preview_is_referenced_by_a_clean_file_name() {
    assert_eq!(preview_name(Path::new("/home/x/Minha capa (1).png")), "Minha_capa__1_.png");
    assert_eq!(preview_name(Path::new("capa.jpg")), "capa.jpg");
    let m = update_message(1, 2, None, &ItemFields::default(), Some("capa.jpg"), &[]);
    assert!(m.update_preview_file() && m.preview_file_name() == "capa.jpg");
    assert!(!update_message(1, 2, None, &ItemFields::default(), None, &[]).update_preview_file());
}

#[test]
fn extra_previews_videos_versions_and_descriptors_go_into_the_update() {
    let fields = ItemFields {
        previews: vec![
            PreviewSpec::File { kind: 0, path: "/x/a.png".into(), replaces: None },
            PreviewSpec::Video { id: "dQw4w9WgXcQ".into(), replaces: None },
            PreviewSpec::File { kind: 3, path: "/x/b.png".into(), replaces: Some(2) },
        ],
        previews_to_remove: vec![0, 4],
        game_branch_min: Some("1.0".into()),
        game_branch_max: Some("2.0".into()),
        descriptors_add: vec![1, 5],
        descriptors_remove: vec![3],
        ..Default::default()
    };
    let uploaded = vec![("a_1.png".to_string(), "a.png".to_string()), ("b_2.png".to_string(), "b.png".to_string())];
    let m = update_message(480, 9, None, &fields, None, &uploaded);
    assert_eq!(m.previews.len(), 3);
    assert_eq!((m.previews[0].internal_file_name(), m.previews[0].original_file_name(), m.previews[0].preview_type(), m.previews[0].update_index()), ("a_1.png", "a.png", 0, -1));
    assert_eq!((m.previews[1].videoid(), m.previews[1].preview_type()), ("dQw4w9WgXcQ", 1), "YouTube video");
    assert_eq!((m.previews[2].internal_file_name(), m.previews[2].preview_type(), m.previews[2].update_index()), ("b_2.png", 3, 2), "the second file changes index 2");
    assert_eq!(m.previews_to_remove, [0, 4]);
    assert_eq!((m.game_branch_min(), m.game_branch_max()), ("1.0", "2.0"));
    assert_eq!((m.content_descriptors_to_add.clone(), m.content_descriptors_to_remove.clone()), (vec![1, 5], vec![3]));
}

#[test]
fn previews_and_game_versions_in_one_update_are_split_in_two() {
    let video = PreviewSpec::Video { id: "dQw4w9WgXcQ".into(), replaces: None };
    let both = ItemFields { previews: vec![video.clone()], game_branch_min: Some("1.0".into()), ..ItemFields::default() };
    assert!(needs_split(&both));
    let removal = ItemFields { previews_to_remove: vec![2], game_branch_max: Some("2.0".into()), ..ItemFields::default() };
    assert!(needs_split(&removal));
    assert!(!needs_split(&ItemFields { previews: vec![video], ..ItemFields::default() }), "previews only");
    assert!(!needs_split(&ItemFields { game_branch_min: Some("1.0".into()), ..ItemFields::default() }), "versions only");
    assert!(!needs_split(&ItemFields::default()));
}
