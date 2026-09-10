# Host <-> Pico protocol (v1)

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
| 0x03 | SET_MODE       | u8 mode: 0 = picture, 1 = console               |
| 0x04 | CLEAR          | u8 color: 0 = white, 1 = black (full refresh)   |
| 0x10 | IMG_BEGIN      | u16 width (800), u16 height (480), u8 fmt (0 = 1bpp packed, MSB first, 1 = black) |
| 0x11 | IMG_DATA       | u32 offset, then up to 4090 bytes of image data |
| 0x12 | IMG_END        | u8 refresh: 0 = full, 1 = partial (whole frame) |
| 0x20 | CONSOLE_WRITE  | raw bytes (tty output stream, up to 4096)       |
| 0x21 | CONSOLE_RESIZE | reserved                                        |
| 0x30 | SLEEP          | none (panel deep sleep; any later command wakes)|
| 0x31 | RESET_BOOTSEL  | none (reboot into USB bootloader)               |

Image buffer is exactly 800*480/8 = 48000 bytes. IMG_DATA writes into the
staging buffer at `offset`; IMG_END pushes it to the panel. Image mode is
independent of SET_MODE: IMG_END always draws, and switches mode to picture.

## Responses (device -> host)

| type | name  | payload                                          |
|------|-------|--------------------------------------------------|
| 0x80 | ACK   | optional data (see below)                        |
| 0x81 | NAK   | u8 error code, then optional ASCII message       |
| 0x82 | BUSY  | none; device is mid-refresh, host should retry   |

ACK payloads:
- PING: 4 bytes "PONG"
- INFO: ASCII, e.g. `epaper-fw 0.1.0 panel=7in5_v2 w=800 h=480 mode=console`
- others: empty

Error codes: 1 = bad length, 2 = bad state (e.g. IMG_DATA before IMG_BEGIN),
3 = out of range (offset+len > 48000), 4 = unknown command, 5 = crc.

The device never sends unsolicited frames.

## Console semantics (device side)

- Text grid: 8x16 font -> 100 columns x 30 rows.
- Handles: printable ASCII, `\n` (LF = newline; cursor to column 0 too, since
  the pty output will be raw, i.e. LF and CRLF both work), `\r`, `\b`, `\t`
  (8-col stops), BEL ignored, DEL ignored.
- ANSI/CSI: `ESC [ H`, `ESC [ row ; col H`, `ESC [ J` (0/2), `ESC [ K`,
  `ESC [ n A/B/C/D`, `ESC [ ... m` (ignored), `ESC [ ? ... h/l` (ignored),
  `ESC c` (reset). Unknown CSI sequences are consumed and ignored. OSC
  (`ESC ]`) consumed up to BEL or ST.
- Refresh policy: mark dirty rows; when no new bytes for 250 ms, or dirty
  region >= half screen, push a partial refresh of the dirty bounding box.
  After 10 partial refreshes, or on full clear / scroll wrap, do a full
  refresh to remove ghosting. Never block USB reception during a refresh:
  keep servicing TinyUSB and buffering input in a ring buffer while busy.
