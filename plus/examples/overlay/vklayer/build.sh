#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/build}
inc=${VULKAN_INCLUDE:-}
if [[ -z "$inc" ]]; then
  for c in /usr/include /var/lib/flatpak/runtime/org.freedesktop.Sdk/x86_64/*/*/files/include; do [[ -f "$c/vulkan/vk_layer.h" ]] && inc=$c && break; done
fi
[[ -n "$inc" ]] || { echo "Vulkan headers not found (install vulkan-headers or set VULKAN_INCLUDE)" >&2; exit 1; }
mkdir -p "$out"
gcc -shared -fPIC -O2 -Wall -Wextra -Wno-unused-parameter -Wno-missing-field-initializers -I"$inc" "$here/aurelia_overlay_layer.c" -o "$out/libaurelia_overlay_layer.so" -lpthread
cp "$here/VK_LAYER_AURELIA_overlay.json" "$out/"
echo "$out"
