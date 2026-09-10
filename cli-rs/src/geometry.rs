//! Console grid geometry.
//!
//! The panel is 800x480 and BLIT addresses x in whole bytes, so the framebuffer
//! stride is fixed at 100 bytes. Cell width need not divide 8: rows are
//! rasterised into a full-width strip and the bits are packed, so a column
//! boundary may sit mid-byte. Only the panel dimensions are hard constraints.

use anyhow::{bail, Result};

pub const PANEL_WIDTH: usize = 800;
pub const PANEL_HEIGHT: usize = 480;
pub const STRIDE: usize = PANEL_WIDTH / 8;
pub const FRAME_BYTES: usize = STRIDE * PANEL_HEIGHT;

pub const DEFAULT_COLUMNS: usize = 100;
pub const DEFAULT_ROWS: usize = 30;

/// Below this the font has too few pixels per stem to stay legible on a panel
/// with no greyscale.
pub const MIN_CELL_WIDTH: usize = 5;
pub const MIN_CELL_HEIGHT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub columns: usize,
    pub rows: usize,
    pub cell_width: usize,
    pub cell_height: usize,
}

impl Geometry {
    pub fn width(&self) -> usize {
        PANEL_WIDTH
    }

    pub fn height(&self) -> usize {
        PANEL_HEIGHT
    }

    /// Rows of pixels the grid covers; any remainder stays blank.
    pub fn used_height(&self) -> usize {
        self.rows * self.cell_height
    }
}

/// Validate a terminal size and derive its cell box.
pub fn geometry(columns: usize, rows: usize) -> Result<Geometry> {
    if columns < 1 || rows < 1 {
        bail!("terminal size must be positive");
    }
    let cell_width = PANEL_WIDTH / columns;
    let cell_height = PANEL_HEIGHT / rows;
    if cell_width < MIN_CELL_WIDTH || cell_height < MIN_CELL_HEIGHT {
        bail!(
            "{columns}x{rows} needs a {cell_width}x{cell_height} cell, below the \
             legible minimum of {MIN_CELL_WIDTH}x{MIN_CELL_HEIGHT}; at most {}x{} fits",
            PANEL_WIDTH / MIN_CELL_WIDTH,
            PANEL_HEIGHT / MIN_CELL_HEIGHT,
        );
    }
    Ok(Geometry {
        columns,
        rows,
        cell_width,
        cell_height,
    })
}

pub fn default_geometry() -> Geometry {
    geometry(DEFAULT_COLUMNS, DEFAULT_ROWS).expect("default grid is valid")
}

/// Parse a COLSxROWS option value.
pub fn parse_size(text: &str) -> Result<Geometry> {
    let lowered = text.to_lowercase();
    let parts: Vec<&str> = lowered.split('x').collect();
    let digits = parts.len() == 2
        && parts.iter().all(|part| {
            let trimmed = part.trim();
            !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit())
        });
    if !digits {
        bail!("invalid terminal size {text:?}, expected COLSxROWS");
    }
    geometry(parts[0].trim().parse()?, parts[1].trim().parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grid_matches_panel() {
        let grid = default_geometry();
        assert_eq!((grid.columns, grid.rows), (100, 30));
        assert_eq!((grid.cell_width, grid.cell_height), (8, 16));
        assert_eq!(grid.used_height(), PANEL_HEIGHT);
    }

    #[test]
    fn parses_and_rejects_sizes() {
        assert_eq!(parse_size("80x24").unwrap().columns, 80);
        assert_eq!(parse_size("80X24").unwrap().rows, 24);
        assert!(parse_size("80").is_err());
        assert!(parse_size("axb").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn rejects_illegible_cells() {
        // 200 columns gives a 4px cell, under the legible minimum.
        assert!(geometry(200, 30).is_err());
        assert!(geometry(0, 30).is_err());
    }
}
