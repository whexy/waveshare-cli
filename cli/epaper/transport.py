import glob
import os
import sys
import time

import serial
from serial.tools import list_ports

from .protocol import ACK, BUSY, ERROR_NAMES, NAK, Decoder, encode


class DeviceError(RuntimeError):
    pass


def detect_port():
    ports = sorted(glob.glob('/dev/cu.usbmodem*'))
    for port in list_ports.comports():
        if 'epaper' in (port.product or '').lower():
            return port.device.replace('/dev/tty.', '/dev/cu.')
    if ports:
        return ports[0]
    raise DeviceError('no USB modem found; specify --port')


class Transport:
    def __init__(self, port=None, verbose=False):
        if port is None:
            port = os.environ.get('EPAPER_PORT')
        if port == '':
            raise DeviceError('empty serial port')
        self.serial = serial.Serial(
            port if port is not None else detect_port(),
            115200,
            timeout=0.05,
            write_timeout=5,
        )
        self.verbose = verbose
        self.seq = 0
        self.decoder = Decoder()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.serial.close()

    def request(self, cmd, payload=b'', timeout=30):
        seq = self.seq
        self.seq = (seq + 1) & 255
        packet = encode(cmd, seq, payload)
        deadline = time.monotonic() + timeout
        backoff = 0.05
        resend = True
        while time.monotonic() < deadline:
            if resend:
                if self.verbose:
                    print('TX ' + packet.hex(' '), file=sys.stderr)
                if self.serial.write(packet) != len(packet):
                    raise DeviceError('short serial write')
                resend = False
            data = self.serial.read(self.serial.in_waiting or 1)
            if data and self.verbose:
                print('RX ' + data.hex(' '), file=sys.stderr)
            for frame in self.decoder.feed(data):
                if frame.seq != seq:
                    continue
                if frame.type == ACK:
                    return frame.payload
                if frame.type == NAK:
                    code = frame.payload[0] if frame.payload else 0
                    message = frame.payload[1:].decode('ascii', errors='replace')
                    raise DeviceError(
                        f'{ERROR_NAMES.get(code, "unknown error")} ({code}): {message}'
                    )
                if frame.type == BUSY:
                    time.sleep(min(backoff, max(0, deadline - time.monotonic())))
                    backoff = min(backoff * 2, 1)
                    resend = True
                    break
                raise DeviceError(f'unexpected response type 0x{frame.type:02x}')
        raise TimeoutError(f'command 0x{cmd:02x} timed out after {timeout}s')

    def bootsel(self):
        from .protocol import reset_bootsel

        try:
            self.request(*reset_bootsel(), timeout=2)
        except (serial.SerialException, OSError, TimeoutError):
            # A reset may disconnect CDC before its ACK reaches the host.
            return
