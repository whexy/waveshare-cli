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

    /// The distinct tones the rendered panel carries, darkest first.
    fn tones(&self) -> Vec<u8> {
        let image = ::image::open(&self.output)
            .expect("simulator wrote a PNG")
            .to_luma8();
        assert_eq!((image.width(), image.height()), (800, 480));
        let mut tones: Vec<u8> = image.pixels().map(|pixel| pixel.0[0]).collect();
        tones.sort_unstable();
        tones.dedup();
        tones
    }

    /// Every panel pixel, so two runs can be compared exactly rather than by
    /// which bands happen to carry ink.
    fn pixels(&self) -> Vec<u8> {
        let image = ::image::open(&self.output)
            .expect("simulator wrote a PNG")
            .to_luma8();
        assert_eq!((image.width(), image.height()), (800, 480));
        image
            .pixels()
            .map(|pixel| u8::from(pixel.0[0] < 128))
            .collect()
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

/// Gray has to reach the panel as four tones; a wrong plane split still
/// renders a plausible image, so the count is what catches it.
#[test]
fn a_gray_gradient_reaches_the_panel_as_four_tones() {
    if !available() {
        return;
    }
    let sim = Simulator::start("gray").expect("simulator starts");
    let source = std::env::temp_dir().join("epaper-gray-source.png");
    let mut gradient = ::image::GrayImage::new(800, 480);
    for (x, _y, pixel) in gradient.enumerate_pixels_mut() {
        *pixel = ::image::Luma([(x * 255 / 799) as u8]);
    }
    gradient.save(&source).expect("gradient is written");

    sim.run(&["draw", source.to_str().expect("utf-8 path"), "--gray"], None);
    // IMG_END only starts the refresh; the simulator renders when it finishes.
    std::thread::sleep(std::time::Duration::from_millis(3000));
    assert_eq!(sim.tones(), vec![0, 85, 170, 255]);

    // The same source in mono must stay bilevel, so gray cannot leak into the
    // path the console shares.
    let mono = Simulator::start("gray-mono").expect("simulator starts");
    mono.run(&["draw", source.to_str().expect("utf-8 path")], None);
    std::thread::sleep(std::time::Duration::from_millis(4600));
    assert_eq!(mono.tones(), vec![0, 255]);
}

/// Gray cannot refresh partially, and the failure must be refused up front
/// rather than reaching the panel as a torn frame.
#[test]
fn gray_refuses_a_partial_refresh() {
    if !available() {
        return;
    }
    let sim = Simulator::start("gray-partial").expect("simulator starts");
    let output = Command::new(binary())
        .args(["--port", &sim.port, "draw", "--testcard", "--gray", "--partial"])
        .output()
        .expect("the binary runs");
    assert!(!output.status.success(), "gray with --partial must fail");
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
    // The cursor is hidden so the only ink is text; otherwise the cursor's own
    // reverse-video cell inks whichever band it rests on.
    let round_trip = Simulator::start("altscreen").expect("simulator starts");
    round_trip.run(
        &["console", "--stdin"],
        Some(
            concat!(
                "\x1b[?25lprimary",
                "\x1b[?1049h",
                "\x1b[1;1Halternate screen\r\n\x1b[3;1Hmore alt text",
                "\x1b[?1049l",
            )
            .as_bytes(),
        ),
    );

    // The same primary screen, never having entered the alternate one.
    let reference = Simulator::start("altscreen-reference").expect("simulator starts");
    reference.run(&["console", "--stdin"], Some(b"\x1b[?25lprimary"));

    assert_eq!(
        round_trip.pixels(),
        reference.pixels(),
        "the restored screen does not match a primary-only render"
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

/// A command that exits without writing anything still has to end the session:
/// the pty reports hangup and the loop must reach EOF rather than spin.
#[test]
fn a_silent_command_terminates_the_session() {
    if !available() {
        return;
    }
    let sim = Simulator::start("silent").expect("simulator starts");
    let start = std::time::Instant::now();
    sim.run(&["console", "--", "/bin/sh", "-c", "exit 0"], None);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(30),
        "session did not finish promptly"
    );
}

/// A command that closes its terminal and keeps running holds the session open
/// until it exits, because the pty reports EOF only once the last slave
/// descriptor is gone. The Python CLI waits the same way, so this pins the
/// shared behaviour rather than asserting a bound the port does not have.
#[test]
fn a_command_that_closes_its_terminal_is_waited_for_until_it_exits() {
    if !available() {
        return;
    }
    let sim = Simulator::start("detached").expect("simulator starts");
    let start = std::time::Instant::now();
    sim.run(
        &[
            "console",
            "--",
            "/bin/sh",
            "-c",
            "printf bye; exec 0<&- 1>&- 2>&-; sleep 2",
        ],
        None,
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_secs(2),
        "returned before the child exited: {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "waited {elapsed:?}, far longer than the child ran"
    );
}

/// A scroll region must move only the rows inside it.
#[test]
fn scroll_region_leaves_rows_outside_it_untouched() {
    if !available() {
        return;
    }
    // Rows 3..5 are the scroll region. Feeding a, b, c, d scrolls it once, so
    // the region must end up holding b, c, d with the top row untouched.
    let scrolled = Simulator::start("scrollregion").expect("simulator starts");
    scrolled.run(
        &["console", "--stdin"],
        Some(b"\x1b[?25l\x1b[1;1Htop\x1b[3;5r\x1b[3;1Ha\r\nb\r\nc\r\nd"),
    );

    // The expected end state, written directly without any scrolling.
    let expected = Simulator::start("scrollregion-reference").expect("simulator starts");
    expected.run(
        &["console", "--stdin"],
        Some(b"\x1b[?25l\x1b[1;1Htop\x1b[3;1Hb\x1b[4;1Hc\x1b[5;1Hd"),
    );

    assert_eq!(
        scrolled.pixels(),
        expected.pixels(),
        "the scroll region did not shift its contents as expected"
    );
}
