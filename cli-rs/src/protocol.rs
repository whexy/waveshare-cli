//! Wire framing for the host <-> Pico link. See docs/protocol.md.

use std::fmt;

pub const MAGIC: [u8; 2] = [0xEB, 0x90];
pub const MAX_PAYLOAD: usize = 4096;

pub const ACK: u8 = 0x80;
pub const NAK: u8 = 0x81;
pub const BUSY: u8 = 0x82;

/// Framebuffer size in bytes; IMG_DATA offsets are bounded by it.
pub const FRAMEBUFFER_BYTES: u32 = 48000;

/// IMG_DATA carries a u32 offset ahead of the pixels, so a full payload holds
/// this much data.
pub const IMG_DATA_MAX: usize = 4090;

/// BLIT carries four u16 header fields ahead of the bits.
pub const BLIT_HEADER: usize = 8;

pub fn error_name(code: u8) -> &'static str {
    match code {
        1 => "bad length",
        2 => "bad state",
        3 => "out of range",
        4 => "unknown command",
        5 => "crc",
        _ => "unknown error",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Ping = 0x01,
    Info = 0x02,
    Clear = 0x04,
    Status = 0x05,
    ImgBegin = 0x10,
    ImgData = 0x11,
    ImgEnd = 0x12,
    Blit = 0x13,
    Refresh = 0x14,
    Sleep = 0x30,
    ResetBootsel = 0x31,
}

#[derive(Debug)]
pub struct ProtocolError(String);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

fn err<T>(message: &str) -> Result<T, ProtocolError> {
    Err(ProtocolError(message.to_string()))
}

/// CRC-16/CCITT-FALSE, matching Python's `binascii.crc_hqx(data, 0xFFFF)`.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: u8,
    pub seq: u8,
    pub payload: Vec<u8>,
}

pub fn encode(command: u8, seq: u8, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if payload.len() > MAX_PAYLOAD {
        return err("payload exceeds 4096 bytes");
    }
    let mut body = Vec::with_capacity(4 + payload.len());
    body.push(command);
    body.push(seq);
    body.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    body.extend_from_slice(payload);

    let mut packet = Vec::with_capacity(MAGIC.len() + body.len() + 2);
    packet.extend_from_slice(&MAGIC);
    packet.extend_from_slice(&body);
    packet.extend_from_slice(&crc16(&body).to_le_bytes());
    Ok(packet)
}

#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate bytes and return every frame that completed. On a framing or
    /// CRC error a single byte is dropped so the scan can resynchronise on a
    /// magic sequence that overlaps the corrupted region.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Frame> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            let Some(start) = self
                .buffer
                .windows(MAGIC.len())
                .position(|window| window == MAGIC)
            else {
                // A trailing first magic byte may be completed by the next read.
                let keep = usize::from(self.buffer.last() == Some(&MAGIC[0]));
                self.buffer.drain(..self.buffer.len() - keep);
                break;
            };
            self.buffer.drain(..start);
            if self.buffer.len() < 6 {
                break;
            }
            let length = u16::from_le_bytes([self.buffer[4], self.buffer[5]]) as usize;
            if length > MAX_PAYLOAD {
                self.buffer.remove(0);
                continue;
            }
            let end = 6 + length;
            if self.buffer.len() < end + 2 {
                break;
            }
            let expected = u16::from_le_bytes([self.buffer[end], self.buffer[end + 1]]);
            if crc16(&self.buffer[2..end]) != expected {
                self.buffer.remove(0);
                continue;
            }
            frames.push(Frame {
                frame_type: self.buffer[2],
                seq: self.buffer[3],
                payload: self.buffer[6..end].to_vec(),
            });
            self.buffer.drain(..end + 2);
        }
        frames
    }
}

pub fn ping() -> (Command, Vec<u8>) {
    (Command::Ping, Vec::new())
}

pub fn info() -> (Command, Vec<u8>) {
    (Command::Info, Vec::new())
}

pub fn clear(black: bool) -> (Command, Vec<u8>) {
    (Command::Clear, vec![u8::from(black)])
}

pub fn status() -> (Command, Vec<u8>) {
    (Command::Status, Vec::new())
}

pub fn sleep() -> (Command, Vec<u8>) {
    (Command::Sleep, Vec::new())
}

pub fn reset_bootsel() -> (Command, Vec<u8>) {
    (Command::ResetBootsel, Vec::new())
}

pub fn refresh(full: bool) -> (Command, Vec<u8>) {
    (Command::Refresh, vec![if full { 0 } else { 1 }])
}

pub fn img_begin() -> (Command, Vec<u8>) {
    let mut payload = Vec::with_capacity(5);
    payload.extend_from_slice(&800u16.to_le_bytes());
    payload.extend_from_slice(&480u16.to_le_bytes());
    payload.push(0);
    (Command::ImgBegin, payload)
}

pub fn img_end(partial: bool) -> (Command, Vec<u8>) {
    (Command::ImgEnd, vec![u8::from(partial)])
}

pub fn img_data(offset: u32, data: &[u8]) -> Result<(Command, Vec<u8>), ProtocolError> {
    if offset > FRAMEBUFFER_BYTES
        || offset as usize + data.len() > FRAMEBUFFER_BYTES as usize
        || data.len() > IMG_DATA_MAX
    {
        return err("image chunk out of range");
    }
    let mut payload = Vec::with_capacity(4 + data.len());
    payload.extend_from_slice(&offset.to_le_bytes());
    payload.extend_from_slice(data);
    Ok((Command::ImgData, payload))
}

pub fn blit(
    x_byte: u16,
    y: u16,
    w_bytes: u16,
    h: u16,
    bits: &[u8],
) -> Result<(Command, Vec<u8>), ProtocolError> {
    if !(x_byte < 100 && y < 480 && w_bytes > 0 && w_bytes <= 100 - x_byte && h > 0 && h <= 480 - y)
    {
        return err("BLIT out of bounds");
    }
    if bits.len() != w_bytes as usize * h as usize || bits.len() > MAX_PAYLOAD - BLIT_HEADER {
        return err("BLIT length mismatch");
    }
    let mut payload = Vec::with_capacity(BLIT_HEADER + bits.len());
    for field in [x_byte, y, w_bytes, h] {
        payload.extend_from_slice(&field.to_le_bytes());
    }
    payload.extend_from_slice(bits);
    Ok((Command::Blit, payload))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub busy: u8,
    pub partials_since_full: u16,
    pub ms_since_full: u32,
    pub bbox_valid: u8,
    pub x_byte0: u16,
    pub y0: u16,
    pub x_byte1: u16,
    pub y1: u16,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Status(busy={}, partials_since_full={}, ms_since_full={}, \
             bbox_valid={}, x_byte0={}, y0={}, x_byte1={}, y1={})",
            self.busy,
            self.partials_since_full,
            self.ms_since_full,
            self.bbox_valid,
            self.x_byte0,
            self.y0,
            self.x_byte1,
            self.y1,
        )
    }
}

/// Parse the STATUS payload. The device packs the fields without alignment
/// padding, matching Python's `struct.unpack('<BHIBHHHH', ...)`.
pub fn parse_status(payload: &[u8]) -> Result<Status, ProtocolError> {
    if payload.len() != 16 {
        return err("STATUS must contain 16 bytes");
    }
    let u16_at = |offset: usize| u16::from_le_bytes([payload[offset], payload[offset + 1]]);
    Ok(Status {
        busy: payload[0],
        partials_since_full: u16_at(1),
        ms_since_full: u32::from_le_bytes([payload[3], payload[4], payload[5], payload[6]]),
        bbox_valid: payload[7],
        x_byte0: u16_at(8),
        y0: u16_at(10),
        x_byte1: u16_at(12),
        y1: u16_at(14),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_vector() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
        assert_eq!(crc16(b""), 0xFFFF);
    }

    #[test]
    fn roundtrip_byte_at_a_time() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(256 * 16).collect();
        let frame = encode(Command::Blit as u8, 255, &payload).unwrap();
        let mut decoder = Decoder::new();
        let mut frames = Vec::new();
        for byte in frame {
            frames.extend(decoder.feed(&[byte]));
        }
        assert_eq!(
            frames,
            vec![Frame {
                frame_type: 0x13,
                seq: 255,
                payload,
            }]
        );
    }

    #[test]
    fn resync_past_garbage_damage_and_oversized_length() {
        let valid = encode(1, 2, b"").unwrap();
        let mut damaged = valid.clone();
        *damaged.last_mut().unwrap() ^= 1;
        let mut oversized = MAGIC.to_vec();
        oversized.extend_from_slice(&[1, 1]);
        oversized.extend_from_slice(&4097u16.to_le_bytes());

        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(b"garbage\xeb"), Vec::new());

        let mut rest = b"junk".to_vec();
        rest.extend_from_slice(&damaged);
        rest.extend_from_slice(&oversized);
        rest.extend_from_slice(&valid);
        assert_eq!(
            decoder.feed(&rest),
            vec![Frame {
                frame_type: 1,
                seq: 2,
                payload: Vec::new(),
            }]
        );
    }

    #[test]
    fn multiple_frames_in_one_read() {
        let one = encode(1, 0, b"").unwrap();
        let mut both = one.clone();
        both.extend_from_slice(&one);
        assert_eq!(Decoder::new().feed(&both).len(), 2);
    }

    #[test]
    fn blit_payload_layout() {
        let (_, payload) = blit(2, 16, 1, 16, &[b'0'; 16]).unwrap();
        let mut expected = Vec::new();
        for field in [2u16, 16, 1, 16] {
            expected.extend_from_slice(&field.to_le_bytes());
        }
        expected.extend_from_slice(&[b'0'; 16]);
        assert_eq!(payload, expected);
        assert!(blit(99, 0, 2, 1, b"xx").is_err());
    }

    #[test]
    fn status_field_offsets() {
        // struct.pack('<BHIBHHHH', 1, 2, 300, 1, 2, 16, 3, 32)
        let packed = [
            0x01, 0x02, 0x00, 0x2C, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x10, 0x00, 0x03, 0x00,
            0x20, 0x00,
        ];
        let status = parse_status(&packed).unwrap();
        assert_eq!(status.busy, 1);
        assert_eq!(status.partials_since_full, 2);
        assert_eq!(status.ms_since_full, 300);
        assert_eq!(status.bbox_valid, 1);
        assert_eq!(status.x_byte0, 2);
        assert_eq!(status.y0, 16);
        assert_eq!(status.x_byte1, 3);
        assert_eq!(status.y1, 32);
        assert!(parse_status(&packed[..15]).is_err());
    }

    #[test]
    fn payload_and_chunk_limits() {
        assert!(encode(1, 0, &vec![b'x'; 4097]).is_err());
        let (_, payload) = img_data(0, &[b'x'; IMG_DATA_MAX]).unwrap();
        assert_eq!(payload.len(), 4094);
        assert!(img_data(47999, b"xx").is_err());
    }
}
