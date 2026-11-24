# Overlay and purchases as JSON information for the launcher: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The shim forwards overlay, microtransaction and DLC requests to Aurelia, which publishes them as JSON lines on the stdout of `aurelia bridge serve`; the launcher answers with JSON lines on stdin, and the answer reaches the game and Steam.

**Architecture:** A new reserved bridge call (slot 13) carries requests from the shim to Aurelia and brings back answers and states. In Aurelia, an `OverlayHub` keeps the pending requests, generates the events and applies the launcher's commands. In the shim, `overlay.c` implements the overlay methods and a thread polls for answers and delivers them to the game by callback (`shim_defer`).

**Tech Stack:** Rust (`aurelia`, tokio, serde_json), C (shim, mingw 64/32 bits), Python unittest under Wine (shim tests with `FakeBridge`).

**Spec:** `plus/docs/superpowers/specs/2026-11-16-launcher-overlay-info-design.md` (includes the section "Correction after reading the code": events on stdout and commands on stdin of `bridge serve`).

## Global Constraints

- Aurelia draws no interface at all; JSON only.
- No deadline in Aurelia for `microtxn_request`: it stays pending until the launcher answers, Steam withdraws the request or the game closes (denies and emits `overlay_closed` with `game_exit`).
- `IsOverlayEnabled` only answers yes after `{"cmd":"capable","on":true}`.
- The game thread never blocks on the network: bridge calls in their own thread, results through `shim_defer`.
- No automatic purchase approval; real purchase only with explicit confirmation from the user.
- No Valve code; Steam message formats by reverse engineering and observation.
- No per-game solution: everything by interface family and by request type.
- New events: `overlay_request`, `microtxn_request`, `overlay_closed`, `dlc_changed`; `schema` goes up in `aurelia capabilities --json`.
- Shim tests run in 64 and 32 bits; Aurelia's `cargo test` must stay green.

## Review Focus

- Command for an unknown or already closed `id`: typed error, without bringing the process down (Task 1, Task 3).
- Game closes with a pending `microtxn_request`: the request is denied and `overlay_closed` with `game_exit` is emitted once (Task 1).
- Two microtransactions pending at the same time: independent answers, distinct ids (Task 1).
- Invalid line on stdin (broken JSON, unknown `cmd`, huge line): `error` event, `bridge serve` keeps running (Task 3).
- Giant URL or text in `ActivateGameOverlayToWebPage`: truncated at 2048 bytes, no overflow in the shim (Task 4).
- Launcher never declares `capable`: `IsOverlayEnabled` = 0 and requests are still emitted as events without hanging the game (Task 4).

---

## File Structure

- Create `aurelia/src/bridge/overlay.rs` + `overlay_tests.rs`: `OverlayHub` (state, pending, events) and the slot 13 format.
- Modify `aurelia/src/bridge/mod.rs`: `pub mod overlay;`.
- Modify `aurelia/src/commands/bridge.rs`: slot 13 handler, event printing and stdin reader.
- Modify `aurelia/src/commands/capabilities.rs` (+ `capabilities_tests.rs`): new events and commands, `schema`.
- Create `plus/shim/ifaces/overlay.c`: overlay methods, `capable` state, answer thread, callback 331.
- Modify `plus/shim/shim.h`, `plus/shim/core.c`, `plus/catalog/policy.json` (+ regenerate `shim/generated`): registration and policy output.
- Create `plus/shim/ifaces/microtxn.c`: microtransaction authorization (Task 5).
- Modify `plus/tests/test_shim.py`, `plus/tests/selftest.c`: `FakeBridge` slot 13 and new cases.
- Create `plus/docs/superpowers/notes/microtxn-wire.md`: message format (Task 5).

---

### Task 1: `OverlayHub` in Aurelia

**Files:**
- Create: `aurelia/src/bridge/overlay.rs`, `aurelia/src/bridge/overlay_tests.rs`
- Modify: `aurelia/src/bridge/mod.rs`

**Interfaces:**
- Produces:
  - `pub enum Answer { Approved, Denied(DenyReason) }`, `pub enum DenyReason { User, Game, Steam }`
  - `pub struct OverlayHub` (cloneable through `Arc`) with:
    - `pub fn new() -> (Arc<OverlayHub>, tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>)`
    - `pub fn request(&self, app_id: u32, kind: &str, payload: serde_json::Value) -> String`: emits `overlay_request`, returns the id.
    - `pub fn microtxn(&self, app_id: u32, info: serde_json::Value) -> String`: emits `microtxn_request`, stores it as pending.
    - `pub fn respond(&self, id: &str, approve: bool) -> Result<(), OverlayError>` (pending microtxn only).
    - `pub fn shown(&self, id: &str) -> Result<(), OverlayError>` / `hidden(...)`: queue the state for the shim.
    - `pub fn dismiss(&self, id: &str)`: Steam withdrew it; emits `overlay_closed` with `dismissed_by_steam`.
    - `pub fn close_all(&self, reason: &str)`: game closed; denies pending ones and emits `overlay_closed` with `game_exit`.
    - `pub fn set_capable(&self, on: bool)` / `pub fn capable(&self) -> bool`.
    - `pub fn drain_updates(&self) -> Vec<Update>`, with `pub enum Update { Answer { id: String, answer: Answer }, Shown { id: String }, Hidden { id: String } }`.
    - `pub fn list(&self) -> serde_json::Value`
  - `pub enum OverlayError { UnknownId, NotPending, NotMicrotxn }` with `impl Display`.

- [ ] **Step 1: Write the failing tests** in `overlay_tests.rs`: `request_emits_overlay_request_event_with_unique_ids`; `respond_approve_queues_answer_and_closes_with_answered`; `respond_unknown_id_is_unknown_id_error`; `respond_twice_is_not_pending_error`; `two_microtxns_answer_independently`; `close_all_denies_pending_with_game_exit_once` (a second `close_all` does not emit again); `dismiss_emits_dismissed_by_steam`; `capable_defaults_false_and_toggles`; `shown_and_hidden_queue_updates_in_order`; `list_reports_pending`.
- [ ] **Step 2: Run** `cargo test -p aurelia-steam bridge::overlay -- --nocapture` (adjust to the package name in `Cargo.toml`); expected: FAIL (module does not exist).
- [ ] **Step 3: Implement** `overlay.rs` with an internal `Mutex` and ids as integers in string form ("1", "2"...); events with the spec's fields (`event`, `id`, `app_id`, `kind`/`items`..., `reason`). `respond` on an id that is not a microtxn returns `NotMicrotxn`.
- [ ] **Step 4: Run** the tests; expected: PASS.
- [ ] **Step 5: Commit** `git add src/bridge && git commit -m "overlay: OverlayHub with pending requests, replies and events"`

### Task 2: slot 13 format and handler in the bridge

**Files:**
- Modify: `aurelia/src/bridge/overlay.rs` (format functions), `aurelia/src/commands/bridge.rs`
- Test: `aurelia/src/bridge/overlay_tests.rs`

**Interfaces:**
- Consumes: `OverlayHub` (Task 1).
- Produces:
  - `pub const SLOT_OVERLAY: u16 = 13;` ops: `OP_REQUEST = 1`, `OP_POLL = 2`, `OP_STATE = 3`, `OP_CLOSE_ALL = 4`.
  - `pub fn parse_request(payload: &[u8]) -> Option<(u32 /*app*/, String /*kind*/, serde_json::Value)>`: format `[u8 op=1][u32 app][u16 kind_len][kind][u16 json_len][json]`, JSON truncated at 2048 bytes when read, `None` if short or invalid.
  - `pub fn encode_updates(updates: &[Update]) -> Vec<u8>`: `[u32 n]` and, per item, `[u8 type: 1 answer-approved, 2 answer-denied-user, 3 answer-denied-steam, 4 shown, 5 hidden][u16 id_len][id]`.
  - `pub fn encode_state(capable: bool) -> Vec<u8>`: `[u8 capable]`.

- [ ] **Step 1: Write the failing tests**: `parse_request_roundtrip`, `parse_request_rejects_short_payload`, `parse_request_truncates_oversized_json_to_2048`, `encode_updates_layout`, `encode_state_layout`.
- [ ] **Step 2: Run** the tests; expected: FAIL.
- [ ] **Step 3: Implement** the functions; in `commands/bridge.rs`, in the per-slot dispatch (next to `SLOT_UGC`), handle `SLOT_OVERLAY`: `OP_REQUEST` → `hub.request`, answers `[u16 id_len][id]`; `OP_POLL` → `encode_updates(hub.drain_updates())`; `OP_STATE` → `encode_state(hub.capable())`; `OP_CLOSE_ALL` → `hub.close_all("game_exit")`. The hub is created in `bridge serve` and shared with the handler.
- [ ] **Step 4: Run** `cargo test`; expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "overlay: bridge slot 13 (request, query, state)"`

### Task 3: events on stdout, commands on stdin and `capabilities`

**Files:**
- Modify: `aurelia/src/commands/bridge.rs`, `aurelia/src/commands/capabilities.rs`, `aurelia/src/commands/capabilities_tests.rs`
- Test: `aurelia/src/bridge/overlay_tests.rs` (command parser)

**Interfaces:**
- Consumes: `OverlayHub`, the `UnboundedReceiver<Value>` from `OverlayHub::new`.
- Produces:
  - `pub enum Command { Respond { id: String, approve: bool }, Shown { id: String }, Hidden { id: String }, Capable { on: bool }, List }` and `pub fn parse_command(line: &str) -> Result<Command, String>` in `overlay.rs`. Lines over 8 KiB or an unknown `cmd` become `Err`.
  - `pub fn apply(hub: &OverlayHub, cmd: Command) -> serde_json::Value`: answer `{"event":"result","ok":true}` or `{"event":"error","error":"unknown_id"|"not_pending"|"not_microtxn"}`; `List` returns `{"event":"overlay_list","pending":[...]}`.

- [ ] **Step 1: Write the failing tests**: `parse_command_respond_approve`, `parse_command_rejects_unknown_cmd`, `parse_command_rejects_broken_json`, `parse_command_rejects_oversized_line`, `apply_respond_unknown_id_is_error_event`, `apply_capable_sets_flag`; in `capabilities_tests.rs`, `capabilities_lists_overlay_events_and_commands` (events `overlay_request`, `microtxn_request`, `overlay_closed`, `dlc_changed`).
- [ ] **Step 2: Run** `cargo test overlay capabilities`; expected: FAIL.
- [ ] **Step 3: Implement** the parser and `apply`; in `bridge serve`, after the `{"port","token"}` line, a tokio task prints each `Value` from the receiver with `print_json_line`, and another reads stdin line by line (`tokio::io::BufReader`), applies the command and prints the return; end of stdin does not stop `serve`. Update `capabilities.rs` (list of events, commands, `schema` +1).
- [ ] **Step 4: Run** `cargo test`; expected: PASS. Manual check: `printf '{"cmd":"list"}\n' | ./target/debug/aurelia bridge serve --app 480` prints the port line and then `overlay_list`.
- [ ] **Step 5: Commit** `git commit -am "overlay: events on stdout and commands on stdin of bridge serve; capabilities"`

### Task 4: shim `overlay.c` (requests, state and callbacks)

**Files:**
- Create: `plus/shim/ifaces/overlay.c`
- Modify: `plus/shim/shim.h`, `plus/shim/core.c` (`register_overlay()`), `plus/catalog/policy.json` (remove the implemented methods from policy; regenerate `shim/generated` with `python3 -m tools.gen.emit_shim ... --abi win64` and `python3 -m tools.policy.gen_policy`), `plus/tests/test_shim.py`, `plus/tests/selftest.c`

**Interfaces:**
- Consumes: slot 13 (Task 2).
- Produces:
  - `void register_overlay(void);` and `void shim_overlay_game_exit(void);` (called when the shim is unloaded: sends `OP_CLOSE_ALL`).
  - Implementations registered per family: `IsOverlayEnabled` (queries `OP_STATE`, 1 s cache), `ActivateGameOverlay` (kind = text argument: `friends`, `community`, `players`, `settings`, `official_game_group`, `stats`, `achievements`), `ActivateGameOverlayToUser` (kind `user`, `{"dialog":"...","steam_id":N}`), `ActivateGameOverlayToWebPage` (kind `web_page`, `{"url":...,"mode":N}`, URL truncated at 2048), `ActivateGameOverlayToStore` (kind `store`, `{"app_id":N,"flag":N}`), `ActivateGameOverlayInviteDialog` (kind `invite`, `{"lobby":N}`), `ActivateGameOverlayInviteDialogConnectString`, `ActivateGameOverlayRemotePlayTogetherInviteDialog`.
  - Answer thread (starts on first use): `OP_POLL` every 250 ms; `Shown`/`Hidden` become `GameOverlayActivated_t` (callback 331, `{u8 active, u8 pad[3]...}` according to the size the catalog/DLL use; confirm with the DLL's `GameOverlayActivated_t`) through `shim_defer`; microtxn answers are delivered to Task 5 through `void shim_microtxn_answer(const char *id, int approved, int denied_by_steam)`.

- [ ] **Step 1: Write the failing tests**: `FakeBridge` gains slot 13 (stores the requests received, returns configurable updates); selftest `overlay_requests_reach_the_bridge`: calls `ActivateGameOverlayToStore(appid=1234)` and `ActivateGameOverlayToWebPage` with a 5000-character URL, checks on the bridge: kind `store`, `app_id` 1234, URL with 2048 bytes; `overlay_disabled_until_launcher_declares_capable`: `IsOverlayEnabled` = 0, then with `OP_STATE` capable = 1; `overlay_shown_posts_game_overlay_activated`: `Shown`/`Hidden` updates become callback 331 with `active` 1 and 0.
- [ ] **Step 2: Run** `python3 -m unittest tests.test_shim.OverlayUnderWine` (new class, `@_both`); expected: FAIL.
- [ ] **Step 3: Implement** `overlay.c` following the pattern of `ifaces/ugc.c` (bridge call with `bridge_call_fresh`, own thread, `shim_defer` for the game thread); add the test class.
- [ ] **Step 4: Run** the new class and `python3 -m tools.policy.status` (pending=0); expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "overlay: shim forwards overlay requests to Aurelia and delivers GameOverlayActivated_t"`

### Task 5: microtransaction (decode and implement)

**Files:**
- Create: `plus/docs/superpowers/notes/microtxn-wire.md`, `plus/shim/ifaces/microtxn.c`
- Modify: `plus/tools/bridgeprobe.py` (`microtxn` scenario), `plus/shim/shim.h`, `aurelia/src/bridge/overlay.rs` (if a field is missing), `plus/tests/test_shim.py`, `plus/tests/selftest.c`

**Interfaces:**
- Consumes: `shim_cm_subscribe`/`shim_cm_on_push` (Steam push), `shim_cm_call` (fetch information), `shim_microtxn_answer` (Task 4), slot 13 `OP_REQUEST` with kind `microtxn`.
- Produces: callback 152 `MicroTxnAuthorizationResponse_t {u32 appid, u64 orderid, u8 authorized}` (pack 8) to the game; dismiss callback when Steam withdraws the request.

- [ ] **Step 1: Decode** in `steamclient64.dll` (RTTI/strings: `CClientJobMicroTxnAuthRequest`, `CAPIJobRequestMicroTxnInfo`, `CAPIJobAuthorizeMicroTxn`, `MicroTxnAuthRequestCallback_t`, `MicroTxnAuthDismissCallback_t`) which EMsgs and fields carry: the pushed request (app, order id), the information fetch (items, price, currency, sandbox, refundable) and the answer (authorize/deny). Write the format in `notes/microtxn-wire.md` with the EMsgs and fields. Check: the document lists each message with its EMsg number and protobuf fields, and every statement was seen in the DLL or in a live capture.
- [ ] **Step 2: Write the failing tests** with sample bytes taken from the document: `FakeBridge` pushes the request and answers the information fetch; selftest `microtxn_request_waits_for_launcher_then_posts_152`: checks the `microtxn_request` event on the bridge (items, total, sandbox), then delivers an approved/denied `Answer` and checks callback 152 and the authorization message sent to Steam; `microtxn_dismissed_by_steam_posts_dismiss`; `microtxn_denied_when_game_exits`.
- [ ] **Step 3: Run** the new tests; expected: FAIL.
- [ ] **Step 4: Implement** `microtxn.c`: subscribe to the push, fetch the information (`shim_cm_call`, in a thread), send `OP_REQUEST` with kind `microtxn` and the spec's JSON, on receiving the answer (Task 4) send the authorization message with the result and post callback 152 through `shim_defer`; on close, deny pending ones (`OP_CLOSE_ALL`).
- [ ] **Step 5: Run** 64/32-bit tests; expected: PASS. Live check without buying: with a sandbox transaction or a real order, observe the event and answer **deny**; any real approval only with the user's confirmation.
- [ ] **Step 6: Commit** `git commit -am "overlay: microtransaction (order, information and authorization) with launcher reply"`

### Task 6: DLC and `dlc_changed`

**Files:**
- Modify: `plus/shim/ifaces/apps.c`, `aurelia/src/bridge/snapshot.rs` (new DLC ownership), `aurelia/src/commands/bridge.rs` (`dlc_changed` event), `plus/tests/test_shim.py`, `plus/tests/selftest.c`

**Interfaces:**
- Consumes: `OverlayHub` (to publish events), existing DLC snapshot.
- Produces: event `dlc_changed {app_id, dlc_id, owned, installed}` when ownership or installation changes (periodic check every 60 s while `bridge serve` runs); callback `DlcInstalled_t` (1005 `{u32 appid}`) in the game when it becomes installed; `BIsDlcInstalled` and `GetDLCCount` reflect the updated list.

- [ ] **Step 1: Write the failing tests**: Aurelia `dlc_diff_emits_changed_events_only_for_differences`; selftest `dlc_installed_callback_after_ownership_change` (the fake bridge changes the DLC from not owned to installed between two polls).
- [ ] **Step 2: Run**; expected: FAIL.
- [ ] **Step 3: Implement** the comparison (pure function `diff_dlc(old, new) -> Vec<DlcChange>`), the check loop in `bridge serve` and the callback delivery in the shim.
- [ ] **Step 4: Run** `cargo test` and the shim tests 64/32; expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "overlay: dlc_changed e DlcInstalled_t"`

### Task 7: final verification, documentation and memory

**Files:**
- Modify: `plus/docs/overlay-json.md`, `plus/docs/phase-b-pending.md`

- [ ] **Step 1: Run** the full suite (`python3 -m unittest discover -s tests -t .` in `plus/` and `cargo test` in `aurelia/`); expected: all green.
- [ ] **Step 2: Live check** with L4D2: `aurelia bridge serve --app 550` with a simple stdin/stdout reader; open the store from the game (if there is a path) and confirm `overlay_request`; `{"cmd":"capable","on":true}` turns on `IsOverlayEnabled`.
- [ ] **Step 3: Document** the contract (events, commands, examples, event order, errors) in `plus/docs/overlay-json.md`; update the pending items.
- [ ] **Step 4: Commit** `git commit -am "docs: overlay and purchase JSON contract for launchers"`
