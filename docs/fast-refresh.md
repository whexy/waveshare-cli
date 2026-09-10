# Fast partial refresh on the 7.5" V2 (UC8179)

Measured 2026-09-10 on the real panel with MicroPython probe scripts
(`/tmp/ws/lut.py`, `/tmp/ws/exp{2..6}.py`), BUSY timed from `0x12` to BUSY
high, SPI 8 MHz.

## Why v2 was slow

- The Waveshare "partial" init (`E0=02, E5=6E`) selects an OTP waveform that
  is still multi-phase: BUSY = 3.4 s for *any* window, even one 8x16 cell,
  and the area visibly flips ~3 times. Window size does not matter; the
  waveform does.
- v2 firmware reset, re-initialised and power-cycled the panel on every
  refresh, and sent both planes (0x10 + 0x13) each time.
- Host policy forced a full refresh (4 s) after 10 partial units.

## Recipe that works

Boot once (reset + init + PON); leave the panel powered between updates.

```
reset pulse
01 07 07 3F 3F      PWR
06 17 17 28 17      BTST
00 1F               PSR (OTP LUT, KW, scan up, shift right)
61 03 20 01 E0      TRES 800x480
15 00               DUSPI
50 29 07            CDI: N2OCP=1, DDX=01, CDI=7
60 22               TCON
04, wait BUSY       PON (~175 ms)
```

Fast partial mode (set once; re-send after a full refresh):

```
00 3F               PSR REG=1 (LUT from registers), KW
82 26               VCOM_DC
50 39 07            CDI: BDV=11 (border keeps state), N2OCP=1, DDX=01
20 00 T1 00 00 00 01 + 36x00   LUTC  (VCOM: level 00 = VCOM_DC)
21 00 T1 00 00 00 01 + 36x00   LUTWW (no drive)
22 80 T1 00 00 00 01 + 36x00   LUTKW (level 10 = drive to white)
23 40 T1 00 00 00 01 + 36x00   LUTWK (level 01 = drive to black)
24 00 T1 00 00 00 01 + 36x00   LUTKK (no drive)
25 00 T1 00 00 00 01 + 36x00   LUTBD
```

Each LUT is 42 bytes: 6 groups of `level_byte, T1, T2, T3, T4, repeat`; only
the first group is used. Level byte encodes 4 phases, 2 bits each, MSB first.

Partial update (as many as wanted, back to back):

```
91                  partial in
90 x0h x0l x1h x1l y0h y0l y1h y1l 01   (x1,y1 inclusive; x multiples of 8)
13 <w*h/8 bytes>    NEW plane only. 1 = white on the wire.
12, wait BUSY       DRF
92                  partial out
```

`N2OCP=1` copies NEW into OLD after each refresh, so the old plane never has
to be re-sent; the panel's own OLD RAM is the diff base. Only pixels whose
OLD != NEW are driven; unchanged pixels get LUTWW/LUTKK = no drive, so there
is no flicker outside the changed glyphs.

Full refresh (ghost clean-up; with real content):

```
02, wait BUSY       POF   (~40 ms)  — required, else the full refresh is faint
00 1F               PSR back to OTP LUT
50 29 07
04, wait BUSY       PON   (~175 ms)
10 <48000 x FF>     OLD plane = all white
13 <frame>          NEW plane
12, wait BUSY       ~4.0 s
(then re-send the fast-partial mode block)
```

## Timings (BUSY after 0x12)

| T1 frames | BUSY   | Appearance |
|-----------|--------|------------|
| 6         | 216 ms | very faint |
| 10        | 296 ms | light grey |
| 16        | 418 ms | crisp, black slightly lighter than full |
| 20        | 498 ms | good |
| 24        | 580 ms | close to full-refresh black |
| 30        | 700 ms | no further gain |
| 16, rep 2 | 742 ms | same as 24 |

Frame time ≈ 20 ms at the default 50 Hz PLL (0x30 = 0x3C). BUSY has a ~100 ms
fixed overhead. T1 = 20 is the chosen default: 0.5 s, good contrast.

Both directions (W→K and K→W) work with this single phase. Text drawn, erased
and redrawn 20+ times in the same window showed no visible ghosting. A solid
black 100x100 patch erased with the fast LUT left a very faint shadow; the
periodic full refresh removes it.

## Consequences for the design

- Firmware: one reset at boot; keep powered; track which LUT mode the panel
  is in; partial = window + 0x13 only + DRF; full = POF/PSR/PON/both
  planes/DRF then restore partial mode. Drop the per-refresh reset and the
  0x10 transfer for partials.
- SPI can be raised (GxEPD2 uses 10 MHz; probes ran at 8 MHz). A whole-screen
  0x13 plane is 48 kB: 96 ms at 4 MHz, 48 ms at 8 MHz.
- Host: partials cost 0.5 s regardless of area, so coalescing by time (not by
  area) is what matters. Full refreshes should be rare and only when idle.
