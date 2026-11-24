# Aurelia Plus: history, current state and what is missing

Updated on 2026-11-17. This document gathers the project's purpose, what was done, how each piece works, what was discarded, where we are and what is missing.
Detail documents: `overlay-json.md` (launcher contract), `in-game-overlay.md` (in-game overlay and the LC-PF1 investigation),
`phase-b-pending.md`, `superpowers/notes/microtxn-wire.md` (purchase packet format).

## 1. Purpose

Aurelia is a Rust CLI that talks to Steam. **Aurelia Plus** makes Aurelia **run any Steam game on Linux without the Steam client
installed or open**, using the user's real account (login, game ownership, achievements, cloud, DLC, purchases).

The idea: the game loads our own `steamclient(64).dll` / `steam_api` (the **shim**) instead of Valve's. The shim has no Steam logic: it talks through
a loopback bridge (`aurelia bridge serve`) to Aurelia, which talks to the real Steam. The scope, as the user defined it:

- "the complete project would be a way for aurelia to run all Steam games without having it installed and open. Not necessarily having everything the original dll has."
- "the shim must have everything the games call."
- Aurelia does not draw an overlay: when the game asks for a Steam screen, Aurelia hands the request as **JSON** to the launcher, which draws it.
  Even so, a reference overlay (Electron) exists for testing.
- Once logged in, Aurelia uses the logged-in account for everything, including the web session (exposing `web_session` to the launcher is authorized).

Agreed safety rule: **no real purchase is approved**. The responders only deny/cancel. (Reference test: 500 Coins, R$ 23.95. It was not approved.)

## 2. Architecture

```
game (Wine/Proton) ──> shim (DLLs in C, plus/shim) ──TCP loopback + token──> aurelia bridge serve ──> Steam (CM, Web API)
                                   │                                              │
                                   └─ reserved slots (7..14) ───────────────────┴─> stdout JSON (events) / stdin JSON (commands) ──> launcher
```

Reserved bridge slots: 7 relay call, 8 subscribe, 9 poll, 10 UGC, 11 cloud files, 12 game server, 13 overlay, 14 account information.

Pieces:
- `aurelia/` (Rust): `bridge/` (bridge, `overlay.rs` with the overlay hub and 48 tests), `commands/bridge.rs` (serve, print/command tasks, DLC watcher every 60 s,
  `close_all("game_exit")`), `commands/capabilities.rs` (events and features, `JSON_SCHEMA=2`), `web/web_session.rs` (cookies).
- `plus/shim/` (C, mingw): Steamworks interfaces (`ifaces/*.c`: apps, inventory, microtxn, overlay, p2p, user stats, ...), policy in `policy.json`.
- `plus/launch.sh`: sets up the environment, builds the DLLs with a hash-based cache (in parallel), creates a working directory with 700 permission, starts the bridge, the overlay
  and the game.
- `plus/examples/overlay/` (Electron + Vulkan layer): reference overlay.
- `plus/tests/`: `test_shim.py` (FakeBridge), `selftest.c`, `test_launch.py`; `aurelia/tools/conformance` (conformance probe against the real Steam).

## 3. What was done, in order

### Phase A/B: shim and bridge
- Shim with the interfaces games call; conformance probe that compares call by call with the real Steam (reference saved). 65 distinct methods still
  differ, most of them out of scope.
- Fix for the P2P race (tick thread). Hooks for userstats_preload and SteamApps.
- Authentication ticket created by Aurelia (`GetAuthSessionTicket`, status 0 = confirmed by Steam).
- Security of `bridge.out` (file removed on exit, umask 077, directory 700).
- Validated games: GTA 5 opens through the shim (Rockstar launcher installed by script, as with AC Unity; login works; online tested, BattlEye kicked the player after
  5 s for a reason not yet investigated), Overwatch runs on DXVK/host Vulkan.
- DLC: `GetDLCCount` only counts owned ones, `shim_dlc_apply`, depots installed per version, `dlc_changed` event (check every 60 s).

### JSON overlay contract
- Events: `overlay_request`, `microtxn_request`, `overlay_closed`, `overlay_list`, `dlc_changed`, `web_session`. Commands: `capable`, `shown`, `hidden`, `respond`,
  `steam_url`, `web_session`, `list`. See `overlay-json.md`.
- `IsOverlayEnabled` only answers yes after the launcher declares `capable`. `ActivateGameOverlay*` left the policy and goes to slot 13.
- `web_session` hands cookies per domain (`.steampowered.com`, `.steamcommunity.com`; `steamLoginSecure` http_only) to the launcher.
- Commands accept lines of up to 8 KiB; an invalid line produces `invalid_command` without bringing serve down.

### Purchase (microtransaction)
- Wire: 36-byte legacy header; 5504 (push, binary KeyValues), 5505 (decision: u64 **trans_id** + u32 response), 5506, 5508/5509 (order data). Callbacks 152
  (MicroTxnAuthorizationResponse), 331 (GameOverlayActivated), 703 (SteamAPICallCompleted), 4704 (StartPurchaseResult).
- Inventory `StartPurchase`: `Inventory.PurchaseInit#1` on Steam; the shim returns `StartPurchaseResult` with order and transaction; the game shows "processing".
- The launcher's decision (`respond` / `steam_url` with `steam://close/authorize|cancel`) becomes 5505 by transaction number (with the order number, Steam answered
  Fail). Only after approval does the shim finalize (`PurchaseFinalize`).
- The real approval page is `https://checkout.steampowered.com/checkout/approvetxn/<trans>/?returnurl=steam`, opened by Steam in a 1020x440 browser.
- The language goes in `PurchaseInit` field 2 as an index into the client's table (English 1 ... Brazilian 23, Latin American 28), via `AURELIA_SHIM_LANGUAGE` (derived from `LANG`).
  With 23 Steam answers `eresult=1` and returns order and transaction (confirmed on 2026-11-17).

### Reference overlay
- Window on top (Electron): works, but it is a Linux window (Alt+Tab switches away).
- **In-game** overlay: implicit Vulkan layer `VK_LAYER_AURELIA_overlay` (`plus/examples/overlay/vklayer`), Electron renders offscreen to a shared
  memory, the layer copies the pixels in `vkQueuePresentKHR`. Input through a transparent window without focus. Validated on Overwatch.
  Activation: `AURELIA_PLUS_OVERLAY=ingame` (the value `1` is window mode only).
- Problem found and fixed: the transparent window stayed on screen after the game closed and blocked all clicks. Now there are guards (game gone, limit of
  180 s, non-blocking writes to the FIFO, cleanup on `overlay_closed`). **Always** clean up Electron processes when stopping a test.

### Test speed
- `cargo test -j 2 --lib --bin aurelia` (the external disk froze with many lld jobs); selftest cached by hash.

## 4. LC-PF1 / LC-PF41 on Overwatch: RESOLVED

**Cause**: the shim's `Steam_GetAPICallResult` export was a stub (it returned 0). Overwatch, which reads callbacks by hand, asks for the `StartPurchase` result
through it after 703; without data, it showed the error. Fixed in `shim/exports.c` (uses `shim_callresult_take`), with a test in the selftest (`t_call_results`) and
confirmation in the game (2026-11-17): it stays on "processing" and goes back to the start on cancel. The items below are the history of what was ruled out before finding
the cause; the internal callbacks and the network were **not** the problem (callback 1020048 was kept for fidelity with the real Steam).

Investigation history:

Symptom: when clicking buy, Overwatch shows LC-PF1 (first attempt) or LC-PF41 (later ones) and then "processing your order" and goes back to the start. On the real Steam,
the game just waits on "processing" and goes back to the start only if the user cancels.

Already **ruled out** (measured or tested):
- Structure/order/size of the callbacks (331 of 12 bytes, 703, 4704 of 24 bytes, 152), response time (370 to 590 ms against ~470 ms), 331/703 order (varies on the
  real Steam too), early `PurchaseFinalize`, decision by transaction, 5508/5509, content and authentication of the page, focus/window, variables `SteamOverlayGameId`,
  `SteamClientLaunch`, `SteamEnv`, presence of `GameOverlayRenderer64.dll`, overlay inside the game or outside.
- Network/TLS (Wine `+secur32`): only Blizzard hosts, normal handshakes, 200 responses; the "assertion failure exception" appears from the start of the game.
- Language in `PurchaseInit`: with 23 the order goes out correct; it is not the cause.
- Internal callback 1020048 (1 byte) that the real Steam delivers ~47 ms before the active 331: added to the shim (`overlay.c`); the error continued.

Useful findings from the real Steam (`+steamclient` log): the game reads callbacks **by hand** (`Steam_BGetCallback`), so it sees internal ids (1020048, 1040011, 1040015,
1040044, 1102 ...). After `StartPurchase` the game only calls `RunFrame`/`BGetCallback`.

Open hypotheses:
1. Some internal callback/state that the real client delivers and the shim does not (260-byte 1040011 appears before the inactive 331; callback bodies are not
   in the log).
2. The game calls the Blizzard API (`us.api.blizzard.com`) during the purchase and the server answers differently when the real Steam takes part. Under test: compare the
   TLS connections (hosts and count) of the real Steam with those of the shim at purchase time.
3. `SteamUser` with the account name, `LD_PRELOAD=gameoverlayrenderer.so` (Valve binary; the project does not distribute it), `Steam3Master`.

## 5. Where we are (2026-11-17)

- Ready and working: bridge, shim, real login, GTA 5, playable Overwatch, complete purchase order up to the decision (deny), JSON for the launcher, `web_session`,
  DLC, overlay in a window and inside the game (rendering, reference example).
- Purchase: LC-PF1 is fixed (section 4) and confirmed in the game. What is left is testing **approval** end to end (only with explicit confirmation from the user).
- `steamclient64.dll` exports: all checked against the real DLL; the legacy server and connection ones now follow the real one (section 8).
- Everything committed in the `plus` repository (b0f7c01, 4f2ed5b, d06e593). The full suite (Rust and Python) was **not** run after these changes; only the shim selftests
  in 64 and 32 bits.
- Direction set for the overlay (section 9): Aurelia is the data service; each launcher draws. Electron is just an example.

## 7. How to reproduce the tests

```bash
cd /run/media/rafael/Interno/Aurelia-Plus/plus
AURELIA_PLUS_OVERLAY=ingame AURELIA_SHIM_LOG=/path/shim.log ./launch.sh 2357570 --platform windows -v
../aurelia/target/debug/aurelia stop 2357570      # then kill any remaining Electron/bridge processes by PID
```

Real Steam reference: `WINEDEBUG=+steamclient,+secur32,+timestamp PROTON_LOG=1 PROTON_LOG_DIR=<dir> steam -silent -applaunch 2357570`.
Never use `pkill -f` with a pattern that appears in the command itself; use explicit PIDs.

## 8. `steamclient64.dll` exports: comparison with the real DLL (2026-11-17)

After LC-PF1, I compared the shim's 41 exports with those of the real `steamclient64.dll` (installed in `~/.local/share/Steam`, used only as a local reference):
the names are the same (40 + `CreateInterface`). On the real Overwatch (`+steamclient`) the game uses **only three**: `Steam_BGetCallback` (5618 calls),
`Steam_FreeLastCallback` (109) and `Steam_GetAPICallResult` (1); all three are implemented. Reading the code of the others in the real DLL, they are all wrappers
of 30 to 220 bytes that forward to the Steam engine (IPC):
- `BConnected`, `BLoggedOn`, `CreateSteamPipe`, `CreateGlobalUser`, `CreateLocalUser`, `ConnectToGlobalUser`, `BReleaseSteamPipe`, `ReleaseUser`: the shim returns
  success/1 with a fixed pipe and user (equivalent to "connected and logged in").
- `IsKnownInterface`: the real one looks the name up in a list of interfaces; the shim does the same over the ones it registers. `NotifyMissingInterface`: the shim only logs it.
- `ReleaseThreadLocalMemory`: frees per-thread memory in the real one; in the shim there is nothing to free.
- Legacy game server and connection: **implemented** (they used to return 0). `Steam_GetGSHandle` returns the server object; `GSBLoggedOn`, `GSGetSteamID`, `GSBSecure`,
  `GSLogOn`, `GSLogOff`, `GSSetServerType`, `GSUpdateStatus`, `GSSendSteam3UserConnect`, `GSRemoveUserConnect`, `GSSendUserDisconnect` fall into the same
  implementations as `ISteamGameServer` (null handle returns 0, as in the real one); `Steam_InitiateGameConnection` / `Steam_TerminateGameConnection` use the connection
  of `ISteamUser` (0 if the CGameID is invalid, as in the real one). The arguments were read from the real DLL's code; the exact order of those of `GSSendSteam3UserConnect` /
  `GSSendUserDisconnect` follows the legacy header and was **not verified** with a game. Steam2 (`GSSendSteam2UserConnect`, `GSGetSteam2GetEncryptionKey...`) and
  `GSSendUserStatusResponse` return 0 (in the real one the latter is a literal 0). `Steam_GSSetSpawnCount`, `Steam_SetLocalIPBinding` and `Breakpad_*` have no effect
  observable by the game and remain no-ops. Test: case `legacy_steamclient_exports_follow_the_real_dll` of the selftest (64 and 32 bits).

## 9. Overlay as a data service: direction and inventory (2026-11-17)

**User's decision:** Aurelia is a CLI client and the Electron overlay is just a test. Each launcher draws the interface its own way, but Aurelia has to
deliver as JSON what the Steam overlay delivers, with the same functionality. Pages that are URLs (store, community, purchase approval) stay as `url`
plus `web_session`; the rest (friends, chat, achievements, invites, notifications) needs data and actions in JSON.

**What Aurelia already has** (initial audit; the items from Nov 17 are already wired into `bridge serve`, see 10.2):
- Friends, presence, current game: `Friend` and `run_friends_watcher` (today only the daemon uses it); `read_friends` already feeds the shim snapshot.
- Chat: `send_chat_message`, `recent_chat_messages`, `ChatClient::listen` (message, echo, typing, game invite; `aurelia chat open` already uses it).
- Game invite: `invite_to_game` (requires Steam to know the game is running on the account, see pending items).
- State and friends: `announce_persona`, `add_friend`, `remove_friend`.
- Achievements: `fetch_achievements` (name, description, icons, rarity, unlocked, date). Profile: `user_profile`. Online players: `current_players`.
- Stats and schema: `bridge/userstats.rs`.

**What does not exist yet:** received lobby invites and accepting an invite (O6) and screenshot as an event (O4, out of scope). Out of initial scope: voice, music,
controller configuration, tabbed browser, overlay settings (these belong to the launcher).

**Proposal in four steps** (waiting for the user's ok to start): 1) queries and actions without real time (friends, chat history and sending, achievements, profile,
state, add friend); 2) streams (`friend_status`, received messages); 3) new shim notifications (achievement, screenshot); 4) invites and lobby.
Each group goes into `overlay-json.md` and `capabilities`, with a test in the `FakeBridge`.

**Organization recommendation** (to be agreed): move Electron and the Vulkan layer to `examples/overlay`, `launch.sh` stops starting Electron by default, and an
`aurelia bridge simulate` mode emits sample events for whoever develops a launcher without opening a game.

## 10. Commands and events: what exists (2026-11-17)

Contract of `aurelia --json bridge serve --app <appid>`: one JSON line per event on stdout; one JSON line per command on stdin. An invalid line produces
`invalid_command` (with `detail`) and `serve` continues. Field details in `overlay-json.md`.

### 10.1 Already exist (implemented, tested with the `FakeBridge`/Rust tests)
Commands (stdin):

| Command | What it is for |
|---|---|
| `{"cmd":"capable","on":true}` | declares that a launcher is handling the overlay; only after this does `IsOverlayEnabled` answer yes |
| `{"cmd":"shown","id":"…"}` / `{"cmd":"hidden","id":"…"}` | the launcher showed/hid the request screen; the game receives `GameOverlayActivated_t` (331) active/inactive |
| `{"cmd":"respond","id":"…","approve":true|false}` | decision on a `microtxn_request` (approve or deny) |
| `{"cmd":"steam_url","id":"…","url":"steam://close/authorize|cancel…"}` | forwards the approval page's redirect (authorize approves, cancel denies; any other URL produces `unsupported_url`) |
| `{"cmd":"web_session"}` | delivers the web session cookies (`.steampowered.com`, `.steamcommunity.com`) |
| `{"cmd":"list"}` | pending microtransactions (`overlay_list`) |

Events (stdout): `overlay_request` (store, web page, user profile/dialog, invite, Remote Play invite, friends, and the text of `ActivateGameOverlay`),
`microtxn_request` (wallet purchase: order, transaction, approval URL, items, currency, total), `overlay_closed` (`answered`, `dismissed_by_steam`, `game_exit`),
`overlay_list`, `dlc_changed`, `web_session`, and the responses `result` (`ok`) and `error` (`unknown_id`, `not_pending`, `not_microtxn`, `invalid_command`,
`unsupported_url`). Feature announced in `aurelia capabilities --json`: `bridge.overlay.stdin_control`; `schema` = 2.

Environment and test controls added in `plus/` (they are not part of the launcher contract):

| Variable | Effect |
|---|---|
| `AURELIA_PLUS_OVERLAY=1` / `ingame` | starts the example overlay (window on top / inside the game through the Vulkan layer); `AURELIA_PLUS_OVERLAY_CAPABLE=1` only declares `capable` |
| `AURELIA_SHIM_LANGUAGE` | game language (`GetCurrentGameLanguage` and `PurchaseInit`); default comes from `LANG` |
| `AURELIA_SHIM_LOG=<file>` / `AURELIA_SHIM_TRACE=1` | shim log / trace of each call (includes `Steam_BGetCallback` and `Steam_GetAPICallResult`) |
| `AURELIA_SHIM_OPEN_CHECKOUT` / `AURELIA_SHIM_NO_BROWSER` | open the approval page in the Linux browser / only log the URL |

### 10.2 Queries, actions and social events (implemented on Nov 17; details in `overlay-json.md`)
Every command below accepts `req`, which comes back in the response. SteamID64 is always output as text.

| Command | Response/effect | Basis in Aurelia |
|---|---|---|
| `friends` | `friends` (last list read, refreshed every 3 s) | `read_friends`, `run_friends_watcher` |
| `chat_send` | `result` with server time | `send_chat_message` |
| `chat_history` | `chat_history` | `recent_chat_messages` |
| `achievements` | `achievements` (Aurelia's language if `lang` is missing) | `fetch_achievements` |
| `profile` | `profile` | `user_profile` |
| `persona` | `result` | `announce_persona` |
| `friend_add` / `friend_remove` | `result` | `add_friend`, `remove_friend` |
| `invite` | `result` (only says Steam received the request) | `send_game_invite` (serve declares the game) |
| `players_recent` | `players_recent` (the game's list via `SetPlayedWith`, in `recent_players.json`) | `bridge/social.rs`, shim `SetPlayedWith` |

Events (stdout) without a request: `friend_status` (a friend's state or game changed, joined or left), `chat_message`, `chat_typing`, `chat_invite`, `achievement_unlocked`
(written to Steam; shim `StoreStats` → slot 13 → `fetch_achievements`). The `web_page` `overlay_request` carries `mode` (`default`/`modal`).

Out of scope by the user's decision: screenshot as an event (O4). `lobby_invite`, `accept_invite` and `decline_invite` exist for the open game (O6). `aurelia bridge simulate` exists (O9). Electron and the Vulkan layer are in `plus/examples/overlay`.
Features announced in `aurelia capabilities --json` (`features`): `bridge.overlay.stdin_control`, `bridge.overlay.queries`, `bridge.overlay.social_events`,
`bridge.overlay.achievement_event`, `bridge.overlay.players_recent`.
