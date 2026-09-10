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


class Simulator:
    def __init__(self, output=OUTPUT_PATH, clock=time.monotonic):
        self.output = Path(output)
        self.clock = clock
        self.parser = FrameParser()
        self.framebuffer = bytearray(FRAMEBUFFER_SIZE)
        self.bbox = None
        self.image_active = False
        self.until = 0
        self.partials = 0
        self.last_full = clock()
        self.pending = None

    def poll(self):
        if self.pending is not None and self.clock() >= self.until:
            Image.frombytes('1', (800,480), bytes(b ^ 255 for b in self.pending)).save(self.output)
            self.pending = None

    def start(self, mode):
        self.pending = bytes(self.framebuffer)
        self.until = self.clock() + (.5 if mode else 4.3)
        self.bbox = None
        if mode:
            self.partials += 1
        else:
            self.partials = 0
            self.last_full = self.clock()

    def command(self, command, seq, data):
        self.poll()
        def ack(payload=b''): return make_frame(ACK,seq,payload)
        def nak(code): return make_frame(NAK,seq,bytes([code]))
        if command in (4,16,17,18,19,20,48) and self.pending is not None:
            return make_frame(0x82,seq)
        if command in (1,2,5,48,49):
            if data: return nak(1)
            if command == 1: return ack(b'PONG')
            if command == 2: return ack(b'epaper-sim 0.2.0 panel=7in5_v2 w=800 h=480 proto=2')
            if command == 5:
                return ack(struct.pack('<BHIBHHHH', self.pending is not None,
                           min(self.partials,65535), int((self.clock()-self.last_full)*1000)&0xffffffff,
                           self.bbox is not None, *(self.bbox or (0,0,0,0))))
            return ack()
        if command == 4:
            if len(data)!=1 or data[0]>1: return nak(1)
            self.framebuffer[:] = bytes([255 if data[0] else 0])*48000
            self.start(0)
            return ack()
        if command == 16:
            if len(data)!=5: return nak(1)
            if data != struct.pack('<HHB',800,480,0): return nak(3)
            self.image_active = True
            return ack()
        if command == 17:
            if len(data)<4: return nak(1)
            if not self.image_active: return nak(2)
            offset = struct.unpack_from('<I',data)[0]
            if offset+len(data)-4>48000: return nak(3)
            self.framebuffer[offset:offset+len(data)-4] = data[4:]
            return ack()
        if command == 19:
            if len(data)<8: return nak(1)
            x,y,w,h = struct.unpack_from('<HHHH',data)
            if not w or not h or x+w>100 or y+h>480: return nak(3)
            if len(data)!=8+w*h: return nak(1)
            for r in range(h):
                self.framebuffer[(y+r)*100+x:(y+r)*100+x+w] = data[8+r*w:8+(r+1)*w]
            a,b,c,d = self.bbox or (x,y,x+w,y+h)
            self.bbox = min(a,x),min(b,y),max(c,x+w),max(d,y+h)
            return ack()
        if command in (18,20):
            if len(data)!=1 or data[0]>1: return nak(1)
            if command==18:
                if not self.image_active: return nak(2)
                self.image_active = False
            elif data[0] and self.bbox is None:
                return nak(2)
            self.start(data[0])
            return ack()
        return nak(4)

    def receive(self, data):
        result = bytearray()
        for kind, cmd, seq, payload in self.parser.feed(data):
            if kind != 'frame':
                result.extend(make_frame(NAK,seq,bytes([5 if kind=='crc' else 1])))
            else:
                result.extend(self.command(cmd,seq,payload))
        return bytes(result)


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
            simulator.poll()
    finally:
        os.close(master)
        os.close(slave)
    return 0


if __name__ == "__main__":
    sys.exit(main())
