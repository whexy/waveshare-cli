import unittest
from PIL import Image
from epaper.image import pack, testcard


class ImageTests(unittest.TestCase):
    def test_polarity_and_bit_order(self):
        image = Image.new('L', (800, 480), 255)
        image.putpixel((0, 0), 0)
        data = pack(image)
        self.assertEqual(data, b'\x80' + bytes(47999))
        self.assertEqual(pack(image, invert=True)[0], 0x7f)

    def test_testcard_size(self):
        for fit in ('fit', 'fill', 'stretch'):
            self.assertEqual(len(pack(testcard(), fit=fit, rotate=90, dither=True)), 48000)
