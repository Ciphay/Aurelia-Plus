#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build.sh <output folder>}
mkdir -p "$out"
x86_64-w64-mingw32-gcc -O2 -Wall -mwindows -municode -o "$out/Steam.exe" "$here/steamurl.c"
