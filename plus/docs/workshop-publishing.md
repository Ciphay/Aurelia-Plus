# Publishing to the Workshop (C3)

On 17/Nov/2026 Aurelia started creating, uploading and updating Workshop items, through the CLI and the shim's `ISteamUGC`.

## CLI
```bash
aurelia workshop publish 480 --content ./my-mod --title "My mod" --description "..." --tag map --tag coop --preview cover.png   # new item, private
aurelia workshop publish 480 --item 123456789 --content ./my-mod --change-note "v2"                                           # update
aurelia workshop publish 480 --item 123456789 --visibility public                                                              # metadata only
aurelia workshop delete 480 123456789
```
A new item is born **private**: nothing becomes public without `--visibility public`.

## The path (read from the real client and the protos)
1. `ClientUCMPublishFile` creates the item (in progress, invisible).
2. `ContentBuilder.InitDepotBuild` provides the depot key (32 bytes), the chunk size and the upload server.
3. Aurelia builds the folder locally (`steam_client/workshop_build.rs`): 1 MiB chunks, SHA-1 of each one (the chunk id), `VSZa` compression (Zstandard; what the server asks for), AES-256-CBC
   with the IV encrypted in ECB at the front, encrypted file names in base64, and the serialized manifest (payload, metadata with the CRCs, RSA signature, end marker) inside a zip.
4. `ContentBuilder.StartDepotUpload` opens the upload and returns the `depot_build_handle`.
5. `POST https://<host>/manifestupload?appid&buildid=<handle>` with the manifest; `ContentBuilder.GetMissingDepotChunks` says what is missing;
   `PUT https://<host>/chunkupload/<sha>?appid&depotid&buildid&compressed_crc&compressed_size` for each chunk.
6. `ContentBuilder.FinishDepotUpload` returns the manifest id, and `ClientUCMUpdatePublishedFile` points the item at it and saves the title, description, tags and visibility.
The preview is uploaded through `Cloud.BeginHTTPUpload` (public) + `CommitHTTPUpload` and the item references it by name.

In the legacy `ISteamRemoteStorage` (`PublishWorkshopFile`, `UpdatePublishedFile*`, `DeletePublishedFile`) the path is different: `ClientUCM*` messages through the relay (an app Cloud file, no folder and no SteamPipe), and the lists, details, votes and downloads use the `PublishedFile` service.

## Status: WORKS (live on 21/Nov/2026)
On Spacewar (480), private item: create, upload content, download it back (`workshop install`, the file came out identical), update with `--item` (version 2 with one extra file, which also came down in the download) and delete. No test item was left on the account.

## What the server required (and what the first design got wrong)
The generic 400 from `manifestupload` hid several errors at once; the server only writes the reason in two cases ("invalid compression algorithm" and "corrupt chunk data"), so each step was found by reading the real client's code (`steamclient.so`, static analysis only, the client was never run) and checking against the real manifests (104 in `depotcache` and one from the CDN):
1. **Compression (`compression_method` of `InitDepotBuild`, field 12 = 7)**: the chunks go as `VSZa` (Zstandard), not `VZ`/LZMA. Container: `VSZa` + CRC-32 of the plain data, one zstd frame, and a footer with CRC-32 + **64-bit size** + `zsv` (15 bytes; the CDN's reader tolerates 11). The values 1 (zip) and 5 (`VZ`) are refused by the server with "invalid compression algorithm".
2. **The manifest goes in a zip**, byte for byte like the client's models: one `z` item in deflate, version 20, no flags, date 1980-01-01, no attributes. The zip from the `zip` library (Unix date and attributes) is refused.
3. **`crc_encrypted`** = CRC-32 of the size (4 bytes, little endian) followed by the serialized payload (with the encrypted names); `crc_clear` = 0, written explicitly. Checked against 104 real manifests and the CDN one.
4. **Signature**: SHA-1 of the payload and metadata sections (with the magic numbers and the sizes), signed with the RSA key that `InitDepotBuild` delivers (1024-bit PKCS#8 private key, PKCS#1 v1.5); it goes as `ContentManifestSignature` (131 bytes).
5. **File names**: the plain text is the name plus a 0 byte (and PKCS#7 padding); the base64 comes out in lines of 64 characters, each ended by `\n`, the last one too (like OpenSSL's base64 BIO). The mappings are sorted case-insensitively by the encrypted name, as in the real manifest.
6. **`crc` of each chunk** = Adler-32 of the plain data **with seed 0** (not 1 as in zlib). Checked against files of installed games and their real manifests.
7. The nine metadata fields are always written (including `gid_manifest` 0 and `crc_clear` 0).
The `compressed_crc` of `PUT /chunkupload` is the CRC-32 of the body sent (already encrypted), same as the client's.

## What is verified and what is not
- **Live (21/Nov):** the whole flow above, on Spacewar. Automated tests (without Steam): the builder, the zip, the `VSZa` container, the signature (matches the public half of the key), the CRC and the ordering; the messages, URLs and bridge ops; the game flow in the shim in 64 and 32 bits.
- **Not verified live:** extra previews, videos and version requirements in a real publish (they go in `ClientUCMUpdatePublishedFile`, only tested at the message level), the preview upload through `Cloud.BeginHTTPUpload` and the language numbering.
- **Limits:** an item larger than 2 GiB compressed is refused; the content is built in memory; only the chunk format the server asks for (`compression_method` 7 = `VSZa`, 5 = `VZ` and 1 = zip are accepted in the code but the current server only accepts 7).

Debug variables (development only, the daemon does not inherit variables from the client: run `aurelia kill` first): `AURELIA_WS_DUMP` (the `InitDepotBuild` response), `AURELIA_WS_MANIFEST_DUMP` (the built manifest), `AURELIA_WS_MANIFEST_FILE` (sends that file instead), `AURELIA_WS_DECLARED_SIZE`, `AURELIA_WS_UPLOAD_HOST`, `AURELIA_WS_LOCAL_CS`, `AURELIA_WS_DEPOT`, `AURELIA_WS_METHOD` (forces the chunk compression); and the commands `aurelia workshop mine <app>` and (hidden) `workshop debug-manifest <app> <depot> <manifest> <file>`.

## Dedicated server and playtime
- `BInitWorkshopForGameServer(depot, folder)` sends the folder to Aurelia, which then installs the Workshop items there instead of in the Steam library.
- `StartPlaytimeTracking`, `StopPlaytimeTracking` and `StopPlaytimeTrackingForAllItems` call the `PublishedFile` service (up to 100 items per call).
