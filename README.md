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

The examples below cover the command set defined by the v1 protocol. Exact
options and executable installation are unverified until the CLI implementation
lands.

```sh
epaper ping                         # Verify the framed connection
epaper info                         # Firmware, panel, dimensions, and mode
epaper clear white                  # Full refresh to white (or: black)
epaper draw picture.png             # Dither/pack and display an image
epaper mode picture                 # Select picture mode
epaper mode console                 # Select console mode
epaper write 'hello, e-paper'       # Write terminal bytes
printf 'hello\n' | epaper write -  # Write bytes from standard input
epaper console                      # Run an interactive terminal through a PTY
epaper sleep                        # Put the panel into deep sleep
epaper bootsel                      # Reboot the Pico into its USB bootloader
```

Any command after `sleep` wakes the panel according to the protocol.
`draw` sends an 800×480, packed 1-bit image; host-side scaling/dithering options
are unverified.

## Host-side simulator

`tools/epdsim.py` is a reference implementation of the device side. It creates
a pseudo-terminal, prints its slave path, accepts v1 protocol frames, and
writes simulated display output to `/tmp/epdsim.png` after image and console
refreshes.

```sh
nix-shell -p python3Packages.pyserial python3Packages.pillow \
  --run 'python tools/epdsim.py'
```

In another shell, point the CLI at the printed PTY (the CLI port flag is
unverified). Run simulator tests with:

```sh
nix-shell -p python3Packages.pyserial python3Packages.pillow \
  --run 'python tools/test_epdsim.py'
```

## Protocol

Frames begin with `EB 90`, carry a command, sequence number, little-endian
payload length, and CRC-16/CCITT-FALSE. Commands cover ping/info, mode and clear,
chunked image transfer, console bytes, sleep, and BOOTSEL. Responses echo the
sequence number as ACK, NAK, or BUSY. The normative definition, including
console escape handling and refresh policy, is
[`docs/protocol.md`](docs/protocol.md).

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

- Physical-panel validation of the pico-sdk firmware and CLI is pending.
- Console support is deliberately a small VT subset; unsupported CSI commands
  are consumed rather than rendered.
- The terminal uses printable ASCII, not Unicode.
- E-paper refresh is inherently delayed, partial updates accumulate ghosting,
  and the firmware must periodically perform a full refresh.
- `CONSOLE_RESIZE` is reserved; the console remains fixed at 100×30.
