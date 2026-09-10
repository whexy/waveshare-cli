# waveshare

A webcam (DJI Osmo Pocket 3) points at a Waveshare 7.5" e-paper panel.

## Capturing the panel

Capture goes through the **OBS Virtual Camera**, not the DJI directly. OBS must
be open with the `OsmoPocket3` video capture source active and **Start Virtual
Camera** clicked; this is the steady-state setup and is assumed to be running.

```bash
nix develop   # ffmpeg, imagemagick, coreutils, jq

idx="$(ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 \
  | sed -n '/video devices/,/audio devices/p' \
  | sed -n 's/^.*\[\([0-9][0-9]*\)\] OBS Virtual Camera$/\1/p' | head -n1)"

timeout --signal=KILL 10 ffmpeg -hide_banner -loglevel error \
  -f avfoundation -pixel_format uyvy422 -video_size 1920x1080 -framerate 60 \
  -i "$idx" -frames:v 30 -update 1 -q:v 2 -y /tmp/epaper.jpg
```

Takes ~0.6s and yields a legible 1920x1080 frame of the panel.

Write captures to `/tmp`, never into the repository.

## Constraints

Resolve the device index by name on every capture. AVFoundation indices are
assignment order, not identity: both `OsmoPocket3` and `OBS Virtual Camera`
moved repeatedly within a single session, and a stale index silently captures a
different camera rather than failing.

The virtual camera accepts `-framerate 60` only; 30 is rejected outright.

Discard ~30 warmup frames with `-update 1`. Early frames arrive before auto
exposure settles and blow out the e-paper white.

Bound every capture with `timeout --signal=KILL`. ffmpeg blocked in
AVFoundation's frame wait ignores SIGTERM, so a plain `timeout` never returns.

Capturing the DJI directly (bypassing OBS) does not work on this machine. It
either fails immediately with `Could not lock device for configuration` while
OBS holds the device, or stalls forever delivering no frames while
`UVCAssistant` logs `UVCFrameProcessorH264Decode ... Failed to set decode
session`. The underlying cause is unresolved; the virtual camera avoids it by
letting OBS own the stream negotiation.
