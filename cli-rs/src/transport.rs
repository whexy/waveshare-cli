//! Serial port discovery and the request/response loop.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serialport::{SerialPort, SerialPortType};

use crate::protocol::{encode, error_name, Command, Decoder, ACK, BUSY, NAK};

const BAUD: u32 = 115_200;
const READ_TIMEOUT: Duration = Duration::from_millis(50);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const BACKOFF_START: Duration = Duration::from_millis(50);
const BACKOFF_CAP: Duration = Duration::from_secs(1);

pub fn detect_port() -> Result<String> {
    if let Ok(ports) = serialport::available_ports() {
        for port in &ports {
            if let SerialPortType::UsbPort(info) = &port.port_type {
                let product = info.product.clone().unwrap_or_default();
                if product.to_lowercase().contains("epaper") {
                    // macOS exposes both a tty. and a cu. node; only cu. skips
                    // the DCD carrier wait.
                    return Ok(port.port_name.replace("/dev/tty.", "/dev/cu."));
                }
            }
        }
    }
    // The product string is absent when the descriptor is not read, so fall
    // back to the platform's CDC-ACM naming.
    let pattern = if cfg!(target_os = "macos") {
        "/dev/cu.usbmodem"
    } else {
        "/dev/ttyACM"
    };
    let mut matches: Vec<String> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .filter(|path| path.starts_with(pattern))
        .collect();
    matches.sort();
    matches
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no USB modem found; specify --port"))
}

pub struct Transport {
    port: Box<dyn SerialPort>,
    verbose: bool,
    seq: u8,
    decoder: Decoder,
}

/// Open the port, falling back to leaving the line speed untouched.
///
/// A pty has no line speed, and on macOS setting one fails the whole open with
/// ENOTTY; tools/epdsim.py presents exactly that. Requesting no baud change
/// keeps the simulator usable without weakening the real device, whose speed is
/// set by the first branch.
fn open_port(name: &str) -> Result<Box<dyn SerialPort>> {
    let attempt = |baud: u32| serialport::new(name, baud).timeout(READ_TIMEOUT).open();
    match attempt(BAUD) {
        Ok(port) => Ok(port),
        Err(error) => attempt(0).map_err(|_| anyhow!("cannot open {name}: {error}")),
    }
}

impl Transport {
    pub fn open(port: Option<&str>, verbose: bool) -> Result<Self> {
        let requested = match port {
            Some(name) => Some(name.to_string()),
            None => std::env::var("EPAPER_PORT").ok(),
        };
        let name = match requested {
            Some(name) if name.is_empty() => bail!("empty serial port"),
            Some(name) => name,
            None => detect_port()?,
        };
        let port = open_port(&name)?;
        Ok(Self {
            port,
            verbose,
            seq: 0,
            decoder: Decoder::new(),
        })
    }

    pub fn request(&mut self, command: Command, payload: &[u8]) -> Result<Vec<u8>> {
        self.request_with_timeout(command, payload, DEFAULT_TIMEOUT)
    }

    pub fn request_with_timeout(
        &mut self,
        command: Command,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        let packet = encode(command as u8, seq, payload)?;
        let deadline = Instant::now() + timeout;
        let mut backoff = BACKOFF_START;
        let mut resend = true;
        let mut buffer = [0u8; 4096];

        while Instant::now() < deadline {
            if resend {
                if self.verbose {
                    eprintln!("TX {}", hex(&packet));
                }
                self.port.set_timeout(WRITE_TIMEOUT)?;
                self.port.write_all(&packet)?;
                self.port.flush()?;
                self.port.set_timeout(READ_TIMEOUT)?;
                resend = false;
            }
            let read = match self.port.read(&mut buffer) {
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::TimedOut => 0,
                Err(error) => return Err(error.into()),
            };
            if read > 0 && self.verbose {
                eprintln!("RX {}", hex(&buffer[..read]));
            }
            for frame in self.decoder.feed(&buffer[..read]) {
                if frame.seq != seq {
                    continue;
                }
                match frame.frame_type {
                    ACK => return Ok(frame.payload),
                    NAK => {
                        let code = frame.payload.first().copied().unwrap_or(0);
                        let message =
                            String::from_utf8_lossy(&frame.payload[1.min(frame.payload.len())..])
                                .into_owned();
                        bail!("{} ({}): {}", error_name(code), code, message);
                    }
                    BUSY => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        std::thread::sleep(backoff.min(remaining));
                        backoff = (backoff * 2).min(BACKOFF_CAP);
                        resend = true;
                        break;
                    }
                    other => bail!("unexpected response type 0x{other:02x}"),
                }
            }
        }
        bail!(
            "command 0x{:02x} timed out after {}s",
            command as u8,
            timeout.as_secs()
        )
    }

    /// A reset may disconnect CDC before its ACK reaches the host, so a
    /// missing response is success rather than an error.
    pub fn bootsel(&mut self) -> Result<()> {
        let (command, payload) = crate::protocol::reset_bootsel();
        match self.request_with_timeout(command, &payload, Duration::from_secs(2)) {
            Ok(_) => Ok(()),
            Err(_) => Ok(()),
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_port_is_rejected() {
        let message = match Transport::open(Some(""), false) {
            Ok(_) => panic!("an empty port name must not open"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("empty serial port"), "{message}");
    }

    #[test]
    fn hex_matches_the_python_separator() {
        assert_eq!(hex(&[0xEB, 0x90, 0x01]), "eb 90 01");
    }
}
