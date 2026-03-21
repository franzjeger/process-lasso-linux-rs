//! Shared app icon data (embedded at compile time from assets/icon.png).
pub const RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon_rgba.bin"));
pub const W: u32 = 64;
pub const H: u32 = 64;
