import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import tty

from . import protocol as p
from .font import Font, find_font
from .geometry import FRAME_BYTES, STRIDE
from .geometry import geometry as default_geometry
from .render import Renderer, diff
from .term import TerminalModel

# Batch fast partials for responsiveness; defer disruptive full refreshes to idle.
OUTPUT_IDLE_SECONDS = 0.06
MAX_PENDING_SECONDS = 0.25
CURSOR_IDLE_SECONDS = 0.2
FULL_IDLE_SECONDS = 2
FULL_UNITS = 60
MAINTENANCE_IDLE_SECONDS = 30
MAINTENANCE_UNITS = 10
RESET_UNITS = 10
FINAL_UNITS = 30


def _write_all(fd, data):
    while data:
        count = os.write(fd, data)
        data = data[count:]


class Session:
    def __init__(self, device, geometry=None, font=None):
        self.device = device
        self.geometry = geometry or default_geometry()
        self.font = font or Font(
            find_font(), self.geometry.cell_width, self.geometry.cell_height
        )
        self.model = TerminalModel(self.geometry)
        self.renderer = Renderer(self.geometry, self.font)
        self.sent = bytearray(FRAME_BYTES)
        self.units = 0
        self.last_output = time.monotonic()
        self.pending_since = self.last_output
        self.cursor_only = False
        self.initial = True

    def feed(self, data):
        rows = range(self.geometry.rows)
        before = [self.model.row(r) for r in rows]
        replies = self.model.feed(data)
        content_changed = any(before[r] != self.model.row(r) for r in rows)
        now = time.monotonic()
        if self.pending_since is None:
            self.pending_since = now
            self.cursor_only = not content_changed
        elif content_changed:
            self.cursor_only = False
        self.last_output = now
        return replies

    def tick(self, final=False):
        now = time.monotonic()
        idle = now - self.last_output
        maintenance = (
            idle >= FULL_IDLE_SECONDS
            and (
                self.units >= FULL_UNITS
                or (self.model.reset_requested and self.units >= RESET_UNITS)
            )
        ) or (idle >= MAINTENANCE_IDLE_SECONDS and self.units >= MAINTENANCE_UNITS)
        if not final and not maintenance:
            if self.pending_since is None:
                return False
            if self.cursor_only:
                if idle < CURSOR_IDLE_SECONDS:
                    return False
            elif (
                idle < OUTPUT_IDLE_SECONDS
                and now - self.pending_since < MAX_PENDING_SECONDS
            ):
                return False
        state = p.parse_status(self.device.request(*p.status()))
        if state.busy:
            return False
        current = self.renderer.render(self.model)
        payloads, cost = diff(self.sent, current, self.geometry.cell_height)
        # No host readback exists: establish a known framebuffer once before
        # relying on byte diffs, including clearing pixels from a prior client.
        if self.initial:
            band = (p.MAX_PAYLOAD - 8) // STRIDE
            payloads = [
                p.blit(
                    0,
                    y,
                    STRIDE,
                    min(band, 480 - y),
                    current[y * STRIDE : min(y + band, 480) * STRIDE],
                )[1]
                for y in range(0, 480, band)
            ]
            cost = 3
        full = maintenance or (final and self.units >= FINAL_UNITS)
        if payloads or full:
            for payload in payloads:
                self.device.request(p.Command.BLIT, payload)
            self.device.request(*p.refresh(full))
            self.sent[:] = current
            self.units = 0 if full else self.units + cost
        self.initial = False
        self.pending_since = None
        if full:
            self.model.clear_reset()
        return True

    def finish(self):
        deadline = time.monotonic() + 60
        while not self.tick(final=True):
            if time.monotonic() > deadline:
                raise TimeoutError('final console flush timed out')
            time.sleep(0.05)
        while p.parse_status(self.device.request(*p.status())).busy:
            if time.monotonic() > deadline:
                raise TimeoutError('final refresh timed out')
            time.sleep(0.05)


def _onlcr(data):
    # Piped bytes bypass the tty line discipline that would add CR to LF.
    return data.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')


def text(device, data, geometry=None, font=None):
    session = Session(device, geometry, font)
    session.feed(_onlcr(data))
    session.finish()
    return 0


def pipe_stdin(device, geometry=None, font=None):
    session = Session(device, geometry, font)
    fd = sys.stdin.fileno()
    while True:
        ready, _, _ = select.select([fd], [], [], 0.02)
        if ready:
            data = os.read(fd, 4096)
            if not data:
                break
            session.feed(_onlcr(data))
        session.tick()
    session.finish()
    return 0


def run(device, command, echo=False, geometry=None, font=None):
    session = Session(device, geometry, font)
    cols, rows = session.geometry.columns, session.geometry.rows
    master, slave = pty.openpty()
    child = None
    saved = None
    stdin = sys.stdin.fileno()
    old_winch = signal.signal(signal.SIGWINCH, signal.SIG_IGN)

    def interrupted(signum, frame):
        raise KeyboardInterrupt

    old_term = signal.signal(signal.SIGTERM, interrupted)
    old_hup = signal.signal(signal.SIGHUP, interrupted)
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        # 'linux' has no padding delays, no alternate screen and only the
        # CSI subset pyte implements; vt100 makes curses apps emit $<n> pads.
        env = dict(os.environ, TERM='linux', COLUMNS=str(cols), LINES=str(rows))

        # A new session needs the slave explicitly assigned as controlling tty.
        def child_setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        child = subprocess.Popen(
            command,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            preexec_fn=child_setup,
        )
        os.close(slave)
        slave = -1
        if os.isatty(stdin):
            saved = termios.tcgetattr(stdin)
            tty.setraw(stdin)
        inputs = [master, stdin]
        eof = False
        while not eof:
            ready, _, _ = select.select(inputs, [], [], 0.02)
            if stdin in ready:
                data = os.read(stdin, 4096)
                if data:
                    _write_all(master, data)
                else:
                    inputs.remove(stdin)
                    _write_all(master, b'\x04')
            if master in ready:
                try:
                    data = os.read(master, 4096)
                except OSError as exc:
                    if exc.errno != errno.EIO:
                        raise
                    data = b''
                if data:
                    replies = session.feed(data)
                    if replies:
                        _write_all(master, replies)
                    if echo:
                        _write_all(sys.stdout.fileno(), data)
                else:
                    eof = True
            session.tick()
        session.finish()
        return child.wait(timeout=2)
    finally:
        signal.signal(signal.SIGWINCH, old_winch)
        signal.signal(signal.SIGTERM, old_term)
        signal.signal(signal.SIGHUP, old_hup)
        if saved is not None:
            termios.tcsetattr(stdin, termios.TCSADRAIN, saved)
        os.close(master)
        if slave >= 0:
            os.close(slave)
        if child is not None and child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGHUP)
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            except ProcessLookupError:
                child.wait()
