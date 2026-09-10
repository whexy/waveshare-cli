//! Rasterisation into a shadow framebuffer and the byte diff into BLITs.

use crate::font::Font;
use crate::geometry::{Geometry, FRAME_BYTES, PANEL_HEIGHT, PANEL_WIDTH, STRIDE};
use crate::protocol::{blit, MAX_PAYLOAD};
use crate::term::TerminalModel;

pub struct Renderer {
    geometry: Geometry,
    font: Font,
    shadow: Vec<u8>,
    cursor: Option<(usize, usize, bool)>,
}

impl Renderer {
    pub fn new(geometry: Geometry, font: Font) -> Self {
        Self {
            geometry,
            font,
            shadow: vec![0; FRAME_BYTES],
            cursor: None,
        }
    }

    /// Repaint the damaged rows and return the whole framebuffer.
    pub fn render(&mut self, model: &mut TerminalModel) -> &[u8] {
        let mut rows: Vec<usize> = model.damaged_rows();
        let cursor = model.cursor();
        if Some(cursor) != self.cursor {
            // Both the row the cursor left and the one it arrived on change.
            if let Some((_, previous, _)) = self.cursor {
                rows.push(previous);
            }
            rows.push(cursor.1);
        }
        rows.sort_unstable();
        rows.dedup();

        for row in rows {
            if row >= self.geometry.rows {
                continue;
            }
            let cells: Vec<(String, bool)> = model
                .row(row)
                .into_iter()
                .enumerate()
                .map(|(column, cell)| {
                    let under_cursor = cursor == (column, row, true);
                    (cell.text, cell.inverse ^ under_cursor)
                })
                .collect();
            let strip = self.font.row(&cells, PANEL_WIDTH);
            let top = row * self.geometry.cell_height;
            let start = top * STRIDE;
            let end = ((top + self.geometry.cell_height) * STRIDE).min(FRAME_BYTES);
            let length = end - start;
            self.shadow[start..end].copy_from_slice(&strip[..length]);
        }
        model.clear_damage();
        self.cursor = Some(cursor);
        &self.shadow
    }
}

/// Compare two framebuffers and produce the BLIT payloads that carry the
/// changes, along with the ghosting cost of the refresh they need.
pub fn diff(previous: &[u8], current: &[u8], cell_height: usize) -> (Vec<Vec<u8>>, u32) {
    let mut spans = Vec::new();
    for y in 0..PANEL_HEIGHT {
        let row = y * STRIDE;
        if previous[row..row + STRIDE] == current[row..row + STRIDE] {
            continue;
        }
        let changed: Vec<usize> = (0..STRIDE)
            .filter(|x| previous[row + x] != current[row + x])
            .collect();
        spans.push((changed[0], y, changed[changed.len() - 1] + 1));
    }
    if spans.is_empty() {
        return (Vec::new(), 0);
    }

    // Expand to cell-height bands so glyph-internal blank rows do not fragment
    // a character update into many tiny SPI windows.
    let mut bands: Vec<(usize, (usize, usize))> = Vec::new();
    for (x0, y, x1) in spans {
        let key = y / cell_height;
        match bands.iter_mut().find(|(band, _)| *band == key) {
            Some((_, (left, right))) => {
                *left = (*left).min(x0);
                *right = (*right).max(x1);
            }
            None => bands.push((key, (x0, x1))),
        }
    }
    bands.sort_unstable_by_key(|(band, _)| *band);

    let mut rectangles: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (band, (x0, x1)) in bands {
        let y0 = band * cell_height;
        let y1 = (y0 + cell_height).min(PANEL_HEIGHT);
        match rectangles.last().copied() {
            // Merge with the band above when they touch and overlap in x.
            Some((left, top, right, bottom)) if bottom == y0 && x0 < right && x1 > left => {
                rectangles.pop();
                rectangles.push((left.min(x0), top, right.max(x1), y1));
            }
            _ => rectangles.push((x0, y0, x1, y1)),
        }
    }

    let left = rectangles.iter().map(|r| r.0).min().unwrap();
    let right = rectangles.iter().map(|r| r.2).max().unwrap();
    let top = rectangles.iter().map(|r| r.1).min().unwrap();
    let bottom = rectangles.iter().map(|r| r.3).max().unwrap();
    let area = (right - left) * (bottom - top);
    let units = if area == FRAME_BYTES {
        3
    } else if area >= FRAME_BYTES / 2 {
        2
    } else {
        1
    };

    let mut payloads = Vec::new();
    for (x0, y0, x1, y1) in rectangles {
        let width = x1 - x0;
        // The BLIT header counts against the payload budget, not just pixels.
        let step = (MAX_PAYLOAD - 8) / width;
        let mut y = y0;
        while y < y1 {
            let end = (y + step).min(y1);
            let mut bits = Vec::with_capacity(width * (end - y));
            for row in y..end {
                bits.extend_from_slice(&current[row * STRIDE + x0..row * STRIDE + x1]);
            }
            let (_, payload) = blit(x0 as u16, y as u16, width as u16, (end - y) as u16, &bits)
                .expect("diff rectangles are clipped to the panel");
            payloads.push(payload);
            y = end;
        }
    }
    (payloads, units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::find_font;
    use crate::geometry::geometry;

    /// Grids whose cell height does not divide 480 (28 -> 17, 19 -> 25)
    /// exercise the band clipped against the panel edge.
    const GRIDS: &[(usize, usize)] = &[
        (100, 30),
        (90, 28),
        (107, 30),
        (50, 15),
        (80, 24),
        (66, 19),
        (160, 48),
    ];

    fn session(columns: usize, rows: usize) -> Option<(Geometry, TerminalModel, Renderer)> {
        let grid = geometry(columns, rows).unwrap();
        let font = Font::load(&find_font().ok()?, grid.cell_width, grid.cell_height).ok()?;
        Some((grid, TerminalModel::new(grid), Renderer::new(grid, font)))
    }

    fn header(payload: &[u8]) -> (u16, u16, u16, u16) {
        let read = |offset: usize| u16::from_le_bytes([payload[offset], payload[offset + 1]]);
        (read(0), read(2), read(4), read(6))
    }

    #[test]
    fn single_cell_change_sends_one_band() {
        let Some((grid, mut model, mut renderer)) = session(100, 30) else {
            return;
        };
        model.feed(b"\x1b[?25l");
        let before = renderer.render(&mut model).to_vec();
        model.feed(b"A");
        let (payloads, units) = diff(&before, renderer.render(&mut model), grid.cell_height);
        assert_eq!(payloads.len(), 1);
        assert_eq!(header(&payloads[0]), (0, 0, 1, 16));
        assert_eq!(units, 1);
    }

    #[test]
    fn scrolling_costs_a_whole_screen_and_stays_within_the_payload_limit() {
        let Some((grid, mut model, mut renderer)) = session(100, 30) else {
            return;
        };
        model.feed(b"\x1b[?25l");
        for row in 0..30 {
            let fill = if row % 2 == 0 { b'A' } else { b'B' };
            model.feed(&[fill; 100]);
            model.feed(b"\r\n");
        }
        let before = renderer.render(&mut model).to_vec();
        model.feed(b"\r\n");
        model.feed(&[b'C'; 100]);
        let (payloads, units) = diff(&before, renderer.render(&mut model), grid.cell_height);
        assert_eq!(units, 3);
        assert!(payloads.iter().all(|payload| payload.len() <= MAX_PAYLOAD));
        let covered: u32 = payloads.iter().map(|p| header(p).3 as u32).sum();
        assert_eq!(covered, 480);
    }

    #[test]
    fn half_a_screen_costs_two_units() {
        let mut half = vec![0xFFu8; FRAME_BYTES / 2];
        half.extend(std::iter::repeat_n(0u8, FRAME_BYTES / 2));
        assert_eq!(diff(&[0; FRAME_BYTES], &half, 16).1, 2);
    }

    #[test]
    fn an_unchanged_frame_sends_nothing() {
        let frame = vec![0u8; FRAME_BYTES];
        assert_eq!(diff(&frame, &frame, 16), (Vec::new(), 0));
    }

    #[test]
    fn cursor_inverts_its_cell_and_blank_screens_have_no_ink() {
        let Some((_, mut model, mut renderer)) = session(100, 30) else {
            return;
        };
        // The cursor block inverts its cell, so the first byte is fully inked.
        assert_eq!(renderer.render(&mut model)[0], 0xFF);
        model.feed(b"\x1b[?25l");
        assert!(renderer.render(&mut model).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn last_grid_row_inks_its_band_and_never_the_remainder() {
        for (columns, rows) in GRIDS.iter().copied() {
            let Some((grid, mut model, mut renderer)) = session(columns, rows) else {
                return;
            };
            // Reverse video fills the whole cell box, so the last grid row is
            // inked edge to edge and any bleed is unambiguous.
            model.feed(format!("\x1b[?25l\x1b[{rows};1H\x1b[7m").as_bytes());
            model.feed(&vec![b'W'; columns]);
            let shadow = renderer.render(&mut model).to_vec();
            assert_eq!(shadow.len(), FRAME_BYTES);
            let last = (grid.used_height() - 1) * STRIDE;
            assert_ne!(
                shadow[last..last + STRIDE],
                vec![0u8; STRIDE][..],
                "{columns}x{rows}: last grid row rendered blank"
            );
            let tail = grid.used_height() * STRIDE;
            assert!(
                shadow[tail..].iter().all(|byte| *byte == 0),
                "{columns}x{rows}: ink bled past the last grid row"
            );
        }
    }

    /// Payload shape and coverage cannot show that a rectangle carries the
    /// right pixels at the right place; replaying onto the previous frame and
    /// comparing against the target does.
    #[test]
    fn replaying_blits_reproduces_the_frame_exactly() {
        for (columns, rows) in GRIDS.iter().copied() {
            let Some((grid, mut model, mut renderer)) = session(columns, rows) else {
                return;
            };
            model.feed(b"\x1b[?25l");
            let before = renderer.render(&mut model).to_vec();
            model.feed("\x1b[7mhead\x1b[0m \u{2500}\u{252c}\u{2500} \u{e0b0} tail\r\n".as_bytes());
            for _ in 0..rows {
                model.feed(&vec![b'x'; columns]);
                model.feed(b"\r\n");
            }
            let current = renderer.render(&mut model).to_vec();
            let (payloads, _) = diff(&before, &current, grid.cell_height);
            let mut replayed = before.clone();
            for payload in payloads {
                let (x0, y0, width, height) = header(&payload);
                let body = &payload[8..];
                for line in 0..height as usize {
                    let start = (y0 as usize + line) * STRIDE + x0 as usize;
                    replayed[start..start + width as usize]
                        .copy_from_slice(&body[line * width as usize..(line + 1) * width as usize]);
                }
            }
            assert_eq!(replayed, current, "{columns}x{rows}: replay diverged");
        }
    }

    #[test]
    fn blits_are_well_formed_and_cover_every_change() {
        for (columns, rows) in GRIDS.iter().copied() {
            let Some((grid, mut model, mut renderer)) = session(columns, rows) else {
                return;
            };
            model.feed(b"\x1b[?25l");
            let before = renderer.render(&mut model).to_vec();
            for _ in 0..rows {
                model.feed(&vec![b'W'; columns]);
                model.feed(b"\r\n");
            }
            let current = renderer.render(&mut model).to_vec();
            let (payloads, _) = diff(&before, &current, grid.cell_height);
            let mut covered = std::collections::HashSet::new();
            for payload in &payloads {
                let (x0, y0, width, height) = header(payload);
                assert_eq!(payload.len() - 8, width as usize * height as usize);
                assert!(payload.len() <= MAX_PAYLOAD);
                assert!(x0 as usize + width as usize <= STRIDE);
                assert!(y0 as usize + height as usize <= PANEL_HEIGHT);
                for y in y0 as usize..y0 as usize + height as usize {
                    for x in x0 as usize..x0 as usize + width as usize {
                        covered.insert((x, y));
                    }
                }
            }
            for y in 0..PANEL_HEIGHT {
                for x in 0..STRIDE {
                    if before[y * STRIDE + x] != current[y * STRIDE + x] {
                        assert!(
                            covered.contains(&(x, y)),
                            "{columns}x{rows}: ({x},{y}) missed"
                        );
                    }
                }
            }
        }
    }

    /// A cell height that does not divide 480 puts the last band past the
    /// panel; unclipped it becomes a BLIT the device rejects outright.
    #[test]
    fn band_crossing_the_panel_edge_stays_in_bounds() {
        let mut current = vec![0u8; FRAME_BYTES];
        current[479 * STRIDE] = 0xFF;
        let (payloads, _) = diff(&[0; FRAME_BYTES], &current, 17);
        assert!(!payloads.is_empty());
        for payload in payloads {
            let (x0, y0, width, height) = header(&payload);
            assert!(y0 as usize + height as usize <= PANEL_HEIGHT);
            blit(x0, y0, width, height, &payload[8..]).expect("wire contract violated");
        }
    }

    /// A full-height column is the worst case for chunking: the header has to
    /// be counted against the 4096-byte budget, not just the pixels.
    #[test]
    fn tall_narrow_rectangles_respect_the_payload_limit() {
        for width in [1usize, 7, 9, 37, 100] {
            let mut current = vec![0u8; FRAME_BYTES];
            for y in 0..PANEL_HEIGHT {
                let start = y * STRIDE;
                current[start..start + width].fill(0xFF);
            }
            let (payloads, _) = diff(&vec![0; FRAME_BYTES], &current, 16);
            for payload in payloads {
                assert!(payload.len() <= MAX_PAYLOAD, "width {width} overran");
                let (x0, y0, w, h) = header(&payload);
                blit(x0, y0, w, h, &payload[8..]).expect("wire contract violated");
            }
        }
    }
}
