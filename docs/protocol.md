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
| 0x10 | IMG_BEGIN      | u16 width (800), u16 height (480), u8 fmt (0 = 1bpp packed, MSB first, 1 = black) |
| 0x11 | IMG_DATA       | u32 offset, then up to 4090 bytes of image data |
| 0x12 | IMG_END        | u8 refresh: 0 = full, 1 = partial (whole frame) |
| 0x13 | BLIT           | u16 x_byte, u16 y, u16 w_bytes, u16 h, then w_bytes*h bits (row-major, MSB = leftmost pixel, 1 = black) |
| 0x14 | REFRESH        | u8 mode: 0 = full, 1 = partial (dirty bbox)     |
| 0x30 | SLEEP          | none (panel deep sleep; any later command wakes)|
| 0x31 | RESET_BOOTSEL  | none (reboot into USB bootloader)               |

The framebuffer is exactly 800*480/8 = 48000 bytes, 100 bytes per row.
IMG_DATA writes into it at `offset`; IMG_END pushes the whole frame to the
panel.

BLIT writes a rectangle straight into the framebuffer (`x_byte + w_bytes <=
100`, `y + h <= 480`, payload length must equal `8 + w_bytes*h`; NAK 3 / 1
otherwise) and grows the device's dirty bounding box. REFRESH partial
refreshes exactly that bbox (x range in whole bytes, y range in rows) and
clears it; NAK 2 if the bbox is empty. REFRESH full refreshes the whole panel
and clears the bbox. Both ACK immediately; the refresh runs asynchronously and
the host polls STATUS.

While a refresh is in flight the device answers BLIT, IMG_*, CLEAR, REFRESH and
SLEEP with BUSY (0x82); PING, INFO and STATUS always answer. This is what
keeps the old-plane bookkeeping correct: nothing can be written outside the
refreshed window during a refresh.

## Responses (device -> host)

| type | name  | payload                                          |
|------|-------|--------------------------------------------------|
| 0x80 | ACK   | optional data (see below)                        |
| 0x81 | NAK   | u8 error code, then optional ASCII message       |
| 0x82 | BUSY  | none; device is mid-refresh, host should retry   |

ACK payloads:
- PING: 4 bytes "PONG"
- INFO: ASCII, e.g. `epaper-fw 0.2.0 panel=7in5_v2 w=800 h=480 proto=2`
- STATUS: u8 busy (1 = refresh in flight), u16 partials_since_full,
  u32 ms_since_full (LE), u8 bbox_valid, u16 x_byte0, u16 y0, u16 x_byte1,
  u16 y1 (bbox is [x0,x1) x [y0,y1), only meaningful if bbox_valid)
- others: empty

Error codes: 1 = bad length, 2 = bad state (e.g. IMG_DATA before IMG_BEGIN),
3 = out of range (offset+len > 48000), 4 = unknown command, 5 = crc.

The device never sends unsolicited frames.

## Console (host side)

The CLI runs the command in a pty, feeds output to a terminal emulator
(pyte, `TERM=vt100`, 100x30), rasterises dirty rows with an 8x16 font into a
host shadow of the framebuffer, byte-diffs against the last frame sent, emits
BLITs for the changed spans (adjacent rows with overlapping x spans merged),
and then exactly one REFRESH per batch.

Batching policy (host):
- flush when pty output idle >= 100 ms, or dirty pending >= 400 ms;
- cursor-only changes debounce 300 ms;
- never flush while STATUS says busy; keep accumulating.

Ghosting budget (host): band partial = 1 unit, >= half screen = 2, whole
screen (scroll) = 3. Use REFRESH full instead of partial when units >= 10,
or on ED 2 / reset with units >= 3, or pty idle >= 30 s with units >= 3.
Scrolling is a whole-screen partial, never an automatic full refresh.

Cursor is drawn by the host as a reverse-video cell.
