# epaper host CLI

Python 3.11+ on macOS/POSIX; dependencies: pyserial, Pillow and pyte.
Install `python -m pip install ./cli` in a virtual environment, or from the
repository root:

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
is rejected. Automatic discovery is only used when neither is specified.
`text` and each `console` invocation create a fresh screen. Strings are literal;
use shell quoting or printf for control bytes. The fixed 100x30 TERM=linux PTY
feeds pyte on the host. Spleen ASCII glyphs, reverse attributes and a reverse
cursor are rasterised to 1-bit pixels; unsupported characters become `?`.
The host diffs frames, coalesces BLITs and applies the protocol's refresh budget.

`--fit fit` letterboxes, `fill` crops, `stretch` resizes; rotation is
counterclockwise. Images default to threshold 128; `--dither` selects
Floyd–Steinberg. Transparent pixels composite on white.

See the repository README for simulator use and known limitations. Integration
tests launch tools/epdsim.py with a private PTY and temporary PNG output.
Regenerate the embedded font with `cli/tools/gen_font.py INPUT.psfu OUTPUT.py`
using Spleen's `share/consolefonts/spleen-8x16.psfu` from nixpkgs#spleen.
