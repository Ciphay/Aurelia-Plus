# Phase B pending items

Phase B is implemented and tested wherever it can be tested alone (a suite of 99 tests under Wine in 64 and 32 bits, Aurelia tests,
live checks against Steam). What is missing depends on something that is not in the code.

| Pending item | What is missing | Who unblocks it |
|---|---|---|
| Live P2P signaling through Steam | A second Steam account to actually exchange `CMsgClientP2PConnectionInfo`. Today there is only a test with a fake "CM" between two processes | User's second account |
| Server login with persistent token | A real GSLT (created at `steamcommunity.com/dev/managegameservers`, giving the AppID). Only the error path has been seen ("invalid credential") | User creates the token |
| Signed inventory (`SerializeResult`/`DeserializeResult`) | An account with items, to see Steam's real response and the signature format | Account with items |
| Real purchase (`StartPurchase`) | Explicit confirmation of item and amount (candidate: Overwatch, 500 Coins, R$ 23,95); the user approves the payment on the Steam page. Command: `LIVE_BUY=<itemdef> python3 -m tests.live_check 2357570 win64 livebuy` | User confirms |
| Workshop publishing | Uploading content to SteamPipe (`CMsgClientUCMUpdatePublishedFile`); it publishes to the account, so only with confirmation. Suggested test: private item on Spacewar (480) | User confirms; large piece of work |
| Interoperate with official Steam / Valve relay | Implement the GameNetworkingSockets protocol (Valve certificates, encryption, ICE, SDR routes). Today P2P only talks between Aurelia Plus shims | Separate project |
| Microtransaction (`microtxn_request`) | Decrypt the legacy binary format of the Steam messages (EMsg 5504 to 5509; there is no protobuf in `steamclient64.dll`) and validate with a real sandbox purchase or a real order, answering **deny** | User confirms any approval |
| IP sockets: IPv6 | Only mapped IPv4; pure IPv6 is not accepted | Low priority |

Resolved in the last round: IP sockets (direct transport) and `RequestStoreAuthURL` (service `Econ.ClientGetItemShopOverlayAuthURL#1`,
confirmed live).

Resolved afterwards: overlay as JSON for launchers (`docs/overlay-json.md`), `dlc_changed`, and the P2P clock race that closed sessions as idle.
