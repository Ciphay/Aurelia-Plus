#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
appid=${1:-2357570}
work=${AURELIA_PLUS_WORK:-$here/run}
api=${AURELIA_PROBE_API_UNIX:-$HOME/.local/share/Steam/steamapps/common/Overwatch/steam_api64.dll}
mkdir -p "$work"
x86_64-w64-mingw32-gcc -Wall -o "$work/ticket_probe.exe" "$here/tests/ticket_probe.c"
rm -f /tmp/ticket-probe-shim.log
export AURELIA_PLUS_PROBE_EXE="Z:\\${work//\//\\}\\ticket_probe.exe"
export AURELIA_PROBE_API="Z:${api//\//\\}"
export AURELIA_PROBE_LOG='Z:\tmp\ticket-probe-shim.log'
bash "$here/launch.sh" "$appid" > "$work/shim-ticket-run.log" 2>&1 || true
cat /tmp/ticket-probe-shim.log 2>&1 | cut -c1-4000
