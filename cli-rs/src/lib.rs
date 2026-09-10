//! Host-side driver for the Pico 2 attached Waveshare 7.5" e-paper panel.
//!
//! The binary is a thin argument-parsing shell over these modules; they are a
//! library so integration tests can exercise the wire format directly.

pub mod console;
pub mod font;
pub mod geometry;
pub mod image;
pub mod protocol;
pub mod render;
pub mod term;
pub mod transport;
