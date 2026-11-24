#!/usr/bin/env bash
set -euo pipefail
arch=${1:?usage: gns_loop_run.sh <x86_64|i686> [folder]}
here=$(cd "$(dirname "$0")/.." && pwd)
work=${2:-/tmp/gns-loop-$arch}
rm -rf "$work"; mkdir -p "$work"
"$arch-w64-mingw32-g++" -O1 -Wall -I "$here/tests" -I "$here/run/gns/gns/include" -DSTEAMNETWORKINGSOCKETS_STATIC_LINK -static \
  "$here/tests/gns_loop_probe.cpp" -o "$work/probe.exe"
win() { local p=${1//\//\\}; printf 'Z:%s' "$p"; }
dll=$here/run/shimsteam/legacycompat/steamclient64.dll
[[ $arch == i686 ]] && dll=$here/run/shimsteam/legacycompat/steamclient.dll
export AURELIA_PLUS_PROBE_EXE="$work/probe.exe" AURELIA_SHIM_LOG="$work/shim.log"
export AURELIA_PLUS_GAME_ENV="PROBE_DLL=$(win "$dll") PROBE_OUT=$(win "$work/out.txt") ${GNS_LOOP_ENV:-}"
bash "$here/launch.sh" 500 > "$work/launch.log" 2>&1 || true
cat "$work/out.txt" 2>/dev/null || { echo "no output; launcher:"; tail -5 "$work/launch.log"; }
