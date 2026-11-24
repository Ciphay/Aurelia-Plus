#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
unset AURELIA_PLUS_PROBE_EXE AURELIA_PROBE_API AURELIA_PROBE_LOG AURELIA_SHIM_TRACE AURELIA_SHIM_TICKET_FILE AURELIA_SHIM_LOG AURELIA_PLUS_WINEDEBUG AURELIA_PLUS_GAME_ARGS AURELIA_PLUS_GAME_ENV
env_file="${AURELIA_PLUS_WORK:-$here/run}/wrapper-${SteamAppId:-}.env"
[[ -f "$env_file" ]] || env_file="${AURELIA_PLUS_WORK:-$here/run}/wrapper.env"
source "$env_file"
: "${AURELIA_PLUS_STEAM_DIR:?fake Steam folder missing}"
[[ -n "${AURELIA_PLUS_WINEDEBUG:-}" ]] && export WINEDEBUG="$AURELIA_PLUS_WINEDEBUG"
if [[ -n "${AURELIA_PLUS_GAME_ENV:-}" ]]; then
  read -r -a extra_env <<< "$AURELIA_PLUS_GAME_ENV"
  for kv in "${extra_env[@]}"; do [[ "$kv" == *=* ]] && export "$kv"; done
fi
export STEAM_COMPAT_CLIENT_INSTALL_PATH="$AURELIA_PLUS_STEAM_DIR"
export WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}lsteamclient=d"
export AURELIA_SHIM_LOG="${AURELIA_SHIM_LOG:-Z:\\tmp\\aurelia-shim.log}"
if [[ -n "${AURELIA_PLUS_PROBE_EXE:-}" ]]; then
  args=("$@")
  args[$((${#args[@]} - 1))]="$AURELIA_PLUS_PROBE_EXE"
  exec "${args[@]}"
fi
if [[ -n "${AURELIA_PLUS_GAME_ARGS:-}" ]]; then
  read -r -a extra <<< "$AURELIA_PLUS_GAME_ARGS"
  exec "$@" "${extra[@]}"
fi
exec "$@"
