#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
source "${AURELIA_PLUS_WORK:-$here/run}/record.env"
if [[ -n "${AURELIA_PROBE_COMPAT:-}" ]]; then mkdir -p "$AURELIA_PROBE_COMPAT"; export STEAM_COMPAT_DATA_PATH="$AURELIA_PROBE_COMPAT"; fi
args=("$@")
args[$((${#args[@]} - 1))]="$AURELIA_PLUS_PROBE_EXE"
exec "${args[@]}"
