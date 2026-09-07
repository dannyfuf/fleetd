//! Parsers for Git's stable machine-readable formats.

use std::borrow::Cow;

pub(crate) mod commits;
pub mod diff;
pub(crate) mod path;
pub(crate) mod refs;
pub(crate) mod status;

/// Converts display-oriented Git bytes without collapsing distinct invalid UTF-8 identities.
pub(crate) fn text(bytes: &[u8]) -> Cow<'_, str> {
    let mut error = match std::str::from_utf8(bytes) {
        Ok(text) => return Cow::Borrowed(text),
        Err(error) => error,
    };
    let mut rendered = String::with_capacity(bytes.len());
    let mut remaining = bytes;
    loop {
        let valid = error.valid_up_to();
        rendered.push_str(&String::from_utf8_lossy(&remaining[..valid]));
        let invalid = error.error_len().unwrap_or(remaining.len() - valid);
        for byte in &remaining[valid..valid + invalid] {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            rendered.push('\\');
            rendered.push('x');
            rendered.push(char::from(HEX[usize::from(byte >> 4)]));
            rendered.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        remaining = &remaining[valid + invalid..];
        if remaining.is_empty() {
            break;
        }
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                rendered.push_str(valid);
                break;
            }
            Err(next) => error = next,
        }
    }
    Cow::Owned(rendered)
}
