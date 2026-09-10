import pyte


class _Screen(pyte.Screen):
    reset_requested = False
    replies = None

    def write_process_input(self, data):
        if self.replies is not None:
            self.replies.append(data.encode('latin-1'))

    # pyte dispatches private-mode CSI variants with private=True and its
    # own report_device_status does not accept that; DEC private DSRs are
    # rare and safe to ignore.
    def report_device_status(self, mode, **kwargs):
        if kwargs.get('private'):
            return
        super().report_device_status(mode)

    def erase_in_display(self, how=0, *args, **kwargs):
        if how == 2:
            self.reset_requested = True
        super().erase_in_display(how, *args, **kwargs)

    def reset(self):
        super().reset()
        self.reset_requested = True


class TerminalModel:
    def __init__(self, geometry):
        self.geometry = geometry
        self.screen = _Screen(geometry.columns, geometry.rows)
        self.stream = pyte.ByteStream(self.screen)
        self.screen.reset_requested = False
        self.screen.replies = []

    def feed(self, data):
        """Feed pty output; returns bytes the application expects back on its
        input (DSR/DA replies), which the caller must write to the pty."""
        self.stream.feed(data)
        replies = b''.join(self.screen.replies)
        self.screen.replies.clear()
        return replies

    @property
    def dirty(self):
        return self.screen.dirty

    @property
    def cursor(self):
        c = self.screen.cursor
        return min(c.x, self.geometry.columns - 1), c.y, not c.hidden

    def cell(self, row, col):
        return self.screen.buffer[row][col]

    def row(self, row):
        """Every cell of a row in column order, including untouched ones.

        pyte stores lines sparsely, so indexing the defaultdict is what
        materialises the default cell for columns never written to."""
        line = self.screen.buffer[row]
        return [line[column] for column in range(self.geometry.columns)]

    @property
    def reset_requested(self):
        return self.screen.reset_requested

    def clear_reset(self):
        self.screen.reset_requested = False
