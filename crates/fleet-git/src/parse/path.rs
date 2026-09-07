use std::path::{Path, PathBuf};

use crate::{GitError, Result};

pub(crate) fn diff_git_paths(value: &[u8]) -> Result<(PathBuf, PathBuf)> {
    let (old, new) = if value.starts_with(b"\"") {
        let (old, rest) = quoted(value)?;
        let rest = rest.strip_prefix(b" ").ok_or_else(|| {
            GitError::parse("diff path", "missing separator between quoted paths")
        })?;
        let new = exact_path(rest)?;
        (old, new)
    } else {
        split_unquoted_diff_paths(value)?
    };
    Ok((strip_side(old, b'a'), strip_side(new, b'b')))
}

pub(crate) fn header_path(value: &[u8]) -> Result<Option<PathBuf>> {
    let bytes = if value.starts_with(b"\"") {
        let (bytes, trailing) = quoted(value)?;
        if !trailing.is_empty() && !trailing.starts_with(b"\t") {
            return Err(GitError::parse(
                "diff path",
                "trailing data after quoted path",
            ));
        }
        bytes
    } else {
        value
            .split(|byte| *byte == b'\t')
            .next()
            .unwrap_or(value)
            .to_vec()
    };
    if bytes == b"/dev/null" {
        return Ok(None);
    }
    Ok(Some(strip_any_side(bytes)))
}

pub(crate) fn extended_header_path(value: &[u8]) -> Result<PathBuf> {
    let bytes = if value.starts_with(b"\"") {
        let (bytes, trailing) = quoted(value)?;
        if !trailing.is_empty() {
            return Err(GitError::parse(
                "diff path",
                "trailing data after quoted path",
            ));
        }
        bytes
    } else {
        value.to_vec()
    };
    Ok(crate::parse::status::bytes_to_path(&bytes))
}

pub(crate) fn prefixed_path(prefix: &[u8], path: &Path) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&path_bytes(path));
    quote(&bytes)
}

fn split_unquoted_diff_paths(value: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    if let Some(split) = value.windows(4).position(|window| window == b" \"b/") {
        let old = value[..split].to_vec();
        let new = exact_path(&value[split + 1..])?;
        if !old.starts_with(b"a/") {
            return Err(GitError::parse("diff path", "invalid old path prefix"));
        }
        return Ok((old, new));
    }
    let candidates = value
        .windows(3)
        .enumerate()
        .filter(|(_, window)| *window == b" b/")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let split = candidates
        .iter()
        .copied()
        .find(|index| {
            value
                .get(2..*index)
                .zip(value.get(index + 3..))
                .is_some_and(|(old, new)| old == new)
        })
        .or_else(|| candidates.first().copied())
        .ok_or_else(|| GitError::parse("diff path", "missing b/ path"))?;
    let old = value[..split].to_vec();
    let new = value[split + 1..].to_vec();
    if !old.starts_with(b"a/") || !new.starts_with(b"b/") {
        return Err(GitError::parse("diff path", "invalid path side prefix"));
    }
    Ok((old, new))
}

fn exact_path(value: &[u8]) -> Result<Vec<u8>> {
    if value.starts_with(b"\"") {
        let (bytes, trailing) = quoted(value)?;
        if !trailing.is_empty() {
            return Err(GitError::parse("diff path", "trailing data after path"));
        }
        Ok(bytes)
    } else {
        Ok(value.to_vec())
    }
}

fn quoted(value: &[u8]) -> Result<(Vec<u8>, &[u8])> {
    let mut bytes = Vec::new();
    let mut index = 1;
    while let Some(&byte) = value.get(index) {
        if byte == b'"' {
            return Ok((bytes, &value[index + 1..]));
        }
        if byte != b'\\' {
            bytes.push(byte);
            index += 1;
            continue;
        }
        index += 1;
        let escaped = *value
            .get(index)
            .ok_or_else(|| GitError::parse("diff path", "unterminated escape"))?;
        match escaped {
            b'a' => bytes.push(0x07),
            b'b' => bytes.push(0x08),
            b't' => bytes.push(b'\t'),
            b'n' => bytes.push(b'\n'),
            b'v' => bytes.push(0x0b),
            b'f' => bytes.push(0x0c),
            b'r' => bytes.push(b'\r'),
            b'"' | b'\\' => bytes.push(escaped),
            b'0'..=b'7' => {
                let mut decoded = escaped - b'0';
                for _ in 0..2 {
                    let Some(next @ b'0'..=b'7') = value.get(index + 1).copied() else {
                        break;
                    };
                    index += 1;
                    decoded = decoded.wrapping_mul(8).wrapping_add(next - b'0');
                }
                bytes.push(decoded);
            }
            _ => {
                return Err(GitError::parse(
                    "diff path",
                    format!("unsupported escape \\{}", char::from(escaped)),
                ));
            }
        }
        index += 1;
    }
    Err(GitError::parse("diff path", "unterminated quoted path"))
}

fn quote(bytes: &[u8]) -> Vec<u8> {
    if !bytes
        .iter()
        .any(|byte| *byte < 0x20 || *byte >= 0x7f || matches!(*byte, b'"' | b'\\'))
    {
        return bytes.to_vec();
    }
    let mut quoted = Vec::with_capacity(bytes.len() + 2);
    quoted.push(b'"');
    for &byte in bytes {
        match byte {
            b'\t' => quoted.extend_from_slice(b"\\t"),
            b'\n' => quoted.extend_from_slice(b"\\n"),
            b'\r' => quoted.extend_from_slice(b"\\r"),
            b'"' => quoted.extend_from_slice(b"\\\""),
            b'\\' => quoted.extend_from_slice(b"\\\\"),
            0x20..=0x7e => quoted.push(byte),
            _ => quoted.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
    }
    quoted.push(b'"');
    quoted
}

fn strip_any_side(bytes: Vec<u8>) -> PathBuf {
    if bytes.starts_with(b"a/") || bytes.starts_with(b"b/") {
        crate::parse::status::bytes_to_path(&bytes[2..])
    } else {
        crate::parse::status::bytes_to_path(&bytes)
    }
}

fn strip_side(bytes: Vec<u8>, side: u8) -> PathBuf {
    if bytes.starts_with(&[side, b'/']) {
        crate::parse::status::bytes_to_path(&bytes[2..])
    } else {
        crate::parse::status::bytes_to_path(&bytes)
    }
}

fn path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().into()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes().into()
    }
}
