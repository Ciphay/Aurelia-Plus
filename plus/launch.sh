#!/usr/bin/env bash
set -euo pipefail
appid=${1:?usage: launch.sh <appid> [aurelia play options]}
shift
here=$(cd "$(dirname "$0")" && pwd)
work=${AURELIA_PLUS_WORK:-$here/run}
aurelia=${AURELIA_BIN:-$here/../aurelia/target/debug/aurelia}

umask 077
bash "$here/prepare.sh" "$work"

: > "$work/bridge.out"
overlay_in=/dev/null
if [[ "${AURELIA_PLUS_OVERLAY_CAPABLE:-}" == 1 || "${AURELIA_PLUS_OVERLAY:-}" == 1 || "${AURELIA_PLUS_OVERLAY:-}" == ingame ]]; then
  overlay_in="$work/overlay-in.fifo"
  rm -f "$overlay_in"
  mkfifo "$overlay_in"
  exec 9<> "$overlay_in"
  printf '{"cmd":"capable","on":true}\n' >&9
fi
"$aurelia" --json bridge serve --app "$appid" < "$overlay_in" > "$work/bridge.out" 2> "$work/bridge.err" &
bridge=$!
overlay_pid=
trap 'kill "$bridge" 2>/dev/null || true; [[ -n "$overlay_pid" ]] && kill "$overlay_pid" 2>/dev/null; exec 9>&- 2>/dev/null || true; rm -f "$work/bridge.out" "${AURELIA_OVERLAY_SHM:-}"' EXIT
for _ in $(seq 1 60); do [[ -s "$work/bridge.out" ]] && break; sleep 1; done
if [[ ! -s "$work/bridge.out" ]]; then echo "the bridge did not start:" >&2; cat "$work/bridge.err" >&2; exit 1; fi
bridge_field() { python3 -I -c 'import json,sys; print(json.loads(open(sys.argv[1]).readline())[sys.argv[2]])' "$work/bridge.out" "$1" 2> /dev/null; }
if ! port=$(bridge_field port) || ! token=$(bridge_field token); then
  echo "the bridge failed: $(head -c 400 "$work/bridge.out")" >&2
  exit 1
fi
export AURELIA_BRIDGE_PORT=$port AURELIA_BRIDGE_TOKEN=$token
export SteamOverlayGameId=$appid SteamClientLaunch=1 SteamEnv=1
if [[ -z "${AURELIA_SHIM_LANGUAGE:-}" ]]; then
  case "${LANG:-en}" in pt_BR*) AURELIA_SHIM_LANGUAGE=brazilian ;; pt*) AURELIA_SHIM_LANGUAGE=portuguese ;; de*) AURELIA_SHIM_LANGUAGE=german ;; fr*) AURELIA_SHIM_LANGUAGE=french ;; es_ES*) AURELIA_SHIM_LANGUAGE=spanish ;; es*) AURELIA_SHIM_LANGUAGE=latam ;; it*) AURELIA_SHIM_LANGUAGE=italian ;; ru*) AURELIA_SHIM_LANGUAGE=russian ;; pl*) AURELIA_SHIM_LANGUAGE=polish ;; ja*) AURELIA_SHIM_LANGUAGE=japanese ;; *) AURELIA_SHIM_LANGUAGE=english ;; esac
fi
export AURELIA_SHIM_LANGUAGE
if [[ "${AURELIA_PLUS_OVERLAY:-}" == ingame ]]; then
  bash "$here/examples/overlay/vklayer/build.sh" "$work/vklayer" > /dev/null
  export AURELIA_OVERLAY_SHM=/dev/shm/aurelia-overlay-$$
  : > "$AURELIA_OVERLAY_SHM"; truncate -s 16M "$AURELIA_OVERLAY_SHM"
  export VK_LAYER_PATH="$work/vklayer" VK_INSTANCE_LAYERS=VK_LAYER_AURELIA_overlay VK_LOADER_LAYERS_ENABLE='*AURELIA*'
fi
if [[ -n "${AURELIA_PLUS_OVERLAY:-}" ]]; then
  env -u VK_INSTANCE_LAYERS -u VK_LAYER_PATH -u VK_LOADER_LAYERS_ENABLE OVERLAY_INGAME=$([[ "$AURELIA_PLUS_OVERLAY" == ingame ]] && echo 1 || echo 0) "${AURELIA_PLUS_ELECTRON:-/usr/lib/electron42/electron}" --ozone-platform=x11 --disable-gpu "$here/examples/overlay" --events "$work/bridge.out" --commands "$overlay_in" > "$work/overlay.log" 2>&1 &
  overlay_pid=$!
fi
umask 077
{
  for v in AURELIA_BRIDGE_PORT AURELIA_BRIDGE_TOKEN AURELIA_PLUS_PROBE_EXE AURELIA_PROBE_API AURELIA_PROBE_LOG AURELIA_SHIM_LOG AURELIA_SHIM_TICKET_FILE AURELIA_SHIM_TRACE AURELIA_SHIM_OPEN_CHECKOUT AURELIA_SHIM_GNS AURELIA_SHIM_NO_BROWSER SteamOverlayGameId SteamClientLaunch SteamEnv AURELIA_SHIM_LANGUAGE AURELIA_OVERLAY_SHM VK_LAYER_PATH VK_INSTANCE_LAYERS VK_LOADER_LAYERS_ENABLE AURELIA_PLUS_WINEDEBUG AURELIA_PLUS_GAME_ARGS AURELIA_PLUS_GAME_ENV; do
    [[ -n "${!v:-}" ]] && printf 'export %s=%q\n' "$v" "${!v}"
  done
  printf 'export AURELIA_PLUS_STEAM_DIR=%q\n' "$work/shimsteam"
} > "$work/wrapper.env"

"$aurelia" play "$appid" -x "$here/launch-wrapper.sh" -N -k "$@"
