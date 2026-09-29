//! UTF-16 in and out of Win32.

/// A NUL-terminated UTF-16 copy of `s`.
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The string in a UTF-16 buffer, up to its first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_utf16() {
        let w = to_wide("Grüezi ▸ 大福");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "Grüezi ▸ 大福");
    }

    #[test]
    fn stops_at_the_first_nul() {
        assert_eq!(from_wide(&[0x61, 0x62, 0, 0x63]), "ab");
        assert_eq!(from_wide(&[0x61]), "a");
    }
}
