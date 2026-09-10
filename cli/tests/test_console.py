import struct
import unittest
from unittest.mock import patch

from epaper.console import Session
from epaper.protocol import Command
from epaper.transport import Transport, DeviceError


class Device:
    def __init__(self):
        self.busy = False
        self.calls = []

    def request(self, cmd, payload=b''):
        self.calls.append((cmd,payload))
        if cmd == Command.STATUS:
            return struct.pack('<BHIBHHHH',self.busy,0,0,0,0,0,0,0)
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
        self.assertFalse(self.tick_at(.059))
        self.assertEqual(self.device.calls, [])
        self.device.busy = True
        self.assertFalse(self.tick_at(.06))
        self.assertEqual(self.device.calls, [(Command.STATUS, b'')])
        self.assertTrue(self.session.initial)
        self.device.busy = False
        self.assertTrue(self.session.tick())
        blits = [data for cmd, data in self.device.calls if cmd == Command.BLIT]
        self.assertEqual(len(blits), 12)
        self.assertEqual(sum(len(data)-8 for data in blits), 48000)
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))
        self.assertEqual(self.session.units, 3)

    def test_max_pending_during_continuous_output_stays_partial(self):
        self.establish()
        self.session.units = 60
        self.session.feed(b'a')
        for now in (.05, .1, .15, .2, .249):
            self.clock.return_value = now
            self.session.feed(b'x')
            self.assertFalse(self.session.tick())
        self.assertTrue(self.tick_at(.25))
        self.assertEqual(self.device.calls[-1], (Command.REFRESH, b'\1'))

    def test_cursor_debounce(self):
        self.establish()
        self.session.feed(b'\x1b[5;5H')
        self.assertFalse(self.tick_at(.199))
        self.assertTrue(self.tick_at(.2))
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
        self.assertTrue(self.tick_at(.06))
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

    def test_bootsel_lost_ack_and_empty_port(self):
        device = Transport.__new__(Transport)
        with patch.object(device,'request',side_effect=TimeoutError):
            device.bootsel()
        with self.assertRaises(DeviceError):
            Transport('')


class TerminalModelTests(unittest.TestCase):
    def test_dsr_and_da_replies_are_returned(self):
        from epaper.term import TerminalModel
        model = TerminalModel()
        self.assertEqual(model.feed(b'\x1b[6n'), b'\x1b[1;1R')
        self.assertTrue(model.feed(b'\x1b[c').startswith(b'\x1b[?'))
        self.assertEqual(model.feed(b'\x1b[?6n'), b'')

    def test_piped_lf_returns_to_column_zero(self):
        from epaper.console import _onlcr
        model = __import__('epaper.term', fromlist=['TerminalModel']).TerminalModel()
        model.feed(_onlcr(b'ab\ncd\r\nef\n'))
        self.assertEqual([r.rstrip() for r in model.screen.display[:3]], ['ab', 'cd', 'ef'])

    def test_box_drawing_has_glyphs(self):
        from epaper.font import glyph
        self.assertNotEqual(glyph('\u2500'), glyph('?'))
        self.assertEqual(glyph('\u4e2d'), glyph('?'))
