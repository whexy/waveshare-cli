//! Terminal model backed by alacritty_terminal.
//!
//! The emulator owns alt-screen, scroll regions, reflow and wide characters,
//! so this module is only a seam: it feeds bytes in, collects the replies the
//! application expects back on its own input, and exposes the damaged rows and
//! cursor that the renderer needs.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use crate::geometry::Geometry;

/// Replies the terminal wants written back to the pty, such as DSR and DA
/// responses. Collected rather than written directly because the pty is owned
/// by the session loop.
#[derive(Clone, Default)]
pub struct Replies(Arc<Mutex<Vec<u8>>>);

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.0.lock().unwrap().extend_from_slice(text.as_bytes()),
            Event::TextAreaSizeRequest(format) => {
                // The panel has no window, so report the grid in cells only.
                let size = alacritty_terminal::event::WindowSize {
                    num_lines: 0,
                    num_cols: 0,
                    cell_width: 0,
                    cell_height: 0,
                };
                self.0
                    .lock()
                    .unwrap()
                    .extend_from_slice(format(size).as_bytes());
            }
            _ => {}
        }
    }
}

/// The grid size, in the shape alacritty wants it.
///
/// No scrollback: the panel shows the viewport and nothing else, so
/// `total_lines` equals `screen_lines`.
#[derive(Clone, Copy)]
struct GridSize {
    columns: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// One cell, reduced to what a 1-bit panel can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellView {
    pub text: String,
    pub inverse: bool,
}

/// Full-screen erase and reset schedule a full panel refresh once the output
/// goes idle.
///
/// Detected by watching the byte stream rather than by intercepting the
/// terminal: alacritty's `Handler` has 74 methods that all default to doing
/// nothing, so a hand-written forwarding wrapper silently discards whatever it
/// omits. An earlier draft of this module did exactly that and swallowed
/// `CSI ?1049h`, which sent alt-screen output to the primary grid.
fn schedules_full_refresh(data: &[u8]) -> bool {
    let mut index = 0;
    while let Some(offset) = data[index..].iter().position(|&byte| byte == 0x1B) {
        let escape = index + offset;
        match data.get(escape + 1) {
            // RIS
            Some(b'c') => return true,
            Some(b'[') => {
                let mut cursor = escape + 2;
                while data
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b';')
                {
                    cursor += 1;
                }
                // ED 2 clears the screen, ED 3 also drops scrollback.
                if data.get(cursor) == Some(&b'J')
                    && matches!(&data[escape + 2..cursor], b"2" | b"3")
                {
                    return true;
                }
            }
            _ => {}
        }
        index = escape + 1;
    }
    false
}

pub struct TerminalModel {
    pub geometry: Geometry,
    term: Term<Replies>,
    parser: Processor<StdSyncHandler>,
    replies: Replies,
    reset_requested: bool,
}

impl TerminalModel {
    pub fn new(geometry: Geometry) -> Self {
        let size = GridSize {
            columns: geometry.columns,
            rows: geometry.rows,
        };
        let replies = Replies::default();
        Self {
            geometry,
            term: Term::new(Config::default(), &size, replies.clone()),
            parser: Processor::new(),
            replies,
            reset_requested: false,
        }
    }

    /// Feed pty output; returns bytes the application expects back on its own
    /// input, which the caller must write to the pty.
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if schedules_full_refresh(data) {
            self.reset_requested = true;
        }
        self.parser.advance(&mut self.term, data);
        std::mem::take(&mut *self.replies.0.lock().unwrap())
    }

    /// Rows changed since the last call to [`Self::clear_damage`].
    ///
    /// The cursor's own row is always reported, because alacritty leaves cursor
    /// drawing to the client, so this never comes back empty. Repainting a row
    /// costs nothing on the wire: the shadow-buffer diff downstream drops it
    /// again when the pixels are unchanged.
    ///
    /// alacritty also reports the changed column range per line. The renderer
    /// repaints whole rows regardless, because the BLIT protocol addresses x in
    /// whole bytes and a cell boundary can sit mid-byte, so the shadow-buffer
    /// diff stays the authority on what actually has to be sent.
    pub fn damaged_rows(&mut self) -> Vec<usize> {
        match self.term.damage() {
            TermDamage::Full => (0..self.geometry.rows).collect(),
            TermDamage::Partial(lines) => lines
                .map(|line| line.line)
                .filter(|row| *row < self.geometry.rows)
                .collect(),
        }
    }

    pub fn clear_damage(&mut self) {
        self.term.reset_damage();
    }

    /// Cursor column, row and whether it is visible.
    pub fn cursor(&self) -> (usize, usize, bool) {
        let point = self.term.grid().cursor.point;
        let column = (point.column.0).min(self.geometry.columns.saturating_sub(1));
        let row = point.line.0.max(0) as usize;
        (
            column,
            row.min(self.geometry.rows.saturating_sub(1)),
            self.term.mode().contains(TermMode::SHOW_CURSOR),
        )
    }

    /// Every cell of a row in column order.
    pub fn row(&self, row: usize) -> Vec<CellView> {
        let line = &self.term.grid()[Line(row as i32)];
        (0..self.geometry.columns)
            .map(|column| {
                let cell = &line[Column(column)];
                // The second half of a double-width character carries no glyph
                // of its own; the wide char is drawn from its leading cell.
                let blank = cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    || cell.flags.contains(Flags::HIDDEN);
                let mut text = if blank {
                    String::new()
                } else {
                    cell.c.to_string()
                };
                if !blank {
                    if let Some(marks) = cell.zerowidth() {
                        text.extend(marks.iter().copied());
                    }
                }
                CellView {
                    text,
                    inverse: cell.flags.contains(Flags::INVERSE),
                }
            })
            .collect()
    }

    pub fn reset_requested(&self) -> bool {
        self.reset_requested
    }

    pub fn clear_reset(&mut self) {
        self.reset_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::geometry;

    fn model(columns: usize, rows: usize) -> TerminalModel {
        TerminalModel::new(geometry(columns, rows).unwrap())
    }

    fn text_of(model: &TerminalModel, row: usize) -> String {
        model
            .row(row)
            .iter()
            .map(|cell| {
                if cell.text.is_empty() {
                    ' '
                } else {
                    cell.text.chars().next().unwrap()
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn dsr_and_da_replies_are_returned() {
        let mut model = model(100, 30);
        assert_eq!(model.feed(b"\x1b[6n"), b"\x1b[1;1R".to_vec());
        assert!(model.feed(b"\x1b[c").starts_with(b"\x1b[?"));
    }

    #[test]
    fn rows_use_the_configured_width() {
        let mut model = model(80, 24);
        model.feed(&"x".repeat(100).into_bytes());
        assert_eq!(model.row(0).len(), 80);
        assert_eq!(text_of(&model, 1), "x".repeat(20));
    }

    /// pyte could not do this at all; it is the reason for the rewrite.
    #[test]
    fn alt_screen_round_trip_restores_the_primary_grid() {
        let mut model = model(100, 30);
        model.feed(b"primary text");
        model.feed(b"\x1b[?1049h\x1b[1;1Halternate");
        assert_eq!(text_of(&model, 0), "alternate");
        model.feed(b"\x1b[?1049l");
        assert_eq!(text_of(&model, 0), "primary text");
    }

    #[test]
    fn scroll_region_confines_scrolling() {
        let mut model = model(20, 10);
        model.feed(b"\x1b[1;1Hkeep");
        // Confine scrolling to rows 5..8, fill it, then force one more line.
        model.feed(b"\x1b[5;8r\x1b[5;1Ha\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(text_of(&model, 0), "keep", "row outside the region moved");
        assert_eq!(text_of(&model, 4), "b", "region did not scroll");
        assert_eq!(text_of(&model, 7), "e");
    }

    #[test]
    fn wide_characters_occupy_one_cell_and_blank_their_spacer() {
        let mut model = model(20, 4);
        model.feed("\u{4f60}\u{597d}".as_bytes());
        let row = model.row(0);
        assert_eq!(row[0].text, "\u{4f60}");
        assert_eq!(row[1].text, "", "spacer must not repaint the glyph");
        assert_eq!(row[2].text, "\u{597d}");
        assert_eq!(row[3].text, "");
    }

    #[test]
    fn damage_narrows_to_changed_rows() {
        let mut model = model(100, 30);
        model.feed(b"\x1b[?25l");
        model.clear_damage();
        model.feed(b"\x1b[10;40HX");
        let rows = model.damaged_rows();
        assert!(rows.contains(&9), "row 9 not reported damaged: {rows:?}");
        assert!(rows.len() < 30, "expected partial damage, got {rows:?}");

        // Only the cursor row survives a clear; alacritty reports it every
        // time because drawing the cursor is the client's job.
        model.clear_damage();
        assert_eq!(model.damaged_rows(), vec![9]);
    }

    #[test]
    fn full_screen_erase_and_reset_schedule_a_refresh() {
        for sequence in [&b"\x1b[2J"[..], &b"\x1b[3J"[..], &b"\x1bc"[..]] {
            let mut model = model(100, 30);
            model.feed(sequence);
            assert!(
                model.reset_requested(),
                "{:?} should schedule a refresh",
                String::from_utf8_lossy(sequence)
            );
            model.clear_reset();
            assert!(!model.reset_requested());
        }
        // A partial erase is ordinary output and must not force a full refresh.
        for sequence in [&b"\x1b[J"[..], &b"\x1b[0J"[..], &b"\x1b[1J"[..]] {
            let mut model = model(100, 30);
            model.feed(sequence);
            assert!(!model.reset_requested());
        }
    }

    #[test]
    fn cursor_reports_position_and_visibility() {
        let mut model = model(100, 30);
        model.feed(b"\x1b[5;7H");
        assert_eq!(model.cursor(), (6, 4, true));
        model.feed(b"\x1b[?25l");
        assert!(!model.cursor().2);
    }

    #[test]
    fn inverse_video_is_reported() {
        let mut model = model(20, 4);
        model.feed(b"\x1b[7mAB");
        let row = model.row(0);
        assert!(row[0].inverse && row[1].inverse);
        assert!(!row[2].inverse);
    }
}
