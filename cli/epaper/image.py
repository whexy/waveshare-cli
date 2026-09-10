from datetime import datetime

from PIL import Image, ImageDraw, ImageOps

SIZE = (800, 480)


def testcard():
    image = Image.new('RGB', SIZE, 'white')
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 0, 799, 479), outline='black', width=3)
    draw.text((20, 20), 'PICO 2 / WAVESHARE 800 x 480', fill='black')
    draw.text((20, 45), datetime.now().astimezone().isoformat(timespec='seconds'), fill='black')
    for i in range(12):
        draw.rectangle((20 + i * 64, 90, 75 + i * 64, 155), fill='black' if i % 2 else 'white', outline='black')
    draw.rectangle((30, 190, 370, 360), fill='black')
    draw.text((50, 220), 'WHITE ON BLACK / epaper testcard', fill='white')
    draw.ellipse((440, 190, 750, 370), fill='black')
    draw.ellipse((510, 230, 680, 330), fill='white')
    draw.line((20, 450, 780, 390), fill='black', width=2)
    return image


def pack(image, fit='fit', rotate=0, dither=False, threshold=128, invert=False):
    if fit not in ('fit', 'fill', 'stretch') or rotate not in (0, 90, 180, 270):
        raise ValueError('invalid fit or rotation')
    if not 0 <= threshold <= 255:
        raise ValueError('threshold must be between 0 and 255')
    image = ImageOps.exif_transpose(image).convert('RGBA')
    background = Image.new('RGBA', image.size, 'white')
    background.alpha_composite(image)
    image = background.convert('L').rotate(rotate, expand=True)
    if fit == 'stretch':
        image = image.resize(SIZE, Image.Resampling.LANCZOS)
    elif fit == 'fill':
        image = ImageOps.fit(image, SIZE, method=Image.Resampling.LANCZOS)
    else:
        image = ImageOps.pad(image, SIZE, method=Image.Resampling.LANCZOS, color=255)
    if dither:
        image = image.convert('1', dither=Image.Dither.FLOYDSTEINBERG)
    else:
        image = image.point(lambda p: 255 if p >= threshold else 0, mode='1')
    # Pillow packs white as one; the wire contract packs black as one.
    return bytes(b ^ (0 if invert else 255) for b in image.tobytes())


def load(path, **options):
    with Image.open(path) as image:
        return pack(image, **options)
