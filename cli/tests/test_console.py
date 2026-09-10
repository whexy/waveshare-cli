import struct
import unittest
from unittest.mock import patch

from epaper.console import Session
from epaper.protocol import Command
from epaper.transport import DeviceError, Transport


class Device:
    def __init__(self):
        self.busy = False
        self.calls = []

    def request(self, cmd, payload=b''):
        self.calls.append((cmd, payload))
        if cmd == Command.STATUS:
            return struct.pack('<BHIBHHHH', self.busy, 0, 0, 0, 0, 0, 0, 0)
        return b''


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.clock = patch('epaper.console.time.monotonic').start()
        self.addCleanup(patch.stopall)
        self.clock.return_value = 0
        self.device = Device()
        self.session = Session(self.device)

    def tick_at(self, now, **kwargs):
        self.clock.return_value = now
        return self.session.tick(**kwargs)

    def establish(self):
        self.session.tick(final=True)
        self.device.calls.clear()

    def test_idle_busy_and_initial_shadow(self):
        self.session.feed(b'hello')
        self.assertFalse(self.tick_at(0.059))
        self.assertEqual(self.device.calls, [])
        self.device.busy = True
        self.assertFalse(self.tick_at(0.06))
        self.assertEqual(self.device.calls, [(Command.STATUS, b'')])
        self.assertTrue(self.session.initial)
        self.device.busy = False
        self.assertTrue(self.session.tick())
        blits = [data for cmd, data in self.device.calls if cmd == Command.BLIT]
        self.assertEqual(len(blits), 12)
        self.assertEqual(sum(len(data) - 8 for data in blits), 48000)
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))
        self.assertEqual(self.session.units, 3)

    def test_max_pending_during_continuous_output_stays_partial(self):
        self.establish()
        self.session.units = 60
        self.session.feed(b'a')
        for now in (0.05, 0.1, 0.15, 0.2, 0.249):
            self.clock.return_value = now
            self.session.feed(b'x')
            self.assertFalse(self.session.tick())
        self.assertTrue(self.tick_at(0.25))
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))

    def test_cursor_debounce(self):
        self.establish()
        self.session.feed(b'\x1b[5;5H')
        self.assertFalse(self.tick_at(0.199))
        self.assertTrue(self.tick_at(0.2))
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))

    def test_idle_full_thresholds_without_pending_output(self):
        for units, before, at in ((60, 1.999, 2), (10, 29.999, 30)):
            with self.subTest(units=units):
                self.clock.return_value = 0
                self.session = Session(self.device)
                self.establish()
                self.session.units = units
                self.assertFalse(self.tick_at(before))
                self.device.busy = True
                self.assertFalse(self.tick_at(at))
                self.assertEqual(self.session.units, units)
                self.device.busy = False
                self.assertTrue(self.session.tick())
                self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\0'))
                self.assertEqual(self.session.units, 0)

    def test_below_idle_budgets_does_not_refresh(self):
        self.establish()
        for units, now in ((59, 2), (9, 30)):
            self.session.units = units
            self.assertFalse(self.tick_at(now))
        self.assertEqual(self.device.calls, [])

    def test_reset_intent_survives_partial_until_idle(self):
        self.establish()
        self.session.units = 10
        self.session.feed(b'\x1b[2Jhello')
        self.assertTrue(self.tick_at(0.06))
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))
        self.assertTrue(self.session.model.reset_requested)
        self.assertFalse(self.tick_at(1.999))
        self.assertTrue(self.tick_at(2))
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\0'))
        self.assertFalse(self.session.model.reset_requested)

    def test_reset_below_budget_and_final_threshold(self):
        self.establish()
        self.session.feed(b'\x1b[2J')
        self.session.units = 9
        self.assertTrue(self.tick_at(2))
        self.assertTrue(self.session.model.reset_requested)
        self.assertNotIn((Command.REFRESH, b'\0'), self.device.calls)
        self.session.model.clear_reset()
        self.session.units = 29
        self.device.calls.clear()
        self.session.tick(final=True)
        self.assertNotIn((Command.REFRESH, b'\0'), self.device.calls)
        self.session.units = 30
        self.session.tick(final=True)
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\0'))

    def test_initial_upload_tiles_the_panel_exactly(self):
        from epaper.geometry import STRIDE, geometry

        for columns, rows in ((100, 30), (90, 28), (66, 19), (160, 48)):
            with self.subTest(size=(columns, rows)):
                device = Device()
                session = Session(device, geometry(columns, rows))
                session.feed(b'\x1b[?25lhello')
                session.tick(final=True)
                seen = {}
                for cmd, payload in device.calls:
                    if cmd != Command.BLIT:
                        continue
                    x0, y0, w, h = struct.unpack_from('<HHHH', payload)
                    self.assertEqual((x0, w), (0, STRIDE))
                    self.assertEqual(len(payload) - 8, w * h)
                    self.assertLessEqual(len(payload), 4096)
                    for y in range(y0, y0 + h):
                        self.assertNotIn(y, seen, f'row {y} uploaded twice')
                        seen[y] = True
                self.assertEqual(sorted(seen), list(range(480)))

    def test_bootsel_lost_ack_and_empty_port(self):
        device = Transport.__new__(Transport)
        with patch.object(device, 'request', side_effect=TimeoutError):
            device.bootsel()
        with self.assertRaises(DeviceError):
            Transport('')


class TerminalModelTests(unittest.TestCase):
    def test_dsr_and_da_replies_are_returned(self):
        from epaper.geometry import geometry
        from epaper.term import TerminalModel

        model = TerminalModel(geometry())
        self.assertEqual(model.feed(b'\x1b[6n'), b'\x1b[1;1R')
        self.assertTrue(model.feed(b'\x1b[c').startswith(b'\x1b[?'))
        self.assertEqual(model.feed(b'\x1b[?6n'), b'')

    def test_piped_lf_returns_to_column_zero(self):
        from epaper.console import _onlcr
        from epaper.geometry import geometry
        from epaper.term import TerminalModel

        model = TerminalModel(geometry())
        model.feed(_onlcr(b'ab\ncd\r\nef\n'))
        self.assertEqual(
            [r.rstrip() for r in model.screen.display[:3]], ['ab', 'cd', 'ef']
        )

    def test_piped_lf_uses_the_configured_width(self):
        from epaper.console import _onlcr
        from epaper.geometry import geometry
        from epaper.term import TerminalModel

        model = TerminalModel(geometry(80, 24))
        model.feed(_onlcr(b'x' * 100))
        self.assertEqual(len(model.row(0)), 80)
        self.assertEqual(model.screen.display[1].rstrip(), 'x' * 20)
