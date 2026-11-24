#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=${GNS_WORK:-$here/../../run/gns}
if [[ -x "$work/host-protoc/protoc" ]]; then exit 0; fi
cmake -S "$work/protobuf" -B "$work/build-protoc-host" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -Dprotobuf_BUILD_TESTS=OFF -Dprotobuf_BUILD_PROTOC_BINARIES=ON -Dprotobuf_BUILD_SHARED_LIBS=OFF \
  -Dprotobuf_FORCE_FETCH_DEPENDENCIES=ON -Dprotobuf_WITH_ZLIB=OFF
cmake --build "$work/build-protoc-host" --target protoc
mkdir -p "$work/host-protoc"
cp "$work/build-protoc-host/protoc" "$work/host-protoc/protoc"
