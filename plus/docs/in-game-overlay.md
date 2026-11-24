# In-game overlay (the way Steam does it)

Status: **under construction**. The "window on top" overlay (`plus/examples/overlay/main.js`) works, but it is a Linux window: you can Alt+Tab away from it. Steam's overlay is
drawn **inside the game's frames**. This document is the plan and a record of what has already been measured.

## How Steam does it
- `gameoverlayrenderer` is loaded into the game process (`LD_PRELOAD`); it intercepts the presentation of each frame (Vulkan: `vkQueuePresentKHR`;
  OpenGL: `glXSwapBuffers`) and draws the interface on top of the frame.
- The interface (web pages, in CEF) is rendered by the Steam client, outside the game, and reaches the layer through shared memory.
- While the overlay is active, mouse and keyboard go to it and not to the game; the game is notified through `GameOverlayActivated_t`.
- Under Proton, `lsteamclient` itself blocks the game's input events when it sees that notification (seen in the log: "overlay activated, disabling all input events").

## Our design
1. **Vulkan layer** (`plus/examples/overlay/vklayer/`, native C): implicit/explicit layer `VK_LAYER_AURELIA_overlay`. It intercepts `vkCreateSwapchainKHR` (adds
   `TRANSFER_DST`), and in `vkQueuePresentKHR` it copies the overlay's pixels to the center of the image before presenting. DXVK runs Overwatch on top of
   Vulkan, so the native layer applies (like MangoHud).
2. **Pixels**: the overlay (Electron) renders the page **offscreen** and writes BGRA to shared memory (`/dev/shm`), with the header
   `{magic, seq, visible, width, height, frame_w, frame_h}`; the layer reads it when `seq` changes.
3. **Input**: a transparent, **unfocused** window covers the screen and forwards the mouse (move, click, scroll) to the offscreen page through `sendInputEvent`; the game
   keeps system focus (which Steam preserves).
4. **Wiring**: `launch.sh` with `AURELIA_PLUS_OVERLAY=ingame` exports `VK_LAYER_PATH`, `VK_INSTANCE_LAYERS` and `AURELIA_OVERLAY_SHM` to the game.

## Measured on real Steam (Overwatch, Proton `+steamclient`)
- Purchase: `IsOverlayEnabled`, `StartPurchase`; after that the game just waits ("PROCESSING YOUR ORDER") and no longer calls anything from Steam.
- Notifications: `GameOverlayActivated_t` (331, 12 bytes) active and `SteamAPICallCompleted_t` (703) with the result of `StartPurchase` (24 bytes), in the order 331→703 or
  703→331 (it varies, ~15 ms); on decision: 331 inactive and 152 (`MicroTxnAuthorizationResponse_t`) 230 ms to 2 s later.
- The approval page is opened by the overlay in a 1020x440 browser and decides through `steam://close/authorize` or `steam://close/cancel`.

## RESOLVED: cause of LC-PF1 / LC-PF41 (2026-11-17)
Overwatch reads the callbacks **by hand** (`Steam_BGetCallback`) and, on seeing the 703 from `StartPurchase`, asks for the result through the **DLL export**
`Steam_GetAPICallResult(pipe, call, output, 24, 4704, &failed)`. The shim had that export as a stub that returned 0 without filling anything in; the game
concluded that the purchase had failed and showed the error before any decision. Now `exports.c` delivers the result through `shim_callresult_take` (once,
with a check of the expected type). Confirmed in the game: no error, it stays on "processing" and returns to the start on cancel, as on real Steam. How it was found: the
real Steam log (`+steamclient`) showed `Steam_GetAPICallResult 1, x, ..., 24, 4704` right after the 703, and the shim's `AURELIA_SHIM_TRACE=1` showed
no equivalent call. Lesson: compare **all** export calls, not just the interface ones. The text below is the record of the hypotheses
ruled out along the way.

## Investigation of the Overwatch purchase error (LC-PF1 on the 1st attempt, LC-PF41 on the following ones)
With the overlay (window on top or inside the game) the game shows an error **before** the decision comes out; on real Steam it just waits on "PROCESSING YOUR ORDER" and returns to the
start on cancel. The game executable is obfuscated: the code cannot be read. Already **ruled out** (each item was measured or tested):
- Data sent to the game: `StartPurchase` structure (24 bytes, same as the real `steamclient64.dll`), same order and transaction between `StartPurchaseResult` and callback 152,
  `GameOverlayActivated_t` (12 bytes), result timing (678 ms versus ~470 ms), 331/703 order (it varies on real Steam too).
- Early finalization of the order (`PurchaseFinalize`): removed; it only runs after approval.
- The decision: 5505 by transaction (the order made Steam answer Fail); Steam answers OK; callback 152 delivered.
- The 5508/5509 request (purchase data): done, Steam answers OK.
- The page: opens authenticated and correct (account, wallet, item, price).
- Focus/window: the error appears even without an overlay window (auto-responder) and with the overlay inside the game.
- Environment variables that Steam sets: `SteamOverlayGameId`, `SteamClientLaunch`, `SteamEnv` are now exported by `launch.sh`; the error persisted.
- The `GameOverlayRenderer64.dll` module is loaded in the game under the shim; the Windows modules are the same as on real Steam.

Remaining differences between the game process on real Steam and under the shim (measured through `/proc/<pid>/environ` and `maps`, see the script in `plus/docs`):
- `LD_PRELOAD=…/gameoverlayrenderer.so` (Linux) and `ENABLE_VK_LAYER_VALVE_steam_overlay_1=1`: only on real Steam. It is a Valve binary; the project does not distribute them.
- `SteamUser`: account name on real Steam (`rafalbr12`), number `76561198…` under the shim.
- `Steam3Master=127.0.0.1:<port>` (Steam client IPC), `STEAM_COMPAT_*`, pressure-vessel container: only on real Steam.
- Hypotheses not yet tested: `SteamUser` with the account name; messages that the real client sends to Steam at the start of the purchase (capturing CM network traffic is not possible);
  how the game reacts to `IsOverlayEnabled`/`BOverlayNeedsPresent` over the course of the purchase.

- **Network/TLS ruled out** (Wine `+secur32,+winhttp,+wininet,+crypt`, one purchase clicked): the game's hosts are only `overwatch.ingest.gdp`, `overwatch-assets-online2.playoverwatch.net`,
  `cdn.blz-contentstack.com`, `us.api.blizzard.com`, `oauth.battle.net`, `prod.depot.battle.net` and `telemetry-in.battle.net`; no store/payment host. The GnuTLS `ASSERT`s
  (`kx.c:718`, `buffers.c`) are normal handshake fragmentation and the HTTP responses are 200; the "assertion failure exception" in thread 0180 already appears at line 91 (game start),
  and comes from the game itself. There is no certificate/network error tied to the purchase error, so the failure is decided locally by the game.

## Known open issues
- Overwatch still shows a purchase error (LC-PF1 on the 1st attempt, LC-PF41 on the following ones) with the "window on top" overlay, **before** the decision comes out. Everything
  the game receives from Steam already matches the real thing (order, sizes, timing ~0.7 s versus ~0.47 s). The cause was not found; the game is obfuscated.
  Open hypothesis: the game losing focus when the Linux window opens (the in-game overlay does not take focus away).
