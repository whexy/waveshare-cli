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

from .protocol import console_write, set_mode


def _write_all(fd, data):
    while data:
        count = os.write(fd, data)
        data = data[count:]


def pipe_stdin(device):
    device.request(*set_mode(True))
    while data := os.read(sys.stdin.fileno(), 4096):
        device.request(*console_write(data))
    return 0


def run(device, command, echo=False, cols=100, rows=30):
    if not 1 <= cols <= 65535 or not 1 <= rows <= 65535:
        raise ValueError('terminal dimensions must be 1..65535')
    device.request(*set_mode(True))
    master, slave = pty.openpty()
    child = None
    saved = None
    stdin = sys.stdin.fileno()
    pending = bytearray()
    last_send = time.monotonic()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        env = dict(os.environ, TERM='vt100', COLUMNS=str(cols), LINES=str(rows))
        # A new session needs the slave explicitly assigned as controlling tty.
        def child_setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        child = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                                 env=env, preexec_fn=child_setup)
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
                    pending.extend(data)
                    if echo:
                        _write_all(sys.stdout.fileno(), data)
                else:
                    eof = True
            if pending and (len(pending) >= 4096 or time.monotonic() - last_send >= 0.02 or eof):
                while pending:
                    device.request(*console_write(pending[:4096]))
                    del pending[:4096]
                last_send = time.monotonic()
            if child.poll() is not None and not ready:
                eof = True
        if pending:
            device.request(*console_write(pending))
        return child.wait(timeout=2)
    finally:
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
