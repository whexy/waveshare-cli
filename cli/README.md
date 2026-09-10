# epaper host CLI

Python 3.11+ on macOS/POSIX. Install with `python -m pip install ./cli`
(prefer a virtual environment). Alternatively, from `cli/`, use:

```sh
nix shell --impure --expr 'let p = import <nixpkgs> {}; in p.python3.withPackages (ps: [ ps.pyserial ps.pillow ])'
python -m unittest -v
python -m epaper --help
```

Examples after installation:

```sh
epaper ping
epaper info
epaper draw photo.png --fit fit --dither
epaper draw --testcard
epaper draw photo.png --partial
epaper mode console
epaper write 'hello\nworld\n'
printf 'hello\n' | epaper console --stdin
epaper console --echo -- /bin/zsh
epaper clear
epaper sleep
```

Global `--port` and `--verbose` precede the subcommand. Auto-detection prefers
USB products containing `epaper`, then the first `/dev/cu.usbmodem*`.
`--fit fit` letterboxes on white, `fill` crops, and `stretch` distorts to fit.
Rotation is counterclockwise. Default monochrome conversion is threshold 128;
`--dither` selects Floyd–Steinberg instead. Transparent pixels composite on white.

`console` defaults to `$SHELL`, with TERM=vt100 and a 100x30 PTY. The firmware
has a fixed 100x30 grid: `--cols` and `--rows` affect the child PTY only, not
the device (CONSOLE_RESIZE is reserved). Rendering is ASCII with the limited
ANSI subset in the protocol; Unicode/full-screen applications may not render
faithfully. Keyboard input stays on the Mac. Ctrl-C is sent to the child while
the local terminal is raw; exit the child shell to finish. Local echo is opt-in.
USB ACK/backpressure can delay input processing during panel refreshes.

For a hardware-free manual smoke test, from `cli/` in the environment above:

```sh
python -m tests.fake_device
# Prints e.g. /dev/ttys006; leave running, then in another terminal:
python -m epaper --port /dev/ttys006 ping
# PONG
```

The automated integration test creates its own fake PTY and checks ping, info,
testcard upload, stdin streaming, and a shell PTY. The fake ACKs every command;
it does not simulate actual rendering or firmware validation.
