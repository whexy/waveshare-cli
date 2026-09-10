import struct
import unittest

from epaper import protocol as p


class ProtocolTests(unittest.TestCase):
    def test_crc_vector(self):
        self.assertEqual(p.crc16(b'123456789'), 0x29B1)
        self.assertEqual(p.crc16(b''), 0xFFFF)

    def test_roundtrip(self):
        payload = bytes(range(256)) * 16
        frame = p.encode(p.Command.BLIT, 255, payload)
        decoder = p.Decoder()
        result = []
        for byte in frame:
            result += decoder.feed(bytes([byte]))
        self.assertEqual(result, [p.Frame(0x13, 255, payload)])

    def test_resync(self):
        valid = p.encode(1, 2)
        damaged = bytearray(valid)
        damaged[-1] ^= 1
        oversized = p.MAGIC + struct.pack('<BBH', 1, 1, 4097)
        decoder = p.Decoder()
        self.assertEqual(decoder.feed(b'garbage\xeb'), [])
        self.assertEqual(
            decoder.feed(b'junk' + damaged + oversized + valid), [p.Frame(1, 2, b'')]
        )

    def test_multiple(self):
        self.assertEqual(len(p.Decoder().feed(p.encode(1, 0) * 2)), 2)

    def test_blit_status(self):
        self.assertEqual(
            p.blit(2, 16, 1, 16, b'0' * 16)[1],
            struct.pack('<HHHH', 2, 16, 1, 16) + b'0' * 16,
        )
        state = p.parse_status(struct.pack('<BHIBHHHH', 1, 2, 300, 1, 2, 16, 3, 32))
        self.assertEqual((state.busy, state.y1), (1, 32))
        with self.assertRaises(ValueError):
            p.blit(99, 0, 2, 1, b'xx')

    def test_limits(self):
        with self.assertRaises(ValueError):
            p.encode(1, 0, b'x' * 4097)
        self.assertEqual(len(p.img_data(0, b'x' * 4090)[1]), 4094)
        with self.assertRaises(ValueError):
            p.img_data(47999, b'xx')


if __name__ == '__main__':
    unittest.main()
