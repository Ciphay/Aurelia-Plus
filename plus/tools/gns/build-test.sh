#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=${GNS_WORK:-$here/../../run/gns}
arch=${1:-x86_64}
libs=$(ls "$work"/prefix-$arch/lib/lib*.a | grep -v lite)
"$arch-w64-mingw32-g++" -std=c++17 -O1 -I"$work/gns/include" -I"$here/include-alias" -DSTEAMNETWORKINGSOCKETS_STATIC_LINK \
  "$here/test/gns_trust_test.cpp" -o "$work/gns_trust_test-$arch.exe" \
  "$work/build-gns-$arch/src/libGameNetworkingSockets_s.a" -Wl,--start-group $libs -Wl,--end-group \
  -static -lws2_32 -lbcrypt -liphlpapi -lcrypt32 -lole32 -lshell32 -ladvapi32 -lwinmm -ldbghelp
