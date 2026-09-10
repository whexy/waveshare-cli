//! Console session: refresh batching policy and the pty plumbing.

use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::pty::{openpty, Winsize};
use nix::sys::signal::{kill, sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::termios::{self, SetArg, Termios};
use nix::unistd::Pid;

use crate::font::Font;
use crate::geometry::{Geometry, FRAME_BYTES, STRIDE};
use crate::protocol as p;
use crate::render::{diff, Renderer};
use crate::term::TerminalModel;
use crate::transport::Transport;

// Batch fast partials for responsiveness; defer disruptive full refreshes to
// idle. These encode the panel's ghosting limits: a wrong value degrades the
// physical display, not just the output.
const OUTPUT_IDLE: Duration = Duration::from_millis(60);
const MAX_PENDING: Duration = Duration::from_millis(250);
const CURSOR_IDLE: Duration = Duration::from_millis(200);
const FULL_IDLE: Duration = Duration::from_secs(2);
const FULL_UNITS: u32 = 60;
const MAINTENANCE_IDLE: Duration = Duration::from_secs(30);
const MAINTENANCE_UNITS: u32 = 10;
const RESET_UNITS: u32 = 10;
const FINAL_UNITS: u32 = 30;

/// How long a child gets to exit on its own before the next escalation, and
/// how long a finished command is waited for. Matches the Python timeout.
const CHILD_GRACE: Duration = Duration::from_secs(2);

/// alacritty implements the full sequence set, so the pty advertises a real
/// terminal rather than the reduced `linux` the pyte version was limited to.
const TERM: &str = "xterm-256color";

pub struct Session<'a> {
    device: &'a mut Transport,
    geometry: Geometry,
    model: TerminalModel,
    renderer: Renderer,
    sent: Vec<u8>,
    units: u32,
    last_output: Instant,
    pending_since: Option<Instant>,
    cursor_only: bool,
    initial: bool,
}

impl<'a> Session<'a> {
    pub fn new(device: &'a mut Transport, geometry: Geometry, font: Font) -> Self {
        let now = Instant::now();
        Self {
            device,
            geometry,
            model: TerminalModel::new(geometry),
            renderer: Renderer::new(geometry, font),
            sent: vec![0; FRAME_BYTES],
            units: 0,
            last_output: now,
            pending_since: Some(now),
            cursor_only: false,
            initial: true,
        }
    }

    /// Feed pty output; returns the replies that must go back to the pty.
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let before: Vec<_> = (0..self.geometry.rows).map(|r| self.model.row(r)).collect();
        let replies = self.model.feed(data);
        let content_changed = (0..self.geometry.rows).any(|row| before[row] != self.model.row(row));
        let now = Instant::now();
        match self.pending_since {
            None => {
                self.pending_since = Some(now);
                self.cursor_only = !content_changed;
            }
            Some(_) if content_changed => self.cursor_only = false,
            Some(_) => {}
        }
        self.last_output = now;
        replies
    }

    pub fn tick(&mut self, final_flush: bool) -> Result<bool> {
        let now = Instant::now();
        let idle = now.duration_since(self.last_output);
        let maintenance = (idle >= FULL_IDLE
            && (self.units >= FULL_UNITS
                || (self.model.reset_requested() && self.units >= RESET_UNITS)))
            || (idle >= MAINTENANCE_IDLE && self.units >= MAINTENANCE_UNITS);

        if !final_flush && !maintenance {
            let Some(pending) = self.pending_since else {
                return Ok(false);
            };
            if self.cursor_only {
                if idle < CURSOR_IDLE {
                    return Ok(false);
                }
            } else if idle < OUTPUT_IDLE && now.duration_since(pending) < MAX_PENDING {
                return Ok(false);
            }
        }

        let (command, payload) = p::status();
        let status = p::parse_status(&self.device.request(command, &payload)?)?;
        if status.busy != 0 {
            return Ok(false);
        }

        let current = self.renderer.render(&mut self.model).to_vec();
        let (mut payloads, mut cost) = diff(&self.sent, &current, self.geometry.cell_height);

        // No host readback exists: establish a known framebuffer once before
        // relying on byte diffs, including clearing pixels from a prior client.
        if self.initial {
            let band = (p::MAX_PAYLOAD - 8) / STRIDE;
            payloads = (0..480)
                .step_by(band)
                .map(|y| {
                    let height = band.min(480 - y);
                    let (_, payload) = p::blit(
                        0,
                        y as u16,
                        STRIDE as u16,
                        height as u16,
                        &current[y * STRIDE..(y + height) * STRIDE],
                    )
                    .expect("full-width bands are in bounds");
                    payload
                })
                .collect();
            cost = 3;
        }

        let full = maintenance || (final_flush && self.units >= FINAL_UNITS);
        if !payloads.is_empty() || full {
            for payload in payloads {
                self.device.request(p::Command::Blit, &payload)?;
            }
            let (command, payload) = p::refresh(full);
            self.device.request(command, &payload)?;
            self.sent.copy_from_slice(&current);
            self.units = if full { 0 } else { self.units + cost };
        }
        self.initial = false;
        self.pending_since = None;
        if full {
            self.model.clear_reset();
        }
        Ok(true)
    }

    pub fn finish(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.tick(true)? {
            if Instant::now() > deadline {
                bail!("final console flush timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        loop {
            let (command, payload) = p::status();
            let status = p::parse_status(&self.device.request(command, &payload)?)?;
            if status.busy == 0 {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("final refresh timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Piped bytes bypass the tty line discipline that would add CR to LF.
fn onlcr(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut previous = 0u8;
    for &byte in data {
        if byte == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(byte);
        previous = byte;
    }
    out
}

/// Whether stdin has data worth reading.
///
/// A bare POLLHUP does not count. Redirecting stdin from /dev/null leaves it
/// permanently signalled, and treating that as readable sends the child an
/// immediate EOF, which kills a full-screen application before it draws.
fn stdin_readable(fd: &PollFd<'_>) -> bool {
    fd.revents()
        .is_some_and(|events| events.intersects(PollFlags::POLLIN | PollFlags::POLLERR))
}

/// Whether the pty master has data or has hung up.
///
/// Hangup counts here, unlike for stdin: it is how the master reports that the
/// child closed its end, and the following read returns the EOF that ends the
/// session. macOS raises POLLIN alongside POLLHUP so the distinction is
/// invisible there, but POSIX allows a bare POLLHUP and ignoring it would spin
/// the loop forever.
fn master_readable(fd: &PollFd<'_>) -> bool {
    fd.revents().is_some_and(|events| {
        events.intersects(PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP)
    })
}

fn write_all(fd: BorrowedFd<'_>, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        let count = nix::unistd::write(fd, data)?;
        data = &data[count..];
    }
    Ok(())
}

pub fn text(device: &mut Transport, data: &[u8], geometry: Geometry, font: Font) -> Result<u8> {
    let mut session = Session::new(device, geometry, font);
    session.feed(&onlcr(data));
    session.finish()?;
    Ok(0)
}

pub fn pipe_stdin(device: &mut Transport, geometry: Geometry, font: Font) -> Result<u8> {
    let mut session = Session::new(device, geometry, font);
    let stdin = std::io::stdin();
    let mut buffer = [0u8; 4096];
    loop {
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        if poll(&mut fds, PollTimeout::from(20u16))? > 0 {
            let count = (&stdin).read(&mut buffer)?;
            if count == 0 {
                break;
            }
            session.feed(&onlcr(&buffer[..count]));
        }
        session.tick(false)?;
    }
    session.finish()?;
    Ok(0)
}

/// Set by the signal handler; the session loop polls it and unwinds normally.
///
/// A handler cannot restore termios or reap the child itself, because neither
/// is async-signal-safe. It only records the request.
static TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn note_termination(_signum: libc::c_int) {
    TERMINATED.store(true, Ordering::SeqCst);
}

/// Turns SIGTERM and SIGHUP into an orderly exit, and restores the previous
/// dispositions when the session ends.
///
/// Without this, default termination skips every destructor and leaves the
/// invoking terminal in raw mode. SIGKILL cannot be caught and is not covered.
struct Signals {
    previous: Vec<(Signal, SigAction)>,
}

impl Signals {
    fn install() -> Self {
        TERMINATED.store(false, Ordering::SeqCst);
        let mut previous = Vec::new();
        let terminate = SigAction::new(
            SigHandler::Handler(note_termination),
            SaFlags::empty(),
            SigSet::empty(),
        );
        for signal in [Signal::SIGTERM, Signal::SIGHUP] {
            // Safety: the handler only stores into an AtomicBool.
            if let Ok(old) = unsafe { sigaction(signal, &terminate) } {
                previous.push((signal, old));
            }
        }
        // The panel's grid is fixed, so a resized host window changes nothing.
        let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        if let Ok(old) = unsafe { sigaction(Signal::SIGWINCH, &ignore) } {
            previous.push((Signal::SIGWINCH, old));
        }
        Self { previous }
    }

    fn terminated() -> bool {
        TERMINATED.load(Ordering::SeqCst)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for (signal, action) in &self.previous {
            let _ = unsafe { sigaction(*signal, action) };
        }
    }
}

/// Restores the terminal attributes it replaced, on every exit path.
struct RawMode {
    fd: OwnedFd,
    saved: Termios,
}

impl RawMode {
    fn enter() -> Option<Self> {
        let stdin = std::io::stdin();
        if !nix::unistd::isatty(stdin.as_fd()).unwrap_or(false) {
            return None;
        }
        let saved = termios::tcgetattr(stdin.as_fd()).ok()?;
        // Duplicated before the attributes change so that a failure here
        // cannot leave the terminal raw with no guard to restore it.
        let fd = stdin.as_fd().try_clone_to_owned().ok()?;
        let mut raw = saved.clone();
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &raw).ok()?;
        Some(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(self.fd.as_fd(), SetArg::TCSADRAIN, &self.saved);
    }
}

/// Reaps the child on every exit path, including errors and unwinding.
///
/// Dropping a `std::process::Child` neither kills nor waits, so an early return
/// would otherwise leave the command running against a closed pty.
struct ChildGuard {
    child: std::process::Child,
    reaped: bool,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    /// Wait for an already-finishing child, bounding how long the caller can
    /// be held up by one that never exits.
    fn wait_briefly(&mut self) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + CHILD_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    return Some(status);
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => return None,
                Err(_) => return None,
            }
        }
    }
}

impl ChildGuard {
    /// Signal the child's process group, falling back to the process itself.
    ///
    /// `run` puts the child in its own session, so the group carries any
    /// grandchildren with it. A child that never called setsid still shares
    /// this process's group, where the negative-pid form would either fail
    /// with ESRCH or signal the test runner, so it is signalled directly.
    fn signal(&self, signal: Signal) {
        let pid = self.child.id() as i32;
        let own_group = nix::unistd::getpgid(Some(Pid::from_raw(pid)))
            .is_ok_and(|group| group == Pid::from_raw(pid));
        let target = if own_group {
            Pid::from_raw(-pid)
        } else {
            Pid::from_raw(pid)
        };
        let _ = kill(target, signal);
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            self.reaped = true;
            return;
        }
        // Hang up first, as a real terminal would, and escalate only if the
        // command ignores it.
        self.signal(Signal::SIGHUP);
        if self.wait_briefly().is_some() {
            return;
        }
        self.signal(Signal::SIGKILL);
        let _ = self.child.wait();
        self.reaped = true;
    }
}

pub fn run(
    device: &mut Transport,
    command: &[String],
    echo: bool,
    geometry: Geometry,
    font: Font,
) -> Result<u8> {
    let winsize = Winsize {
        ws_row: geometry.rows as u16,
        ws_col: geometry.columns as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(Some(&winsize), None)?;
    let slave_fd = pty.slave.as_raw_fd();

    let child = {
        use std::os::unix::process::CommandExt;
        let mut builder = std::process::Command::new(&command[0]);
        builder
            .args(&command[1..])
            .env("TERM", TERM)
            .env("COLUMNS", geometry.columns.to_string())
            .env("LINES", geometry.rows.to_string())
            .stdin(pty.slave.try_clone()?)
            .stdout(pty.slave.try_clone()?)
            .stderr(pty.slave.try_clone()?);
        unsafe {
            // A new session needs the slave explicitly assigned as its
            // controlling tty, otherwise job control signals never arrive.
            builder.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        builder.spawn()?
    };
    drop(pty.slave);

    // Installed before the guards below so their Drop runs while the previous
    // dispositions are still replaced.
    let _signals = Signals::install();
    let mut child = ChildGuard::new(child);
    let _raw = RawMode::enter();
    let mut session = Session::new(device, geometry, font);
    let stdin = std::io::stdin();
    let master = pty.master;
    let mut buffer = [0u8; 4096];
    let mut stdin_open = true;

    let status = loop {
        if Signals::terminated() {
            // The guards restore the terminal and reap the child on the way
            // out; 128 + SIGTERM matches the usual shell convention.
            return Ok(143);
        }

        let mut fds = vec![PollFd::new(master.as_fd(), PollFlags::POLLIN)];
        if stdin_open {
            fds.push(PollFd::new(stdin.as_fd(), PollFlags::POLLIN));
        }
        match poll(&mut fds, PollTimeout::from(20u16)) {
            Ok(_) => {}
            // A caught signal interrupts the wait; the check above handles it.
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
        }

        if stdin_open && fds.len() > 1 && stdin_readable(&fds[1]) {
            match (&stdin).read(&mut buffer) {
                Ok(0) | Err(_) => {
                    stdin_open = false;
                    // Signal EOF to the child rather than leaving it waiting.
                    write_all(master.as_fd(), b"\x04")?;
                }
                Ok(count) => write_all(master.as_fd(), &buffer[..count])?,
            }
        }

        if master_readable(&fds[0]) {
            match nix::unistd::read(master.as_fd(), &mut buffer) {
                // EIO is how a pty reports that the child closed its end.
                Ok(0) | Err(nix::errno::Errno::EIO) => break child.wait_briefly(),
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => return Err(error.into()),
                Ok(count) => {
                    let replies = session.feed(&buffer[..count]);
                    if !replies.is_empty() {
                        write_all(master.as_fd(), &replies)?;
                    }
                    if echo {
                        std::io::stdout().write_all(&buffer[..count])?;
                        std::io::stdout().flush()?;
                    }
                }
            }
        }
        session.tick(false)?;
    };

    session.finish()?;
    // A command that closed its terminal but kept running is not waited for
    // beyond the grace period; the guard hangs it up on the way out.
    Ok(status.and_then(|status| status.code()).unwrap_or(0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onlcr_adds_carriage_returns_only_where_missing() {
        assert_eq!(onlcr(b"a\nb"), b"a\r\nb".to_vec());
        assert_eq!(onlcr(b"a\r\nb"), b"a\r\nb".to_vec());
        assert_eq!(onlcr(b"a\n\nb"), b"a\r\n\r\nb".to_vec());
        assert_eq!(onlcr(b"plain"), b"plain".to_vec());
    }

    /// Dropping the guard must not leave the command running against a closed
    /// pty, which is what a bare `std::process::Child` would do.
    #[test]
    fn child_guard_reaps_a_command_that_ignores_hangup() {
        let child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("trap '' HUP; sleep 30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("sh starts");
        let pid = child.id();
        let guard = ChildGuard::new(child);

        let start = Instant::now();
        drop(guard);
        let elapsed = start.elapsed();

        assert!(
            elapsed < CHILD_GRACE * 3,
            "guard took {elapsed:?} to escalate"
        );
        // The process is reaped, so signalling the pid must fail.
        std::thread::sleep(Duration::from_millis(100));
        let alive = kill(Pid::from_raw(pid as i32), None).is_ok();
        assert!(!alive, "child {pid} survived the guard");
    }

    #[test]
    fn refresh_policy_constants_match_the_panel_budget() {
        // Named so a careless edit to a ghosting-critical number fails here
        // rather than on the glass.
        assert_eq!(OUTPUT_IDLE.as_millis(), 60);
        assert_eq!(MAX_PENDING.as_millis(), 250);
        assert_eq!(CURSOR_IDLE.as_millis(), 200);
        assert_eq!(FULL_IDLE.as_secs(), 2);
        assert_eq!(FULL_UNITS, 60);
        assert_eq!(MAINTENANCE_IDLE.as_secs(), 30);
        assert_eq!(MAINTENANCE_UNITS, 10);
        assert_eq!(RESET_UNITS, 10);
        assert_eq!(FINAL_UNITS, 30);
    }
}
