import struct
import tempfile
import unittest
from pathlib import Path

from epdsim import ACK, Console, FrameParser, Simulator, crc16, make_frame


class ConsoleTest(unittest.TestCase):
    def test_ls_style_output_and_controls(self):
        console = Console()
        console.feed(b"total 8\n-rw-r--r--  1 user staff 12 file.txt\n")
        self.assertEqual("".join(console.grid[0][:7]), "total 8")
        self.assertEqual("".join(console.grid[1][:10]), "-rw-r--r--")

        console.feed(b"abc\bZ\ncol1\tcol2")
        self.assertEqual("".join(console.grid[2][:3]), "abZ")
        self.assertEqual("".join(console.grid[3][8:12]), "col2")

    def test_csi_clear_home_and_cursor_moves(self):
        console = Console()
        console.feed(b"garbage\x1b[2J\x1b[Hhome\x1b[3;6HX\x1b[2DY")
        self.assertEqual("".join(console.grid[0][:4]), "home")
        self.assertEqual(console.grid[2][5], "X")
        self.assertEqual(console.grid[2][4], "Y")
        self.assertTrue(all(char == " " for char in console.grid[1]))

    def test_scroll_past_thirty_rows(self):
        console = Console()
        for row in range(35):
            console.feed(f"line-{row:02d}\n".encode())
        self.assertEqual("".join(console.grid[0][:7]), "line-06")
        self.assertEqual("".join(console.grid[28][:7]), "line-34")
        self.assertTrue(console.force_full)

    def test_osc_and_unknown_csi_are_consumed(self):
        console = Console()
        console.feed(b"A\x1b]0;title\x07B\x1b[?25lC\x1b[31mD")
        self.assertEqual("".join(console.grid[0][:4]), "ABCD")


class ProtocolTest(unittest.TestCase):
    def test_crc_known_vector(self):
        self.assertEqual(crc16(b"123456789"), 0x29B1)

    def test_parser_handles_fragmentation_and_resync(self):
        frame = make_frame(1, 7)
        parser = FrameParser()
        self.assertEqual(parser.feed(b"junk" + frame[:4]), [])
        events = parser.feed(frame[4:])
        self.assertEqual(events, [("frame", 1, 7, b"")])

    def test_ping_and_image_render(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screen.png"
            simulator = Simulator(path)
            response = simulator.receive(make_frame(1, 9))
            parsed = FrameParser().feed(response)
            self.assertEqual(parsed, [("frame", ACK, 9, b"PONG")])

            self.assertEqual(FrameParser().feed(simulator.receive(make_frame(0x10, 1, struct.pack("<HHB", 800, 480, 0))))[0][1], ACK)
            image = bytes([0x80]) + bytes(47999)
            for offset in range(0, len(image), 4090):
                payload = struct.pack("<I", offset) + image[offset : offset + 4090]
                simulator.receive(make_frame(0x11, 2, payload))
            simulator.receive(make_frame(0x12, 3, b"\x00"))
            self.assertTrue(path.exists())
            self.assertEqual(path.stat().st_size > 0, True)

    def test_console_idle_refresh(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "console.png"
            simulator = Simulator(path)
            simulator.receive(make_frame(0x20, 1, b"hello"))
            self.assertFalse(path.exists())
            self.assertTrue(simulator.console.refresh_due(simulator.console.last_input + 0.251))
            simulator.render_console()
            self.assertTrue(path.exists())


if __name__ == "__main__":
    unittest.main()
