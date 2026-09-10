from .geometry import FRAME_BYTES, PANEL_HEIGHT, PANEL_WIDTH, STRIDE
from .protocol import MAX_PAYLOAD, blit


class Renderer:
    def __init__(self, geometry, font):
        self.geometry = geometry
        self.font = font
        self.shadow = bytearray(FRAME_BYTES)
        self.cursor = None

    def render(self, model):
        geometry = self.geometry
        rows = set(model.dirty)
        cursor = model.cursor
        if cursor != self.cursor:
            if self.cursor:
                rows.add(self.cursor[1])
            rows.add(cursor[1])
        for row in rows:
            if row >= geometry.rows:
                continue
            strip = self.font.row(
                (
                    (cell.data, cell.reverse ^ (cursor == (column, row, True)))
                    for column, cell in enumerate(model.row(row))
                ),
                PANEL_WIDTH,
            )
            top = row * geometry.cell_height
            self.shadow[top * STRIDE : (top + geometry.cell_height) * STRIDE] = (
                strip.tobytes()
            )
        model.dirty.clear()
        self.cursor = cursor
        return self.shadow


def diff(previous, current, cell_height):
    spans = []
    for y in range(PANEL_HEIGHT):
        row = y * STRIDE
        if previous[row : row + STRIDE] == current[row : row + STRIDE]:
            continue
        changed = [x for x in range(STRIDE) if previous[row + x] != current[row + x]]
        spans.append((changed[0], y, changed[-1] + 1))
    if not spans:
        return [], 0
    # Expand to cell-height bands so glyph-internal blank rows do not fragment
    # a character update into many tiny SPI windows.
    bands = {}
    for x0, y, x1 in spans:
        key = y // cell_height
        a, b = bands.get(key, (STRIDE, 0))
        bands[key] = min(a, x0), max(b, x1)
    rectangles = []
    for band, (x0, x1) in sorted(bands.items()):
        y0 = band * cell_height
        y1 = min(y0 + cell_height, PANEL_HEIGHT)
        if (
            rectangles
            and rectangles[-1][3] == y0
            and x0 < rectangles[-1][2]
            and x1 > rectangles[-1][0]
        ):
            a, b, c, _ = rectangles.pop()
            rectangles.append((min(a, x0), b, max(c, x1), y1))
        else:
            rectangles.append((x0, y0, x1, y1))
    left = min(r[0] for r in rectangles)
    right = max(r[2] for r in rectangles)
    top = min(r[1] for r in rectangles)
    bottom = max(r[3] for r in rectangles)
    area = (right - left) * (bottom - top)
    units = 3 if area == FRAME_BYTES else 2 if area >= FRAME_BYTES // 2 else 1
    payloads = []
    for x0, y0, x1, y1 in rectangles:
        width = x1 - x0
        step = (MAX_PAYLOAD - 8) // width
        for y in range(y0, y1, step):
            end = min(y + step, y1)
            bits = b''.join(
                current[r * STRIDE + x0 : r * STRIDE + x1] for r in range(y, end)
            )
            payloads.append(blit(x0, y, width, end - y, bits)[1])
    return payloads, units
