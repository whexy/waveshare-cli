#!/usr/bin/env python3
"""Reference device-side simulator for the e-paper USB protocol."""

from __future__ import annotations

import argparse
import os
import select
import signal
import struct
import sys
import termios
import time
from pathlib import Path

import serial  # noqa: F401 - required alongside Pillow for host-side experiments
from PIL import Image, ImageDraw, ImageFont

MAGIC = b"\xeb\x90"
WIDTH = 800
HEIGHT = 480
FRAMEBUFFER_SIZE = WIDTH * HEIGHT // 8
MAX_PAYLOAD = 4096
OUTPUT_PATH = Path("/tmp/epdsim.png")

ACK = 0x80
NAK = 0x81


def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def make_frame(frame_type: int, seq: int, payload: bytes = b"") -> bytes:
    body = struct.pack("<BBH", frame_type, seq, len(payload)) + payload
    return MAGIC + body + struct.pack("<H", crc16(body))


class FrameParser:
    def __init__(self) -> None:
        self.buffer = bytearray()

    def feed(self, data: bytes) -> list[tuple[str, int, int, bytes]]:
        self.buffer.extend(data)
        events: list[tuple[str, int, int, bytes]] = []
        while True:
            start = self.buffer.find(MAGIC)
            if start < 0:
                self.buffer[:] = self.buffer[-1:] if self.buffer.endswith(MAGIC[:1]) else b""
                break
            if start:
                del self.buffer[:start]
            if len(self.buffer) < 6:
                break
            frame_type, seq, length = struct.unpack_from("<BBH", self.buffer, 2)
            if length > MAX_PAYLOAD:
                events.append(("length", frame_type, seq, b""))
                del self.buffer[0]
                continue
            total = 8 + length
            if len(self.buffer) < total:
                break
            body = bytes(self.buffer[2 : 6 + length])
            payload = bytes(self.buffer[6 : 6 + length])
            received_crc = struct.unpack_from("<H", self.buffer, 6 + length)[0]
            del self.buffer[:total]
            if crc16(body) != received_crc:
                events.append(("crc", frame_type, seq, b""))
            else:
                events.append(("frame", frame_type, seq, payload))
        return events


class Console:
    columns = 100
    rows = 30

    def __init__(self) -> None:
        self.grid = [[" "] * self.columns for _ in range(self.rows)]
        self.row = 0
        self.column = 0
        self.state = "normal"
        self.csi = ""
        self.dirty_rows: set[int] = set()
        self.force_full = False
        self.last_input = 0.0
        self.partial_refreshes = 0

    def reset(self) -> None:
        self.grid = [[" "] * self.columns for _ in range(self.rows)]
        self.row = self.column = 0
        self.state = "normal"
        self.dirty_rows.update(range(self.rows))
        self.force_full = True

    def _mark(self, row: int) -> None:
        self.dirty_rows.add(max(0, min(self.rows - 1, row)))

    def _scroll(self) -> None:
        self.grid.pop(0)
        self.grid.append([" "] * self.columns)
        self.row = self.rows - 1
        self.dirty_rows.update(range(self.rows))
        self.force_full = True

    def _newline(self) -> None:
        self.column = 0
        self.row += 1
        if self.row >= self.rows:
            self._scroll()

    def _put(self, char: str) -> None:
        self.grid[self.row][self.column] = char
        self._mark(self.row)
        self.column += 1
        if self.column >= self.columns:
            self._newline()

    @staticmethod
    def _params(raw: str) -> list[int | None]:
        raw = raw.lstrip("?")
        if not raw:
            return []
        result = []
        for value in raw.split(";"):
            try:
                result.append(int(value) if value else None)
            except ValueError:
                result.append(None)
        return result

    def _csi_command(self, final: str) -> None:
        params = self._params(self.csi)
        first = params[0] if params and params[0] is not None else 0
        if final in "Hf":
            row = params[0] if len(params) > 0 and params[0] is not None else 1
            column = params[1] if len(params) > 1 and params[1] is not None else 1
            self.row = max(0, min(self.rows - 1, row - 1))
            self.column = max(0, min(self.columns - 1, column - 1))
        elif final == "A":
            self.row = max(0, self.row - (first or 1))
        elif final == "B":
            self.row = min(self.rows - 1, self.row + (first or 1))
        elif final == "C":
            self.column = min(self.columns - 1, self.column + (first or 1))
        elif final == "D":
            self.column = max(0, self.column - (first or 1))
        elif final == "J":
            if first == 2:
                self.grid = [[" "] * self.columns for _ in range(self.rows)]
                self.dirty_rows.update(range(self.rows))
                self.force_full = True
            elif first == 0:
                self.grid[self.row][self.column :] = [" "] * (self.columns - self.column)
                for row in range(self.row + 1, self.rows):
                    self.grid[row] = [" "] * self.columns
                self.dirty_rows.update(range(self.row, self.rows))
        elif final == "K":
            self.grid[self.row][self.column :] = [" "] * (self.columns - self.column)
            self._mark(self.row)
        # SGR, private modes, and unknown CSI commands are intentionally consumed.

    def feed(self, data: bytes) -> None:
        self.last_input = time.monotonic()
        for byte in data:
            if self.state == "osc":
                if byte == 0x07:
                    self.state = "normal"
                elif byte == 0x1B:
                    self.state = "osc_esc"
                continue
            if self.state == "osc_esc":
                self.state = "normal" if byte == ord("\\") else "osc"
                continue
            if self.state == "esc":
                if byte == ord("["):
                    self.csi = ""
                    self.state = "csi"
                elif byte == ord("]"):
                    self.state = "osc"
                elif byte == ord("c"):
                    self.reset()
                else:
                    self.state = "normal"
                continue
            if self.state == "csi":
                if 0x40 <= byte <= 0x7E:
                    self._csi_command(chr(byte))
                    self.state = "normal"
                elif len(self.csi) < 64:
                    self.csi += chr(byte)
                else:
                    self.state = "normal"
                continue
            if byte == 0x1B:
                self.state = "esc"
            elif byte == 0x0A:
                self._newline()
            elif byte == 0x0D:
                self.column = 0
            elif byte == 0x08:
                self.column = max(0, self.column - 1)
            elif byte == 0x09:
                self.column = min(self.columns - 1, ((self.column // 8) + 1) * 8)
            elif byte in (0x07, 0x7F):
                pass
            elif 0x20 <= byte <= 0x7E:
                self._put(chr(byte))

    def refresh_due(self, now: float | None = None) -> bool:
        if not self.dirty_rows:
            return False
        return len(self.dirty_rows) >= self.rows // 2 or (now or time.monotonic()) - self.last_input >= 0.250

    def refreshed(self) -> None:
        if self.force_full or self.partial_refreshes >= 9:
            self.partial_refreshes = 0
        else:
            self.partial_refreshes += 1
        self.force_full = False
        self.dirty_rows.clear()


class Simulator:
    def __init__(self, output_path: Path = OUTPUT_PATH) -> None:
        self.output_path = output_path
        self.parser = FrameParser()
        self.console = Console()
        self.framebuffer = bytearray(FRAMEBUFFER_SIZE)
        self.staging = bytearray(FRAMEBUFFER_SIZE)
        self.image_active = False
        self.mode = 0
        self.sleeping = False

    @staticmethod
    def _font() -> ImageFont.ImageFont:
        for path in (
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Monaco.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ):
            try:
                return ImageFont.truetype(path, 13)
            except OSError:
                pass
        return ImageFont.load_default()

    def render_console(self) -> None:
        image = Image.new("1", (WIDTH, HEIGHT), 1)
        draw = ImageDraw.Draw(image)
        font = self._font()
        for row, cells in enumerate(self.console.grid):
            draw.text((0, row * 16), "".join(cells), fill=0, font=font, spacing=0)
        self.framebuffer[:] = _image_to_framebuffer(image)
        image.save(self.output_path)
        self.console.refreshed()

    def render_framebuffer(self) -> None:
        image = Image.new("1", (WIDTH, HEIGHT), 1)
        pixels = image.load()
        for y in range(HEIGHT):
            offset = y * (WIDTH // 8)
            for x in range(WIDTH):
                pixels[x, y] = 0 if self.framebuffer[offset + x // 8] & (0x80 >> (x & 7)) else 1
        image.save(self.output_path)

    @staticmethod
    def _nak(seq: int, code: int, message: str) -> bytes:
        return make_frame(NAK, seq, bytes([code]) + message.encode("ascii"))

    def command(self, frame_type: int, seq: int, payload: bytes) -> bytes:
        self.sleeping = False
        if frame_type == 0x01:
            return make_frame(ACK, seq, b"PONG") if not payload else self._nak(seq, 1, "bad length")
        if frame_type == 0x02:
            if payload:
                return self._nak(seq, 1, "bad length")
            mode = "console" if self.mode else "picture"
            return make_frame(ACK, seq, f"epdsim 0.1.0 panel=7in5_v2 w=800 h=480 mode={mode}".encode())
        if frame_type == 0x03:
            if len(payload) != 1 or payload[0] > 1:
                return self._nak(seq, 1, "bad mode")
            self.mode = payload[0]
            return make_frame(ACK, seq)
        if frame_type == 0x04:
            if len(payload) != 1 or payload[0] > 1:
                return self._nak(seq, 1, "bad color")
            self.framebuffer[:] = bytes([0xFF if payload[0] else 0x00]) * FRAMEBUFFER_SIZE
            self.render_framebuffer()
            return make_frame(ACK, seq)
        if frame_type == 0x10:
            if len(payload) != 5:
                return self._nak(seq, 1, "bad length")
            width, height, fmt = struct.unpack("<HHB", payload)
            if (width, height, fmt) != (WIDTH, HEIGHT, 0):
                return self._nak(seq, 3, "unsupported image")
            self.staging[:] = bytes(FRAMEBUFFER_SIZE)
            self.image_active = True
            return make_frame(ACK, seq)
        if frame_type == 0x11:
            if len(payload) < 4:
                return self._nak(seq, 1, "bad length")
            if not self.image_active:
                return self._nak(seq, 2, "IMG_BEGIN required")
            offset = struct.unpack_from("<I", payload)[0]
            chunk = payload[4:]
            if offset + len(chunk) > FRAMEBUFFER_SIZE:
                return self._nak(seq, 3, "image range")
            self.staging[offset : offset + len(chunk)] = chunk
            return make_frame(ACK, seq)
        if frame_type == 0x12:
            if len(payload) != 1 or payload[0] > 1:
                return self._nak(seq, 1, "bad refresh")
            if not self.image_active:
                return self._nak(seq, 2, "IMG_BEGIN required")
            self.framebuffer[:] = self.staging
            self.image_active = False
            self.mode = 0
            self.render_framebuffer()
            return make_frame(ACK, seq)
        if frame_type == 0x20:
            self.mode = 1
            self.console.feed(payload)
            if self.console.refresh_due():
                self.render_console()
            return make_frame(ACK, seq)
        if frame_type == 0x21:
            return self._nak(seq, 4, "reserved")
        if frame_type == 0x30:
            if payload:
                return self._nak(seq, 1, "bad length")
            self.sleeping = True
            return make_frame(ACK, seq)
        if frame_type == 0x31:
            return make_frame(ACK, seq) if not payload else self._nak(seq, 1, "bad length")
        return self._nak(seq, 4, "unknown command")

    def receive(self, data: bytes) -> bytes:
        responses = bytearray()
        for kind, frame_type, seq, payload in self.parser.feed(data):
            if kind == "crc":
                responses.extend(self._nak(seq, 5, "crc"))
            elif kind == "length":
                responses.extend(self._nak(seq, 1, "bad length"))
            else:
                responses.extend(self.command(frame_type, seq, payload))
        return bytes(responses)


def _image_to_framebuffer(image: Image.Image) -> bytes:
    pixels = image.load()
    output = bytearray(FRAMEBUFFER_SIZE)
    for y in range(HEIGHT):
        offset = y * (WIDTH // 8)
        for x in range(WIDTH):
            if pixels[x, y] == 0:
                output[offset + x // 8] |= 0x80 >> (x & 7)
    return bytes(output)


def configure_slave(fd: int) -> None:
    attrs = termios.tcgetattr(fd)
    attrs[0] = attrs[0] & ~(termios.IXON | termios.IXOFF | termios.ICRNL | termios.INLCR)
    attrs[1] = attrs[1] & ~termios.OPOST
    attrs[3] = attrs[3] & ~(termios.ICANON | termios.ECHO | termios.ISIG)
    attrs[6][termios.VMIN] = 1
    attrs[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, attrs)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT_PATH)
    args = parser.parse_args()
    master, slave = os.openpty()
    configure_slave(slave)
    slave_path = os.ttyname(slave)
    print(slave_path, flush=True)
    simulator = Simulator(args.output)
    running = True

    def stop(_signum: int, _frame: object) -> None:
        nonlocal running
        running = False

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    try:
        while running:
            readable, _, _ = select.select([master], [], [], 0.050)
            if readable:
                try:
                    data = os.read(master, 8192)
                except OSError:
                    data = b""
                if data:
                    response = simulator.receive(data)
                    if response:
                        os.write(master, response)
            if simulator.console.refresh_due():
                simulator.render_console()
    finally:
        os.close(master)
        os.close(slave)
    return 0


if __name__ == "__main__":
    sys.exit(main())
