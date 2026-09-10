# epaper host CLI

Python 3.11+ on macOS/POSIX; dependencies: pyserial, Pillow and pyte.

The supported install path is the Nix package, built for `aarch64-darwin`,
`aarch64-linux` and `x86_64-linux`:

```sh
nix run .#epaper -- --help
nix build .#epaper          # also runs the unit and integration tests
```

Otherwise `python -m pip install ./cli` in a virtual environment. From the
repository root, against the working tree:

```sh
nix develop -c bash -c 'cd cli && python -m epaper --help'
nix develop -c bash -c 'cd cli && python -m unittest -v'
```

```sh
epaper ping
epaper status
epaper draw photo.png --fit fit --dither
epaper draw --testcard
epaper text 'hello'
printf 'hello\r\nworld' | epaper console --stdin
epaper console --echo -- /bin/zsh
epaper refresh --full
epaper clear
epaper sleep
epaper bootsel
```

Global `--port PORT` or `EPAPER_PORT` selects a device. An explicitly empty port
is rejected. Automatic discovery is only used when neither is specified; it
matches the `epaper` USB product string, then falls back to `/dev/cu.usbmodem*`
on macOS and `/dev/ttyACM*` elsewhere.
`text` and each `console` invocation create a fresh screen. Strings are literal;
use shell quoting or printf for control bytes. The fixed 100x30 TERM=linux PTY
feeds pyte on the host. Spleen ASCII glyphs, reverse attributes and a reverse
cursor are rasterised to 1-bit pixels; unsupported characters become `?`.
The host diffs frames and coalesces BLITs at 60 ms output idle or 250 ms pending;
cursor-only updates debounce 200 ms. STATUS busy defers all transfers. Each
session first sends its whole shadow with a partial refresh (no readback exists).
Full refreshes wait for 2 s idle with at least 60 partial units, or 30 s idle
with at least 10 units. ED 2 / reset retains a cleanup request until 2 s idle
with at least 10 units; final flush cleans up at 30 units. Band/half/whole-screen
partials cost 1/2/3 units. See docs/protocol.md for the refresh policy.

`--fit fit` letterboxes, `fill` crops, `stretch` resizes; rotation is
counterclockwise. Images default to threshold 128; `--dither` selects
Floyd–Steinberg. Transparent pixels composite on white.

See the repository README for simulator use and known limitations. Integration
tests launch tools/epdsim.py with a private PTY and temporary PNG output.
Regenerate the embedded font with `cli/tools/gen_font.py INPUT.psfu OUTPUT.py`
using Spleen's `share/consolefonts/spleen-8x16.psfu` from nixpkgs#spleen.
