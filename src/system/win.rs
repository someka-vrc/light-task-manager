//! Small conversions needed wherever Win32 is called.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

/// FILETIME ticks (100 ns) in one second.
pub const SEC: i64 = 10_000_000;

/// A null-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The text of a UTF-16 buffer up to its first null.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// The current time as a FILETIME value.
pub fn filetime_now() -> i64 {
    let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// Converts 0xRRGGBB to a GDI COLORREF (0x00BBGGRR).
pub fn colorref(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}
