import binascii
import struct
from dataclasses import dataclass
from enum import IntEnum

MAGIC = b'\xeb\x90'
MAX_PAYLOAD = 4096


class Command(IntEnum):
    PING = 0x01
    INFO = 0x02
    CLEAR = 0x04
    STATUS = 0x05
    IMG_BEGIN = 0x10
    IMG_DATA = 0x11
    IMG_END = 0x12
    BLIT = 0x13
    REFRESH = 0x14
    SLEEP = 0x30
    RESET_BOOTSEL = 0x31


ACK, NAK, BUSY = 0x80, 0x81, 0x82
ERROR_NAMES = {
    1: 'bad length',
    2: 'bad state',
    3: 'out of range',
    4: 'unknown command',
    5: 'crc',
}


def crc16(data):
    return binascii.crc_hqx(data, 0xFFFF)


@dataclass(frozen=True)
class Frame:
    type: int
    seq: int
    payload: bytes


def encode(cmd, seq, payload=b''):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError('payload exceeds 4096 bytes')
    body = struct.pack('<BBH', cmd, seq, len(payload)) + payload
    return MAGIC + body + struct.pack('<H', crc16(body))


class Decoder:
    def __init__(self):
        self.buffer = bytearray()

    def feed(self, data):
        self.buffer.extend(data)
        frames = []
        while True:
            start = self.buffer.find(MAGIC)
            if start < 0:
                self.buffer[:] = (
                    self.buffer[-1:] if self.buffer.endswith(MAGIC[:1]) else b''
                )
                break
            del self.buffer[:start]
            if len(self.buffer) < 6:
                break
            cmd, seq, length = struct.unpack_from('<BBH', self.buffer, 2)
            if length > MAX_PAYLOAD:
                del self.buffer[0]
                continue
            end = 6 + length
            if len(self.buffer) < end + 2:
                break
            if (
                crc16(self.buffer[2:end])
                != struct.unpack_from('<H', self.buffer, end)[0]
            ):
                del self.buffer[0]
                continue
            frames.append(Frame(cmd, seq, bytes(self.buffer[6:end])))
            del self.buffer[: end + 2]
        return frames


def ping():
    return Command.PING, b''


def info():
    return Command.INFO, b''


def clear(black=False):
    return Command.CLEAR, bytes([int(black)])


def img_begin():
    return Command.IMG_BEGIN, struct.pack('<HHB', 800, 480, 0)


def img_end(partial=False):
    return Command.IMG_END, bytes([int(partial)])


def sleep():
    return Command.SLEEP, b''


def reset_bootsel():
    return Command.RESET_BOOTSEL, b''


def img_data(offset, data):
    if not 0 <= offset <= 48000 or offset + len(data) > 48000 or len(data) > 4090:
        raise ValueError('image chunk out of range')
    return Command.IMG_DATA, struct.pack('<I', offset) + data


def status():
    return Command.STATUS, b''


def refresh(full=False):
    return Command.REFRESH, bytes([0 if full else 1])


def blit(x_byte, y, w_bytes, h, bits):
    if not (
        0 <= x_byte < 100
        and 0 <= y < 480
        and 0 < w_bytes <= 100 - x_byte
        and 0 < h <= 480 - y
    ):
        raise ValueError('BLIT out of bounds')
    if len(bits) != w_bytes * h or len(bits) > 4088:
        raise ValueError('BLIT length mismatch')
    return Command.BLIT, struct.pack('<HHHH', x_byte, y, w_bytes, h) + bytes(bits)


@dataclass(frozen=True)
class Status:
    busy: int
    partials_since_full: int
    ms_since_full: int
    bbox_valid: int
    x_byte0: int
    y0: int
    x_byte1: int
    y1: int


def parse_status(payload):
    if len(payload) != 16:
        raise ValueError('STATUS must contain 16 bytes')
    return Status(*struct.unpack('<BHIBHHHH', payload))
