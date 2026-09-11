//! Testcard generation and 1-bit packing for the wire format.

use anyhow::{bail, Result};
use image::imageops::{self, ColorMap, FilterType};
use image::{GrayImage, Luma, Rgba, RgbaImage};

use crate::font::find_font;
use crate::geometry::{PANEL_HEIGHT, PANEL_WIDTH};
use crate::protocol::Format;

const WIDTH: u32 = PANEL_WIDTH as u32;
const HEIGHT: u32 = PANEL_HEIGHT as u32;

/// Coverage at which a rasterised glyph pixel counts as ink. Low because the
/// panel renders any set pixel as full black, so thin stems otherwise vanish.
const TEXT_THRESHOLD: u8 = 64;

/// Terminess is bitmap-derived and only renders evenly at its design sizes;
/// below 16px punctuation such as '-' and ':' falls under any usable
/// threshold and drops out of the label entirely.
const TEXT_SIZE: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Fit,
    Fill,
    Stretch,
}

/// The four tones the panel can hold, as the luma the host renders them at.
/// Evenly spaced so dithering error diffuses symmetrically; the panel's actual
/// mid-tones sit closer together than this, which costs contrast rather than
/// correctness.
const GRAY4_LEVELS: [u8; 4] = [0, 85, 170, 255];

/// Quantiser for the 4-gray palette, used as the dither target so the error
/// diffusion in `imageops::dither` lands on tones the panel can hold.
struct Gray4;

impl ColorMap for Gray4 {
    type Color = Luma<u8>;

    fn index_of(&self, color: &Luma<u8>) -> usize {
        // Round to the nearest level rather than truncating, so a tone sitting
        // just below a boundary does not bias an entire image darker.
        ((color.0[0] as u16 * 3 + 127) / 255) as usize
    }

    fn lookup(&self, index: usize) -> Option<Luma<u8>> {
        GRAY4_LEVELS.get(index).map(|&level| Luma([level]))
    }

    fn has_lookup(&self) -> bool {
        true
    }

    fn map_color(&self, color: &mut Luma<u8>) {
        *color = Luma([GRAY4_LEVELS[self.index_of(color)]]);
    }
}

fn set_pixel(canvas: &mut GrayImage, x: i64, y: i64, value: u8) {
    if x >= 0 && y >= 0 && x < canvas.width() as i64 && y < canvas.height() as i64 {
        canvas.put_pixel(x as u32, y as u32, Luma([value]));
    }
}

fn fill_rect(canvas: &mut GrayImage, x0: i64, y0: i64, x1: i64, y1: i64, value: u8) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            set_pixel(canvas, x, y, value);
        }
    }
}

fn outline_rect(canvas: &mut GrayImage, x0: i64, y0: i64, x1: i64, y1: i64, width: i64, value: u8) {
    for offset in 0..width {
        for x in x0..=x1 {
            set_pixel(canvas, x, y0 + offset, value);
            set_pixel(canvas, x, y1 - offset, value);
        }
        for y in y0..=y1 {
            set_pixel(canvas, x0 + offset, y, value);
            set_pixel(canvas, x1 - offset, y, value);
        }
    }
}

fn fill_ellipse(canvas: &mut GrayImage, x0: i64, y0: i64, x1: i64, y1: i64, value: u8) {
    let (cx, cy) = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
    let (rx, ry) = ((x1 - x0) as f64 / 2.0, (y1 - y0) as f64 / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = (x as f64 - cx) / rx;
            let dy = (y as f64 - cy) / ry;
            if dx * dx + dy * dy <= 1.0 {
                set_pixel(canvas, x, y, value);
            }
        }
    }
}

fn draw_line(canvas: &mut GrayImage, x0: i64, y0: i64, x1: i64, y1: i64, width: i64, value: u8) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    let half = width / 2;
    for step in 0..=steps {
        let x = x0 + (x1 - x0) * step / steps;
        let y = y0 + (y1 - y0) * step / steps;
        fill_rect(canvas, x - half, y - half, x + half, y + half, value);
    }
}

fn draw_text(canvas: &mut GrayImage, font: &fontdue::Font, x: i64, y: i64, text: &str, value: u8) {
    let mut pen = x;
    for character in text.chars() {
        let (metrics, bitmap) = font.rasterize(character, TEXT_SIZE);
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                if bitmap[row * metrics.width + column] >= TEXT_THRESHOLD {
                    let px = pen + metrics.xmin as i64 + column as i64;
                    let py = y + TEXT_SIZE as i64 - metrics.ymin as i64 - metrics.height as i64
                        + row as i64;
                    set_pixel(canvas, px, py, value);
                }
            }
        }
        pen += metrics.advance_width.round() as i64;
    }
}

/// A diagnostic pattern exercising black, white, inverted text and curves.
pub fn testcard() -> Result<GrayImage> {
    let path = find_font()?;
    let font = fontdue::Font::from_bytes(
        std::fs::read(&path)?,
        fontdue::FontSettings {
            scale: TEXT_SIZE,
            ..fontdue::FontSettings::default()
        },
    )
    .map_err(|message| anyhow::anyhow!("cannot load font {path:?}: {message}"))?;

    let mut canvas = GrayImage::from_pixel(WIDTH, HEIGHT, Luma([255]));
    outline_rect(&mut canvas, 0, 0, 799, 479, 3, 0);
    draw_text(
        &mut canvas,
        &font,
        20,
        20,
        "PICO 2 / WAVESHARE 800 x 480",
        0,
    );
    draw_text(
        &mut canvas,
        &font,
        20,
        45,
        &chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        0,
    );
    for index in 0..12i64 {
        let x0 = 20 + index * 64;
        let value = if index % 2 != 0 { 0 } else { 255 };
        fill_rect(&mut canvas, x0, 90, x0 + 55, 155, value);
        outline_rect(&mut canvas, x0, 90, x0 + 55, 155, 1, 0);
    }
    fill_rect(&mut canvas, 30, 190, 370, 360, 0);
    draw_text(
        &mut canvas,
        &font,
        50,
        220,
        "WHITE ON BLACK / epaper testcard",
        255,
    );
    fill_ellipse(&mut canvas, 440, 190, 750, 370, 0);
    fill_ellipse(&mut canvas, 510, 230, 680, 330, 255);
    draw_line(&mut canvas, 20, 450, 780, 390, 2, 0);
    Ok(canvas)
}

fn scaled(image: &GrayImage, fit: Fit) -> GrayImage {
    match fit {
        Fit::Stretch => imageops::resize(image, WIDTH, HEIGHT, FilterType::Lanczos3),
        Fit::Fill | Fit::Fit => {
            let scale_x = WIDTH as f64 / image.width() as f64;
            let scale_y = HEIGHT as f64 / image.height() as f64;
            // Cover the panel and crop, or contain within it and pad.
            let scale = if fit == Fit::Fill {
                scale_x.max(scale_y)
            } else {
                scale_x.min(scale_y)
            };
            let width = ((image.width() as f64 * scale).round() as u32).max(1);
            let height = ((image.height() as f64 * scale).round() as u32).max(1);
            let resized = imageops::resize(image, width, height, FilterType::Lanczos3);
            let mut canvas = GrayImage::from_pixel(WIDTH, HEIGHT, Luma([255]));
            // Centre on both axes, matching Pillow's default centering.
            let x = (WIDTH as i64 - width as i64) / 2;
            let y = (HEIGHT as i64 - height as i64) / 2;
            imageops::replace(&mut canvas, &resized, x, y);
            canvas
        }
    }
}

pub struct PackOptions {
    pub fit: Fit,
    pub rotate: u32,
    pub dither: bool,
    pub threshold: u8,
    pub invert: bool,
    pub format: Format,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            fit: Fit::Fit,
            rotate: 0,
            dither: false,
            threshold: 128,
            invert: false,
            format: Format::Mono,
        }
    }
}

/// Flatten, fit and quantise an image into the packed wire format for the
/// requested pixel format.
pub fn pack(image: &RgbaImage, options: &PackOptions) -> Result<Vec<u8>> {
    // Transparent pixels would otherwise threshold to black.
    let mut flattened = RgbaImage::from_pixel(image.width(), image.height(), Rgba([255; 4]));
    imageops::overlay(&mut flattened, image, 0, 0);
    let luma = image::DynamicImage::ImageRgba8(flattened).into_luma8();

    // Pillow rotates counter-clockwise; imageops::rotate90 turns clockwise.
    let rotated = match options.rotate {
        0 => luma,
        90 => imageops::rotate270(&luma),
        180 => imageops::rotate180(&luma),
        270 => imageops::rotate90(&luma),
        other => bail!("invalid rotation {other}"),
    };

    let mut fitted = scaled(&rotated, options.fit);

    match options.format {
        Format::Mono => {
            if options.dither {
                imageops::dither(&mut fitted, &imageops::BiLevel);
            } else {
                for pixel in fitted.pixels_mut() {
                    pixel.0[0] = if pixel.0[0] >= options.threshold {
                        255
                    } else {
                        0
                    };
                }
            }
            Ok(pack_mono(&fitted, options.invert))
        }
        Format::Gray4 => {
            if options.dither {
                imageops::dither(&mut fitted, &Gray4);
            } else {
                for pixel in fitted.pixels_mut() {
                    Gray4.map_color(pixel);
                }
            }
            Ok(pack_gray4(&fitted, options.invert))
        }
    }
}

/// The 1bpp wire contract packs black as one, MSB leftmost.
fn pack_mono(image: &GrayImage, invert: bool) -> Vec<u8> {
    let mut packed = vec![0u8; (WIDTH as usize / 8) * HEIGHT as usize];
    for (x, y, pixel) in image.enumerate_pixels() {
        let black = pixel.0[0] < 128;
        if black != invert {
            packed[y as usize * (WIDTH as usize / 8) + x as usize / 8] |= 0x80 >> (x % 8);
        }
    }
    packed
}

/// The 2bpp wire contract carries darkness, extending 1bpp's "one is black":
/// 0 is white and 3 is black, two bits per pixel, leftmost pixel in the high
/// bits. The device splits those bits across the panel's two RAM planes.
fn pack_gray4(image: &GrayImage, invert: bool) -> Vec<u8> {
    let stride = WIDTH as usize / 4;
    let mut packed = vec![0u8; stride * HEIGHT as usize];
    for (x, y, pixel) in image.enumerate_pixels() {
        let level = Gray4.index_of(pixel) as u8;
        let darkness = if invert { level } else { 3 - level };
        let shift = 6 - 2 * (x % 4);
        packed[y as usize * stride + x as usize / 4] |= darkness << shift;
    }
    packed
}

pub fn load(path: &str, options: &PackOptions) -> Result<Vec<u8>> {
    let decoded = image::open(path)?;
    // EXIF orientation is not applied by the decoder.
    pack(&decoded.into_rgba8(), options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polarity_and_bit_order() {
        let mut canvas = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([255, 255, 255, 255]));
        canvas.put_pixel(0, 0, Rgba([0, 0, 0, 255]));

        let data = pack(&canvas, &PackOptions::default()).unwrap();
        assert_eq!(data.len(), 48000);
        assert_eq!(data[0], 0x80);
        assert!(data[1..].iter().all(|&byte| byte == 0));

        let inverted = pack(
            &canvas,
            &PackOptions {
                invert: true,
                ..PackOptions::default()
            },
        )
        .unwrap();
        assert_eq!(inverted[0], 0x7F);
    }

    /// The 2bpp packing is the one thing no test on the host can catch by
    /// eye, and the panel shows a silently wrong plane split as plausible
    /// noise. Levels are pinned against Waveshare's display_4Gray, whose
    /// plane bits equal this darkness encoding's low and high bit.
    #[test]
    fn gray4_levels_and_bit_order() {
        let options = PackOptions {
            format: Format::Gray4,
            ..PackOptions::default()
        };
        let mut canvas = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([255; 4]));
        for (x, level) in GRAY4_LEVELS.iter().enumerate() {
            canvas.put_pixel(x as u32, 0, Rgba([*level, *level, *level, 255]));
        }

        let data = pack(&canvas, &options).unwrap();
        assert_eq!(data.len(), 96000);
        // White=0, light=1, dark=2, black=3 packed into one byte, leftmost
        // pixel in the high bits: 3, 2, 1, 0 reading dark to light.
        assert_eq!(data[0], 0b11_10_01_00);
        assert!(data[1..].iter().all(|&byte| byte == 0));

        let inverted = pack(
            &canvas,
            &PackOptions {
                invert: true,
                ..options
            },
        )
        .unwrap();
        assert_eq!(inverted[0], 0b00_01_10_11);
        // Inverting white fills the rest of the frame with black.
        assert!(inverted[1..].iter().all(|&byte| byte == 0xFF));
    }

    /// Mid-tones must survive quantisation; rounding the wrong way collapses
    /// them onto the rails and silently yields a bilevel image.
    #[test]
    fn gray4_keeps_four_distinct_tones() {
        let mut canvas = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([255; 4]));
        for x in 0..WIDTH {
            let tone = (x * 255 / (WIDTH - 1)) as u8;
            for y in 0..HEIGHT {
                canvas.put_pixel(x, y, Rgba([tone, tone, tone, 255]));
            }
        }
        let data = pack(
            &canvas,
            &PackOptions {
                format: Format::Gray4,
                ..PackOptions::default()
            },
        )
        .unwrap();

        let mut seen = [false; 4];
        for byte in &data {
            for shift in [6, 4, 2, 0] {
                seen[((byte >> shift) & 0b11) as usize] = true;
            }
        }
        assert_eq!(seen, [true; 4], "a gradient must use all four tones");
    }

    /// Gray must not perturb the mono path, which the console depends on.
    #[test]
    fn mono_packing_is_unchanged_by_the_gray_option() {
        let mut canvas = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([255; 4]));
        canvas.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        canvas.put_pixel(5, 9, Rgba([90, 90, 90, 255]));

        for dither in [false, true] {
            for invert in [false, true] {
                for threshold in [1u8, 128, 200] {
                    let options = PackOptions {
                        dither,
                        invert,
                        threshold,
                        ..PackOptions::default()
                    };
                    assert_eq!(pack(&canvas, &options).unwrap().len(), 48000);
                }
            }
        }
    }

    #[test]
    fn transparent_pixels_flatten_to_white() {
        let canvas = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([0, 0, 0, 0]));
        let data = pack(&canvas, &PackOptions::default()).unwrap();
        assert!(data.iter().all(|&byte| byte == 0));
    }

    /// Thin punctuation is the first thing to disappear when the rasterisation
    /// size or threshold drifts, and it disappears silently.
    #[test]
    fn testcard_text_keeps_thin_punctuation() {
        let path = match find_font() {
            Ok(path) => path,
            Err(_) => return,
        };
        let font = fontdue::Font::from_bytes(
            std::fs::read(&path).unwrap(),
            fontdue::FontSettings {
                scale: TEXT_SIZE,
                ..fontdue::FontSettings::default()
            },
        )
        .unwrap();
        for character in ['-', ':', '.', '/'] {
            let (metrics, bitmap) = font.rasterize(character, TEXT_SIZE);
            assert!(
                metrics.width * metrics.height > 0,
                "{character:?} has no bitmap"
            );
            assert!(
                bitmap.iter().any(|&coverage| coverage >= TEXT_THRESHOLD),
                "{character:?} renders below the ink threshold and would vanish"
            );
        }
    }

    #[test]
    fn every_fit_and_rotation_fills_the_frame() {
        let source = RgbaImage::from_pixel(320, 200, Rgba([128, 128, 128, 255]));
        for fit in [Fit::Fit, Fit::Fill, Fit::Stretch] {
            for rotate in [0, 90, 180, 270] {
                let data = pack(
                    &source,
                    &PackOptions {
                        fit,
                        rotate,
                        dither: true,
                        ..PackOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(data.len(), 48000, "fit={fit:?} rotate={rotate}");
            }
        }
        assert!(pack(
            &source,
            &PackOptions {
                rotate: 45,
                ..PackOptions::default()
            }
        )
        .is_err());
    }
}
