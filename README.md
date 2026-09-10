# Pico 2 e-paper display

Firmware and a macOS CLI for a Raspberry Pi Pico 2 driving a Waveshare 7.5-inch
e-paper V2 (800×480). The device supports complete 1-bit pictures and a
100×30 serial-console display. Host and device communicate over framed USB CDC.

The pico-sdk firmware and CLI are under active development; unless explicitly
noted below, their behavior is currently **unverified** on the physical panel.
The panel wiring and full-refresh sequence have been verified independently.

## Wiring

| E-paper signal | Pico 2 GPIO | Notes |
|---|---:|---|
| RST | GP9 | Active low |
| DC | GP8 | Data high, command low |
| CS | GP5 | Active low |
| BUSY | GP10 | Low while busy, pulled up |
| CLK | GP6 | SPI0 SCK |
| DIN | GP7 | SPI0 TX/MOSI |
| MISO | GP4 | Assigned by the existing setup; unused by the panel |
| VCC | 3V3 | 3.3 V |
| GND | GND | Common ground |

SPI0 mode 0 at 4 MHz is verified on this wiring. See
[`docs/hardware-baseline.md`](docs/hardware-baseline.md) for the hardware test.

## Development environment

```sh
nix develop
```

The development shell supplies the Pico SDK, ARM compiler, CMake, picotool,
Python dependencies, and webcam-capture tools (unverified until its package
changes land).

## Build and flash

Build the Pico 2 firmware (unverified):

```sh
firmware/build.sh
```

To enter BOOTSEL while the currently installed MicroPython firmware is running:

```sh
mpremote connect /dev/cu.usbmodem1101 exec 'import machine; machine.bootloader()'
```

After this project's firmware is installed, the CLI can request BOOTSEL
(unverified):

```sh
epaper bootsel
```

Load the resulting image (unverified):

```sh
picotool load -f firmware/build/epaper_fw.uf2
```

USB serial device names can change. Supply the CLI's port option when automatic
discovery does not select the Pico (consult `epaper --help`; unverified).

## CLI

The host owns terminal emulation (pyte), ASCII bitmap rasterisation and refresh
batching. The firmware receives only pixels. Install with `python -m pip install
./cli` in a virtual environment, or run directly from source:

```sh
nix develop -c bash -c 'cd cli && python -m epaper --help'
```

Installed command examples:

```sh
epaper ping
epaper info
epaper status
epaper clear                       # white; --black selects black
epaper draw picture.png --dither
epaper draw --testcard
epaper text 'hello, e-paper'
printf 'hello\r\nworld' | epaper console --stdin
epaper console --echo -- /bin/zsh
epaper refresh --full
epaper sleep
epaper bootsel
```

Global `--port PORT` (or `EPAPER_PORT`) selects the serial device. `text` starts
a new 100x30 screen; shell quoting controls literal escapes. For escape bytes
use `printf` piped to `console --stdin`. `console` defaults to `$SHELL`, sets
`TERM=linux`, and keeps the PTY fixed at 100x30. Keyboard input stays on the Mac;
exit the child shell to finish. `--echo` mirrors output locally.

The host flushes after 100 ms idle or 400 ms pending output, with a 300 ms
cursor-motion debounce. Byte differences become bounded BLIT rectangles. The
host polls STATUS while the panel is busy and accumulates output rather than
redrawing. Scrolling uses partial refresh; an area-weighted ghosting budget
periodically requests full refresh. A new console session uploads a complete
frame to establish its shadow state. Only one client may drive the device.

## Host-side simulator and tests

```sh
nix develop -c python tools/epdsim.py
# Prints a PTY: supply that exact nonempty path to epaper --port.
nix develop -c bash -c 'cd cli && python -m unittest -v'
nix develop -c bash -c 'cd tools && python -m unittest -v'
```

The simulator validates BLIT bounds, tracks the dirty bounding box, simulates
500 ms partial / 4 s full BUSY periods, and writes `/tmp/epdsim.png` after
refresh completion. `--output /tmp/other.png` selects a different output.
Integration tests use temporary output files and a private PTY, never automatic
serial discovery.

## Protocol

Frames begin with `EB 90`, carry a command, sequence number, little-endian
payload length, and CRC-16/CCITT-FALSE. BLIT uses byte-addressed x coordinates
and pixel-addressed y coordinates. STATUS reports refresh state; REFRESH uses
the union of pending BLITs. Image transfer, clear, sleep and BOOTSEL remain
available. See [`docs/protocol.md`](docs/protocol.md) for the exact contract.

## Verification with the webcam

OBS must be open with the `OsmoPocket3` source active and **Start Virtual
Camera** enabled. Always resolve the OBS virtual-camera index by name; AVFoundation
indices are not stable. The virtual camera accepts 60 fps, and approximately 30
warmup frames must be discarded for exposure to settle.

```sh
nix develop

idx="$(ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 \
  | sed -n '/video devices/,/audio devices/p' \
  | sed -n 's/^.*\[\([0-9][0-9]*\)\] OBS Virtual Camera$/\1/p' | head -n1)"

timeout --signal=KILL 10 ffmpeg -hide_banner -loglevel error \
  -f avfoundation -pixel_format uyvy422 -video_size 1920x1080 -framerate 60 \
  -i "$idx" -frames:v 30 -update 1 -q:v 2 -y /tmp/epaper.jpg
```

Captures belong in `/tmp`, never in the repository. Do not capture the DJI
directly: OBS owns its stream negotiation, and direct AVFoundation capture is
known to fail or hang. Keep the hard KILL timeout because a blocked frame wait
can ignore SIGTERM.

## Known limitations

- The framebuffer protocol and host-rendered console still need physical-panel
  verification; simulator results do not establish waveform quality.
- Rasterisation uses Spleen ASCII 8x16 plus synthesised box-drawing glyphs;
  other Unicode becomes `?`.
- pyte is a VT emulator, not a complete modern terminal; TERM is intentionally
  linux (no padding, no alternate screen). Reverse-video attributes are supported; color and styled glyphs are not.
- Partial refresh accumulates ghosting, and periodic full refresh visibly flashes.
- The console remains fixed at 100x30 and does not resize with the Mac terminal.
