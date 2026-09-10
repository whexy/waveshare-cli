#!/usr/bin/env bash
# Build and run the host-side console/protocol tests with the native compiler.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

cc -std=c11 -Wall -Wextra -Werror -O1 \
  -I "$here/../src" \
  "$here/test_console.c" "$here/../src/console.c" "$here/../src/protocol.c" \
  -o "$out/test_console"

"$out/test_console"
