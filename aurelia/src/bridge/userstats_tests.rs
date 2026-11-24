use super::*;

fn kv_obj(key: &str, body: &[u8]) -> Vec<u8> {
    let mut v = vec![0x00];
    v.extend_from_slice(key.as_bytes());
    v.push(0);
    v.extend_from_slice(body);
    v.push(0x08);
    v
}
fn kv_str(key: &str, val: &str) -> Vec<u8> {
    let mut v = vec![0x01];
    v.extend_from_slice(key.as_bytes());
    v.push(0);
    v.extend_from_slice(val.as_bytes());
    v.push(0);
    v
}
fn kv_int(key: &str, val: i32) -> Vec<u8> {
    let mut v = vec![0x02];
    v.extend_from_slice(key.as_bytes());
    v.push(0);
    v.extend_from_slice(&val.to_le_bytes());
    v
}

fn schema() -> Vec<u8> {
    let kills = [kv_int("type", 1), kv_str("name", "kills"), kv_int("default", 5)].concat();
    let acc = [kv_int("type", 2), kv_str("name", "accuracy"), {
        let mut v = vec![0x03];
        v.extend_from_slice(b"default\0");
        v.extend_from_slice(&0.5f32.to_le_bytes());
        v
    }]
    .concat();
    let bits = [kv_obj("0", &kv_str("name", "ACH_FIRST")), kv_obj("1", &kv_str("name", "ACH_SECOND"))].concat();
    let ach = [kv_int("type", 4), kv_str("name", "ach1"), kv_obj("bits", &bits)].concat();
    let stats = [kv_obj("1", &kills), kv_obj("2", &acc), kv_obj("3", &ach)].concat();
    let mut blob = kv_obj("2357570", &kv_obj("stats", &stats));
    blob.push(0x08);
    blob
}

#[test]
fn schema_yields_stats_and_achievement_bits() {
    let defs = parse_stat_defs(&schema());
    assert_eq!(defs.len(), 3);
    assert_eq!((defs[0].id, defs[0].kind, defs[0].name.as_str(), defs[0].default), (1, 1, "kills", 5));
    assert_eq!((defs[1].id, defs[1].kind, defs[1].default), (2, 2, 0.5f32.to_bits()));
    assert_eq!(defs[2].kind, 4);
    assert_eq!(defs[2].bits, vec![(0, "ACH_FIRST".to_string()), (1, "ACH_SECOND".to_string())]);
}

#[test]
fn garbage_schema_gives_no_stats() {
    assert!(parse_stat_defs(&[]).is_empty());
    assert!(parse_stat_defs(&[0xFF, 0x00, 0x13]).is_empty());
}

fn rd32(b: &[u8], o: &mut usize) -> u32 {
    let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
    *o += 4;
    v
}
fn rdname(b: &[u8], o: &mut usize) -> String {
    let n = u16::from_le_bytes(b[*o..*o + 2].try_into().unwrap()) as usize;
    *o += 2;
    let s = String::from_utf8(b[*o..*o + n].to_vec()).unwrap();
    *o += n;
    s
}

#[test]
fn encoded_reply_carries_values_defaults_and_unlock_times() {
    let defs = parse_stat_defs(&schema());
    let values = HashMap::from([(1u32, 42u32)]);
    let blocks = HashMap::from([(3u32, vec![1_700_000_000u32, 0])]);
    let b = encode_user_stats(0xABCD, &defs, &values, &blocks);
    let mut o = 0;
    assert_eq!(rd32(&b, &mut o), 0xABCD);
    assert_eq!(rd32(&b, &mut o), 2);
    assert_eq!([rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o)], [1, 1, 42, 5]);
    assert_eq!(rdname(&b, &mut o), "kills");
    assert_eq!([rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o)], [2, 2, 0.5f32.to_bits(), 0.5f32.to_bits()]);
    assert_eq!(rdname(&b, &mut o), "accuracy");
    assert_eq!(rd32(&b, &mut o), 2);
    assert_eq!([rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o)], [3, 0, 1_700_000_000]);
    assert_eq!(rdname(&b, &mut o), "ACH_FIRST");
    assert_eq!([rd32(&b, &mut o), rd32(&b, &mut o), rd32(&b, &mut o)], [3, 1, 0]);
    assert_eq!(rdname(&b, &mut o), "ACH_SECOND");
    assert_eq!(o, b.len());
}

#[test]
fn store_payload_roundtrips_and_rejects_truncation() {
    let mut p = 7u32.to_le_bytes().to_vec();
    p.extend_from_slice(&2u32.to_le_bytes());
    for v in [1u32, 42, 3, 0b11] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(parse_store_payload(&p), Some((7, vec![(1, 42), (3, 0b11)])));
    assert_eq!(parse_store_payload(&p[..p.len() - 1]), None);
    assert_eq!(parse_store_payload(&[1, 2]), None);
}

#[test]
fn display_section_lists_title_description_hidden_and_global_percent() {
    let items = vec![
        DisplayInfo { api_name: "ACH_A".into(), title: "First".into(), description: "Win once".into(), hidden: false, hidden_declared: true, percent: 12.5 },
        DisplayInfo { api_name: "ACH_B".into(), title: "Segredo".into(), description: String::new(), hidden: true, hidden_declared: true, percent: 0.0 },
    ];
    let b = encode_display(&items);
    let mut o = 0;
    assert_eq!(rd32(&b, &mut o), 2);
    assert_eq!(rdname(&b, &mut o), "ACH_A");
    assert_eq!(rdname(&b, &mut o), "First");
    assert_eq!(rdname(&b, &mut o), "Win once");
    assert_eq!(b[o], 0);
    o += 1;
    assert_eq!(f32::from_le_bytes(b[o..o + 4].try_into().unwrap()), 12.5);
    o += 4;
    assert_eq!(rdname(&b, &mut o), "ACH_B");
    assert_eq!(rdname(&b, &mut o), "Segredo");
    assert_eq!(rdname(&b, &mut o), "");
    assert_eq!(b[o], 1);
    assert_eq!(o + 5, b.len());
}

#[test]
fn parse_stat_defs_accepts_textual_types_from_modern_schemas() {
    let bits = [kv_obj("1", &kv_str("name", "ACH_WIN_ONE_GAME")), kv_obj("2", &kv_str("name", "ACH_WIN_100_GAMES"))].concat();
    let games = [kv_str("type", "INT"), kv_str("name", "NumGames")].concat();
    let speed = [kv_str("type", "AVGRATE"), kv_str("name", "AverageSpeed")].concat();
    let ach = [kv_str("type", "ACHIEVEMENTS"), kv_obj("bits", &bits)].concat();
    let stats = [kv_obj("1", &games), kv_obj("2", &speed), kv_obj("3", &ach)].concat();
    let blob = kv_obj("480", &kv_obj("stats", &stats));
    let defs = parse_stat_defs(&blob);
    assert_eq!(defs.iter().map(|d| d.kind).collect::<Vec<_>>(), vec![1, 3, KIND_ACHIEVEMENTS]);
    let reply = encode_user_stats(0, &defs, &HashMap::new(), &HashMap::new());
    assert_eq!(u32::from_le_bytes(reply[4..8].try_into().unwrap()), 2);
}

#[test]
fn an_achievement_whose_schema_has_no_hidden_key_is_flagged_as_not_declared() {
    let items = vec![DisplayInfo { api_name: "A".into(), title: String::new(), description: String::new(), hidden: false, hidden_declared: false, percent: 0.0 }];
    let b = encode_display(&items);
    let mut o = 4;
    for _ in 0..3 {
        let n = u16::from_le_bytes([b[o], b[o + 1]]) as usize;
        o += 2 + n;
    }
    assert_eq!(b[o], 2, "bit 1 = not declared, bit 0 = not hidden");
}

#[test]
fn an_unreadable_schema_says_nothing_so_the_store_value_stands() {
    assert!(schema_hidden_declared(&[]).is_none());
    assert!(schema_hidden_declared(b"\x00lixo").is_none());
}
