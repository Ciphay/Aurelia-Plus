# Microtransaction: Steam message format (being decoded)

Source: `steamclient64.dll` (Windows Steam, RTTI and code). No protobuf: these messages use the legacy header (without the protobuf marker).

## EMsgs (confirmed in `enums_clientserver.proto`)
| EMsg | Name | Direction |
|---|---|---|
| 5504 (0x1580) | `ClientMicroTxnAuthRequest` | Steam → client (push): there is a purchase to authorize |
| 5505 (0x1581) | `ClientMicroTxnAuthorize` | client → Steam: approve or deny |
| 5506 (0x1582) | `ClientMicroTxnAuthorizeResponse` | Steam → client: result |
| 5508 (0x1584) | `ClientGetMicroTxnInfo` | client → Steam: request the purchase data |
| 5509 (0x1585) | `ClientGetMicroTxnInfoResponse` | Steam → client: items, price, currency... |

## 36-byte legacy header (seen being built in `CAPIJobAuthorizeMicroTxn`)
`u32 emsg` · `u8 header_size = 0x24` · `u16 header_version = 2` · `u64 target_job_id = -1` · `u64 source_job_id = -1` · `u8 canary = 0xEF` ·
`u64 steam_id` · `i32 session_id`. The body follows right after.

## 5505 `ClientMicroTxnAuthorize` (decoded: body is built at `0x1389fefc0`)
12-byte body: `u64 order_id` (+0) · `u32 auth_response` (+8).
`auth_response`: 1 = authorize, 2 = deny, 3 = auto-deny (the DLL only accepts 1 to 3; text "eAuthResponse == Authorize || Deny || AutoDeny").

## 5506 `ClientMicroTxnAuthorizeResponse` (partly decoded)
The body starts with `u32 eresult` (1 = OK). A byte at +4, when nonzero, indicates extra data before the result is read: the format
of that data has not been decoded. With `eresult == 1`, the client looks up the purchase's appid in the order map (`m_mapMicroTxnAuthRequest`, key: `order_id`) and
posts callback 152 to the game.

## Callback 152 `MicroTxnAuthorizationResponse_t` (24 bytes, seen being built)
`u32 appid` (+0) · `u64 order_id` (+8) · `u8 authorized` (+16), with `authorized = (auth_response == 1)`.

## What is still to be decoded
- **5504** (the push): body fields (the read routine does complex parsing; there is a 1-byte flag at the start of the body and the error message
  "Could not read microtransaction info from message"). `order_id` and `appid` are presumed.
- **5508/5509**: fields of the request and the response (items, price, currency, sandbox, refundable, region). The DLL exposes `GetMicroTxnAppID`,
  `GetMicroTxnOrderID`, `BGetMicroTxnPrice`, `GetMicroTxnSteamRealm`, `GetMicroTxnLineItemCount`, `BGetMicroTxnLineItem`, `BIsSandboxMicroTxn`,
  `BMicroTxnRequiresCachedPmtMethod`, `BMicroTxnRefundable`: these are the fields the response carries.
- Confirm everything with **real bytes**: a sandbox purchase or a real order, observed through the shim relay (`AURELIA_SHIM_LOG`). Any real approval
  requires explicit confirmation from the user; the test response must be **deny**.

## Capture (implemented)
When it picks up a `SteamUser0xx` interface, the shim subscribes to EMsg 5504 and writes one line to the log for each push:
`microtxn: push 5504 (<n> bytes): <hex>` (up to 2048 bytes; nothing is answered or sent to Steam). To capture:
1. Turn on the shim log: `AURELIA_SHIM_LOG=/path/shim.log` (`plus/launch.sh` already writes the shim log to the working folder).
2. Open a game that uses microtransactions and start a purchase **without approving it**.
3. Look for `microtxn: push 5504` in the log and keep the line; it is the input for decoding the 5504 body.
The 5508/5509 bytes are not captured yet (the Steam client requests them after the 5504); with the 5504 in hand, the next step is to request the 5508 through the relay
with the `order_id` and record the 5509.
