# Overlay and purchases inside games: JSON information for the launcher

## Intent
Aurelia is a CLI backend. It does not draw an overlay, an approval window or a store. Aurelia Plus (shim + Aurelia) must deliver, as
JSON, everything a launcher needs to draw those screens in its own interface, and accept the user's answers back,
so that the game and Steam behave as if an overlay existed. Underlying goal: games work completely, including
microtransactions and DLC purchases, without Aurelia imposing an interface.

## Goals
- Every screen request made by the game (store, web page, dialogs, invites) reaches the launcher as a JSON event.
- Microtransactions (`MicroTxn`): Steam pushes the authorization request; the shim fetches items, price, currency, sandbox and refund info, publishes the
  event and returns to Steam the answer (authorize/deny) given by the launcher; only then does the game receive the callback.
- DLC purchase: after ownership changes (purchase made by the launcher/Aurelia), the game is notified (`DlcInstalled_t`, `BIsDlcInstalled`).
- Overlay state for the game: `IsOverlayEnabled` answers yes only when the launcher declares that it can handle the overlay; `GameOverlayActivated_t`
  is sent when the launcher says it showed or hid the screen, so the game can pause.
- Versioned contract, described in `aurelia capabilities --json`.

## Non-goals
- No window, overlay, approval screen or store drawn by Aurelia or by the shim.
- Do not type card data or passwords; payment is always the user's (wallet or saved method, in the launcher's interface).
- Do not touch any launcher in this step; the contract is consumed later.
- Interoperate with the official Steam client over P2P (a separate project).

## Anti-goals (it would be a failure even if it "works")
- Approving a microtransaction without the user. Denial only happens when the user denies, when the game closes or when Steam withdraws the request; never by silence.
- A game that hangs waiting for the launcher: authorization is an asynchronous callback, so the game keeps running while the request is pending.
- Per-game shortcuts: everything by interface family and by request type.

## Constraints
- No Valve code and no SDK with an unverified license; formats discovered by reverse engineering the client DLL and by live observation.
- Real purchase only with explicit confirmation from the user (item and total); approval tests use the fake bridge and sandbox mode.
- The shim talks to Aurelia only through the bridge (reserved slots already used); the game thread never blocks on the network (use `shim_defer`).

## Approaches considered
- **A (chosen): JSON stream of `aurelia play --json` + CLI commands that talk to the daemon.** It reuses the existing event system and daemon;
  a launcher that already starts the game through Aurelia receives the events and answers with `aurelia overlay …`. Cost: the launcher must
  start the game through `aurelia play` to see the stream.
- **B: a local socket that the launcher subscribes to**, independent of `play`. It serves whoever starts the game from outside, but it is a new piece (port, token,
  reconnection). It remains as an evolution if A is not enough.
- **C: an inbox file watched by the launcher.** Simple, but with latency and no reliable delivery; discarded.

## Design

### Contract (events, in the JSON stream)
```json
{"event":"overlay_request","id":"7","app_id":550,"kind":"store","target":{"app_id":1234,"dlc":true}}
{"event":"overlay_request","id":"8","app_id":550,"kind":"web_page","url":"https://..."}
{"event":"microtxn_request","id":"9","app_id":550,"order_id":"...","sandbox":false,"refundable":false,
 "currency":"BRL","items":[{"name":"...","qty":1,"price":"..."}],"total":"..."}
{"event":"overlay_closed","id":"9","reason":"answered|dismissed_by_steam|game_exit"}
{"event":"dlc_changed","app_id":550,"dlc_id":1234,"owned":true,"installed":false}
```
`kind` of `overlay_request`: `store`, `web_page`, `user`, `invite`, `friends`, `achievements`, `community`, `settings`, `remote_play_invite`.
The `id` field is unique per run. New fields may be added; the consumer ignores what it does not recognize (`schema` goes up with each change).

### Contract (commands)
```
aurelia overlay list [--json]
aurelia overlay respond <id> --approve | --deny
aurelia overlay shown <id>      # the launcher showed the screen: the game receives GameOverlayActivated_t {active:1}
aurelia overlay hidden <id>     # hid it: {active:0}
aurelia overlay capable on|off  # declares that a launcher is handling the overlay (turns on IsOverlayEnabled)
```
`respond` only applies to `microtxn_request`; for the others it is `shown`/`hidden`. A command for an unknown or already closed id fails with a typed error.

### Shim (C)
- The `ActivateGameOverlay*` and `IsOverlayEnabled` methods stop being policy: they become an implementation that posts `overlay_request` through the bridge (new
  reserved call 13, short deadline, without blocking the game thread) and queries the "capable" state.
- Microtransaction: subscribe to Steam's authorization push (via `shim_cm_subscribe`), fetch the transaction information through a service/message
  (format to be decoded in the DLL: `CClientJobMicroTxnAuthRequest`, `CAPIJobRequestMicroTxnInfo`, `CAPIJobAuthorizeMicroTxn`), publish the event and,
  on the answer, send authorize/deny and post `MicroTxnAuthorizationResponse_t` (callback 152).
- DLC: react to the ownership change (snapshot/relay) and post `DlcInstalled_t` (1005).

### Aurelia (Rust)
- Bridge: slot 13 receives `overlay_request` from the shim and keeps the queue per run; it returns to the shim the answers and the `shown/hidden` states.
- New events in the `play --json` stream and `overlay *` commands talking to the daemon; `capabilities` describes both.
- `microtxn_request` has no deadline in Aurelia (as on real Steam): it stays pending until the launcher answers, Steam withdraws the request
  (`MicroTxnAuthDismissCallback_t`, emits `overlay_closed {reason:"dismissed_by_steam"}`) or the game closes (denies and emits `game_exit`).

### Tests
- Fake bridge: shim ↔ Aurelia with simulated overlay and microtransaction requests; answer approve, deny and expire the deadline; 64 and 32 bits.
- Aurelia: tests of the JSON contract (events and commands, unknown ids, schema).
- Live, without buying: observe a real (or sandbox) request and deny it. Any real approval only with the user's confirmation.
- Pending request with no answer: the game keeps running; when the game closes the request is denied and the `overlay_closed` event is emitted.

## What is left for whoever builds it
- The exact format of Steam's microtransaction messages (the DLL has the classes mentioned; the rest is live observation).
- Additional `kind` values according to the overlay methods found in the catalog.
- Final name of the reserved bridge calls and of the `schema` field.

## My decisions (to be checked)
- No deadline for approval (the user's decision, same as real Steam).
- `IsOverlayEnabled` only turns on with `overlay capable on` (or an equivalent flag in `play`).
- Events in the same `play --json` stream, with no separate channel.

## Correction after reading the code (16/Nov/2026)
The bridge is the process `aurelia bridge serve --app N` (long-lived, separate from `aurelia play`; today it prints `{"port","token"}` on the first line and
keeps going). So the real channel is its own, in the same style as `aurelia social open --json` (JSON events per line on stdout, JSON commands per
line on stdin):
- Events: JSON lines on the **stdout of `bridge serve`**, after the `{"port","token"}` line.
- Commands: JSON lines on the **stdin of `bridge serve`**: `{"cmd":"respond","id":"9","approve":true}`, `{"cmd":"shown","id":"7"}`,
  `{"cmd":"hidden","id":"7"}`, `{"cmd":"capable","on":true}`, `{"cmd":"list"}`.
- The `aurelia overlay …` subcommands from the sketch are out of scope for this step (they would be a wrapper over another control channel); the command names
  above are the same. The rest of the contract does not change.
