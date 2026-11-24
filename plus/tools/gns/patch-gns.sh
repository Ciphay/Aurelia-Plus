#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src=${1:-$here/../../run/gns/gns}
python3 -I - "$src/src/steamnetworkingsockets/clientlib" <<'P'
import sys, os
base = sys.argv[1]
patches = [
    ("csteamnetworkingsockets.cpp", "GameNetworkingSockets_SetAppID",
     "STEAMNETWORKINGSOCKETS_INTERFACE ISteamNetworkingUtils *SteamNetworkingUtils_LibV4()\n{\n\tstatic CSteamNetworkingUtils s_utils;\n\treturn &s_utils;\n}\n",
     "\n// Aurelia: the app this process claims to be (the Steam client does this internally).\nSTEAMNETWORKINGSOCKETS_INTERFACE void GameNetworkingSockets_SetAppID( uint32 nAppID )\n{\n\tstatic_cast<CSteamNetworkingUtils *>( SteamNetworkingUtils_LibV4() )->SetAppID( nAppID );\n}\n"),
    ("steamnetworkingsockets_socketthread.cpp", "if ( RtlDllShutdownInProgress() ) return;",
     "\t\tatexit( []{\n",
     "#ifdef _WIN32\n\t\t\t// Aurelia: process shutdown; the other threads are already dead (perhaps holding the lock). The system releases the rest.\n\t\t\tif ( RtlDllShutdownInProgress() ) return;\n#endif\n"),
    ("csteamnetworkingsockets.cpp", "FnSteamNetAuthenticationStatusChanged )\n\t\t#else",
     "\t\tswitch ( x.nCallback )\n\t\t{\n\t\t\tDISPATCH_CALLBACK( SteamNetConnectionStatusChangedCallback_t, FnSteamNetConnectionStatusChanged )\n\t\t#ifdef STEAMNETWORKINGSOCKETS_ENABLE_SDR\n\t\t\tDISPATCH_CALLBACK( SteamNetAuthenticationStatus_t, FnSteamNetAuthenticationStatusChanged )\n",
     "\t\t#else\n\t\t\t// Aurelia: the certificate state also changes without SDR (SetCertificate); it is queued and must be delivered\n\t\t\tDISPATCH_CALLBACK( SteamNetAuthenticationStatus_t, FnSteamNetAuthenticationStatusChanged )\n\t\t#endif\n\t\t#ifdef STEAMNETWORKINGSOCKETS_ENABLE_SDR\n"),
    ("csteamnetworkingsockets.h", "static bool s_bAureliaP2PLoopback;",
     "\tstatic constexpr bool m_TEST_bEnableP2PLoopbackOptimization = true;\n",
     "\t// Aurelia: test between processes of the same account (the local shortcut does not let P2P to our own identity go through Steam)\n\tstatic bool s_bAureliaP2PLoopback;\n"),
    ("csteamnetworkingsockets.h", "#define m_TEST_bEnableP2PLoopbackOptimization",
     "\tstatic bool s_bAureliaP2PLoopback;\n",
     "\t#define m_TEST_bEnableP2PLoopbackOptimization s_bAureliaP2PLoopback\n"),
    ("csteamnetworkingsockets.cpp", "GameNetworkingSockets_TEST_EnableP2PLoopback",
     "\tstatic_cast<CSteamNetworkingUtils *>( SteamNetworkingUtils_LibV4() )->SetAppID( nAppID );\n}\n",
     "\n// Aurelia: only for testing between processes of the same account.\nbool CSteamNetworkingSockets::s_bAureliaP2PLoopback = true;\nSTEAMNETWORKINGSOCKETS_INTERFACE void GameNetworkingSockets_TEST_EnableP2PLoopback( bool bEnable )\n{\n\tCSteamNetworkingSockets::s_bAureliaP2PLoopback = bEnable;\n}\n"),
    ("steamnetworkingsockets_socketthread.cpp", "RtlDllShutdownInProgress( void )",
     "static std::thread *s_pServiceThread = nullptr;\n",
     "#ifdef _WIN32\nextern \"C\" __declspec(dllimport) BOOLEAN NTAPI RtlDllShutdownInProgress( void );\n#endif\n"),
]
for name, marker, anchor, add in patches:
    p = os.path.join(base, name)
    s = open(p).read()
    if marker in s:
        print(name, "already applied"); continue
    assert anchor in s, f"anchor not found in {name}"
    open(p, "w").write(s.replace(anchor, anchor + add, 1))
    print(name, "applied")
P
