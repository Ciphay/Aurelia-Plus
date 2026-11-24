# Bridge as a persistent service (plan and result, Nov 23, 2026)

Phases 0 to 5 are implemented; section 8 gives what was measured and how each piece works. Sections 1 to 7 are the original plan.

Goal: open a game with the shim without waiting for the bridge to come up. Today L4D2 takes 11.4 to 15.6 s until the port is ready; the target is **p95 under 2 s with the service warm** and
**opening without a network too**. System boot and Vorbis OS do not wait for Aurelia for anything.

## 1. How it is today (read from the code and measured)
- `aurelia --json bridge serve --app N` (`aurelia/src/commands/bridge.rs`, `cmd_bridge_serve`) runs **inside the daemon**, which already lives between games. The cost is in what the function
  redoes for every game. Times measured on L4D2: session 1.6 s; waiting for the logon friends list 5.0 s (the largest share); declaring the game and waiting for the tokens 0.8 to 1.1 s;
  cloud down 1.2 s; snapshot 2.7 s (game list, display names, app info, DLCs).
- The daemon has an idle timeout (`AURELIA_DAEMON_IDLE_TIMEOUT`) and exits when the binary changes on disk (`aurelia/src/daemon/server.rs`).
- `declare_playing` (`aurelia/src/steam_client/client.rs`) sends an empty `CMsgClientGamesPlayed`, waits 500 ms and declares **only** that app; `clear_playing` empties everything.
  Two games at the same time step on each other: when one closes, the other stops showing as running.
- Each `bridge serve` creates its own social observer (`spawn_social`): with two games, friend events are duplicated.
- Without a network, `authed_client()` fails, the bridge does not come up and `play --plus` gives up (`the bridge ended before starting`). The shim already keeps the last snapshot if the bridge dies
  while the game is open, but it cannot open a game without a bridge.
- The launcher depends on the `bridge serve --app N` contract: port and token on the first stdout line, JSON events on stdout, commands on stdin, `stop` at the end.

## 2. Principles
1. **Boot never waits for the service.** No ordering dependency, no network wait, no `Type=notify`.
2. **The worst case is today's, never worse.** A game launched while the service is still warming up uses whatever is ready and does the rest on the spot.
3. **Without a network the game opens** with the last good state, and what the game writes goes to a queue and is uploaded when the network returns.
4. **Full compatibility of the launcher contract:** `bridge serve --app N` stays the same from the outside.
5. **No change in behavior for the game** other than being faster and surviving offline.

## 3. Architecture
Two scopes of state inside the daemon (which becomes the service).

**Account scope, hot, only once** (new module, for example `aurelia/src/bridge/account_cache.rs`):
- CM session with reconnection and growing backoff;
- friends roster and display names (CM when there is presence; otherwise public profile);
- the account's game list and app info per app (DLCs, cloud quota, branches);
- web session cookies;
- **one** social observer for all games, with events distributed to each attached launcher;
- **snapshot on disk** (`data_dir()/bridge/<steamid>/`): account, persona, games, app info, names, last ownership ticket per app, stats and achievements per app.
  Atomic writes (temporary file and rename), format version, 0600 permission.

**Game scope, cheap** (`cmd_bridge_serve` shrinks to "attach an app session"):
- declare the game (now over the **set** of declared games) and collect the tokens;
- cloud down (skipped when nothing changed) and cloud up on exit;
- game snapshot assembled from the hot cache;
- port and token per game for the shim, overlay hub, DLC watcher and stats.

**Connection state.** The bridge tells the shim whether Steam is connected. Today the shim uses "bridge reachable" as `shim_session_online()`; it will use the bridge's `connected` field
instead (`plus/shim/core.c`, `bridge_client.c`). `BLoggedOn`, `BConnected` and `IsOverlayEnabled` follow the field.

## 4. Phases

### Phase 0: measure and lock down (small)
- Keep the per-step times that already appear in the bridge log; add the time to the port of the **second** game and the offline path.
- Record the baseline: 11.4 to 15.6 s on L4D2.
- Measurement script (`tools/`) that opens and closes a game N times and prints p50 and p95.
- Done when: the measurement runs unattended and the baseline is recorded in this file.

### Phase 1: account cache and deadlines, installing nothing (medium)
- Account scope in the daemon with expiry per type (roster and names, games, app info) and invalidation by CM push (license, friends or persona changes).
- Persist to disk the public profile names and the snapshot per account and per app.
- `cmd_bridge_serve` parallelizes declare+tokens, cloud down and snapshot assembly; the 5 s wait for the roster goes away when the cache exists.
- Cloud down is skipped when nothing changed (compare the app's cloud change number).
- **Deadline on online open:** tokens and cloud have a short timeout (value to be measured, starting point 3 s); if it expires, the game opens in the Phase 4 offline mode and connects later.
- Done when: with the daemon warm, the second and later games come out in under 2 s at p95; the first one after starting the daemon drops to under 6 s.

### Phase 2: service that starts in the background (medium)
- `systemd --user` unit and activation socket (socket in `$XDG_RUNTIME_DIR/aurelia/`, 0600):
  - `Type=simple`, no `Before=`, no `Requires=`, **no** `After=network-online.target`;
  - `Nice=15`, `CPUSchedulingPolicy=idle`, `IOSchedulingClass=idle`, `MemoryHigh` as a ceiling;
  - `Restart=on-failure`, long `RestartSec`, small `StartLimitBurst`.
- The daemon opens the socket right away and answers commands in milliseconds. Warm-up only starts after a fixed delay (starting point 20 s) and when there is a network;
  low-priority tasks; a game's request goes first.
- Without `systemd --user` (Vorbis launched by script): `aurelia daemon start --background` does `setsid`, lowers the priority and returns immediately; the Vorbis script calls it.
- No default idle timeout while installed; `AURELIA_DAEMON_IDLE_TIMEOUT` still applies for those who did not install.
- Do not fight with the real Steam when it is open: do not announce presence (`host_steam_owns_presence`).
- `aurelia service install|uninstall|status`.
- Done when: `systemd-analyze critical-chain` and the time to the Vorbis screen are the same before and after, within measurement noise; the system boots with Steam offline,
  with the account logged out and with the service disabled.

### Phase 3: truly multiple games (medium)
- Set of declared games: every change sends the full set to the CM; closing a game removes only that app. This fixes today's bug.
- A single social observer, with events distributed to the attached launchers.
- Cloud up and ticket cleanup per game, before removing the app from the set.
- Binary update without killing games: the new daemon comes up alongside on another socket and the old one exits when no game is left (the rule "keep the old daemon while there is a game" already exists).
- `aurelia bridge status`: sessions per app, cache ages, connection state and queue size.
- Done when: two games at the same time, closing one does not affect the other; friend events reach each launcher only once.

### Phase 4: offline mode (large)
- **Detection:** without a network, the service does not try to log in in a loop; it stays offline and retries in the background with growing backoff.
- **Offline open:** the bridge comes up in milliseconds serving the snapshot from disk, with `connected=false`. The ownership ticket is the last cached one.
- **Shim:** `BLoggedOn`, `BConnected` and `IsOverlayEnabled` follow `connected`; games that require Steam online show their own error, as on the real client in offline mode.
- **Queue on disk** for changes made offline: `StoreStats`, achievements, play time and cloud up. Idempotent, versioned, and drained on reconnection.
- **Reconnection with the game open:** log in in the background, drain the queue, notify the shim and deliver `SteamServersConnected_t` to the game, without restarting anything.
- **No cache (first use):** a clear message saying the game must be opened once with a network.
- Done when: with the network off, a game that was opened before opens in under 2 s; when the network comes back, the queue is uploaded and the game receives the connection.

### Phase 5: finishing (small)
- Log with rotation, timing metrics in `bridge status` and optional pre-warming of the most played apps (ticket and stats).
- Documentation for the launcher (`overlay-json.md`): new connection events and what changes with the service.

## 5. Tests
- **Unit:** expiry and invalidation of the caches; set of declared games; atomic reading and writing of the snapshot on disk (including truncated file and old version); offline queue
  (order, idempotency, partial failure); growing reconnection backoff.
- **Integration, without Steam:** the bridge tests with a fake bridge (`tests/test_shim.py`) gain the `connected` field; test of two games in sequence and at the same time; the daemon
  surviving the end of the game; side-by-side update.
- **Manual measurement:** L4D2 and Overwatch, cold, warm and offline; `systemd-analyze critical-chain`; time to the Vorbis screen.
- The existing tests (`cargo test --release`, the shim suite) keep passing in each phase.

## 6. Risks and open questions
- **Validity of the cached ticket offline:** I did not measure whether the game still accepts the last ticket after hours or days without a network. Needs a test in Phase 4 before promising anything.
- **Stale cache:** DLC ownership, license and names. Mitigation: invalidation by CM push and short expiry; the bridge corrects it at the next warm-up.
- **One session per account:** Steam only accepts one active "in game" per account; this does not change.
- **Memory and long life:** the service lives for hours; it needs a memory ceiling and a long test.
- **Offline presence does not bring names:** the public profile lookup needs a network and leaves the critical path (it stays in the disk cache).
- **How Vorbis starts the session:** decides whether Phase 2 uses the `systemd --user` unit or the `daemon start --background` called by the Vorbis script.
- **Platform:** the plan applies to Linux; Windows and macOS stay out until someone asks.

## 7. Suggested order
Phase 0, then Phase 1 (biggest gain, installs nothing), Phase 3 (fixes the two-game bug), Phase 2 (service) and Phase 4 (offline). Phase 5 last.
The two-game bug can be fixed before all the others, because it is independent.

## 8. Result (Nov 23, 2026)

### Measurements (L4D2, test account, Linux, warm daemon)
| Case | Before | After |
|---|---|---|
| Port ready, 2nd game onward | 11.4 to 15.6 s (includes 5 s waiting for the roster) | **p50 1.35 s, maximum 1.56 s** in 8 runs; one series during a slow Steam period gave 2.1 s |
| First open after the daemon starts | 11.4 to 15.6 s | 2.1 s (session 0.7 s + roster 0.25 s + parallel step 1.1 s) |
| Open without a network, game opened before | did not open | **0.11 to 0.15 s** to the port |
| Two games at the same time | the second removed the first from "running" | declared set: `[550, 480]`; closing 480 leaves `[550]`; closing 550 leaves `[]` |
| Boot | | only the socket unit enters boot (219 µs in `systemd-analyze --user blame`); the service starts on the first connection |

The parallel step (declare and wait for the token, Cloud down, snapshot) is bounded by Cloud (1.35 s) and the token (1.0 s); both come from Steam and vary with it.

### Phase 2: service (`systemd --user`)
- `aurelia service install|uninstall|status`. Installs `aurelia-daemon.socket` (socket activation in `$XDG_RUNTIME_DIR`) and `aurelia-daemon.service` (`Type=simple`, no `Before=`/`After=`/`Requires=`, `Nice=15`, `CPUSchedulingPolicy=idle`, `IOSchedulingClass=idle`, `MemoryHigh`, `Restart=on-failure`).
- Only the socket is tied to `default.target`; the daemon is born from the first connection (any Aurelia command, including `play --plus`). Nothing waits for the network or for login.
- Without `systemd --user` (Vorbis by script): `aurelia daemon start` does `setsid`, lowers the priority and returns immediately.
- In managed mode the client waits for the daemon's marker file and, if the binary changed, restarts with `systemctl --user restart` instead of killing the process.
- Account warm-up (names, app info for the 5 most played apps, ticket) starts 20 s later (`AURELIA_WARMUP_DELAY`) and repeats every 30 min.

### Phase 3: multiple games
- Set of declared games as a multiset (`declared_games`); closing a game removes only that app. The social roster is single in the daemon and each bridge reads it.
- `aurelia bridge status [--json]`: sessions (app, online, time to port, opened offline, port, queue), declared games, per account the number of names, app infos and offline snapshots with the age of each, queue, log size.

### Phase 4: offline mode
- **Open:** `open_online` fails (no session in 8 s, token in 10 s or Cloud in 30 s) and the bridge comes up with the last good snapshot (`bridge-cache/<steamid>/snapshot-<app>.json`, written on every online open), with `online:false`. Without a snapshot: typed `offline` error asking to open the game once with a network.
- **Shim:** `BLoggedOn` and `BConnected` follow the connection state (reserved call 15, a dedicated connection, 1.5 s cache); `IsOverlayEnabled` too. Opened offline, the game receives `SteamServerConnectFailure_t` (EResult 3, still trying). When the bridge starts saying connected, the game receives `SteamServersConnected_t`; if it drops, `SteamServersDisconnected_t`.
- **Offline responses:** stats come from the last disk copy (`stats-<app>.bin`); `StoreStats` goes to the queue; relay signatures are kept and redone on reconnection; authentication tickets, rich presence, UGC and file-based Cloud say there is no connection; the overlay (hub) works.
- **Queue on disk** (`queue.json`, atomic, 0600, versioned): stats of the same app are merged (the last value of each id wins) and the Cloud upload is one mark per app. It drains on reconnection and every 30 s if something fails; it also drains on the next online open (marked saves are uploaded **before** Cloud down, so they do not become a conflict). If `StoreStats` fails with the game open (connection dropped), the value goes into the queue.
- **Reconnection with the game open:** attempts with growing backoff (2, 4, 8 … 60 s). On connecting: declares the game, gets tokens, uploads the queue (no Cloud down: the local saves are the newest), redoes the signatures, turns on the handler, the DLC watcher, the social observer and the stats prefetch, and emits the `connection` event.
- **On closing the game offline** an upload mark is left for Cloud, which is uploaded on the next connection.

### Phase 5: finishing
- Log with rotation (`bridge.log`, 4 MB, 3 old files).
- New events and states for the launcher in `overlay-json.md` (`connection`).
- `AURELIA_BRIDGE_OFFLINE_FOR=<s>` (read by the daemon) makes online opens fail for that many seconds: used to test offline and reconnection without taking the network down.

### How it was verified
- Live: activated socket (daemon stopped, one command brings it up), open without a network in an empty network namespace (`unshare -rn`: 0.11 s, the queue receives the Cloud upload), reconnection with the offline period forced (offline in 0.15 s, connected in 7.6 s, queue drained), two games at the same time with `bridge status`.
- Tests: unit tests for queue, reconnection backoff, log, log rotation and cache; in the shim, `OfflineCases` (offline snapshot becomes a connection and the callback arrives) in 64 and 32 bits.

## 9. Opening fast for all games (Nov 23, 2026, afternoon)
Warming up 5 apps left the others slow. Measured on Overwatch (first open after reboot, outside the warm-up): port in **5.8 s** = session 2.3 s + Cloud 2.8 s + snapshot 3.5 s (app info without cache) + token 1.06 s.

| Change | Effect |
|---|---|
| App info (DLC, quota, branches) without hard expiry: expired (10 min) still opens the game immediately and a background fetch renews it (up to 30 days; `invalidate_app` still forces a fetch) | any game already seen opens without waiting for the network |
| Warm-up went from 5 to **all installed** (most recent first, 300 ms between each, only those older than 6 h) | games never opened also come out ready |
| `aurelia bridge prewarm --app N [--json]` (event `prewarmed`): the launcher calls it when the game is selected | an advantage of 1 to 3 s on any app, even without a cache |
| Session heartbeat: a cheap query every 60 s; two failures in a row discard and restore the session (and the open games are declared again) | nobody pays for the login at play time |
| Warm-up retries at 10 s, 20 s ... up to 5 min when the session does not exist (network not up yet at login) | before, it waited 30 min |
| Cloud: profiling showed the real cost (UFS 0.6 s + OS target 1.2 to 1.4 s + changelist 0.5 s). The UFS configuration and the OS target (library + product info) now stay in memory (10 min with background refresh; 24 h) and warm-up prepares them for all installed apps | Cloud drops from 2.8 s to **0.5 to 0.6 s** (only the change query, which is the check itself) |

Result (Overwatch): port in **1.0 to 1.2 s** (was 5.8 s). Installed apps never opened: 1.3 to 1.7 s. The limit now is the Steam token (1.0 s), which only arrives after declaring the game.
New log line: `cloud step <step>: N ms` in `bridge.log` shows where Cloud spends time.
