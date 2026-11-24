#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
appid=${1:-2357570}
api=${AURELIA_PROBE_API_UNIX:-$HOME/.local/share/Steam/steamapps/common/Overwatch/steam_api64.dll}
work=${AURELIA_PLUS_WORK:-$here/run}
mkdir -p "$work"
x86_64-w64-mingw32-gcc -Wall -o "$work/steam_init_probe.exe" "$here/tests/steam_init_probe.c"
rm -f /tmp/steam-init-probe.log /tmp/aurelia-shim.log
export AURELIA_PLUS_PROBE_EXE="Z:\\${work//\//\\}\\steam_init_probe.exe"
export AURELIA_PROBE_API="Z:${api//\//\\}"
export AURELIA_PROBE_LOG='Z:\tmp\steam-init-probe.log'
bash "$here/launch.sh" "$appid" > "$work/probe-run.log" 2>&1 || true
echo "--- probe"; cat /tmp/steam-init-probe.log 2>&1 || echo "(no probe log)"
echo "--- shim"; cat /tmp/aurelia-shim.log 2>&1 | tail -40 || echo "(no shim log)"
grep -q '^DONE$' /tmp/steam-init-probe.log
