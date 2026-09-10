#!/usr/bin/env bash
# Configure and build the Pico 2 firmware into firmware/build.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
build="$here/build"

if [ -z "${PICO_SDK_PATH:-}" ]; then
  echo "PICO_SDK_PATH is unset; run inside 'nix develop'." >&2
  exit 1
fi

cmake -G Ninja -S "$here" -B "$build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DPICO_BOARD=pico2 \
  -DPICO_PLATFORM=rp2350-arm-s

cmake --build "$build"

echo
echo "UF2: $build/epaper_fw.uf2"
