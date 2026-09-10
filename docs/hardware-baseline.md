# Hardware baseline (verified 2026-09-10 by the orchestrator)

Facts established by driving the real panel and confirming with the OBS webcam
capture (`/tmp/epaper_probe.jpg`). Treat as authoritative over any doc.

## Board state

- Pico 2 (RP2350) enumerates as `/dev/cu.usbmodem1101`, USB vendor
  "MicroPython", product "Board in FS mode". Runs MicroPython 1.27.0
  (`RPI_PICO2`), with a Waveshare demo `main.py` on the filesystem.
- `nix shell nixpkgs#mpremote -c mpremote connect /dev/cu.usbmodem1101 ...`
  works for `exec`, `run`, `cat`, `fs`.
- To flash pico-sdk firmware, put the board into BOOTSEL. From MicroPython:
  `mpremote exec "import machine; machine.bootloader()"` reboots into the
  RP2350 USB bootloader (mounts as `RP2350` volume; picotool can then `load`).
  Once our firmware runs, picotool `reboot -f -u` is expected to work if the
  firmware includes the USB stdio/picotool reset interface.

## Wiring (Pico GPIO -> e-Paper HAT), from the on-board main.py

| Signal | GPIO |
|--------|------|
| RST    | 9    |
| DC     | 8    |
| CS     | 5    |
| BUSY   | 10   |
| SCK    | 6 (SPI0 SCK)  |
| MOSI/DIN | 7 (SPI0 TX) |
| MISO   | 4 (unused by panel) |

SPI0, mode 0, 4 MHz verified working. BUSY: 1 = idle, 0 = busy (pull-up).

## Panel driver sequence verified working (800x480, 7.5" V2)

```
reset: RST=1 20ms, RST=0 2ms, RST=1 20ms
0x01: 07 07 3F 3F        power setting
0x06: 17 17 28 17        booster soft start
0x04, wait 100ms, busy   power on
0x00: 1F                 panel setting (KW, LUT from OTP)
0x61: 03 20 01 E0        resolution 800x480
0x15: 00
0x50: 10 07              VCOM / data interval
0x60: 22                 TCON
image: 0x10 + 48000 bytes (old, all 0x00), 0x13 + 48000 bytes (new, INVERTED)
0x12, wait 100ms, busy   refresh  -> busy released after ~3.9 s
sleep: 0x02 busy; 0x07 A5
```

Busy polling: send 0x71 then read BUSY, repeat every 10 ms.

Framebuffer: MicroPython `MONO_HLSB` (row-major, 100 bytes/row, MSB = leftmost
pixel) where 1 = white. The bytes sent after 0x13 were the bitwise inverse of
that buffer, i.e. on the wire for register 0x13: bit 1 = BLACK, bit 0 = white,
MSB first, top-left pixel first. Register 0x10 was filled with 0x00.

The capture confirmed correct orientation: "TOP-LEFT" text at the top-left
corner of the panel as seen from the camera; "BOTTOM-RIGHT" at the bottom
right; no mirroring. Full refresh took ~3.9 s.

Probe script: `/tmp/ws/probe75.py` (MicroPython). Upstream C driver copied to
`/tmp/ws/RaspberryPi_JetsonNano_c_lib_e-Paper_EPD_7in5_V2.c`; spec text at
`/tmp/ws/spec.txt`.

## Nix attrs verified on aarch64-darwin

pico-sdk 2.2.0 (`pico-sdk.override { withSubmodules = true; }` ok),
picotool 2.2.0-a4, gcc-arm-embedded 15.2.rel1, cmake 4.1.6, python3 3.13,
mpremote 1.25.0.
