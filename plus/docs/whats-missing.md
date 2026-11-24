# What is still missing (Nov 16, 2026)

Size: P = hours, M = days, G = project. "Unblocks" says what depends on something outside the code.

## A. Purchases and overlay (approved plan: `superpowers/plans/2026-11-16-launcher-overlay-info.md`)
| # | Item | Size | Unblocks |
|---|---|---|---|
| A1 | `OverlayHub` + slot 13 + events on stdout / commands on stdin of `bridge serve` | M | nothing |
| A2 | Shim: `ActivateGameOverlay*`, `IsOverlayEnabled`, `GameOverlayActivated_t` | M | A1 |
| A3 | Microtransaction (`MicroTxn`): decrypt Steam messages, request items/price, authorize/deny | M | RE of the DLL; a live test needs a sandbox transaction |
| A4 | DLC: `dlc_changed`, `DlcInstalled_t`, ownership updated after purchase | P | A1 |
| A5 | Native store in the launcher with cart and checkout (separate session) | G | your decision: wallet balance only in v1? |
| A6 | Real test purchase (inventory: `StartPurchase`) | P | your confirmation of item and amount |

## B. Network and online
| # | Item | Size | Unblocks |
|---|---|---|---|
| B1 | P2P compatible with official Steam (GameNetworkingSockets: Valve certificates, encryption, ICE, relay) | G | separate project |
| B2 | Live P2P signaling (today only a fake "CM") | P | second Steam account |
| B3 | Server login with persistent token (success path) | P | a real GSLT that you create |
| B4 | IP sockets over IPv6 | P | low priority |

## C. Account and data
| # | Item | Size | Unblocks |
|---|---|---|---|
| C1 | `BIsPhoneVerified`, `BIsTwoFactorEnabled`, `GetPlayerSteamLevel` (level already done): capture account data from the CM | P | nothing |
| C2 | Signed inventory (`SerializeResult`/`DeserializeResult`) | M | account with items |
| C3 | Workshop publishing (SteamPipe) | G | your confirmation; private item on Spacewar |
| C4 | `GetNewSession` (match telemetry, `SteamGameStats`) | P | decrypting the message |

## D. Steamworks fidelity
| # | Item | Size | Unblocks |
|---|---|---|---|
| D1 | Reduce the 661 differing responses in the comparison with real Steam, starting with the ones games use | M | nothing |
| D2 | Error codes for `SteamGameStats` and `SteamMatchGameSearch` (8/9 instead of 1) | P | nothing |
| D3 | `GetNumAchievements`/`GetNumStats` on app 480; `GetFriendCount(0)` | P | investigate |
| D4 | Compare with real arguments (scenarios per family), not only zeroed ones | M | nothing |
| D5 | Steam Input (controllers) and Remote Play: today "no controller" / "unavailable" | G | device to test with |

## E. Games and environment
| # | Item | Size | Unblocks |
|---|---|---|---|
| E1 | More games from different families (EA, Rockstar, Battle.net, anti-cheat) | M | installed games; logins done by you |
| E2 | Kernel anti-cheat | — | out of reach (depends on the publisher) |
| E3 | Heavy DRM (Denuvo and others) | ? | case by case |

## F. Project phases
| # | Item | Size |
|---|---|---|
| F1 | Phase D: native `steamclient.so` (Linux games) | G |
| F2 | Phase E: final review and documentation; launcher integration only when you ask | M |
| F3 | Silence signature warnings (the benign `ARITY` no longer appears) and declare generic signatures per version | P |

## Suggested order
A1 → A2 → A4 → A3 (decrypt) → C1 → D1/D2/D3 → E1 → F1. B1, C3 and A5 are projects of their own, outside the short queue.
