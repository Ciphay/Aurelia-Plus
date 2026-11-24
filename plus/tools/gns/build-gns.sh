#!/usr/bin/env bash
set -euo pipefail
arch=${1:?usage: build-gns.sh <x86_64|i686>}
here=$(cd "$(dirname "$0")" && pwd)
work=${GNS_WORK:-$here/../../run/gns}
flags="-I$here/include-alias -include $here/valve_root.h -DSTEAMNETWORKINGSOCKETS_STATIC_LINK"
"$here/patch-gns.sh" "$work/gns" >/dev/null
cmake -S "$work/gns" -B "$work/build-gns-$arch" -G Ninja -DCMAKE_TOOLCHAIN_FILE="$here/mingw-$arch.cmake" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_FLAGS="$flags" -DCMAKE_CXX_FLAGS="$flags" \
  -DBUILD_STATIC_LIB=ON -DBUILD_SHARED_LIB=OFF -DUSE_CRYPTO=BCrypt -DUSE_CRYPTO25519=Reference -DENABLE_ICE=ON -DUSE_STEAMWEBRTC=OFF \
  -DProtobuf_USE_STATIC_LIBS=ON -DCMAKE_PREFIX_PATH="$work/prefix-$arch" -DProtobuf_PROTOC_EXECUTABLE="${PROTOC:-/usr/bin/protoc}" -DCMAKE_INSTALL_PREFIX="$work/prefix-$arch"
cmake --build "$work/build-gns-$arch"
