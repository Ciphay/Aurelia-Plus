use std::io::Read;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, generic_array::GenericArray, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::Aes256;
use anyhow::{anyhow, bail, Context, Result};
use sha1::{Digest, Sha1};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use steam_vent_proto::content_manifest::{content_manifest_payload::file_mapping::ChunkData, content_manifest_payload::FileMapping, ContentManifestMetadata, ContentManifestPayload, ContentManifestSignature};
use steam_vent_proto::protobuf::Message;

pub const DEFAULT_CHUNK_SIZE: usize = 1 << 20;
pub const MAX_BUILD_BYTES: u64 = 2 << 30;

const PAYLOAD_MAGIC: u32 = 0x71F6_17D0;
const METADATA_MAGIC: u32 = 0x1F48_12BE;
const SIGNATURE_MAGIC: u32 = 0x1B81_B817;
const END_MAGIC: u32 = 0x32C4_15AB;

const FLAG_DIRECTORY: u32 = 64;
const FLAG_EXECUTABLE: u32 = 32;

#[derive(Debug, Clone)]
pub struct BuiltChunk {
    pub sha: [u8; 20],
    pub payload: Vec<u8>,
    pub payload_crc: u32,
}

#[derive(Debug)]
pub struct BuiltDepot {
    pub manifest: Vec<u8>,
    pub chunks: Vec<BuiltChunk>,
    pub files: usize,
    pub original_bytes: u64,
    pub compressed_bytes: u64,
}

pub fn encrypt_with_iv(data: &[u8], key: [u8; 32], iv: [u8; 16]) -> Vec<u8> {
    let mut iv_block = iv;
    Aes256::new(GenericArray::from_slice(&key)).encrypt_block(GenericArray::from_mut_slice(&mut iv_block[..]));
    let body = cbc::Encryptor::<Aes256>::new(GenericArray::from_slice(&key), GenericArray::from_slice(&iv)).encrypt_padded_vec_mut::<Pkcs7>(data);
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(&iv_block);
    out.extend_from_slice(&body);
    out
}

fn random_iv() -> [u8; 16] {
    rand::random::<[u8; 16]>()
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (0u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

pub fn vz_compress(plain: &[u8]) -> Result<Vec<u8>> {
    use xz2::stream::{LzmaOptions, Stream};
    let options = LzmaOptions::new_preset(6).map_err(|e| anyhow!("lzma options: {e}"))?;
    let mut stream = Stream::new_lzma_encoder(&options).map_err(|e| anyhow!("lzma encoder: {e}"))?;
    let mut alone = Vec::with_capacity(plain.len() / 2 + 64);
    let mut input = plain;
    loop {
        let before_in = stream.total_in();
        let before_out = alone.len();
        alone.reserve(64 * 1024);
        let status = stream
            .process_vec(input, &mut alone, if input.is_empty() { xz2::stream::Action::Finish } else { xz2::stream::Action::Run })
            .map_err(|e| anyhow!("lzma: {e}"))?;
        input = &input[(stream.total_in() - before_in) as usize..];
        if matches!(status, xz2::stream::Status::StreamEnd) {
            break;
        }
        if input.is_empty() && alone.len() == before_out && !matches!(status, xz2::stream::Status::Ok) {
            bail!("lzma encoder stalled");
        }
    }
    if alone.len() < 13 {
        bail!("lzma output too small");
    }
    let crc = crc32fast::hash(plain);
    let mut out = Vec::with_capacity(alone.len() + 10);
    out.extend_from_slice(&[b'V', b'Z', b'a']);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&alone[..5]);
    out.extend_from_slice(&alone[13..]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out.extend_from_slice(&[b'z', b'v']);
    Ok(out)
}

pub fn vsz_compress(plain: &[u8]) -> Result<Vec<u8>> {
    let frame = zstd::bulk::compress(plain, 9).context("zstd compression failed")?;
    let crc = crc32fast::hash(plain);
    let mut out = Vec::with_capacity(frame.len() + 23);
    out.extend_from_slice(b"VSZa");
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&frame);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
    out.extend_from_slice(b"zsv");
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Zip,
    Vz,
    Vsz,
}

impl Compression {
    pub fn from_method(method: u32) -> Compression {
        match method {
            1 => Compression::Zip,
            5 => Compression::Vz,
            _ => Compression::Vsz,
        }
    }

    pub fn compress(self, plain: &[u8]) -> Result<Vec<u8>> {
        match self {
            Compression::Zip => zip_compress(plain),
            Compression::Vz => vz_compress(plain),
            Compression::Vsz => vsz_compress(plain),
        }
    }
}

pub fn zip_compress(plain: &[u8]) -> Result<Vec<u8>> {
    use flate2::{write::DeflateEncoder, Compression as Level};
    use std::io::Write;
    let mut deflate = DeflateEncoder::new(Vec::new(), Level::new(6));
    deflate.write_all(plain)?;
    let data = deflate.finish()?;
    let crc = crc32fast::hash(plain);
    let (csize, usize_) = (data.len() as u32, plain.len() as u32);
    let mut out = Vec::with_capacity(data.len() + 100);
    out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&csize.to_le_bytes());
    out.extend_from_slice(&usize_.to_le_bytes());
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, b'z']);
    out.extend_from_slice(&data);
    let central = out.len() as u32;
    out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&csize.to_le_bytes());
    out.extend_from_slice(&usize_.to_le_bytes());
    out.extend_from_slice(&[0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'z']);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0x01, 0x00, 0x01, 0x00, 0x2f, 0, 0, 0]);
    out.extend_from_slice(&central.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    Ok(out)
}

pub fn chunk_payload(plain: &[u8], key: [u8; 32]) -> Result<Vec<u8>> {
    chunk_payload_with(plain, key, Compression::Vz)
}

pub fn chunk_payload_with(plain: &[u8], key: [u8; 32], compression: Compression) -> Result<Vec<u8>> {
    Ok(encrypt_with_iv(&compression.compress(plain)?, key, random_iv()))
}

fn encrypt_name(name: &str, key: [u8; 32]) -> String {
    use base64::Engine;
    let mut plain = name.as_bytes().to_vec();
    plain.push(0);
    let encoded = base64::engine::general_purpose::STANDARD.encode(encrypt_with_iv(&plain, key, random_iv()));
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 64 + 1);
    for line in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or_default());
        out.push('\n');
    }
    out
}

struct Entry {
    path: PathBuf,
    name: String,
    directory: bool,
    executable: bool,
}

fn collect(root: &Path) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).min_depth(1).follow_links(false).sort_by_file_name() {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        let file_type = entry.file_type();
        if !file_type.is_file() && !file_type.is_dir() {
            continue;
        }
        let rel = entry.path().strip_prefix(root).context("path outside the item folder")?;
        let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("\\");
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            file_type.is_file() && entry.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
        };
        #[cfg(not(unix))]
        let executable = false;
        out.push(Entry { path: entry.path().to_path_buf(), name, directory: file_type.is_dir(), executable });
    }
    Ok(out)
}

pub fn build_depot(dir: &Path, depot_id: u32, key: [u8; 32], chunk_size: usize, now: u32) -> Result<BuiltDepot> {
    build_depot_with(dir, depot_id, key, chunk_size, now, Compression::Vz)
}

pub fn build_depot_with(dir: &Path, depot_id: u32, key: [u8; 32], chunk_size: usize, now: u32, compression: Compression) -> Result<BuiltDepot> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let chunk_size = if chunk_size == 0 { DEFAULT_CHUNK_SIZE } else { chunk_size };
    let entries = collect(dir)?;
    let mut payload = ContentManifestPayload::new();
    let mut chunks: Vec<BuiltChunk> = Vec::new();
    let mut seen: std::collections::HashSet<[u8; 20]> = std::collections::HashSet::new();
    let (mut original, mut compressed, mut files, mut unique) = (0u64, 0u64, 0usize, 0u32);
    let mut stored = 0u64;
    for entry in &entries {
        let mut mapping = FileMapping::new();
        mapping.set_filename(encrypt_name(&entry.name, key));
        let mut name_hash = Sha1::new();
        name_hash.update(entry.name.to_lowercase().as_bytes());
        mapping.set_sha_filename(name_hash.finalize().to_vec());
        if entry.directory {
            mapping.set_flags(FLAG_DIRECTORY);
            mapping.set_size(0);
            mapping.set_sha_content(vec![0u8; 20]);
        } else {
            files += 1;
            let mut file = std::fs::File::open(&entry.path).with_context(|| format!("opening {}", entry.path.display()))?;
            let mut content_hash = Sha1::new();
            let (mut offset, mut buffer) = (0u64, vec![0u8; chunk_size]);
            loop {
                let mut filled = 0;
                while filled < chunk_size {
                    let n = file.read(&mut buffer[filled..])?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                if filled == 0 {
                    break;
                }
                let plain = &buffer[..filled];
                content_hash.update(plain);
                let sha: [u8; 20] = Sha1::digest(plain).into();
                let wire = chunk_payload_with(plain, key, compression)?;
                let mut data = ChunkData::new();
                data.set_sha(sha.to_vec());
                data.set_crc(adler32(plain));
                data.set_offset(offset);
                data.set_cb_original(filled as u32);
                data.set_cb_compressed(wire.len() as u32);
                mapping.chunks.push(data);
                original += filled as u64;
                compressed += wire.len() as u64;
                offset += filled as u64;
                if seen.insert(sha) {
                    stored += wire.len() as u64;
                    if stored > MAX_BUILD_BYTES {
                        bail!("the item is larger than {} MiB after compression; upload it in parts", MAX_BUILD_BYTES >> 20);
                    }
                    unique += 1;
                    let payload_crc = crc32fast::hash(&wire);
                    chunks.push(BuiltChunk { sha, payload: wire, payload_crc });
                }
                if filled < chunk_size {
                    break;
                }
            }
            mapping.set_size(offset);
            mapping.set_sha_content(content_hash.finalize().to_vec());
            mapping.set_flags(if entry.executable { FLAG_EXECUTABLE } else { 0 });
        }
        payload.mappings.push(mapping);
    }
    if files == 0 {
        bail!("{} has no files to publish", dir.display());
    }
    payload.mappings.sort_by_cached_key(|m| m.filename().to_ascii_lowercase());
    let mut metadata = ContentManifestMetadata::new();
    metadata.set_depot_id(depot_id);
    metadata.set_gid_manifest(0);
    metadata.set_creation_time(now);
    metadata.set_filenames_encrypted(true);
    metadata.set_cb_disk_original(original);
    metadata.set_cb_disk_compressed(compressed);
    metadata.set_unique_chunks(unique);
    metadata.set_crc_encrypted(manifest_crc(&payload.write_to_bytes()?));
    metadata.set_crc_clear(0);
    let manifest = serialize_manifest(&payload, &metadata)?;
    Ok(BuiltDepot { manifest, chunks, files, original_bytes: original, compressed_bytes: compressed })
}

pub fn manifest_crc(payload: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&(payload.len() as u32).to_le_bytes());
    hasher.update(payload);
    hasher.finalize()
}

pub fn serialize_manifest(payload: &ContentManifestPayload, metadata: &ContentManifestMetadata) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (magic, body) in [(PAYLOAD_MAGIC, payload.write_to_bytes()?), (METADATA_MAGIC, metadata.write_to_bytes()?), (SIGNATURE_MAGIC, Vec::new())] {
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
    }
    out.extend_from_slice(&END_MAGIC.to_le_bytes());
    Ok(out)
}

const SHA1_DIGEST_INFO: [u8; 15] = [0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14];

pub fn sign_manifest(manifest: &[u8], key_der: &[u8]) -> Result<Vec<u8>> {
    let section_end = |from: usize, magic: u32| -> Result<usize> {
        let head = manifest.get(from..from + 8).ok_or_else(|| anyhow!("the manifest is truncated"))?;
        if u32::from_le_bytes(head[..4].try_into()?) != magic {
            bail!("unexpected section in the manifest");
        }
        let end = from + 8 + u32::from_le_bytes(head[4..].try_into()?) as usize;
        if end > manifest.len() {
            bail!("the manifest is truncated");
        }
        Ok(end)
    };
    let signed_end = section_end(section_end(0, PAYLOAD_MAGIC)?, METADATA_MAGIC)?;
    let key = RsaPrivateKey::from_pkcs8_der(key_der).or_else(|_| RsaPrivateKey::from_pkcs1_der(key_der)).context("the RSA key from InitDepotBuild is not a private key")?;
    let mut to_sign = SHA1_DIGEST_INFO.to_vec();
    to_sign.extend_from_slice(&Sha1::digest(&manifest[..signed_end]));
    let signature = key.sign(Pkcs1v15Sign::new_unprefixed(), &to_sign).context("signing the manifest failed")?;
    let mut message = ContentManifestSignature::new();
    message.set_signature(signature);
    let body = message.write_to_bytes()?;
    let mut out = manifest[..signed_end].to_vec();
    out.extend_from_slice(&SIGNATURE_MAGIC.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&END_MAGIC.to_le_bytes());
    Ok(out)
}

pub fn chunk_id_hex(sha: &[u8; 20]) -> String {
    hex::encode(sha)
}

#[cfg(test)]
#[path = "workshop_build_tests.rs"]
mod tests;
