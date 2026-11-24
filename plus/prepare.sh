#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=${1:-${AURELIA_PLUS_WORK:-$here/run}}
umask 077
mkdir -p "$work/shimsteam/legacycompat"
chmod 700 "$work"
src_hash=$( (cd "$here" && find -L shim winrt-textinput steamurl tools/gen tools/policy catalog -type f \( -name '*.c' -o -name '*.h' -o -name '*.py' -o -name '*.json' -o -name '*.def' -o -name '*.sh' \) -not -path '*/__pycache__/*' | LC_ALL=C sort | xargs cat) | sha256sum | cut -c1-24)
if [[ "$(cat "$work/build/.src-hash" 2> /dev/null || true)" != "$src_hash" || ! -f "$work/build/steamclient64.dll" || ! -f "$work/build32/steamclient.dll" || ! -f "$work/build/windows.ui.core.textinput.dll" || ! -f "$work/build32/windows.ui.core.textinput.dll" || ! -f "$work/build/Steam.exe" ]]; then
  bash "$here/shim/build.sh" "$work/build" > /dev/null &
  build64=$!
  bash "$here/shim/build.sh" "$work/build32" win32 > /dev/null &
  build32=$!
  bash "$here/winrt-textinput/build.sh" "$work/build" > /dev/null
  bash "$here/winrt-textinput/build.sh" "$work/build32" win32 > /dev/null 2>&1
  bash "$here/steamurl/build.sh" "$work/build" > /dev/null
  wait "$build64"
  wait "$build32"
  printf '%s' "$src_hash" > "$work/build/.src-hash"
fi
cmp -s "$work/build/steamclient64.dll" "$work/shimsteam/legacycompat/steamclient64.dll" || cp "$work/build/steamclient64.dll" "$work/shimsteam/legacycompat/steamclient64.dll"
cmp -s "$work/build32/steamclient.dll" "$work/shimsteam/legacycompat/steamclient.dll" || cp "$work/build32/steamclient.dll" "$work/shimsteam/legacycompat/steamclient.dll"
cmp -s "$work/build/windows.ui.core.textinput.dll" "$work/shimsteam/legacycompat/windows.ui.core.textinput.dll" || cp "$work/build/windows.ui.core.textinput.dll" "$work/shimsteam/legacycompat/windows.ui.core.textinput.dll"
cmp -s "$work/build32/windows.ui.core.textinput.dll" "$work/shimsteam/legacycompat/windows.ui.core.textinput32.dll" || cp "$work/build32/windows.ui.core.textinput.dll" "$work/shimsteam/legacycompat/windows.ui.core.textinput32.dll"
cmp -s "$work/build/Steam.exe" "$work/shimsteam/legacycompat/Steam.exe" || cp "$work/build/Steam.exe" "$work/shimsteam/legacycompat/Steam.exe"
