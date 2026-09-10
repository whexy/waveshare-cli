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

# Nerd Font patched JetBrains Mono advances 600/1000 upem. Deriving the pixel
# size from the cell width keeps the advance integral and the grid exact.
ADVANCE_RATIO = 0.6

# Coverage at which a pixel counts as inked. Below half because the panel
# renders any set pixel as full black, so thin stems otherwise disappear.
THRESHOLD = 110

ENV_FONT = 'EPAPER_FONT'

# Combining sequences make the key space unbounded, so the cache is capped
# rather than left to track everything a long session ever displayed. Far
# larger than the working set of any one screen.
CACHE_ENTRIES = 4096

_NAMES = (
    'JetBrainsMonoNerdFontMono-Regular.ttf',
    'JetBrainsMonoNLNerdFontMono-Regular.ttf',
    'JetBrainsMonoNerdFont-Regular.ttf',
)

_NERD_SUBDIR = ('fonts', 'truetype', 'NerdFonts', 'JetBrainsMono')


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
        for name in _NAMES:
            for path in (
                os.path.join(root, *_NERD_SUBDIR, name),
                os.path.join(root, 'fonts', name),
                os.path.join(root, name),
            ):
                if os.path.isfile(path):
                    return path
    raise ValueError(
        'no console font found; install a JetBrains Mono Nerd Font or point '
        f'{ENV_FONT} at a font file'
    )


class Font:
    """Rasterises cells and rows onto a fixed cell grid."""

    def __init__(self, path, cell_width, cell_height):
        self.path = path
        self.width = cell_width
        self.height = cell_height
        try:
            self.face = ImageFont.truetype(path, cell_width / ADVANCE_RATIO)
        except OSError as exc:
            raise ValueError(f'cannot load font {path!r}: {exc}') from exc
        ascent, descent = self.face.getmetrics()
        # Centre the em box in the cell. Glyphs that overflow are clipped to
        # the cell, which is exactly what full-height box drawing relies on.
        self.baseline = (cell_height - (ascent + descent)) // 2
        self._blank = Image.new('1', (cell_width, cell_height), 0)
        self._cache = OrderedDict()

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
