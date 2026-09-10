import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]


class IntegrationTests(unittest.TestCase):
    def test_console_simulator(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'epdsim.png'
            sim = subprocess.Popen(
                [
                    sys.executable,
                    str(ROOT / 'tools/epdsim.py'),
                    '--output',
                    str(output),
                ],
                stdout=subprocess.PIPE,
                text=True,
            )
            try:
                port = sim.stdout.readline().strip()
                self.assertTrue(
                    port.startswith('/dev/ttys') or port.startswith('/dev/pts/'), port
                )

                def run(*args, data=None):
                    result = subprocess.run(
                        [sys.executable, '-m', 'epaper', '--port', port, *args],
                        input=data,
                        capture_output=True,
                        timeout=20,
                        env=dict(os.environ, EPAPER_PORT=port),
                    )
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    return result

                self.assertEqual(run('ping').stdout.strip(), b'PONG')
                run('text', 'hello')
                image = Image.open(output).convert('L')
                self.assertEqual(image.crop((0, 16, 800, 480)).getextrema(), (255, 255))
                self.assertEqual(image.crop((0, 0, 800, 16)).getextrema()[0], 0)
                run('console', '--stdin', data=b'first\r\nsecond')
                image = Image.open(output).convert('L')
                self.assertEqual(image.crop((0, 32, 800, 480)).getextrema(), (255, 255))
                self.assertEqual(image.crop((0, 16, 800, 32)).getextrema()[0], 0)
                run('console', '--', '/bin/sh', '-c', 'printf shell')
                run('status')
            finally:
                sim.terminate()
                sim.wait(timeout=5)
                sim.stdout.close()
