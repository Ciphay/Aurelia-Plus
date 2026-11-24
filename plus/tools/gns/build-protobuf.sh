#!/usr/bin/env bash
set -euo pipefail
arch=${1:?usage: build-protobuf.sh <x86_64|i686>}
here=$(cd "$(dirname "$0")" && pwd)
work=${GNS_WORK:-$here/../../run/gns}
src=$work/protobuf
prefix=$work/prefix-$arch
cmake -S "$src" -B "$work/build-protobuf-$arch" -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE="$here/mingw-$arch.cmake" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" \
  -Dprotobuf_BUILD_TESTS=OFF -Dprotobuf_BUILD_PROTOC_BINARIES=OFF -Dprotobuf_BUILD_SHARED_LIBS=OFF \
  -Dprotobuf_FORCE_FETCH_DEPENDENCIES=ON -Dprotobuf_INSTALL=ON -Dprotobuf_WITH_ZLIB=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON
cmake --build "$work/build-protobuf-$arch"
cmake --install "$work/build-protobuf-$arch"
