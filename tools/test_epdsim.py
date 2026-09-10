import struct
import tempfile
import unittest
from pathlib import Path
from epdsim import Simulator, FrameParser, make_frame, crc16


class SimulatorTests(unittest.TestCase):
    def test_refresh_busy_bbox(self):
        with tempfile.TemporaryDirectory() as tmp:
            now = [100.]
            sim = Simulator(Path(tmp)/'frame.png', lambda: now[0])
            def send(cmd,data=b''):
                return FrameParser().feed(sim.receive(make_frame(cmd,7,data)))[0]
            self.assertEqual(crc16(b'123456789'),0x29b1)
            self.assertEqual(send(20,b'\1')[3],b'\2')
            self.assertEqual(send(19,struct.pack('<HHHH',99,479,2,1)+b'xx')[3],b'\3')
            self.assertEqual(send(19,struct.pack('<HHHH',3,16,1,16)+b'\xff'*16)[1],0x80)
            self.assertEqual(struct.unpack('<BHIBHHHH',send(5)[3])[3:],(1,3,16,4,32))
            self.assertEqual(send(20,b'\1')[1],0x80)
            self.assertEqual(send(19,b'')[1],0x82)
            self.assertEqual(send(5)[3][0],1)
            now[0] += .6
            self.assertEqual(send(5)[3][0],0)
            self.assertTrue(sim.output.exists())
            self.assertEqual(send(20,b'\0')[1],0x80)
            now[0] += 3.9
            self.assertEqual(send(5)[3][0],1)
            now[0] += .2
            self.assertEqual(send(5)[3][0],0)


if __name__ == '__main__': unittest.main()
