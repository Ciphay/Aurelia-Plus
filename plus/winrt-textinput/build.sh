#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build.sh <output folder> [win32]}
cc=x86_64-w64-mingw32-gcc
[[ "${2:-}" == win32 ]] && cc=i686-w64-mingw32-gcc
mkdir -p "$out"
"$cc" -shared -O2 -w -fno-strict-aliasing -I"$here/compat" -I"$here" -DWINVER=0x0A00 -D_WIN32_WINNT=0x0A00 \
  -o "$out/windows.ui.core.textinput.dll" "$here/main.c" "$here/vector.c" "$here/weakref.c" "$here/textinput.def" \
  -lole32 -luuid -lruntimeobject
