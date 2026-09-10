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

# Whether Python accepts or rejects each boundary, so the Rust port can be
# checked for matching rejection and not only for matching bytes.
boundaries = []


def boundary(name, call):
    try:
        call()
    except ValueError:
        boundaries.append({'name': name, 'accepted': False})
    else:
        boundaries.append({'name': name, 'accepted': True})


boundary('encode_payload_4096', lambda: p.encode(1, 0, b'x' * 4096))
boundary('encode_payload_4097', lambda: p.encode(1, 0, b'x' * 4097))
boundary('img_data_4090', lambda: p.img_data(0, b'x' * 4090))
boundary('img_data_4091', lambda: p.img_data(0, b'x' * 4091))
boundary('img_data_end_exact', lambda: p.img_data(48000 - 10, b'x' * 10))
boundary('img_data_end_over', lambda: p.img_data(48000 - 10, b'x' * 11))
boundary('img_data_offset_48000', lambda: p.img_data(48000, b''))
boundary('blit_full_width', lambda: p.blit(0, 0, 100, 40, b'z' * 4000))
boundary('blit_x_at_edge', lambda: p.blit(99, 0, 1, 1, b'z'))
boundary('blit_x_past_edge', lambda: p.blit(100, 0, 1, 1, b'z'))
boundary('blit_width_overruns', lambda: p.blit(99, 0, 2, 1, b'zz'))
boundary('blit_y_at_edge', lambda: p.blit(0, 479, 1, 1, b'z'))
boundary('blit_y_past_edge', lambda: p.blit(0, 480, 1, 1, b'z'))
boundary('blit_height_overruns', lambda: p.blit(0, 479, 1, 2, b'zz'))
boundary('blit_zero_width', lambda: p.blit(0, 0, 0, 1, b''))
boundary('blit_zero_height', lambda: p.blit(0, 0, 1, 0, b''))
boundary('blit_body_too_short', lambda: p.blit(0, 0, 2, 2, b'zzz'))
boundary('blit_body_too_long', lambda: p.blit(0, 0, 2, 2, b'zzzzz'))
boundary('blit_body_at_limit', lambda: p.blit(0, 0, 1, 4088, b'z' * 4088))
boundary('blit_body_past_limit', lambda: p.blit(0, 0, 1, 4089, b'z' * 4089))
boundary('status_15_bytes', lambda: p.parse_status(b'x' * 15))
boundary('status_16_bytes', lambda: p.parse_status(b'x' * 16))
boundary('status_17_bytes', lambda: p.parse_status(b'x' * 17))

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
            'boundaries': boundaries,
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
