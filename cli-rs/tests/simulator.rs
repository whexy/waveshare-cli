//! Drives the binary against tools/epdsim.py, the reference device simulator.
//!
//! The simulator speaks the real wire protocol over a pty and renders whatever
//! it receives to a PNG, so these assertions are about pixels that reached a
//! device, not about internal state.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli-rs sits in the repository")
        .to_path_buf()
}

fn binary() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_epaper")).to_path_buf()
}

/// The simulator, killed on drop so a failing assertion cannot leak it.
struct Simulator {
    child: Child,
    port: String,
    output: PathBuf,
}

impl Simulator {
    fn start(label: &str) -> Option<Self> {
        let output = std::env::temp_dir().join(format!("epaper-sim-{label}.png"));
        let _ = std::fs::remove_file(&output);
        let mut child = Command::new("python3")
            .arg(repository().join("tools/epdsim.py"))
            .arg("--output")
            .arg(&output)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut port = String::new();
        reader.read_line(&mut port).ok()?;
        let port = port.trim().to_string();
        if port.is_empty() {
            let _ = child.kill();
            return None;
        }
        Some(Self {
            child,
            port,
            output,
        })
    }

    fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> std::process::Output {
        let mut command = Command::new(binary());
        command
            .arg("--port")
            .arg(&self.port)
            .args(args)
            .env("EPAPER_PORT", &self.port)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("binary starts");
        if let Some(data) = stdin {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("stdin is piped")
                .write_all(data)
                .expect("child accepts stdin");
        }
        let output = child.wait_with_output().expect("binary exits");
        assert!(
            output.status.success(),
            "epaper {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Rows of the rendered panel that carry any ink, in cell-height bands.
    fn inked_bands(&self, cell_height: u32) -> Vec<u32> {
        let image = ::image::open(&self.output)
            .expect("simulator wrote a PNG")
            .to_luma8();
        assert_eq!((image.width(), image.height()), (800, 480));
        let mut bands = Vec::new();
        for band in 0..480 / cell_height {
            let inked = (band * cell_height..(band + 1) * cell_height)
                .any(|y| (0..800).any(|x| image.get_pixel(x, y).0[0] < 128));
            if inked {
                bands.push(band);
            }
        }
        bands
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The simulator needs python3 and Pillow; skip rather than fail where absent.
fn available() -> bool {
    Command::new("python3")
        .arg("-c")
        .arg("import PIL, serial")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn ping_returns_pong() {
    if !available() {
        return;
    }
    let sim = Simulator::start("ping").expect("simulator starts");
    let output = sim.run(&["ping"], None);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "PONG");
}

#[test]
fn text_inks_only_the_first_row() {
    if !available() {
        return;
    }
    let sim = Simulator::start("text").expect("simulator starts");
    sim.run(&["text", "hello"], None);
    assert_eq!(sim.inked_bands(16), vec![0]);
}

#[test]
fn piped_lines_ink_one_band_each() {
    if !available() {
        return;
    }
    let sim = Simulator::start("stdin").expect("simulator starts");
    sim.run(&["console", "--stdin"], Some(b"first\r\nsecond"));
    assert_eq!(sim.inked_bands(16), vec![0, 1]);
}

/// pyte could not do this at all. Entering the alternate screen, drawing into
/// it and leaving must restore the primary screen's pixels.
#[test]
fn alt_screen_round_trip_restores_the_primary_screen() {
    if !available() {
        return;
    }
    let sim = Simulator::start("altscreen").expect("simulator starts");
    // The cursor is hidden so the only ink is text; otherwise the cursor's own
    // reverse-video cell inks whichever band it rests on.
    let script = concat!(
        "\x1b[?25lprimary",
        "\x1b[?1049h",
        "\x1b[1;1Halternate screen\r\n\x1b[3;1Hmore alt text",
        "\x1b[?1049l",
    );
    sim.run(&["console", "--stdin"], Some(script.as_bytes()));
    assert_eq!(
        sim.inked_bands(16),
        vec![0],
        "alternate screen content survived the switch back"
    );
}

/// The reason TERM moved from `linux` to `xterm-256color`: a real curses
/// application has to draw on the panel, box characters and all.
///
/// The frame is sampled while the application still holds the alternate
/// screen, because leaving it correctly restores the primary screen and the
/// panel would then show an empty shell.
#[test]
fn a_curses_application_renders_on_the_panel() {
    if !available() {
        return;
    }
    let script = std::env::temp_dir().join("epaper-curses-probe.py");
    std::fs::write(
        &script,
        "import curses, time\ndef main(s):\n    curses.curs_set(0)\n    s.box()\n    s.addstr(2, 4, 'CURSES', curses.A_REVERSE)\n    s.hline(4, 4, curses.ACS_HLINE, 40)\n    s.refresh()\n    time.sleep(3.0)\ncurses.wrapper(main)\n",
    )
    .expect("probe script is writable");

    let sim = Simulator::start("curses").expect("simulator starts");
    let mut child = Command::new(binary())
        .arg("--port")
        .arg(&sim.port)
        .args(["console", "--"])
        .arg("python3")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("binary starts");

    // Sample while the alternate screen is still up.
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let bands = sim.inked_bands(16);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&script);

    assert!(
        bands.contains(&0),
        "the box border did not reach the panel: {bands:?}"
    );
    assert!(
        bands.len() >= 3,
        "expected a full-screen curses layout, got bands {bands:?}"
    );
}

/// A scroll region must move only the rows inside it.
#[test]
fn scroll_region_leaves_rows_outside_it_untouched() {
    if !available() {
        return;
    }
    let sim = Simulator::start("scrollregion").expect("simulator starts");
    // Confine scrolling to rows 3..5, then push enough lines to scroll it.
    let script = "\x1b[?25l\x1b[1;1Htop\x1b[3;5r\x1b[3;1Ha\r\nb\r\nc\r\nd";
    sim.run(&["console", "--stdin"], Some(script.as_bytes()));
    let bands = sim.inked_bands(16);
    assert!(bands.contains(&0), "row outside the region was cleared");
    assert!(
        bands.iter().all(|band| *band < 5),
        "ink escaped the scroll region: {bands:?}"
    );
}
