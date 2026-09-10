import struct
import unittest

from epaper.font import Font, find_font
from epaper.geometry import FRAME_BYTES, STRIDE, geometry
from epaper.protocol import MAX_PAYLOAD, blit
from epaper.render import Renderer, diff
from epaper.term import TerminalModel

# Grids whose cell height does not divide 480 (28 -> 17, 19 -> 25) exercise the
# band clipped against the panel edge.
GRIDS = ((100, 30), (90, 28), (107, 30), (50, 15), (80, 24), (66, 19), (160, 48))


def session(columns=100, rows=30):
    grid = geometry(columns, rows)
    font = Font(find_font(), grid.cell_width, grid.cell_height)
    return grid, TerminalModel(grid), Renderer(grid, font)


class RenderTests(unittest.TestCase):
    def test_single_cell(self):
        grid, model, renderer = session()
        model.feed(b'\x1b[?25l')
        before = bytes(renderer.render(model))
        model.feed(b'A')
        payloads, units = diff(before, renderer.render(model), grid.cell_height)
        self.assertEqual(len(payloads), 1)
        self.assertEqual(struct.unpack_from('<HHHH', payloads[0]), (0, 0, 1, 16))
        self.assertEqual(units, 1)

    def test_scroll_and_units(self):
        grid, model, renderer = session()
        model.feed(b'\x1b[?25l')
        model.feed(b'\r\n'.join(bytes([65 + r % 2]) * 100 for r in range(30)))
        before = bytes(renderer.render(model))
        model.feed(b'\r\n' + b'C' * 100)
        payloads, units = diff(before, renderer.render(model), grid.cell_height)
        self.assertEqual(units, 3)
        self.assertTrue(all(len(p) <= 4096 for p in payloads))
        self.assertEqual(sum(struct.unpack_from('<HHHH', p)[3] for p in payloads), 480)
        half = b'\xff' * (FRAME_BYTES // 2) + bytes(FRAME_BYTES // 2)
        self.assertEqual(diff(bytes(FRAME_BYTES), half, 16)[1], 2)

    def test_reverse_and_cursor(self):
        grid, model, renderer = session()
        # The cursor block inverts its cell, so the first byte is fully inked.
        self.assertEqual(renderer.render(model)[0], 255)
        model.feed(b'\x1b[?25l\x1b[7m ')
        self.assertEqual(renderer.render(model)[0], 255)
        model.feed(b'\x1b[2J')
        self.assertTrue(model.reset_requested)

    def test_blank_screen_has_no_ink(self):
        _, model, renderer = session()
        model.feed(b'\x1b[?25l')
        self.assertEqual(bytes(renderer.render(model)), bytes(FRAME_BYTES))


class GeometryTests(unittest.TestCase):
    def test_last_row_inks_its_band_and_never_the_remainder(self):
        # 28 and 19 rows leave 4 and 5 pixel rows spare below the grid; a cell
        # that overflowed its band would show up there.
        for columns, rows in GRIDS:
            with self.subTest(size=(columns, rows)):
                grid, model, renderer = session(columns, rows)
                # Reverse video fills the whole cell box, so the last grid row
                # is inked edge to edge and any bleed is unambiguous.
                model.feed(b'\x1b[?25l\x1b[%d;1H\x1b[7m' % rows + b'W' * columns)
                shadow = bytes(renderer.render(model))
                self.assertEqual(len(shadow), FRAME_BYTES)
                last = (grid.used_height - 1) * STRIDE
                self.assertNotEqual(
                    shadow[last : last + STRIDE],
                    bytes(STRIDE),
                    'last grid row rendered blank',
                )
                tail = grid.used_height * STRIDE
                self.assertEqual(
                    shadow[tail:],
                    bytes(FRAME_BYTES - tail),
                    'ink bled past the last grid row',
                )

    def test_replaying_blits_reproduces_the_frame_exactly(self):
        # Payload shape and coverage cannot show that a rectangle carries the
        # right pixels at the right place; replaying onto the previous frame
        # and comparing against the target does.
        for columns, rows in GRIDS:
            with self.subTest(size=(columns, rows)):
                grid, model, renderer = session(columns, rows)
                model.feed(b'\x1b[?25l')
                before = bytes(renderer.render(model))
                model.feed(
                    '\x1b[7mhead\x1b[0m \u2500\u252c\u2500 \ue0b0 tail\r\n'.encode()
                )
                model.feed(b'\r\n'.join(b'x' * columns for _ in range(rows)))
                current = bytes(renderer.render(model))
                payloads, _ = diff(before, current, grid.cell_height)
                replayed = bytearray(before)
                for payload in payloads:
                    x0, y0, w, h = struct.unpack_from('<HHHH', payload)
                    body = payload[8:]
                    for line in range(h):
                        start = (y0 + line) * STRIDE + x0
                        replayed[start : start + w] = body[line * w : (line + 1) * w]
                self.assertEqual(bytes(replayed), current)

    def test_blits_are_well_formed_and_cover_every_change(self):
        for columns, rows in GRIDS:
            with self.subTest(size=(columns, rows)):
                grid, model, renderer = session(columns, rows)
                model.feed(b'\x1b[?25l')
                before = bytes(renderer.render(model))
                model.feed(b'\r\n'.join(b'W' * columns for _ in range(rows)))
                current = bytes(renderer.render(model))
                payloads, _ = diff(before, current, grid.cell_height)
                covered = set()
                for payload in payloads:
                    x0, y0, w, h = struct.unpack_from('<HHHH', payload)
                    self.assertEqual(len(payload) - 8, w * h)
                    self.assertLessEqual(len(payload), MAX_PAYLOAD)
                    self.assertLessEqual(x0 + w, STRIDE)
                    self.assertLessEqual(y0 + h, 480)
                    covered.update(
                        (x, y) for y in range(y0, y0 + h) for x in range(x0, x0 + w)
                    )
                changed = {
                    (x, y)
                    for y in range(480)
                    for x in range(STRIDE)
                    if before[y * STRIDE + x] != current[y * STRIDE + x]
                }
                self.assertEqual(changed - covered, set())

    def test_band_crossing_the_panel_edge_stays_in_bounds(self):
        # A cell height that does not divide 480 puts the last band past the
        # panel; unclipped it becomes a BLIT the device rejects outright.
        current = bytearray(FRAME_BYTES)
        current[479 * STRIDE] = 0xFF
        payloads, _ = diff(bytes(FRAME_BYTES), bytes(current), 17)
        self.assertTrue(payloads)
        for payload in payloads:
            x0, y0, w, h = struct.unpack_from('<HHHH', payload)
            self.assertLessEqual(y0 + h, 480)
            # blit() enforces the wire contract, so reconstructing must not raise.
            blit(x0, y0, w, h, payload[8:])

    def test_tall_narrow_rectangles_respect_the_payload_limit(self):
        # A full-height column is the worst case for chunking: the header has
        # to be counted against the 4096-byte budget, not just the pixels.
        for width in (1, 7, 9, 37, 100):
            with self.subTest(width=width):
                current = bytearray(FRAME_BYTES)
                for y in range(480):
                    start = y * STRIDE
                    current[start : start + width] = b'\xff' * width
                payloads, _ = diff(bytes(FRAME_BYTES), bytes(current), 16)
                for payload in payloads:
                    self.assertLessEqual(len(payload), MAX_PAYLOAD)
                    x0, y0, w, h = struct.unpack_from('<HHHH', payload)
                    blit(x0, y0, w, h, payload[8:])

    def test_rejects_illegible_and_malformed_sizes(self):
        from epaper.geometry import parse_size

        for bad in ('200x30', '100x60', 'wide', '100', '100x', 'x30'):
            with self.subTest(size=bad), self.assertRaises(ValueError):
                parse_size(bad)
        self.assertEqual(parse_size('80x24').cell_width, 10)
