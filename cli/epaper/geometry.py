"""Console grid geometry.

The panel is 800x480 and BLIT addresses x in whole bytes, so the framebuffer
stride is fixed at 100 bytes. Cell width need not divide 8: rows are rasterised
into a full-width strip and Pillow packs the bits, so a column boundary may sit
mid-byte. Only the panel dimensions are hard constraints.
"""

from dataclasses import dataclass

PANEL_WIDTH = 800
PANEL_HEIGHT = 480
STRIDE = PANEL_WIDTH // 8
FRAME_BYTES = STRIDE * PANEL_HEIGHT

DEFAULT_COLUMNS = 100
DEFAULT_ROWS = 30

# Below this the font has too few pixels per stem to stay legible on a panel
# with no greyscale.
MIN_CELL_WIDTH = 5
MIN_CELL_HEIGHT = 10


@dataclass(frozen=True)
class Geometry:
    columns: int
    rows: int
    cell_width: int
    cell_height: int

    @property
    def width(self):
        return PANEL_WIDTH

    @property
    def height(self):
        return PANEL_HEIGHT

    @property
    def used_height(self):
        """Rows of pixels the grid covers; any remainder stays blank."""
        return self.rows * self.cell_height


def geometry(columns=DEFAULT_COLUMNS, rows=DEFAULT_ROWS):
    """Validate a terminal size and derive its cell box."""
    if columns < 1 or rows < 1:
        raise ValueError('terminal size must be positive')
    cell_width = PANEL_WIDTH // columns
    cell_height = PANEL_HEIGHT // rows
    if cell_width < MIN_CELL_WIDTH or cell_height < MIN_CELL_HEIGHT:
        raise ValueError(
            f'{columns}x{rows} needs a {cell_width}x{cell_height} cell, below the '
            f'legible minimum of {MIN_CELL_WIDTH}x{MIN_CELL_HEIGHT}; '
            f'at most {PANEL_WIDTH // MIN_CELL_WIDTH}x'
            f'{PANEL_HEIGHT // MIN_CELL_HEIGHT} fits'
        )
    return Geometry(columns, rows, cell_width, cell_height)


def parse_size(text):
    """Parse a COLSxROWS option value."""
    parts = text.lower().split('x')
    if len(parts) != 2 or not all(p.strip().isdigit() for p in parts):
        raise ValueError(f'invalid terminal size {text!r}, expected COLSxROWS')
    return geometry(int(parts[0]), int(parts[1]))
