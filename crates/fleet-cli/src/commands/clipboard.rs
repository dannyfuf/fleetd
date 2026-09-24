//! OSC 52 clipboard writes that never depend on a Fleet daemon.

use crate::args::{ClipboardArgs, ClipboardCommand, ClipboardCopyArgs};
use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fleet_proto::TERMINAL_CLIPBOARD_MAX_BYTES;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
};

const OSC52_PREFIX: &[u8] = b"\x1b]52;c;";
const OSC52_SUFFIX: u8 = 0x07;

pub(super) fn run(arguments: ClipboardArgs) -> Result<()> {
    let mut terminal = open_controlling_terminal()?;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    copy_to(arguments, &mut input, &mut terminal)
}

fn open_controlling_terminal() -> Result<File> {
    OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("could not open controlling terminal /dev/tty")
}

fn copy_to(arguments: ClipboardArgs, input: &mut impl Read, output: &mut impl Write) -> Result<()> {
    let ClipboardCommand::Copy(arguments) = arguments.command;
    let text = read_text(arguments, input)?;
    write_osc52(&text, output)
}

fn read_text(arguments: ClipboardCopyArgs, input: &mut impl Read) -> Result<Vec<u8>> {
    if let Some(text) = arguments.text {
        return Ok(text.into_bytes());
    }

    let mut text = Vec::new();
    input
        .take((TERMINAL_CLIPBOARD_MAX_BYTES + 1) as u64)
        .read_to_end(&mut text)
        .context("could not read clipboard text from standard input")?;
    Ok(text)
}

fn write_osc52(text: &[u8], output: &mut impl Write) -> Result<()> {
    if text.len() > TERMINAL_CLIPBOARD_MAX_BYTES {
        bail!(
            "clipboard input is {} bytes; maximum is {} bytes",
            text.len(),
            TERMINAL_CLIPBOARD_MAX_BYTES
        );
    }
    std::str::from_utf8(text).context("clipboard input is not valid UTF-8")?;

    let encoded = STANDARD.encode(text);
    let mut sequence = Vec::with_capacity(OSC52_PREFIX.len() + encoded.len() + 1);
    sequence.extend_from_slice(OSC52_PREFIX);
    sequence.extend_from_slice(encoded.as_bytes());
    sequence.push(OSC52_SUFFIX);
    output
        .write_all(&sequence)
        .context("could not write OSC 52 clipboard sequence")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn assert_sequence(text: &[u8], expected: &[u8]) {
        let mut output = Vec::new();
        write_osc52(text, &mut output).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(output, expected);
    }

    #[test]
    fn osc52_bytes_are_exact_for_ascii_unicode_and_whitespace() {
        for (text, expected) in [
            (&b"a b"[..], &b"\x1b]52;c;YSBi\x07"[..]),
            ("café 🙂".as_bytes(), &b"\x1b]52;c;Y2Fmw6kg8J+Zgg==\x07"[..]),
            (
                &b"\nline one\tline two\n"[..],
                &b"\x1b]52;c;CmxpbmUgb25lCWxpbmUgdHdvCg==\x07"[..],
            ),
            (&b"  padded  "[..], &b"\x1b]52;c;ICBwYWRkZWQgIA==\x07"[..]),
            (&b""[..], &b"\x1b]52;c;\x07"[..]),
        ] {
            assert_sequence(text, expected);
        }
    }

    #[test]
    fn exact_cap_is_accepted() {
        let text = vec![b'a'; TERMINAL_CLIPBOARD_MAX_BYTES];
        let mut output = Vec::new();
        write_osc52(&text, &mut output).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(output.first(), Some(&0x1b));
        assert_eq!(output.last(), Some(&OSC52_SUFFIX));
    }

    #[test]
    fn cap_plus_one_is_rejected_before_writing() {
        let text = vec![b'a'; TERMINAL_CLIPBOARD_MAX_BYTES + 1];
        let mut output = Vec::new();
        let error = write_osc52(&text, &mut output).unwrap_err();
        assert!(error.to_string().contains("maximum is 1048576 bytes"));
        assert!(output.is_empty());
    }

    #[test]
    fn invalid_utf8_is_rejected_before_writing() {
        let mut output = Vec::new();
        let error = write_osc52(&[0xff], &mut output).unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8"));
        assert!(output.is_empty());
    }

    #[test]
    fn argument_text_takes_precedence_and_stdin_is_preserved_verbatim() {
        let mut argument_output = Vec::new();
        copy_to(
            ClipboardArgs {
                command: ClipboardCommand::Copy(ClipboardCopyArgs {
                    text: Some(" argument ".to_owned()),
                }),
            },
            &mut &b"stdin"[..],
            &mut argument_output,
        )
        .unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(argument_output, b"\x1b]52;c;IGFyZ3VtZW50IA==\x07");

        let mut stdin_output = Vec::new();
        copy_to(
            ClipboardArgs {
                command: ClipboardCommand::Copy(ClipboardCopyArgs { text: None }),
            },
            &mut &b"\tstdin\n"[..],
            &mut stdin_output,
        )
        .unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(stdin_output, b"\x1b]52;c;CXN0ZGluCg==\x07");
    }
}
