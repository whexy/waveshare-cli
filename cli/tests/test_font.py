import os
import unittest
from unittest.mock import patch

from epaper.font import ENV_FONT, Font, find_font


class FontDiscoveryTests(unittest.TestCase):
    def test_env_override_wins_and_must_exist(self):
        with patch.dict(os.environ, {ENV_FONT: '/nonexistent/font.ttf'}):
            with self.assertRaises(ValueError):
                find_font()
        with patch.dict(os.environ, {ENV_FONT: find_font()}):
            self.assertTrue(os.path.isfile(find_font()))

    def test_unreadable_font_is_reported(self):
        with self.assertRaises(ValueError):
            Font(__file__, 8, 16)


class RasterTests(unittest.TestCase):
    def setUp(self):
        self.font = Font(find_font(), 8, 16)

    def test_cell_dimensions_and_packing(self):
        cell = self.font.cell('A')
        self.assertEqual(cell.size, (8, 16))
        self.assertEqual(len(cell.tobytes()), 16)

    def test_blank_cells_have_no_ink(self):
        for text in ('', ' '):
            self.assertEqual(self.font.cell(text).tobytes(), bytes(16))

    def test_inverse_is_the_complement(self):
        plain = self.font.cell('A').tobytes()
        inverse = self.font.cell('A', inverse=True).tobytes()
        self.assertEqual(bytes(a ^ 0xFF for a in plain), inverse)

    def test_nerd_and_box_glyphs_are_distinct_and_inked(self):
        blank = self.font.cell(' ').tobytes()
        seen = {}
        for text in ('\ue0b0', '\uf09b', '\u2500', '\u2502', '\u2588', 'A'):
            bits = self.font.cell(text).tobytes()
            self.assertNotEqual(bits, blank, f'{text!r} rendered blank')
            self.assertNotIn(bits, seen, f'{text!r} collided with {seen.get(bits)!r}')
            seen[bits] = text

    def test_descenders_are_not_clipped(self):
        for text in 'gjy':
            pixels = self.font.cell(text).load()
            inked = [y for y in range(16) if any(pixels[x, y] for x in range(8))]
            self.assertGreaterEqual(max(inked), 12, f'{text!r} descender clipped')

    def test_horizontal_rule_joins_across_cells(self):
        # A dashed rule is the symptom of monochrome rasterisation; the scanline
        # carrying the rule must be solid across the seam between two cells.
        row = self.font.row([('\u2500', False)] * 4, 32)
        scanlines = [row.crop((0, y, 32, y + 1)).tobytes() for y in range(16)]
        self.assertIn(b'\xff\xff\xff\xff', scanlines)

    def test_full_block_fills_the_cell_width(self):
        row = self.font.row([('\u2588', False)] * 2, 16)
        solid = [
            y for y in range(16) if row.crop((0, y, 16, y + 1)).tobytes() == b'\xff\xff'
        ]
        self.assertGreaterEqual(len(solid), 14)

    def test_cells_are_cached_and_bounded(self):
        first = self.font.cell('A')
        self.assertIs(first, self.font.cell('A'))
        self.assertIsNot(first, self.font.cell('A', inverse=True))

    def test_cache_evicts_instead_of_growing_without_limit(self):
        from epaper.font import CACHE_ENTRIES

        for code in range(CACHE_ENTRIES + 200):
            self.font.cell(f'x{code}')
        self.assertLessEqual(len(self.font._cache), CACHE_ENTRIES)

    def test_glyphs_are_centred_in_a_tall_cell(self):
        # A 48px cell is far taller than the em box; an uncentred baseline
        # parks every line of text against the top of its row.
        font = Font(find_font(), 8, 48)
        pixels = font.cell('X').load()
        inked = [y for y in range(48) if any(pixels[x, y] for x in range(8))]
        self.assertLessEqual(abs(min(inked) - (47 - max(inked))), 1)

    def test_row_places_cells_at_the_column_pitch(self):
        row = self.font.row([(' ', False), ('A', False)], 16)
        self.assertEqual(row.crop((0, 0, 8, 16)).tobytes(), bytes(16))
        self.assertEqual(
            row.crop((8, 0, 16, 16)).tobytes(), self.font.cell('A').tobytes()
        )
