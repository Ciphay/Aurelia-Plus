# overrides

One `*.json` file per interface with the signatures of the hand-implemented methods:
`{"SteamUser023": {"GetSteamID": {"ret": "uint64", "args": ["void*"]}}}`.
It takes precedence over any other source. Methods without a signature get the stub that logs the call (`shim_unimplemented`).
