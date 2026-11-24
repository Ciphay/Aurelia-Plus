#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
appid=${1:-2357570}
work=${AURELIA_PLUS_WORK:-$here/run}
api=${AURELIA_PROBE_API_UNIX:-$HOME/.local/share/Steam/steamapps/common/Overwatch/steam_api64.dll}
aurelia=${AURELIA_BIN:-$here/../aurelia/target/debug/aurelia}
mkdir -p "$work"
x86_64-w64-mingw32-gcc -Wall -o "$work/ticket_probe.exe" "$here/tests/ticket_probe.c"
rm -f /tmp/ticket-probe-real.log
umask 077
{
  printf 'export AURELIA_PLUS_PROBE_EXE=%q\n' "Z:\\${work//\//\\}\\ticket_probe.exe"
  printf 'export AURELIA_PROBE_API=%q\n' "Z:${api//\//\\}"
  printf 'export AURELIA_PROBE_LOG=%q\n' 'Z:\tmp\ticket-probe-real.log'
  [[ -n "${AURELIA_PROBE_COMPAT:-}" ]] && printf 'export AURELIA_PROBE_COMPAT=%q\n' "$AURELIA_PROBE_COMPAT"
  [[ -n "${AURELIA_PROBE_HOLD:-}" ]] && printf 'export AURELIA_PROBE_HOLD=%q\n' "$AURELIA_PROBE_HOLD"
} > "$work/record.env"
chmod +x "$here/tests/record_wrapper.sh"
"$aurelia" play "$appid" -s -x "$here/tests/record_wrapper.sh" -N -k > "$work/record-run.log" 2>&1 || true
cat /tmp/ticket-probe-real.log 2>&1 | cut -c1-4000
