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
    def test_batch_busy_and_budget(self):
        with patch('epaper.console.time.monotonic') as clock:
            clock.return_value = 0
            device = Device(); session = Session(device)
            session.feed(b'hello')
            clock.return_value = .099
            self.assertFalse(session.tick())
            self.assertEqual(device.calls,[])
            clock.return_value = .1
            device.busy = True
            self.assertFalse(session.tick())
            self.assertEqual([c for c,_ in device.calls],[Command.STATUS])
            device.busy = False
            self.assertTrue(session.tick())
            self.assertEqual(device.calls[-1],(Command.REFRESH,b'\1'))
            self.assertEqual(session.units,3)
            session.feed(b'\x1b[5;5H')
            clock.return_value = .399
            self.assertFalse(session.tick())
            clock.return_value = .401
            self.assertTrue(session.tick())
            session.units = 10
            session.feed(b'x')
            clock.return_value = .6
            self.assertTrue(session.tick())
            self.assertEqual(device.calls[-1],(Command.REFRESH,b'\0'))
            self.assertEqual(session.units,0)
            session.units = 3
            clock.return_value = 31
            self.assertTrue(session.tick())
            self.assertEqual(device.calls[-1],(Command.REFRESH,b'\0'))

    def test_reset_forces_full_with_budget(self):
        device = Device(); session = Session(device)
        session.tick(final=True)
        session.feed(b'\x1b[2J')
        session.tick(final=True)
        self.assertEqual(device.calls[-1],(Command.REFRESH,b'\0'))

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
