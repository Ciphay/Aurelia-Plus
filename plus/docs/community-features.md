# Community features in the shim (24/Nov/2026)

What used to return "feature failure" now works outside the Steam client. Each feature uses the path that exists without it; the code is in `plus/shim/ifaces/community.c`, `otherstats.c` and `userstats.c`, and the account side is in `aurelia/src/bridge/` (`clanchat`, `webops`, `otherstats`, `appsquery`).

## New reserved calls (shim → bridge)
| Slot | Purpose | Format |
|---|---|---|
| 15 | Is Steam connected? | `[]` -> `[u8]` |
| 16 | Licenses: purchase date of an app | `[u8 1][u32 app]` -> `[u32]` |
| 17 | Web: Workshop favorites (1 and 2) and Market eligibility (3) | `[u8 op][u32 app][u64 id]` -> `[i32 1 ok]` or 20 bytes (Market) |
| 18 | Group chat: join (1), leave (2), speak (3), members (4), fetch messages (5) | `[u8 op][u64 clan][body]` |
| 19 | Other player's data: read (1) and write (2), through the user session or the server session | `[u8 op][u8 server][u64 player][...]` |

## How each feature works
- **Followers**: `ClientFSGetFollowerCount` (7515), `ClientFSGetIsFollowing` (7517) and `ClientFSEnumerateFollowingList` (7519) messages through the relay; the results are `FriendsGetFollowerCount_t` (344), `FriendsIsFollowing_t` (345) and `FriendsEnumerateFollowingList_t` (346).
- **FileShare**: `ClientUFSShareFile` (5232); the file must be in the app's Cloud.
- **Clan officers**: `ClientAMGetClanOfficers` (5531); only the count exists outside the client.
- **Group chat (clan)**: `ClanChatRooms.GetClanChatRoomInfo` finds the clan's group, `ChatRoom.JoinChatRoomGroup` joins, `ChatRoom.SendChatMessage` speaks, `ChatRoom.LeaveChatRoomGroup` leaves; messages arrive as `ChatRoomClient.NotifyIncomingChatMessage` notifications and become the `GameConnectedClanChatMsg_t` callback (338). `GetClanChatMessage` reads the message by callback number.
- **Parties**: `Parties` service (`CreateBeacon`, `JoinParty`, `ChangeNumOpenSlots`, `DestroyBeacon`, `OnReservationCompleted`, `CancelReservation`). The places to advertise come from the Steam client's groups, so the shim answers zero places.
- **Workshop favorites**: form POST to `https://steamcommunity.com/sharedfiles/favorite` and `/unfavorite` with `id`, `appid` and the `sessionid` of the account's web session.
- **Market**: the eligibility check (`/market/eligibilitycheck/`) returns the `webTradeEligibility` cookie; from it come `allowed`, the time it is released, and the days of Steam Guard and of a new device, and the reason (`EMarketNotAllowedReasonFlags`) is derived from those.
- **Another player's stats and game server**: `ClientGetUserStats` and `ClientStoreUserStats2`, through the user session or the bridge's game server session. The shim keeps the data of up to 16 players; the server changes it, marks achievements (the achievements field is a bitfield rebuilt from the bits) and saves.
- **Embedded browser**: nothing is embedded. `LoadURL` posts `HTML_StartRequest_t`; `AllowStartRequest(true)` hands the URL to the overlay/launcher (`web_page` request) and posts `HTML_URLChanged_t`, `HTML_ChangedTitle_t` and `HTML_FinishedRequest_t`. Paint, mouse and keyboard events do not apply.
- **Timeline**: each call becomes the `timeline_event` event for the launcher, with `kind` (`instant`, `range`, `range_start`, `range_update`, `range_end`, `remove`, `tooltip`, `tooltip_clear`, `game_mode`, `phase_start`, `phase_end`, `phase_id`, `phase_tag`, `phase_attribute`, `open_phase`, `open_event`), `title`, `description`, `icon`, `priority`, `offset`, `duration`, `clip_priority`, `handle` and `text`.
- **Coplay**: the history is also kept in the real client's file (`userdata/<account>/config/coplay_<steamid64>.vdf`).
- **Purchase date**: logon licenses + PICS packages, cached.

## New event for the launcher
`timeline_event` (stdout of `bridge serve`), for example: `{"event":"timeline_event","app_id":550,"kind":"instant","title":"Ace","description":"5 kills","icon":"trophy","priority":3,"offset":-2.5,"duration":0,"clip_priority":1,"handle":1,"text":""}`.

## New tools
- `python -m tools.catalog.pevtables <steamclient.dll> <saida.json>`: shape of the official client's vtables (via RTTI).
- `python -m tools.catalog.fill_missing <catalog.json> <steamclient.dll>`: fills the catalog with what `lsteamclient` does not know.
- `python -m tests.live_check <app> win64 live_workshop` (variables `LIVE_VARIANT=plain|video|versions|all|legacy`, `LIVE_APP`, `LIVE_KEEP`): Workshop against the real Steam, with private items that the test deletes.
