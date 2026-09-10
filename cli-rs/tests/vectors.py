"""Regenerate vectors.json from the Python CLI.

The Rust port is checked against these bytes rather than against a second
reading of the spec, so the two implementations cannot drift apart silently.

    python3 cli-rs/tests/vectors.py > cli-rs/tests/vectors.json
"""

import json
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'cli'))

from epaper import protocol as p

cases = []


def add(name, data):
    cases.append({'name': name, 'hex': data.hex()})


add('encode_ping', p.encode(p.Command.PING, 0, b''))
add('encode_blit_seq255', p.encode(p.Command.BLIT, 255, bytes(range(256)) * 4))
add('encode_status_seq7', p.encode(p.Command.STATUS, 7, b''))
add('encode_maxpayload', p.encode(p.Command.IMG_DATA, 3, b'\xa5' * 4096))

for name, ctor in [
    ('ping', p.ping()),
    ('info', p.info()),
    ('status', p.status()),
    ('sleep', p.sleep()),
    ('bootsel', p.reset_bootsel()),
    ('clear_white', p.clear(False)),
    ('clear_black', p.clear(True)),
    ('refresh_full', p.refresh(True)),
    ('refresh_partial', p.refresh(False)),
    ('img_begin', p.img_begin()),
    ('img_end_full', p.img_end(False)),
    ('img_end_partial', p.img_end(True)),
]:
    add('ctor_' + name, bytes([ctor[0]]) + ctor[1])

add(
    'ctor_img_data',
    bytes([p.img_data(4090, b'z' * 100)[0]]) + p.img_data(4090, b'z' * 100)[1],
)
add(
    'ctor_blit',
    bytes([p.blit(3, 32, 4, 8, bytes(range(32)))[0]])
    + p.blit(3, 32, 4, 8, bytes(range(32)))[1],
)

crcs = {
    '123456789': p.crc16(b'123456789'),
    'empty': p.crc16(b''),
    'eb90': p.crc16(b'\xeb\x90'),
    'long': p.crc16(bytes(range(256)) * 8),
}

status_payload = struct.pack('<BHIBHHHH', 1, 700, 123456, 1, 5, 48, 90, 464)
status = p.parse_status(status_payload)

print(
    json.dumps(
        {
            'cases': cases,
            'crcs': crcs,
            'status_hex': status_payload.hex(),
            'status_fields': [
                status.busy,
                status.partials_since_full,
                status.ms_since_full,
                status.bbox_valid,
                status.x_byte0,
                status.y0,
                status.x_byte1,
                status.y1,
            ],
        }
    )
)
