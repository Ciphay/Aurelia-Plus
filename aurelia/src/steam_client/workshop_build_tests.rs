use super::*;
use aes::cipher::{block_padding::Pkcs7, BlockDecrypt, BlockDecryptMut};
use steam_vent_proto::protobuf::Message;

const KEY: [u8; 32] = [7u8; 32];

fn decrypt(mut data: Vec<u8>) -> Vec<u8> {
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&data[..16]);
    aes::Aes256Dec::new(GenericArray::from_slice(&KEY)).decrypt_block(GenericArray::from_mut_slice(&mut iv[..]));
    cbc::Decryptor::<Aes256>::new(GenericArray::from_slice(&KEY), GenericArray::from_slice(&iv)).decrypt_padded_mut::<Pkcs7>(&mut data[16..]).unwrap().to_vec()
}

fn vz_decompress(vz: &[u8]) -> Vec<u8> {
    use std::io::Read;
    assert_eq!(&vz[..3], b"VZa");
    let footer = &vz[vz.len() - 10..];
    let (crc, size) = (u32::from_le_bytes(footer[..4].try_into().unwrap()), u32::from_le_bytes(footer[4..8].try_into().unwrap()));
    assert_eq!(&footer[8..], b"zv");
    let mut alone = vz[7..12].to_vec();
    alone.extend_from_slice(&u64::MAX.to_le_bytes());
    alone.extend_from_slice(&vz[12..vz.len() - 10]);
    let mut out = Vec::new();
    xz2::read::XzDecoder::new_stream(&alone[..], xz2::stream::Stream::new_lzma_decoder(u64::MAX).unwrap()).read_to_end(&mut out).unwrap();
    assert_eq!(out.len() as u32, size);
    assert_eq!(crc32fast::hash(&out), crc);
    out
}

#[test]
fn adler32_matches_the_reference_value() {
    assert_eq!(adler32(b"Wikipedia"), 0x11DD_0397);
    assert_eq!(adler32(b""), 0);
}

#[test]
fn encryption_puts_the_encrypted_iv_first_and_round_trips() {
    let plain = b"o conteudo do item".to_vec();
    let wire = encrypt_with_iv(&plain, KEY, [9u8; 16]);
    assert_eq!(wire.len(), 16 + 32, "16 do IV e dois blocos de 16 com o preenchimento");
    assert_ne!(&wire[..16], &[9u8; 16], "o IV vai cifrado");
    assert_eq!(decrypt(wire), plain);
}

#[test]
fn vz_round_trips_and_shrinks_repetitive_data() {
    let plain: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
    let vz = vz_compress(&plain).unwrap();
    assert!(vz.len() < plain.len() / 4);
    assert_eq!(vz_decompress(&vz), plain);
    assert_eq!(vz_decompress(&vz_compress(b"x").unwrap()), b"x");
}

#[test]
fn a_chunk_payload_decrypts_to_a_vz_that_decompresses_to_the_chunk() {
    let plain = vec![42u8; 5000];
    assert_eq!(vz_decompress(&decrypt(chunk_payload(&plain, KEY).unwrap())), plain);
}

fn parse_manifest(bytes: &[u8]) -> (ContentManifestPayload, ContentManifestMetadata) {
    let (mut at, mut payload, mut metadata) = (0usize, None, None);
    while at + 8 <= bytes.len() {
        let magic = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        if magic == END_MAGIC {
            assert_eq!(at + 4, bytes.len(), "nothing after the final marker");
            break;
        }
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..at + 8 + len];
        match magic {
            PAYLOAD_MAGIC => payload = Some(ContentManifestPayload::parse_from_bytes(body).unwrap()),
            METADATA_MAGIC => metadata = Some(ContentManifestMetadata::parse_from_bytes(body).unwrap()),
            SIGNATURE_MAGIC => assert_eq!(len, 0),
            other => panic!("magic desconhecido {other:#x}"),
        }
        at += 8 + len;
    }
    (payload.unwrap(), metadata.unwrap())
}

fn decrypt_name(name: &str) -> String {
    use base64::Engine;
    assert!(name.ends_with('\n') && name.split_terminator('\n').all(|line| line.len() <= 64), "base64 in lines of 64 ended by a newline: {name:?}");
    let joined: String = name.split('\n').collect();
    String::from_utf8(decrypt(base64::engine::general_purpose::STANDARD.decode(joined).unwrap())).unwrap().trim_end_matches('\0').to_string()
}

fn item_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), vec![1u8; 2500]).unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("b.bin"), (0..3000u32).map(|i| i as u8).collect::<Vec<_>>()).unwrap();
    std::fs::write(dir.path().join("sub").join("copia.txt"), vec![1u8; 2500]).unwrap();
    dir
}

#[test]
fn the_depot_lists_every_file_with_encrypted_names_and_correct_chunks() {
    let dir = item_folder();
    let built = build_depot(dir.path(), 480, KEY, 1024, 1_700_000_000).unwrap();
    let (payload, meta) = parse_manifest(&built.manifest);
    assert_eq!((meta.depot_id(), meta.gid_manifest(), meta.filenames_encrypted(), meta.creation_time()), (480, 0, true, 1_700_000_000));
    assert_eq!(built.files, 3);
    let mut names: Vec<(String, u32, u64)> = payload.mappings.iter().map(|m| (decrypt_name(m.filename()), m.flags(), m.size())).collect();
    names.sort();
    assert_eq!(
        names,
        vec![("a.txt".into(), 0, 2500), ("sub".into(), 64, 0), ("sub\\b.bin".into(), 0, 3000), ("sub\\copia.txt".into(), 0, 2500)]
    );
    let a = payload.mappings.iter().find(|m| decrypt_name(m.filename()) == "a.txt").unwrap();
    assert_eq!(a.chunks.len(), 3, "2500 bytes in chunks of 1024");
    assert_eq!(a.chunks.iter().map(|c| c.cb_original()).collect::<Vec<_>>(), [1024, 1024, 452]);
    assert_eq!(a.chunks.iter().map(|c| c.offset()).collect::<Vec<_>>(), [0, 1024, 2048]);
    assert_eq!(a.sha_content(), Sha1::digest(vec![1u8; 2500]).as_slice());
    assert_eq!(a.chunks[0].crc(), adler32(&[1u8; 1024]));
}

#[test]
fn identical_chunks_are_uploaded_once_and_each_one_decodes_to_its_sha() {
    let dir = item_folder();
    let built = build_depot(dir.path(), 480, KEY, 1024, 1).unwrap();
    let (payload, meta) = parse_manifest(&built.manifest);
    let listed: usize = payload.mappings.iter().map(|m| m.chunks.len()).sum();
    assert_eq!(listed, 3 + 3 + 3, "a.txt, copia.txt (3 each) and b.bin (3000 bytes = 3 chunks)");
    assert_eq!(built.chunks.len(), 4);
    assert_eq!(meta.unique_chunks() as usize, built.chunks.len());
    for chunk in &built.chunks {
        let plain = vz_decompress(&decrypt(chunk.payload.clone()));
        assert_eq!(Sha1::digest(&plain).as_slice(), chunk.sha);
        assert_eq!(crc32fast::hash(&chunk.payload), chunk.payload_crc);
    }
    assert_eq!(built.compressed_bytes, payload.mappings.iter().flat_map(|m| m.chunks.iter()).map(|c| u64::from(c.cb_compressed())).sum::<u64>());
}

#[test]
fn an_empty_folder_and_a_missing_one_are_refused() {
    let empty = tempfile::tempdir().unwrap();
    assert!(build_depot(empty.path(), 480, KEY, 1024, 1).unwrap_err().to_string().contains("no files"));
    assert!(build_depot(&empty.path().join("nao_existe"), 480, KEY, 1024, 1).unwrap_err().to_string().contains("not a folder"));
}

#[test]
fn chunk_ids_are_lowercase_hex() {
    let mut sha = [0u8; 20];
    sha[0] = 0xAB;
    sha[19] = 0x01;
    assert_eq!(chunk_id_hex(&sha), "ab00000000000000000000000000000000000001");
}

#[test]
fn the_manifest_crc_covers_the_length_and_the_payload() {
    assert_eq!(manifest_crc(b"hello manifest payload"), 312314555);
}

#[test]
fn a_built_manifest_carries_the_crc_of_its_encrypted_payload() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"abc").unwrap();
    let built = build_depot(dir.path(), 480, KEY, 1024, 1).unwrap();
    let (payload, meta) = parse_manifest(&built.manifest);
    assert_eq!(meta.crc_encrypted(), manifest_crc(&payload.write_to_bytes().unwrap()));
    assert_ne!(meta.crc_encrypted(), 0);
}

#[test]
fn the_zip_is_byte_for_byte_what_steams_templates_say_and_other_readers_open_it() {
    let plain = b"a manifest, a manifest, a manifest, a manifest".to_vec();
    let zipped = zip_compress(&plain).unwrap();
    assert_eq!(&zipped[..14], &[0x50, 0x4b, 0x03, 0x04, 0x14, 0, 0, 0, 0x08, 0, 0, 0, 0x21, 0], "local header: version 20, no flags, deflate, 1980-01-01");
    assert_eq!(&zipped[26..31], &[1, 0, 0, 0, b'z'], "one entry named z, no extra field");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zipped)).unwrap();
    assert_eq!(archive.len(), 1);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("z").unwrap(), &mut out).unwrap();
    assert_eq!(out, plain);
}

#[test]
fn the_vsz_container_has_the_header_frame_and_15_byte_footer_of_the_client_writer() {
    let plain = vec![7u8; 5000];
    let vsz = vsz_compress(&plain).unwrap();
    assert_eq!(&vsz[..4], b"VSZa");
    let crc = crc32fast::hash(&plain);
    assert_eq!(vsz[4..8], crc.to_le_bytes());
    let footer = &vsz[vsz.len() - 15..];
    assert_eq!(footer[..4], crc.to_le_bytes());
    assert_eq!(footer[4..12], (plain.len() as u64).to_le_bytes());
    assert_eq!(&footer[12..], b"zsv");
    let frame = &vsz[8..vsz.len() - 15];
    assert_eq!(zstd::bulk::decompress(frame, plain.len()).unwrap(), plain);
}

#[test]
fn a_signed_manifest_verifies_with_the_public_half_of_the_build_key() {
    use rsa::pkcs8::EncodePrivateKey;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"abc").unwrap();
    let built = build_depot(dir.path(), 480, KEY, 1024, 1).unwrap();
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).unwrap();
    let der = key.to_pkcs8_der().unwrap();
    let signed = sign_manifest(&built.manifest, der.as_bytes()).unwrap();
    let section_end = |from: usize| from + 8 + u32::from_le_bytes(signed[from + 4..from + 8].try_into().unwrap()) as usize;
    let signed_end = section_end(section_end(0));
    assert_eq!(&signed[..signed_end], &built.manifest[..signed_end], "payload and metadata are untouched");
    let signature_body = &signed[signed_end + 8..signed.len() - 4];
    assert_eq!(signature_body.len(), 131, "a ContentManifestSignature with 128 signature bytes, like the real manifests");
    let message = steam_vent_proto::content_manifest::ContentManifestSignature::parse_from_bytes(signature_body).unwrap();
    let mut digest_info = SHA1_DIGEST_INFO.to_vec();
    digest_info.extend_from_slice(&Sha1::digest(&signed[..signed_end]));
    key.to_public_key().verify(rsa::Pkcs1v15Sign::new_unprefixed(), &digest_info, message.signature()).unwrap();
    assert_eq!(u32::from_le_bytes(signed[signed.len() - 4..].try_into().unwrap()), 0x32C4_15AB);
}
