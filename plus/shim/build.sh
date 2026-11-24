#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build.sh <output folder> [win64|win32]}
arch=${2:-win64}
mkdir -p "$out"
case "$arch" in
  win64)
    cc=x86_64-w64-mingw32-gcc; dll=steamclient64.dll; gen="$here/generated"; def="$here/exports.def" ;;
  win32)
    cc=i686-w64-mingw32-gcc; dll=steamclient.dll; gen="$out/generated32"; def="$here/exports.def"
    (cd "$here/.." && python3 -m tools.gen.emit_shim --catalog catalog/catalog.json --abi win32 --out "$gen" \
       && python3 -m tools.policy.gen_policy --policy catalog/policy.json --out "$gen/policy.c") ;;
  *) echo "unknown architecture: $arch" >&2; exit 2 ;;
esac
case "$arch" in win64) gnsarch=x86_64; cxx=x86_64-w64-mingw32-g++ ;; win32) gnsarch=i686; cxx=i686-w64-mingw32-g++ ;; esac
gnswork=${GNS_WORK:-$here/../run/gns}
gnslib="$gnswork/build-gns-$gnsarch/src/libGameNetworkingSockets_s.a"
if [[ -z "${SHIM_NO_GNS:-}" && -f "$gnslib" ]]; then
  objs="$out/obj"; mkdir -p "$objs"
  for f in "$here"/*.c "$here"/ifaces/*.c "$gen/ifaces.c" "$gen/policy.c"; do
    "$cc" -c -O2 -Wall -Wextra -Wno-unused-parameter -DSHIM_WITH_GNS -I"$here" -I"$gen" "$f" -o "$objs/$(basename "$(dirname "$f")")_$(basename "$f").o" &
    while (( $(jobs -r | wc -l) >= 8 )); do sleep 0.1; done
  done
  wait
  "$cxx" -c -std=c++17 -O2 -DSHIM_WITH_GNS -I"$here" -I"$gen" -I"$gnswork/gns/include" -I"$here/../tools/gns/include-alias" -DSTEAMNETWORKINGSOCKETS_STATIC_LINK "$here/gns_glue.cpp" -o "$objs/gns_glue.o"
  libs=$(ls "$gnswork"/prefix-$gnsarch/lib/lib*.a | grep -v lite)
  sl="$out/staticlibs"; mkdir -p "$sl"
  cp "/usr/$gnsarch-w64-mingw32/lib/libpthread.a" "/usr/$gnsarch-w64-mingw32/lib/libwinpthread.a" "$sl/"
  "$cxx" -shared -O2 "$objs"/*.o "$def" \
    "$gnslib" -Wl,--start-group $libs -Wl,--end-group \
    -lws2_32 -lwinhttp -lbcrypt -lshell32 -liphlpapi -lcrypt32 -lole32 -ladvapi32 -lwinmm -ldbghelp -lntdll \
    -L"$sl" -Wl,--exclude-libs,ALL -s -static-libgcc -static-libstdc++ -o "$out/$dll"
else
"$cc" -shared -O2 -Wall -Wextra -Wno-unused-parameter \
  -I"$here" -I"$gen" \
  "$here"/*.c "$here"/ifaces/*.c "$gen/ifaces.c" "$gen/policy.c" "$def" \
  -lws2_32 -lwinhttp -lbcrypt -lshell32 -liphlpapi -static-libgcc -o "$out/$dll"
fi
