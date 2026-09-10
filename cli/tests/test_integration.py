import subprocess
import sys
import unittest


class FakeDeviceTests(unittest.TestCase):
    def test_cli(self):
        fake = subprocess.Popen([sys.executable, '-m', 'tests.fake_device'], stdout=subprocess.PIPE, text=True)
        try:
            port = fake.stdout.readline().strip()
            def run(*args, input=None):
                return subprocess.run([sys.executable, '-m', 'epaper', '--port', port, *args],
                                      input=input, text=True, capture_output=True, timeout=15)
            result = run('ping')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), 'PONG')
            for args in [('info',), ('draw', '--testcard'), ('console', '--stdin'),
                         ('console', '--echo', '--', '/bin/sh', '-c', 'printf hello')]:
                result = run(*args, input='hi\n' if '--stdin' in args else '')
                self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            fake.terminate()
            fake.wait(timeout=5)
            fake.stdout.close()
