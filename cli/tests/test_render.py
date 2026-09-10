import struct
import unittest

from epaper.render import Renderer, diff
from epaper.term import TerminalModel


class RenderTests(unittest.TestCase):
    def test_single_cell(self):
        model = TerminalModel()
        model.feed(b'\x1b[?25l')
        renderer = Renderer()
        before = bytes(renderer.render(model))
        model.feed(b'A')
        payloads, units = diff(before, renderer.render(model))
        self.assertEqual(len(payloads), 1)
        self.assertEqual(struct.unpack_from('<HHHH', payloads[0]), (0, 0, 1, 16))
        self.assertEqual(units, 1)

    def test_scroll_and_units(self):
        model = TerminalModel()
        model.feed(b'\x1b[?25l')
        model.feed(b'\r\n'.join(bytes([65 + r % 2]) * 100 for r in range(30)))
        renderer = Renderer()
        before = bytes(renderer.render(model))
        model.feed(b'\r\n' + b'C' * 100)
        payloads, units = diff(before, renderer.render(model))
        self.assertEqual(units, 3)
        self.assertTrue(all(len(p) <= 4096 for p in payloads))
        self.assertEqual(sum(struct.unpack_from('<HHHH', p)[3] for p in payloads), 480)
        self.assertEqual(diff(bytes(48000), b'\xff' * 24000 + bytes(24000))[1], 2)

    def test_reverse_and_cursor(self):
        model = TerminalModel()
        renderer = Renderer()
        self.assertEqual(renderer.render(model)[:100], b'\xff' + bytes(99))
        model.feed(b'\x1b[?25l\x1b[7m ')
        self.assertEqual(renderer.render(model)[0], 255)
        model.feed(b'\x1b[2J')
        self.assertTrue(model.reset_requested)
