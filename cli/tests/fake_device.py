"""Run with python -m tests.fake_device; printed PTY accepts epaper --port."""
import os
import pty
import tty

from epaper.protocol import ACK, Command, Decoder, encode


def main():
    master, slave = pty.openpty()
    tty.setraw(slave)
    print(os.ttyname(slave), flush=True)
    decoder = Decoder()
    try:
        while True:
            for frame in decoder.feed(os.read(master, 8192)):
                payload = b''
                if frame.type == Command.PING:
                    payload = b'PONG'
                elif frame.type == Command.INFO:
                    payload = b'epaper-fake 0.1.0 panel=7in5_v2 w=800 h=480 mode=picture'
                packet = encode(ACK, frame.seq, payload)
                while packet:
                    packet = packet[os.write(master, packet):]
    except KeyboardInterrupt:
        pass
    finally:
        os.close(master)
        os.close(slave)


if __name__ == '__main__':
    main()
