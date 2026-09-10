from .font import glyph
from .protocol import blit


class Renderer:
    def __init__(self):
        self.shadow = bytearray(48000)
        self.cursor = None

    def render(self, model):
        rows = set(model.dirty)
        cursor = model.cursor
        if cursor != self.cursor:
            if self.cursor:
                rows.add(self.cursor[1])
            rows.add(cursor[1])
        for row in rows:
            for col in range(100):
                cell = model.cell(row, col)
                inverse = cell.reverse ^ (cursor == (col, row, True))
                for dy, bits in enumerate(glyph(cell.data)):
                    self.shadow[(row * 16 + dy) * 100 + col] = bits ^ (255 if inverse else 0)
        model.dirty.clear()
        self.cursor = cursor
        return self.shadow


def diff(previous, current):
    spans = []
    for y in range(480):
        changed = [x for x in range(100) if previous[y*100+x] != current[y*100+x]]
        if changed:
            spans.append((changed[0], y, changed[-1]+1, y+1))
    if not spans:
        return [], 0
    # Expand to cell-height bands so glyph-internal blank rows do not fragment
    # a character update into many tiny SPI windows.
    bands = {}
    for x0, y, x1, _ in spans:
        key = y // 16
        a, b = bands.get(key, (100, 0))
        bands[key] = min(a, x0), max(b, x1)
    rectangles = []
    for band, (x0, x1) in sorted(bands.items()):
        y0, y1 = band*16, (band+1)*16
        if rectangles and rectangles[-1][3] == y0 and x0 < rectangles[-1][2] and x1 > rectangles[-1][0]:
            a, b, c, _ = rectangles.pop()
            rectangles.append((min(a,x0), b, max(c,x1), y1))
        else:
            rectangles.append((x0,y0,x1,y1))
    left = min(r[0] for r in rectangles); right = max(r[2] for r in rectangles)
    top = min(r[1] for r in rectangles); bottom = max(r[3] for r in rectangles)
    area = (right-left)*(bottom-top)
    units = 3 if area == 48000 else 2 if area >= 24000 else 1
    payloads = []
    for x0,y0,x1,y1 in rectangles:
        width = x1-x0
        step = 4088//width
        for y in range(y0,y1,step):
            end = min(y+step,y1)
            bits = b''.join(current[r*100+x0:r*100+x1] for r in range(y,end))
            payloads.append(blit(x0,y,width,end-y,bits)[1])
    return payloads, units
