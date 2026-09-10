"""Glyph rasterisation for the console.

Cells are rendered by FreeType at a size whose advance matches the cell width,
then thresholded to 1-bit. Rendering antialiased and thresholding, rather than
asking FreeType for a monochrome bitmap, is what makes box-drawing characters
join across cell seams: their strokes sit on fractional pixel boundaries and
monochrome rasterisation drops those below half coverage, which breaks a
horizontal rule into dashes.
"""

import os
from collections import OrderedDict

from PIL import Image, ImageChops, ImageDraw, ImageFont

# Starting guess for pixel size relative to cell width, refined per font by
# measuring the real advance. 0.5 suits the bitmap-derived default; outline
# fonts land near 0.6.
ADVANCE_RATIO = 0.5

# Glyph whose definition is to fill the character cell completely. Aligning to
# its ink is how the cell box is located without trusting line metrics, which
# Nerd Font patching inflates past the cell.
CELL_PROBE = '\u2588'

# Coverage at which a pixel counts as inked. Below half because the panel
# renders any set pixel as full black, so thin stems otherwise disappear.
THRESHOLD = 110

ENV_FONT = 'EPAPER_FONT'

# Combining sequences make the key space unbounded, so the cache is capped
# rather than left to track everything a long session ever displayed. Far
# larger than the working set of any one screen.
CACHE_ENTRIES = 4096

# Terminess is Terminus plus the Nerd Font patch: its outlines are traced from
# the original bitmaps, so at its design sizes every stem lands on a whole
# pixel. An outline font thresholded to 1 bit gives uneven stem weights.
_NAMES = (
    ('Terminess', 'TerminessNerdFontMono-Regular.ttf'),
    ('Terminess', 'TerminessNerdFont-Regular.ttf'),
    ('JetBrainsMono', 'JetBrainsMonoNerdFontMono-Regular.ttf'),
    ('JetBrainsMono', 'JetBrainsMonoNerdFont-Regular.ttf'),
)


def _roots():
    data_dirs = os.environ.get('XDG_DATA_DIRS', '/usr/share:/usr/local/share')
    yield os.path.expanduser('~/.local/share')
    yield os.path.expanduser('~/Library/Fonts')
    yield from (d for d in data_dirs.split(':') if d)
    yield '/Library/Fonts'
    yield '/System/Library/Fonts'


def find_font():
    """Locate the console font. EPAPER_FONT overrides the search."""
    override = os.environ.get(ENV_FONT)
    if override:
        if not os.path.isfile(override):
            raise ValueError(f'{ENV_FONT} is set to {override!r}, which is not a file')
        return override
    for root in _roots():
        for family, name in _NAMES:
            for path in (
                os.path.join(
                    root, 'fonts', 'truetype', 'NerdFonts', family, name
                ),
                os.path.join(root, 'fonts', name),
                os.path.join(root, name),
            ):
                if os.path.isfile(path):
                    return path
    raise ValueError(
        'no console font found; install a Nerd Font such as Terminess or point '
        f'{ENV_FONT} at a font file'
    )


class Font:
    """Rasterises cells and rows onto a fixed cell grid."""

    def __init__(self, path, cell_width, cell_height):
        self.path = path
        self.width = cell_width
        self.height = cell_height
        self.face = self._fit(path, cell_width, cell_height)
        self.baseline = self._align(cell_height)
        self._blank = Image.new('1', (cell_width, cell_height), 0)
        self._cache = OrderedDict()

    def _fit(self, path, cell_width, cell_height):
        """Largest pixel size that fills the cell without overflowing it.

        Bitmap-derived fonts render crisply only at their design size and do
        not share an outline font's advance-to-size ratio, so the size is
        measured rather than assumed. Candidates are scored on how much of the
        cell the font's own block glyph covers, which keeps full-height box
        drawing seamless across rows.
        """
        guess = cell_width / ADVANCE_RATIO
        steps = [round(guess * k / 20) * 20 / 20 for k in (0.7, 0.8, 0.9, 1.0)]
        sizes = {round(s, 2) for s in steps if s > 0}
        sizes |= {float(n) for n in range(4, int(guess * 1.6) + 1)}
        best = None
        for size in sorted(sizes):
            try:
                face = ImageFont.truetype(path, size)
            except OSError:
                # Strike-only bitmap fonts reject every size but their own.
                continue
            advance = face.getlength('M')
            if not advance or advance > cell_width:
                continue
            covered = self._probe_height(face, cell_height)
            score = (min(covered, cell_height), advance)
            if best is None or score > best[0]:
                best = (score, face)
        if best is None:
            try:
                return ImageFont.truetype(path, guess)
            except OSError as exc:
                raise ValueError(f'cannot load font {path!r}: {exc}') from exc
        return best[1]

    @staticmethod
    def _probe_height(face, cell_height):
        """Height of the font's full-block glyph in pixels."""
        canvas = Image.new('L', (cell_height * 4, cell_height * 4), 0)
        draw = ImageDraw.Draw(canvas)
        draw.fontmode = 'L'
        draw.text((0, cell_height), CELL_PROBE, font=face, fill=255)
        box = canvas.point(lambda p: 255 if p >= THRESHOLD else 0).getbbox()
        return 0 if box is None else box[3] - box[1]

    def _align(self, cell_height):
        """Offset that seats the font's own cell box on the grid."""
        probe = Image.new('L', (self.width * 3, cell_height * 3), 0)
        draw = ImageDraw.Draw(probe)
        draw.fontmode = 'L'
        draw.text((0, cell_height), CELL_PROBE, font=self.face, fill=255)
        box = probe.point(lambda p: 255 if p >= THRESHOLD else 0).getbbox()
        if box is None:
            ascent, descent = self.face.getmetrics()
            return (cell_height - (ascent + descent)) // 2
        # Seat the block's top edge on the cell's top edge so full-height box
        # drawing tiles across row boundaries with no seam.
        return cell_height - box[1]

    def cell(self, text, inverse=False):
        """The 1-bit bitmap of one cell, black as one."""
        key = (text, inverse)
        hit = self._cache.get(key)
        if hit is None:
            hit = self._cache[key] = self._render(text, inverse)
            if len(self._cache) > CACHE_ENTRIES:
                self._cache.popitem(last=False)
        else:
            self._cache.move_to_end(key)
        return hit

    def _render(self, text, inverse):
        if not text.strip():
            image = self._blank
        else:
            canvas = Image.new('L', (self.width, self.height), 0)
            draw = ImageDraw.Draw(canvas)
            draw.fontmode = 'L'
            draw.text((0, self.baseline), text, font=self.face, fill=255)
            image = canvas.point(lambda p: 255 if p >= THRESHOLD else 0, mode='1')
        return ImageChops.invert(image) if inverse else image

    def row(self, cells, width):
        """Compose one row of (text, inverse) cells into a 1-bit image."""
        strip = Image.new('1', (width, self.height), 0)
        for column, (text, inverse) in enumerate(cells):
            strip.paste(self.cell(text, inverse), (column * self.width, 0))
        return strip
