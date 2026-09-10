//! Console font discovery.
//!
//! Glyph rasterisation lives alongside the terminal model; this module only
//! locates the font file that both the console and the testcard draw with.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

pub const ENV_FONT: &str = "EPAPER_FONT";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_override_that_is_not_a_file() {
        // The search order is environment dependent, so only the override
        // branch is asserted here.
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
}
