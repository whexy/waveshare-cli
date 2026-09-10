//! Glyph rasterisation for the console.
//!
//! Cells are rendered by the rasteriser at a size whose advance matches the
//! cell width, then thresholded to 1 bit. Rendering antialiased and
//! thresholding, rather than asking for a monochrome bitmap, is what makes
//! box-drawing characters join across cell seams: their strokes sit on
//! fractional pixel boundaries and monochrome rasterisation drops those below
//! half coverage, which breaks a horizontal rule into dashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fontdue::{Font as Face, FontSettings};

/// Starting guess for pixel size relative to cell width, refined per font by
/// measuring the real advance. 0.5 suits the bitmap-derived default; outline
/// fonts land near 0.6.
const ADVANCE_RATIO: f32 = 0.5;

/// Glyph whose definition is to fill the character cell completely. Aligning to
/// its ink is how the cell box is located without trusting line metrics, which
/// Nerd Font patching inflates past the cell.
const CELL_PROBE: char = '\u{2588}';

/// Coverage at which a pixel counts as inked. Below half because the panel
/// renders any set pixel as full black, so thin stems otherwise disappear.
const THRESHOLD: u8 = 110;

pub const ENV_FONT: &str = "EPAPER_FONT";

/// Combining sequences make the key space unbounded, so the cache is capped
/// rather than left to track everything a long session ever displayed. Far
/// larger than the working set of any one screen.
const CACHE_ENTRIES: usize = 4096;

/// Terminess is Terminus plus the Nerd Font patch: its outlines are traced from
/// the original bitmaps, so at its design sizes every stem lands on a whole
/// pixel. An outline font thresholded to 1 bit gives uneven stem weights.
const NAMES: &[(&str, &str)] = &[
    ("Terminess", "TerminessNerdFontMono-Regular.ttf"),
    ("Terminess", "TerminessNerdFont-Regular.ttf"),
    ("JetBrainsMono", "JetBrainsMonoNerdFontMono-Regular.ttf"),
    ("JetBrainsMono", "JetBrainsMonoNerdFont-Regular.ttf"),
];

fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join(".local/share"));
        roots.push(Path::new(&home).join("Library/Fonts"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/share:/usr/local/share".to_string());
    roots.extend(
        data_dirs
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from),
    );
    roots.push(PathBuf::from("/Library/Fonts"));
    roots.push(PathBuf::from("/System/Library/Fonts"));
    roots
}

/// Locate the console font. EPAPER_FONT overrides the search.
pub fn find_font() -> Result<PathBuf> {
    if let Some(override_path) = std::env::var_os(ENV_FONT) {
        if !override_path.is_empty() {
            let path = PathBuf::from(override_path);
            if !path.is_file() {
                bail!("{ENV_FONT} is set to {path:?}, which is not a file");
            }
            return Ok(path);
        }
    }
    for root in roots() {
        for (family, name) in NAMES {
            let candidates = [
                root.join("fonts/truetype/NerdFonts")
                    .join(family)
                    .join(name),
                root.join("fonts").join(name),
                root.join(name),
            ];
            for candidate in candidates {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    bail!(
        "no console font found; install a Nerd Font such as Terminess or point \
         {ENV_FONT} at a font file"
    )
}

/// A cell bitmap, one byte per pixel, 1 where the panel should be black.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    text: String,
    inverse: bool,
}

/// Rasterises cells and rows onto a fixed cell grid.
pub struct Font {
    face: Face,
    size: f32,
    pub width: usize,
    pub height: usize,
    /// Distance from the top of the cell to the rasteriser's baseline.
    baseline: i32,
    cache: HashMap<CacheKey, Vec<bool>>,
    order: Vec<CacheKey>,
}

impl Font {
    pub fn load(path: &Path, cell_width: usize, cell_height: usize) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {path:?}"))?;
        let face = Face::from_bytes(bytes, FontSettings::default())
            .map_err(|message| anyhow::anyhow!("cannot load font {path:?}: {message}"))?;
        let size = Self::fit(&face, cell_width, cell_height);
        let baseline = Self::align(&face, size, cell_height);
        Ok(Self {
            face,
            size,
            width: cell_width,
            height: cell_height,
            baseline,
            cache: HashMap::new(),
            order: Vec::new(),
        })
    }

    /// Largest pixel size that fills the cell without overflowing it.
    ///
    /// Bitmap-derived fonts render crisply only at their design size and do not
    /// share an outline font's advance-to-size ratio, so the size is measured
    /// rather than assumed. Candidates are scored on how much of the cell the
    /// font's own block glyph covers, which keeps full-height box drawing
    /// seamless across rows.
    fn fit(face: &Face, cell_width: usize, cell_height: usize) -> f32 {
        let guess = cell_width as f32 / ADVANCE_RATIO;
        let mut sizes: Vec<f32> = (4..=(guess * 1.6) as usize).map(|n| n as f32).collect();
        for factor in [0.7, 0.8, 0.9, 1.0] {
            sizes.push((guess * factor * 20.0).round() / 20.0);
        }
        sizes.retain(|size| *size > 0.0);
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let mut best: Option<(usize, f32, f32)> = None;
        for size in sizes {
            let advance = face.metrics('M', size).advance_width;
            if advance <= 0.0 || advance > cell_width as f32 {
                continue;
            }
            let covered = Self::probe_height(face, size).min(cell_height);
            let score = (covered, advance);
            if best.is_none_or(|(best_covered, best_advance, _)| {
                (covered, advance) > (best_covered, best_advance)
            }) {
                best = Some((score.0, score.1, size));
            }
        }
        best.map_or(guess, |(_, _, size)| size)
    }

    /// Height of the font's full-block glyph in pixels, thresholded.
    fn probe_height(face: &Face, size: f32) -> usize {
        let (metrics, bitmap) = face.rasterize(CELL_PROBE, size);
        match Self::ink_rows(&bitmap, metrics.width, metrics.height) {
            Some((top, bottom)) => bottom - top + 1,
            None => 0,
        }
    }

    /// First and last rows of the bitmap carrying ink.
    fn ink_rows(bitmap: &[u8], width: usize, height: usize) -> Option<(usize, usize)> {
        let mut top = None;
        let mut bottom = 0;
        for row in 0..height {
            if bitmap[row * width..(row + 1) * width]
                .iter()
                .any(|coverage| *coverage >= THRESHOLD)
            {
                top.get_or_insert(row);
                bottom = row;
            }
        }
        top.map(|top| (top, bottom))
    }

    /// Offset that seats the font's own cell box on the grid.
    ///
    /// Uses the block glyph's ink rather than the face's ascent because Nerd
    /// Font patching inflates the line metrics well past the cell.
    fn align(face: &Face, size: f32, cell_height: usize) -> i32 {
        let (metrics, bitmap) = face.rasterize(CELL_PROBE, size);
        let Some((top, _)) = Self::ink_rows(&bitmap, metrics.width, metrics.height) else {
            let line = face.horizontal_line_metrics(size);
            return line.map_or(cell_height as i32 / 2, |line| {
                (cell_height as f32 - (line.ascent - line.descent)) as i32 / 2 + line.ascent as i32
            });
        };
        // rasterize places the bitmap's bottom at ymin above the baseline, so
        // its top sits at (height + ymin) above it. Seat that top edge on the
        // top of the cell so full-height box drawing tiles across rows.
        (metrics.height as i32 + metrics.ymin) - top as i32
    }

    /// The 1-bit bitmap of one cell, true where the panel is black.
    fn cell(&mut self, text: &str, inverse: bool) -> &[bool] {
        let key = CacheKey {
            text: text.to_string(),
            inverse,
        };
        if !self.cache.contains_key(&key) {
            let rendered = self.render(text, inverse);
            if self.order.len() >= CACHE_ENTRIES {
                if let Some(evicted) = self.order.first().cloned() {
                    self.order.remove(0);
                    self.cache.remove(&evicted);
                }
            }
            self.order.push(key.clone());
            self.cache.insert(key.clone(), rendered);
        } else if let Some(position) = self.order.iter().position(|entry| *entry == key) {
            let entry = self.order.remove(position);
            self.order.push(entry);
        }
        &self.cache[&key]
    }

    fn render(&self, text: &str, inverse: bool) -> Vec<bool> {
        let mut pixels = vec![inverse; self.width * self.height];
        if text.trim().is_empty() {
            return pixels;
        }
        // Combining marks stack onto the base glyph's own origin.
        let mut pen = 0.0f32;
        for (index, character) in text.chars().enumerate() {
            let (metrics, bitmap) = self.face.rasterize(character, self.size);
            let left = pen as i32 + metrics.xmin;
            let top = self.baseline - (metrics.height as i32 + metrics.ymin);
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    if bitmap[row * metrics.width + column] < THRESHOLD {
                        continue;
                    }
                    let x = left + column as i32;
                    let y = top + row as i32;
                    // The block glyph is wider and taller than its cell, so
                    // anything outside the box is clipped rather than wrapped.
                    if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                        continue;
                    }
                    pixels[y as usize * self.width + x as usize] = !inverse;
                }
            }
            if index == 0 {
                pen += metrics.advance_width;
            }
        }
        pixels
    }

    /// Compose one row of cells into a packed 1-bit strip, black as one.
    pub fn row(&mut self, cells: &[(String, bool)], width: usize) -> Vec<u8> {
        let stride = width / 8;
        let mut strip = vec![0u8; stride * self.height];
        for (column, (text, inverse)) in cells.iter().enumerate() {
            let origin = column * self.width;
            if origin >= width {
                break;
            }
            let cell_width = self.width;
            let cell_height = self.height;
            let pixels = self.cell(text, *inverse).to_vec();
            for y in 0..cell_height {
                for x in 0..cell_width {
                    if !pixels[y * cell_width + x] {
                        continue;
                    }
                    let panel_x = origin + x;
                    if panel_x >= width {
                        break;
                    }
                    strip[y * stride + panel_x / 8] |= 0x80 >> (panel_x % 8);
                }
            }
        }
        strip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face() -> Option<Font> {
        let path = find_font().ok()?;
        Font::load(&path, 8, 16).ok()
    }

    #[test]
    fn rejects_an_override_that_is_not_a_file() {
        temp_env::with_var(ENV_FONT, Some("/nonexistent/font.ttf"), || {
            assert!(find_font().is_err());
        });
    }

    #[test]
    fn accepts_an_override_that_exists() {
        let file = std::env::temp_dir().join("epaper-font-probe.ttf");
        std::fs::write(&file, b"not really a font").unwrap();
        temp_env::with_var(ENV_FONT, Some(file.as_os_str()), || {
            assert_eq!(find_font().unwrap(), file);
        });
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn fitted_size_keeps_the_advance_within_the_cell() {
        let Some(font) = face() else { return };
        assert!(font.face.metrics('M', font.size).advance_width <= 8.0);
    }

    /// The block glyph is what box drawing tiles against, so it has to ink the
    /// full cell height with no seam at the top or bottom edge.
    #[test]
    fn block_glyph_fills_the_cell_edge_to_edge() {
        let Some(mut font) = face() else { return };
        let pixels = font.cell("\u{2588}", false).to_vec();
        for y in 0..16 {
            assert!(
                (0..8).all(|x| pixels[y * 8 + x]),
                "block glyph left row {y} unfilled"
            );
        }
    }

    /// Monochrome rasterisation would drop these strokes; antialias plus a
    /// low threshold is what keeps a horizontal rule continuous.
    #[test]
    fn box_drawing_joins_across_the_cell_seam() {
        let Some(mut font) = face() else { return };
        let pixels = font.cell("\u{2500}", false).to_vec();
        let inked: Vec<usize> = (0..16)
            .filter(|y| (0..8).any(|x| pixels[y * 8 + x]))
            .collect();
        assert_eq!(inked.len(), 1, "expected one inked scanline, got {inked:?}");
        let row = inked[0];
        assert!(
            (0..8).all(|x| pixels[row * 8 + x]),
            "horizontal rule is dashed, not continuous"
        );
    }

    #[test]
    fn inverse_swaps_the_cell() {
        let Some(mut font) = face() else { return };
        let plain = font.cell(" ", false).to_vec();
        let inverted = font.cell(" ", true).to_vec();
        assert!(plain.iter().all(|inked| !inked));
        assert!(inverted.iter().all(|inked| *inked));
    }

    #[test]
    fn row_packs_black_as_one_msb_first() {
        let Some(mut font) = face() else { return };
        let cells = vec![("\u{2588}".to_string(), false), (" ".to_string(), false)];
        let strip = font.row(&cells, 800);
        assert_eq!(strip.len(), 100 * 16);
        assert_eq!(strip[0], 0xFF, "first cell should be fully inked");
        assert_eq!(strip[1], 0x00, "second cell should be blank");
    }

    #[test]
    fn cache_reuses_entries_and_stays_bounded() {
        let Some(mut font) = face() else { return };
        let first = font.cell("A", false).to_vec();
        let again = font.cell("A", false).to_vec();
        assert_eq!(first, again);
        assert_eq!(font.cache.len(), 1);
        for index in 0..CACHE_ENTRIES + 64 {
            let text = char::from_u32(0x4E00 + index as u32).unwrap().to_string();
            font.cell(&text, false);
        }
        assert!(font.cache.len() <= CACHE_ENTRIES);
        assert_eq!(font.cache.len(), font.order.len());
    }
}
