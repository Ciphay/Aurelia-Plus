# Overlay and purchases as JSON for the launcher

Aurelia does not draw an overlay. When the game asks for a Steam screen (store, web page, profile, invite, purchase), the shim forwards the request to
Aurelia, and `aurelia bridge serve` publishes it as **a JSON line on stdout**. The launcher does the drawing and answers
with **a JSON line on stdin**. The answer reaches the game.

## How the launcher talks to Aurelia

```
aurelia --json bridge serve --app <appid>
```

- First line of stdout: `{"port":N,"token":"<hex>"}` (the shim's bridge; `launch.sh` hands it to the game).
- After it, each event is a JSON line on stdout; each command is a JSON line on stdin.
- End of stdin does **not** stop `serve`. `serve` ends on a signal (SIGTERM/Ctrl-C) or when the launcher stops it.
- `aurelia capabilities --json` lists the events (`events`) and the `bridge.overlay.stdin_control` feature; the `schema` field went from 1 to 2.
- `plus/launch.sh` sends the `serve` stdout to a file; a launcher that wants to handle the overlay should start `bridge serve` itself and
  read the lines.

## Events (stdout)

Every event has `event`. New fields may appear; ignore what you do not recognize.

### `overlay_request`
The game asked for a screen. `id` is unique per run; `app_id` is the one from `bridge serve`.

```json
{"event":"overlay_request","id":"7","app_id":550,"kind":"store","target":{"app_id":1234,"flag":0}}
{"event":"overlay_request","id":"8","app_id":550,"kind":"web_page","url":"https://...","mode":"default"}
{"event":"overlay_request","id":"9","app_id":550,"kind":"user","dialog":"chat","steam_id":76561198000000042}
{"event":"overlay_request","id":"10","app_id":550,"kind":"invite","lobby":109775240000000001}
{"event":"overlay_request","id":"11","app_id":550,"kind":"invite","connect":"+connect 1.2.3.4"}
{"event":"overlay_request","id":"12","app_id":550,"kind":"remote_play_invite","lobby":109775240000000002}
{"event":"overlay_request","id":"13","app_id":550,"kind":"friends"}
```

The `kind` of `ActivateGameOverlay("...")` is the game's text in lowercase (`friends`, `community`, `players`, `settings`, `official_game_group`,
`stats`, `achievements`; any other text becomes the sanitized text or `other`). Texts over 2048 bytes are truncated.

`web_page` carries `mode`: `default` or `modal` (the window must block the game). Only the newer versions of the interface (`SteamFriends017` onward) report the mode; in the older ones it is always `default`.

### `overlay_closed`
```json
{"event":"overlay_closed","id":"9","reason":"answered"}
```
`reason`: `answered` (the launcher answered), `dismissed_by_steam` (Steam withdrew the request) or `game_exit` (the game closed; it is emitted once per pending
request and the request is denied).

### `connection`
State of the bridge's connection to Steam. It is emitted once at startup (`online:false` if the game started without network, with the snapshot from disk; `true` when the session is ready) and again when the bridge connects later.
`queued` is the number of operations (stats, Cloud upload) that the game saved without network and that have not been uploaded yet.

```json
{"event":"connection","online":false,"queued":1}
```

Offline, the overlay commands (`capable`, `shown`, `respond`, `list`...) work; the account queries (`friends`, `chat_send`, `profile`...) answer `error` with `error:"offline"`, and so does `web_session`. When `connection` returns to `online:true`, everything answers again.
With the service installed (`aurelia service install`) the daemon starts in the background on the first connection and does not close when the game ends: the next startup takes about 1.4 s. `aurelia bridge status --json` shows what is attached.

### `dlc_changed`
```json
{"event":"dlc_changed","app_id":550,"dlc_id":1234,"owned":true,"installed":false}
```
Emitted when the ownership or installation of a DLC changes while the game is open (checked every 60 s). It only tracks the DLCs that the snapshot brought. The
game receives `DlcInstalled_t` when the DLC becomes installed, and `BIsDlcInstalled`/`GetDLCCount` start reflecting the new state.

### `microtxn_request`
The game opened a purchase with the Steam wallet (for example, Overwatch Coins). It is the equivalent of the "Purchase" window that Steam's overlay opens: the launcher
shows **Steam's approval page** in its own browser and returns the decision.

```json
{"event":"microtxn_request","id":"1","app_id":2357570,"order_id":"506252350422944073","trans_id":"410551019264029606",
 "url":"https://checkout.steampowered.com/checkout/approvetxn/410551019264029606/?returnurl=steam","window":{"width":1020,"height":440},
 "sandbox":false,"refundable":false,"requires_cached_payment_method":false,"realm":1,"currency":"BRL","currency_code":7,
 "total":"23.95","total_minor":2395,
 "items":[{"name":"500 Coins","qty":1,"price":"23.95","amount_minor":2395,"item_id":"847133"}]}
```
- `url`: the page that Steam opens in the overlay. It requires the user's Steam web session; without it, the browser asks for a login.
- `window`: the size Steam uses in the overlay (1020x440), only as a suggestion. **The window's appearance is up to the launcher.**
- Values in `*_minor` are the smallest unit of the currency; `price` and `total` come already formatted (currencies without cents, such as JPY, KRW, VND and CLP, come as whole numbers).

How Steam does it (read from the code of its interface): the overlay's browser loads `url`; when the user decides, the page redirects to
`steam://close/authorize...` (approved) or `steam://close/cancel...` (cancelled). The launcher intercepts that redirect and sends the
`steam_url` command (below), or, if it prefers to interpret it itself, `respond`. Aurelia then sends the decision to Steam by the **transaction** number and the game
receives `MicroTxnAuthorizationResponse_t`. If the launcher does not do this, the game shows a purchase failure.

### Command responses
```json
{"event":"result","ok":true}
{"event":"error","error":"unknown_id"}
{"event":"overlay_list","pending":[{"id":"9","app_id":550,"kind":"microtxn"}]}
```
`error`: `unknown_id`, `not_pending`, `not_microtxn` or `invalid_command` (with `detail`). An invalid line (broken JSON, unknown `cmd`, more
than 8 KiB) produces `invalid_command` and `serve` keeps running.

## Commands (stdin)

```json
{"cmd":"capable","on":true}
{"cmd":"shown","id":"7"}
{"cmd":"hidden","id":"7"}
{"cmd":"respond","id":"9","approve":true}
{"cmd":"steam_url","id":"9","url":"steam://close/cancel"}
{"cmd":"list"}
```

- `capable`: declares that a launcher is handling the overlay. **`IsOverlayEnabled` only answers yes after this.** Without declaring it, requests are still emitted
  as events and the game does not hang.
- `shown` / `hidden`: the launcher showed/hid the request's screen. The game receives `GameOverlayActivated_t` with `active` 1 / 0 (callback 331).
- `respond`: only valid for `microtxn_request`. Approving or denying reaches the game and Steam. No purchase is approved automatically.
- `steam_url`: forwards the approval page's redirect: `steam://close/authorize...` approves and `steam://close/cancel...` denies (any other URL produces `unsupported_url`).
- `list`: pending microtransactions.

### Account queries and actions
Every command below accepts `req` (text); it comes back unchanged in the response event. SteamID64 goes as text (it exceeds 2^53 in JavaScript); a number is also accepted on input.

```json
{"cmd":"friends","req":"1"}
{"cmd":"chat_send","steam_id":"7656...","message":"hi","req":"2"}
{"cmd":"chat_history","steam_id":"7656...","count":20,"req":"3"}
{"cmd":"achievements","app_id":480,"lang":"brazilian","req":"4"}
{"cmd":"profile","steam_id":"7656...","req":"5"}
{"cmd":"persona","state":1,"req":"6"}
{"cmd":"friend_add","steam_id":"7656...","req":"7"}
{"cmd":"friend_remove","steam_id":"7656...","req":"8"}
{"cmd":"invite","steam_id":"7656...","connect":"+connect 1.2.3.4:27015","req":"9"}
{"cmd":"players_recent","req":"10"}
```

| Command | Response |
|---|---|
| `friends` | `{"event":"friends","friends":[{"steam_id","persona","state","game_appid"}]}` (last list read; refreshed every 3 s, together with `friend_status`) |
| `chat_send` | `{"event":"result","ok":true,"server_timestamp":N,"modified_message":null}` |
| `chat_history` | `{"event":"chat_history","steam_id","messages":[{"sender","from_self","message","timestamp"}]}`, most recent first; `count` defaults to 20, between 1 and 100 |
| `achievements` | `{"event":"achievements","app_id","achievements":[{"api_name","name","description","hidden","icon_unlocked","icon_locked","global_percent","unlocked","unlock_time"}]}`; `lang` default: Aurelia's language (config or system) |
| `profile` | `{"event":"profile","steam_id","persona_name","profile_url","avatar_url","visibility","is_public","persona_state","game_appid","game_name",...}` |
| `persona` | `result`; `state` from 0 (offline) to 6 (looking to play) |
| `friend_add` | `{"event":"result","ok":true,"steam_id","persona"}` |
| `friend_remove` | `result` |
| `players_recent` | `{"event":"players_recent","players":[{"steam_id","app_id","last_played"}]}`, most recent first (`last_played` in Unix seconds), up to 200 |
| `invite` | `result` (`ok` only says that Steam received the request; it does not confirm delivery) |

`invite` invites the friend to the game that `bridge serve` itself declared on the account (`--app`); `connect` is the line the friend's game receives on accepting and may be omitted. It does not need Steam to be open: serve has already declared the game, so the `check_invite` check (which requires a session reported by Steam) does not apply here.

`players_recent`: Steam has no "recent players" service; the list comes from the game, which calls `ISteamFriends::SetPlayedWith(steamID)` at the end of a match. The shim forwards the call and Aurelia keeps the list in `recent_players.json` (Aurelia's data folder), so it survives between games. A game that never calls `SetPlayedWith` produces nothing; there is no name (use `profile`). There is no event on registration: the launcher queries whenever it wants.

Steam failure: `{"event":"error","error":"query_failed","detail":"...","req":"..."}`. Missing or invalid field: `invalid_command` with `detail` (`missing_field: x`, `invalid_field: x`); `req` comes back if the line was JSON.

### Friend and chat events (stdout, unrequested)
`bridge serve` publishes them on its own while it runs; the launcher does not need to ask.

```json
{"event":"friend_status","change":"updated","steam_id":"7656...","persona":"Ana","state":1,"game_appid":480,"prev_state":0,"prev_game_appid":0}
{"event":"chat_message","steam_id":"7656...","from_self":false,"text":"hi","timestamp":1763400000}
{"event":"chat_typing","steam_id":"7656...","timestamp":1763400000}
{"event":"chat_invite","steam_id":"7656...","text":"...","timestamp":1763400000}
```
- `friend_status`: one event per friend whose `state` changed (0 offline … 6 looking to play) or whose `game_appid` changed (0 = not in a game). `change` is `updated`, `added` (new friend; `prev_*` null) or `removed`. Only the name changing does not produce an event. The list is read every 3 s; the `friends` command always returns the last one read.
- `chat_message`: `steam_id` is always the other side of the conversation; `from_self` is true for a message sent by this account on another device. `text` comes without BBCode when Steam provides it.
- `chat_typing`: the friend is typing. `chat_invite`: game invite received through chat (accepting does not exist yet, see O5/O6).
- Messages only arrive if the account's presence was announced (Aurelia does this when it reads the friends; with the official Steam open on the machine, it is the one that announces).

### Achievement unlocked
```json
{"event":"achievement_unlocked","app_id":480,"api_name":"ACH_WIN_ONE_GAME","name":"Winner","description":"Win one game.","hidden":false,
 "icon_unlocked":"https://...","icon_locked":"https://...","global_percent":0.4,"unlock_time":null}
```
Emitted when the game saves the achievement (`SetAchievement` + `StoreStats`) and Steam confirms; it is the same moment as the game's `UserAchievementStored_t`. Only `app_id` and `api_name` are guaranteed: name, description, icons and rarity come from Steam and are missing if the query fails. The texts come in Aurelia's language (`aurelia config language`; without that, the system's; without that, English). The first achievement fetches the app's definitions, once. `unlock_time` may be null, because the read is done right after the save. It does not create a pending request: there is no `respond` or `shown`.

### Lobby invite received
```json
{"event":"lobby_invite","id":"12","app_id":480,"lobby":"109775240000000001","inviter":"76561198000000002","game":"480"}
{"cmd":"accept_invite","id":"12"}
{"cmd":"decline_invite","id":"12"}
```
With the game open, Steam invites this account to a lobby and the game already received `LobbyInvite_t`; now the launcher also gets to know. `accept_invite` makes the game
receive `GameLobbyJoinRequested_t` (callback 333, lobby and friend), which is what Steam's overlay does on accepting; the game then joins the lobby as if the
user had clicked. Both commands close the invite with `overlay_closed` (`accepted` or `declined`); until then the invite also appears in `list` and is closed with
`game_exit` if the game closes. Lobby and user ids go as text. Errors: `unknown_id` and `not_invite`.

With the **game closed** there is no lobby to join: the invite arrives as `chat_invite` and, when the text carries `+connect_lobby <id>`, the event gets the field
`lobby`; the launcher opens the game with that command line. Aurelia does not open the game on its own.

## Home screen, no game: `aurelia watch`
```bash
aurelia --json watch
```
The same social flow as `bridge serve`, **without opening or declaring a game** (friends do not see "playing X"): `friend_status`, `friend_request`, `chat_message`, `chat_typing`,
`chat_invite`, plus the social commands on stdin (`friends`, `friend_requests`, `join`, `chat_send`, `chat_history`, `achievements`, `profile`, `persona`,
`friend_add`, `friend_remove`, `invite`, `web_session`). It prints `{"event":"ready","watch":true,"steam_id":"..."}` on startup (about 7 s) and ends with Ctrl-C or
SIGTERM; the end of stdin does not stop it. Commands that depend on a game (`respond`, `shown`, `list`, `players_recent`, `accept_invite`) make no sense here.
With the daemon running, it uses the daemon's session and friend list; without the daemon, it keeps its own. Presence follows `aurelia config presence` (default: invisible).

### "Joinable" friend and joining the match
Each friend carries what Steam knows about their match:
```json
{"event":"friend_status","change":"updated","steam_id":"7656...","persona":"Ana","state":1,"game_appid":480,"game_name":"Spacewar",
 "lobby":"1097752400...","server":"1.2.3.4:27015","status":"In match","rich_presence":{"status":"In match","connect":"+connect 1.2.3.4:27015"},
 "joinable":true,"connect":"+connect 1.2.3.4:27015","prev_state":1,"prev_game_appid":480,"prev_joinable":false}
```
- `joinable`: the friend is in a game and there is a way to join. `connect` is the command line the game receives: the `connect` that the game itself published in rich presence; otherwise
  `+connect_lobby <lobby>`; otherwise `+connect <server>`. It is the same rule as Steam's "Join" button. **(the order between lobby and server is my reading; only the
  rich presence `connect` is the convention documented by Steamworks)**
- `rich_presence` is what the game published (`status`, `connect`, `steam_display`...); texts that start with `#` are the game's translation tokens.
- A `friend_status` arrives when the state, the game, the lobby, the server or the rich presence changes (a game that updates rich presence every second produces a lot of events).

**Friend requests received:** `{"event":"friend_request","change":"received"|"closed","steam_id","persona"}`; `{"cmd":"friend_requests"}` answers
`{"event":"friend_requests","requests":[...]}`. Accepting is `friend_add` (Steam treats it as an accept) and declining is `friend_remove`. The requester's name may arrive
empty for a few seconds.

**Joining with the game closed:** `{"cmd":"join","steam_id":"..."}` answers
```json
{"event":"join","steam_id":"7656...","app_id":480,"game_name":"Spacewar","connect":"+connect_lobby 1097752400...","lobby":"1097752400...","installed":true,"needs_install":false}
```
or the error `not_friend` / `not_joinable`. Aurelia only resolves **what to open**; opening is up to the launcher: with Plus, `AURELIA_PLUS_GAME_ARGS="<connect>" launch.sh <app>`;
with `aurelia play`, there is not yet a way to pass arguments to the game (`--game-args` is missing). If `needs_install` is true, the launcher installs first (`aurelia install`).
For an invite received through chat with the game closed, `chat_invite` carries `lobby` when the text has `+connect_lobby <id>`.

### On-screen keyboard (`gamepad_text`)
When the game asks for a text field (`ISteamUtils::ShowGamepadTextInput`), the launcher receives a request and draws its own keyboard:
```json
{"event":"overlay_request","id":"14","app_id":480,"kind":"gamepad_text","mode":0,"line_mode":1,"max_chars":16,"description":"Your name","existing":"John"}
{"cmd":"text_input","id":"14","text":"Ana"}
{"cmd":"text_input","id":"14","cancel":true}
```
`mode`: 0 normal text, 1 password (`EGamepadTextInputMode`); `line_mode`: 0 single line, 1 multiple (`EGamepadTextInputLineMode`). The answer reaches the game as
`GamepadTextInputDismissed_t` (callback 714) and it reads the text with `GetEnteredGamepadTextInput`. The text is truncated at 2048 bytes. Without a launcher that has declared
`capable`, the request is refused (the game uses its own field).

**Floating keyboard** (`ISteamUtils::ShowFloatingGamepadTextInput`): the request goes out with the same `kind` `gamepad_text` and `"floating":true`. Here the text does not come back as
a single answer: each `text_input` becomes **keys typed into the game** (the shim injects them into the focused field) and the keyboard stays open. The launcher sends `text_input` as many
times as it wants; the request only closes with `cancel:true` or with an empty `text`, and then the game receives `FloatingGamepadTextInputDismissed_t` (callback 738).
```json
{"event":"overlay_request","id":"15","app_id":480,"kind":"gamepad_text","floating":true}
{"cmd":"text_input","id":"15","text":"ol"}
{"cmd":"text_input","id":"15","text":"a"}
{"cmd":"text_input","id":"15","text":""}
```
On close the `overlay_closed` event is emitted with `reason` `answered` (closed with an empty `text`) or `cancelled`; if the game itself closes the keyboard (`DismissFloatingGamepadTextInput`), the launcher also receives `overlay_closed`, to hide its own. Key injection was validated in a headless test and then in a real game (confirmed by the user, 21/Nov).

## Developing without a game: `bridge simulate`
```bash
aurelia --json bridge simulate [--app 480] [--interval-ms 400]
```
Same contract as `serve`, with no login, no Steam and no game. It prints `{"event":"ready","simulate":true,"app_id":480}` and, every `--interval-ms`
(0 emits everything at once), a step of sample events: `overlay_request` for store and web page, `microtxn_request`, `friend_status`,
`chat_typing`/`chat_message`/`chat_invite`, `achievement_unlocked` and `dlc_changed`. Commands on stdin answer with sample data in the real format
(`friends`, `chat_history`, `achievements`, `profile`, `players_recent`, `web_session`, ...); `respond`, `steam_url`, `shown`, `hidden`, `list` and
`capable` work for real on the sample purchases. It ends when stdin closes, after emitting all the steps. It writes nothing to disk and
does not touch the account.

## Typical order
1. `{"cmd":"capable","on":true}` at startup.
2. `overlay_request` arrives; the launcher shows the screen and sends `shown`.
3. The user closes it; the launcher sends `hidden`.

## Known limits
- The approval page needs the user's web session; Aurelia knows how to create it (`SteamClient::web_session`), but does not yet hand it to the launcher (handing over cookies is a security decision).
- Approving a real purchase has not yet been tested end to end; denying has (Steam answers OK).
- The tracked DLC list is the one from the `appinfo` of the app read at startup. Ownership and installation of those DLCs are re-evaluated every minute (`dlc_changed`); a DLC that Steam adds to the app during the match does not enter until the next `bridge serve`.
