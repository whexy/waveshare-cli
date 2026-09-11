# Host <-> Pico protocol (v2)

v2 makes the device a dumb framebuffer: the host owns the terminal model
(pyte) and rasterisation; the device only stores pixels and drives refreshes.
v1 console opcodes (SET_MODE 0x03, CONSOLE_WRITE 0x20, CONSOLE_RESIZE 0x21)
are reserved and answered with NAK 4.

Transport: USB CDC (TinyUSB CDC, not pico stdio), 8-bit clean, no CRLF
translation. Both directions use the same frame format.

## Frame

```
offset  size  field
0       2     magic   0xEB 0x90
2       1     type    command / response code
3       1     seq     host-chosen sequence number, echoed in the response
4       2     len     payload length, little-endian, 0..4096
6       len   payload
6+len   2     crc16   CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) over
                      bytes [2, 6+len) i.e. type, seq, len, payload; LE
```

Receiver resyncs by scanning for the magic bytes after any framing/CRC error.
Every host command gets exactly one response frame with the same `seq`.

## Commands (host -> device)

| type | name           | payload                                         |
|------|----------------|-------------------------------------------------|
| 0x01 | PING           | none                                            |
| 0x02 | INFO           | none                                            |
| 0x04 | CLEAR          | u8 color: 0 = white, 1 = black (full refresh)   |
| 0x05 | STATUS         | none                                            |
| 0x10 | IMG_BEGIN      | u16 width (800), u16 height (480), u8 fmt (0 = 1bpp packed, MSB first, 1 = black; 1 = 2bpp 4-gray) |
| 0x11 | IMG_DATA       | u32 offset, then up to 4090 bytes of image data |
| 0x12 | IMG_END        | u8 refresh: 0 = full, 1 = partial (whole frame); NAK 3 for partial in 4-gray |
| 0x13 | BLIT           | u16 x_byte, u16 y, u16 w_bytes, u16 h, then w_bytes*h bits (row-major, MSB = leftmost pixel, 1 = black) |
| 0x14 | REFRESH        | u8 mode: 0 = full, 1 = partial (dirty bbox)     |
| 0x30 | SLEEP          | none (panel deep sleep; any later command wakes)|
| 0x31 | RESET_BOOTSEL  | none (reboot into USB bootloader)               |

The framebuffer is exactly 800*480/8 = 48000 bytes, 100 bytes per row.
IMG_DATA writes into it at `offset`; IMG_END pushes the whole frame to the
panel.

## 4-gray (fmt 1)

The panel holds four tones. `IMG_BEGIN` with fmt 1 selects a separate 2bpp
buffer of 800*480/4 = 96000 bytes, 200 bytes per row, which IMG_DATA offsets
are bounded by instead. Two bits per pixel carry *darkness* — 0 = white,
1 = light, 2 = dark, 3 = black — leftmost pixel in the high bits, extending
1bpp's "one is black". The device splits those bits across the panel's two RAM
planes: 0x10 takes the low bit and 0x13 the high bit, both uninverted (the
inversion 1bpp needs cancels against this encoding; verified against
Waveshare's `display_4Gray` for all four tones).

Gray is whole-panel only, about 2.6 s. It spends both RAM planes on its two
bits, so no plane is left as a diff base and there is no fast partial: IMG_END
with refresh = 1 is answered NAK 3. For the same reason a gray refresh leaves
the device's old plane holding gray bits, so the next mono partial is promoted
to a full refresh, exactly as after power-up or deep sleep. Gray and the fast
partial path are mutually exclusive device modes, not composable flags; the
console therefore stays 1bpp.

BLIT is 1bpp only and addresses x in whole *bytes*, which would mean a
different pixel column at 2bpp; it is not accepted against the gray buffer.

The panel's gray waveform is an undocumented OTP entry selected by
`0xE0 = 0x02` (force temperature) and `0xE5 = 0x5F`, and reportedly only works
on panels sold after 2024-10-23. Hosts should read `gray=4` from INFO rather
than assume it.

BLIT writes a rectangle straight into the framebuffer (`x_byte + w_bytes <=
100`, `y + h <= 480`, payload length must equal `8 + w_bytes*h`; NAK 3 / 1
otherwise) and grows the device's dirty bounding box. REFRESH partial
refreshes exactly that bbox (x range in whole bytes, y range in rows) and
clears it; NAK 2 if the bbox is empty. REFRESH full refreshes the whole panel
and clears the bbox. Both ACK immediately; the refresh runs asynchronously and
the host polls STATUS.

A partial refresh takes about 0.5 s and a full one about 4.3 s. Partial cost is
set by the waveform, not the window: a one-cell bbox costs the same as a
whole-screen one, so a host batching pixels into fewer REFRESHes wins and
shrinking the bbox does not. See `fast-refresh.md` for the panel sequences and
the measurements behind those numbers.

While a refresh is in flight the device answers BLIT, IMG_*, CLEAR, REFRESH and
SLEEP with BUSY (0x82); PING, INFO and STATUS always answer. The device streams
the framebuffer to the panel as the refresh runs, so a write accepted mid-flight
would land on the glass as a torn frame.

SLEEP puts the panel in deep sleep, which drops its configuration. The next
refresh silently re-runs the boot sequence, costing about 0.2 s extra.

## Responses (device -> host)

| type | name  | payload                                          |
|------|-------|--------------------------------------------------|
| 0x80 | ACK   | optional data (see below)                        |
| 0x81 | NAK   | u8 error code, then optional ASCII message       |
| 0x82 | BUSY  | none; device is mid-refresh, host should retry   |

ACK payloads:
- PING: 4 bytes "PONG"
- INFO: ASCII, e.g. `epaper-fw 0.3.0 panel=7in5_v2 w=800 h=480 proto=2 gray=4`
  (`gray=4` advertises 4-gray support; absent means fmt 1 is unavailable)
- STATUS: u8 busy (1 = refresh in flight), u16 partials_since_full,
  u32 ms_since_full (LE), u8 bbox_valid, u16 x_byte0, u16 y0, u16 x_byte1,
  u16 y1 (bbox is [x0,x1) x [y0,y1), only meaningful if bbox_valid)
- others: empty

Error codes: 1 = bad length, 2 = bad state (e.g. IMG_DATA before IMG_BEGIN),
3 = out of range (offset+len > 48000), 4 = unknown command, 5 = crc.

The device never sends unsolicited frames.

## Console (host side)

The CLI runs the command in a pty, feeds output to a terminal emulator
(pyte, `TERM=linux`, 100x30 by default), rasterises dirty rows with a
host-side font into a shadow of the framebuffer, byte-diffs against the last
frame sent, emits BLITs for the changed spans (adjacent rows with overlapping
x spans merged), and then exactly one REFRESH per batch.

The grid is a host concern: the cell box is derived from the selected terminal
size and the wire format is unaffected. Cell width need not be a multiple of 8
because rows are rasterised full-width and packed to the fixed 100-byte stride.

Batching policy (host):
- flush when pty output idle >= 60 ms, or dirty pending >= 250 ms;
- cursor-only changes debounce 200 ms;
- never flush while STATUS says busy; keep accumulating.

Ghosting budget (host): band partial = 1 unit, >= half screen = 2, whole
screen (scroll) = 3. Use REFRESH full when units >= 60 and pty idle >= 2 s,
or pty idle >= 30 s with units >= 10, or on final flush with units >= 30.
ED 2 / reset with units >= 10 schedules a full refresh once idle >= 2 s;
retain reset intent across partial flushes until a full refresh clears it.
Never interrupt active typing with a full refresh.
Scrolling is a whole-screen partial, never an automatic full refresh.

Cursor is drawn by the host as a reverse-video cell.
