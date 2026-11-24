# B1: P2P with official Steam

Goal: have the shim connect P2P (GameNetworkingSockets, `ISteamNetworkingSockets`/`ISteamNetworking`) to official Steam players, and not only between shims.
This note keeps what is already known, where it comes from and what is missing.

## Status

| Phase | What | Status |
|---|---|---|
| 1 | Steam issues a Valve-signed network certificate for a key of ours | **done**: `aurelia debug-cert <app_id>` |
| 2 | Find out how Steam carries P2P signaling | **done**: protos + routing of 5434 confirmed between sessions of the same account (`aurelia debug-signal`); still missing between different accounts |
| 3 | Direct P2P (ICE): embed the open-source GameNetworkingSockets | **done**: enabled by default (`AURELIA_SHIM_GNS=0` goes back to the built-in P2P); games talk to the library (Sockets012/013, Messages002, Utils004 native; older versions through the adapter); real connection tested by IP, P2P and Messages, including **between two processes with signaling through Steam and ICE transport**, in 64 and 32 bits (`tests/gns_loop_run.sh`) |
| 3b | Legacy `ISteamNetworking` with official Steam peers | **blocked**: in the real client it does not use the new library, but the old protocol (libjingle); capturing the format requires a second account. Between shims it still goes through the built-in P2P (`p2p.c`) |
| 4 | Valve's SDR relay | **not done**: client↔relay protocol is closed (see "Phase 4") |

## Phase 1: certificate

EMsg 5621 `ClientNetworkingCertRequest` → response 5622. Request: `key_data` = **raw Ed25519 public key (32 bytes)** and `app_id`.
Response: `cert` (a serialized `CMsgSteamDatagramCertificate`), `ca_key_id` (fixed64) and `ca_signature` (64 bytes).
Measured with app 480: `eresult=1`, `identity_string = "steamid:<id>"`, `legacy_steam_id`, `app_ids=[480]`, validity of 48 h (`time_expiry - time_created = 172800`),
`ca_key_id = 13151444190962006299`. Wrapping the key in a `CMsgSteamDatagramCertificate` inside `key_data` also returns a certificate, but Valve treats the
wrapper as if it were the key: the right format is the raw one.
Not verified: the signature against Valve's public key, and whether an official peer accepts a certificate issued to an Aurelia session (only phase 3 shows that).

## Phase 2: signaling

What is in the protos (`steam-vent-proto-steam`), still **without a capture of real traffic**:

- **Transport:** EMsg 5434 `ClientP2PConnectionInfo` `{steam_id_dest, steam_id_src, app_id, candidate, legacy_connection_id_src, rendezvous}` and 5435
  `ClientP2PConnectionFailInfo` `{steam_id_dest, steam_id_src, app_id, ep2p_session_error, connection_id_dest, close_reason, close_message}`.
  Steam delivers it to the destination peer as a push.
- **Contents of `rendezvous`:** `CMsgSteamNetworkingP2PRendezvous` (`steamnetworkingsockets_messages.proto`), with `connect_request` (signed `crypt` and signed `cert`,
  virtual ports), `connect_ok`, `connection_closed`, `reliable_messages` (they carry ICE, `CMsgICERendezvous`) and `application_messages`.
  Identities as text (`from_identity` / `to_identity`, e.g. `steamid:...`) and connection ids (`from_connection_id` / `to_connection_id`).
- **Measured routing** (`aurelia debug-signal <app> --sender 1|2 --playing both|1|2|none --dest <steamid>`; two sessions of the same account, different logon ids, 5434 sent to itself):
  - Steam **delivers** the 5434 **to the session that is playing the app** (declared `ClientGamesPlayed`), whether it is the sender or another one: `sender 1/playing 1 → 1`, `sender 2/playing 2 → 2`,
    `sender 1/playing 2 → 2`, `sender 2/playing 1 → 1`. With nobody playing (`none`) **nothing** arrives. It is not an echo to the sender.
  - Only one session of the account can play at a time: with both declaring the app, Steam drops one (`closed connection`).
  - The message arrives **rewritten**: without `steam_id_dest`, with `steam_id_src` = sender, and `legacy_connection_id_src` = `from_connection_id` of the `rendezvous` (Steam interprets the
    `rendezvous` on the server). The `rendezvous` arrives intact.
  - Nonexistent destination (maximum account id): **nothing comes back**, not even 5435. Steam silently discards it; whoever connects relies on the timeout and retransmission of GameNetworkingSockets.
  - Destination = **offline friend** (tested with a friend, authorized by them): also **nothing** comes back, not even 5435; Steam silently discards it.
  - **Not yet measured:** delivery between two **different** accounts (and whether it requires friendship, or that the destination plays the same app). It needs a second account.
- There are also 813 `ClientP2PIntroducerMessage` and 7001/7002 `ClientUDSP2PSession*`; not yet investigated.

## Open design decision (phase 3)

Valve's GameNetworkingSockets is open source and already does ICE and encryption, with pluggable signaling (`ConnectP2PCustomSignaling`). The closed part is SDR and the
signaling built into Steam. Two routes for phase 3:

1. **Reimplement** the protocol in the shim's `p2p.c` (handshake with certificates, encryption, ICE).
2. **Embed the open-source GameNetworkingSockets** as the transport engine, with signaling through EMsg 5434 via Aurelia and Valve's certificate (phase 1).
   Risks: compiling with mingw, the dependencies (protobuf, cryptography), and the list of trusted CAs (to accept certificates from official peers, Valve's CA
   public key is needed).

Sources consulted: README_P2P of GameNetworkingSockets (ICE with WebRTC, pluggable signaling, SDR not open source), protos from `steam-vent-proto-steam`.

## Phase 3: trust in Valve's certificates (measured)

The open-source GameNetworkingSockets trusts **one** root defined at compile time (`STEAMNETWORKINGSOCKETS_HARDCODED_ROOT_CA_KEY`, OpenSSH format) and accepts intermediate
certificates through `CertStore_AddCertFromBase64`. Valve's chain can be assembled from public data:

- **Root** (self-signed): id `18220590129359924542` (`SHA-256(32-byte key)`, first 8 bytes in little-endian). It is in plain text in the native `steamclient.so`
  (`ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJrsoE4XUc5iaNVpACyh4fobLbwm02tOo6AIOtNygpuE`).
- **Issuing CA** `13151444190962006299`: the one that signed the certificate Steam gave to Aurelia (phase 1), signed by the root. It comes in `certs` of the SDR configuration.
- **Public SDR configuration**: `https://api.steampowered.com/ISteamApps/GetSDRConfig/v1/?appid=<id>` (no key). It brings `pops` (37 points of presence with the relays' IPs and port ranges),
  `certs` (39 certificates: the root, the issuing CA and per-app certificates), `relay_public_key`, `revoked_keys` (1) and `typical_pings`.
  The `pops` and `typical_pings` data serve phase 4 and the five accepted B1 lines in conformance.

There is no need to ship a Valve binary: the root key is a public constant and the configuration comes from the API at run time.

Toolchain: mingw g++ (64 and 32 bits), cmake, ninja and protoc exist. GameNetworkingSockets accepts `USE_CRYPTO=BCrypt` (native Windows cryptography) and
`USE_CRYPTO25519=Reference`, so the only external dependency is the protobuf library for mingw (it needs to be compiled).

### Signatures verified in code (Python `cryptography`, Ed25519)

- The 39 certificates of the SDR configuration verify against the root key found in `steamclient.so` (including the self-signed root and the CA `13151444190962006299`).
- The certificate Steam issued to Aurelia (phase 1, 97 bytes, 64-byte signature over the `cert` bytes) verifies against the CA `13151444190962006299`.
- Complete chain: **root (steamclient) → CA 13151444190962006299 → Aurelia certificate**, all valid. The signature is over the serialized bytes of `cert`.

### Compiling GameNetworkingSockets for mingw (64 and 32 bits: done)

Scripts in `plus/tools/gns/`: `build-protobuf.sh <x86_64|i686>` and the `mingw-*.cmake` toolchain files; `include-alias/` redirects `<Windows.h>` and `<ShlObj.h>` to mingw's
lowercase headers (Valve's code uses uppercase). Sources downloaded to `plus/run/gns/` (outside version control): protobuf v36.1 (abseil fetched by cmake) and commit
`d534c19` of GameNetworkingSockets. Configuration: `-DBUILD_STATIC_LIB=ON -DUSE_CRYPTO=BCrypt -DUSE_CRYPTO25519=Reference -DENABLE_ICE=ON -DUSE_STEAMWEBRTC=OFF -DProtobuf_USE_STATIC_LIBS=ON`,
with the host's `protoc` 36.1.
Result: `libGameNetworkingSockets_s.a` (3.5 MB) that exports `GameNetworkingSockets_Init`, `SteamAPI_ISteamNetworkingSockets_ConnectP2PCustomSignaling`,
`..._ReceivedP2PCustomSignal(2)`, `..._SetCertificate`. Not yet linked to anything and without Valve's root defined
(`STEAMNETWORKINGSOCKETS_HARDCODED_ROOT_CA_KEY`).

The 32-bit version (`build-protobuf.sh i686` and the same cmake configuration with `mingw-i686.cmake`) also compiled without errors and exports the same functions.

### Valve's root in the library and isolated test (done)

`tools/gns/valve_root.h` defines `STEAMNETWORKINGSOCKETS_HARDCODED_ROOT_CA_KEY` with the `steamclient` root key (the line must carry the `ID<id>` comment, which the library checks).
`tools/gns/build-gns.sh <arch>` compiles with `-DSTEAMNETWORKINGSOCKETS_STATIC_LINK` (without it the library marks everything `dllexport` and its functions leak as exports of the shim's DLL)
and applies `patch-gns.sh`, which adds `GameNetworkingSockets_SetAppID(uint32)` (on Steam it is the client that tells the networking utility the app; without it `SetCertificate` refuses the
certificate: "Cert does not authorize us for App 0").

Test: `tools/gns/test/run-trust-test.sh 480` (Steam closed). It runs `gns_trust_test.cpp` under Wine, which (1) adds the 39 certificates of the SDR configuration to the store through
`CertStore_AddCertFromBase64`, (2) asks the library for the certificate request and hands the public key to Aurelia, (3) Aurelia gets the signed certificate for **that** key from Steam
(`aurelia debug-cert <app> --key <hex>` prints `SIGNED_B64`), (4) `SetCertificate`. Result: **ok**, identity `steamid:<id>`, no warning about an unverifiable chain. Negative control
(`NO_CERTS=1`, without the CA certificates): the library warns `CA key 13151444190962006299 is not known to us`. In other words, the open-source library, with the root built in, accepts the certificate
Steam issues to Aurelia.
Note: `SetCertificate` only **warns** when it cannot verify the chain; that is why the test captures the library's warnings.

### Linking into the shim (done, not used yet)

`shim/build.sh` links the library when `run/gns/build-gns-<arch>/src/libGameNetworkingSockets_s.a` exists (`SHIM_NO_GNS=1` turns it off): each `.c` becomes an object, the final link is in C++
(`shim/gns_glue.cpp`, with `shim_gns_init` and `shim_gns_kill`, not yet called). No symbol collisions; DLL exports identical to the previous ones (44, at both sizes). The DLL went from
1.9 MB to 6.4 MB (64 bits) and 7.8 MB (32 bits).

### Adapter in the shim: identity, certificate and signaling (done, certificate tested end to end)

`shim/gns_glue.cpp` (compiled with `-DSHIM_WITH_GNS`; `shim_gns_start_async` is called in `CreateInterface("SteamClient…")`, unless `AURELIA_SHIM_GNS=0`):

1. `GameNetworkingSockets_SetAppID` and `Init` with the identity `steamid:<account>`;
2. public SDR configuration through WinHTTP: `CertStore_AddCertFromBase64` for each `certs` and `CertStore_AddKeyRevocation` for each `revoked_keys`;
3. `GetCertificateRequest` → public key → EMsg 5621 through the bridge relay (`shim_cm_call`) → the response (fields 4/5/6, the same as in `CMsgSteamDatagramCertificateSigned`) goes straight into `SetCertificate`;
4. **outgoing signaling:** `ISteamNetworkingConnectionSignaling::SendSignal` packs the signal into `CMsgClientP2PConnectionInfo` (`steam_id_dest`, `steam_id_src`, `app_id`, `rendezvous`) and sends it as EMsg 5434 through `shim_cm_post` (single-thread queue, order preserved); the global callback `Callback_CreateConnectionSignaling` makes the ordinary `ConnectP2P` use this;
5. **incoming signaling:** 5434 pushed by Steam (`shim_cm_subscribe`/`shim_cm_on_push`) → `ReceivedP2PCustomSignal`; `OnConnectRequest` and `SendRejectionSignal` answer through the same path.

Test (`tests/gns_start_probe.c` through `launch.sh 480` with `AURELIA_PLUS_PROBE_EXE`, real bridge and Steam): shim log `GNS: SDR configuration: 39 certificates, 0 rejected, 1 revoked keys`,
`GNS[5] AuthStatus (steamid:...):  OK`, `GNS: Valve certificate installed for app 480`, `GNS: ready`. The `AuthStatus OK` comes from the library itself: it considers the certificate trusted.
Not tested: the signaling paths with a real peer.

## Games' interface linked to the library (done, tested)

- **Native objects.** Steam is also this library: `SteamNetworkingMessages002`, `SteamNetworkingUtils004` and `SteamNetworkingSockets013` have the same method table as Steam
  (checked against the catalog, including the argument bytes in 32 bits), and `GetISteamGenericInterface` returns the library's own object (`shim_gns_native`).
  Steam's `SteamNetworkingSockets012` only differs in `SendMessages` (without the `bool bDeleteFailedMessages`, which 013 added): the game receives a bridge object whose table is made of
  trampolines (`mov rcx,[rcx+8]; jmp` / in 32 bits `mov ecx,[ecx+4]; jmp`) to the library's methods, with `SendMessages` adapted.
- **Older versions** (Sockets002–009, Utils001–003) stay in the shim's adapter (`ifaces/sockets.c`, `networkingutils.c`), which delegates each method (`GNS_DELEGATE`).
- **Callbacks:** the library's globals become Steam callbacks in the shim's queue (1221, 1222, 1281, 1251, 1252), delivered when the game processes callbacks (`Steam_BGetCallback` calls
  `RunCallbacks`). The 1222 (certificate state) was not dispatched by the library without SDR: fixed in the patch.
- **Startup:** `Init` with the identity as soon as the game opens Steam; certificate and SDR configuration in a thread that renews them every 24 h (the certificate is valid for 48 h), or in 1 min if it fails.
  If the library does not come up, the shim uses the built-in P2P. ICE with STUN (`stun.l.google.com:19302`, `stun.cloudflare.com:3478`; Steam does not publish its own).
- **Signaling between processes of the same account:** Steam delivers the 5434 to the session; each process discards the echo of its own signals (`from_connection_id` that it sent itself) and the
  `AP2P` signals of the built-in P2P. The bridge now has **one queue of pushed messages per process** (before, one process stole the other's messages).

Fixes in the library (`tools/gns/patch-gns.sh`) and in the adapter:
- null pointers are refused before entering the library (a failure inside it leaves the global lock stuck and all following calls hang; real Steam has the same behavior,
  seen in the probe);
- the library's `atexit` does not run at process shutdown (the service thread has already died, perhaps holding the lock, and the game would not close);
- `GameNetworkingSockets_TEST_EnableP2PLoopback`: for testing only (`AURELIA_SHIM_GNS_TEST_NO_LOOPBACK=1`), it turns off the local shortcut of P2P to one's own identity.

Tests:
- `tests/gns_loop_run.sh <x86_64|i686>` (real bridge and Steam, app 500): **IP** (listen and connect on 127.0.0.1, messages in both directions, the 012's `SendMessages` through the bridge object),
  **P2P** and **Messages** in the same process (the library uses the local shortcut), and **P2P between two processes**: the 5434 goes out through the bridge, comes back through Steam, each side's Valve certificate is
  validated and the connection closes over **ICE** (`P2P ICE steamid:… vport 7`), with messages and Messages. **0 failures in 64 and in 32 bits.**
- Network conformance (`PROBE_ONLY=SteamNetworking`): 0 unexplained differences; the native ones match real Steam line by line, including the hangs after a null pointer.

Not yet tested: an official Steam peer on **another account** (it needs a second account or an online friend who is playing).

## Legacy `ISteamNetworking` (measured; blocked)

In the real `steamclient64.dll` the legacy one is `CP2PNetworkingSession` over libjingle (`cricket::P2PTransportChannel`), with the candidates in the `candidate` field of 5434: it does not go through the new
library. Measured with `tests/legacy_p2p_probe.c` in the real client: `SendP2PPacket` to one's own account is resolved locally (nothing goes out to Steam), so with a single account there is nothing to capture.
To talk to official peers in games that use the legacy one, that protocol has to be captured between two accounts. Between shims, the legacy one still goes through the built-in P2P.

## Phase 4: SDR (assessed; not done)

- The SDR message definitions are embedded in `steamclient64.dll` (`steamdatagram_messages_sdr.proto`, extracted: `ESteamDatagramMsgID` with 34 types, `CMsgSteamDatagramRouterPingReply`,
  `ConnectRequest`, `P2PSessionRequest`…). The ping request to the relay (type 1) is **not** protobuf: candidate formats sent to a São Paulo relay got no answer.
- The real client, under Proton, did not complete `InitRelayNetworkAccess` in the test program (`tests/sdr_ping_probe.c`), and so the packets could not be captured with `strace`.
- What is missing, therefore, is reverse engineering of the binary for packet framing, ping, the session with the relay and encryption, and then testing against the production relays. Without SDR,
  P2P works over ICE; it does not work when both sides are behind symmetric NAT, and there is no connection to the dedicated servers hosted by Valve (CS2, Dota 2, TF2).
