use gpui::{Keystroke, Modifiers};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Keys(Vec<Keystroke>),
    Type(String),
    Scroll {
        /// Sideways wheel units, GPUI-signed: positive scrolls the content right.
        dx: f32,
        /// Vertical wheel units, GPUI-signed: negative scrolls the content down.
        dy: f32,
        /// Where the pointer sits, as a fraction of the window's width.
        x: f32,
        /// Where the pointer sits, as a fraction of the window's height.
        y: f32,
    },
    Wait(Duration),
    Shot(PathBuf),
    Quit,
}

pub(super) fn parse(line: &str) -> Result<Option<Step>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    let rest = rest.trim();
    match command {
        "key" => {
            if rest.is_empty() {
                return Err("key needs at least one keystroke".to_owned());
            }
            let mut keys = Vec::new();
            for token in rest.split_whitespace() {
                let keystroke = Keystroke::parse(token)
                    .map_err(|error| format!("bad keystroke {token:?}: {error}"))?;
                keys.push(keystroke);
            }
            Ok(Some(Step::Keys(keys)))
        }
        // `type` keeps its argument verbatim, spaces included.
        "type" => {
            let text = line.trim_start().strip_prefix("type").unwrap_or("");
            let text = text.strip_prefix(' ').unwrap_or(text);
            if text.is_empty() {
                return Err("type needs some text".to_owned());
            }
            Ok(Some(Step::Type(text.to_owned())))
        }
        // `scroll <dx> <dy> [x-fraction] [y-fraction]`, spelled and signed exactly as the
        // harness `scroll` line in `docs/TESTING-HARNESS.md` §2 — the old `wheel N` is
        // `scroll 0 -N` and `hwheel N` is `scroll -N 0`. The default position is the main
        // panel: three quarters across, half way down.
        "scroll" => {
            let mut parts = rest.split_whitespace();
            let mut axis = |what: &str| -> Result<f32, String> {
                parts
                    .next()
                    .and_then(|token| token.parse::<f32>().ok())
                    .filter(|value: &f32| value.is_finite())
                    .ok_or_else(|| format!("scroll needs a finite {what}, got {rest:?}"))
            };
            let dx = axis("dx")?;
            let dy = axis("dy")?;
            let x = parts
                .next()
                .and_then(|token| token.parse::<f32>().ok())
                .unwrap_or(0.75);
            let y = parts
                .next()
                .and_then(|token| token.parse::<f32>().ok())
                .unwrap_or(0.5);
            Ok(Some(Step::Scroll { dx, dy, x, y }))
        }
        "wait" => rest
            .parse::<u64>()
            .map(|millis| Some(Step::Wait(Duration::from_millis(millis))))
            .map_err(|_| format!("wait needs a millisecond count, got {rest:?}")),
        "shot" => {
            if rest.is_empty() {
                return Err("shot needs a path".to_owned());
            }
            Ok(Some(Step::Shot(PathBuf::from(rest))))
        }
        "quit" => Ok(Some(Step::Quit)),
        other => Err(format!("unknown command {other:?}")),
    }
}

pub(crate) fn keystroke_for(character: char) -> Keystroke {
    let (key, shift) = match character {
        ' ' => ("space".to_owned(), false),
        '\t' => ("tab".to_owned(), false),
        upper if upper.is_ascii_uppercase() => (upper.to_ascii_lowercase().to_string(), true),
        other => match unshifted_ascii(other) {
            Some(unshifted) => (unshifted.to_string(), true),
            None => (other.to_string(), false),
        },
    };
    Keystroke {
        modifiers: Modifiers {
            shift,
            ..Modifiers::none()
        },
        key,
        key_char: (character != '\t').then(|| character.to_string()),
    }
}

// Synthetic dispatch bypasses platform keyboard-layout translation. Supply the composed
// printable character while leaving control/option/command chords untouched.
pub(crate) fn with_simulated_key_char(mut keystroke: Keystroke) -> Keystroke {
    if keystroke.key_char.is_some()
        || keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.platform
    {
        return keystroke;
    }
    keystroke.key_char = match keystroke.key.as_str() {
        "space" => Some(" ".to_owned()),
        key => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => Some(if keystroke.modifiers.shift {
                    shifted_ascii(character).unwrap_or(character).to_string()
                } else {
                    character.to_string()
                }),
                _ => None,
            }
        }
    };
    keystroke
}

const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
const UNSHIFTED: &str = "`1234567890-=[]\\;',./";

fn unshifted_ascii(character: char) -> Option<char> {
    SHIFTED
        .chars()
        .position(|candidate| candidate == character)
        .and_then(|index| UNSHIFTED.chars().nth(index))
}

fn shifted_ascii(character: char) -> Option<char> {
    if character.is_ascii_lowercase() {
        return Some(character.to_ascii_uppercase());
    }
    UNSHIFTED
        .chars()
        .position(|candidate| candidate == character)
        .and_then(|index| SHIFTED.chars().nth(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_commands_are_reported_rather_than_guessed() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("   "), Ok(None));
        assert_eq!(parse("# a note"), Ok(None));
        assert_eq!(parse("key"), Err("key needs at least one keystroke".into()));
        assert_eq!(parse("type"), Err("type needs some text".into()));
        assert_eq!(parse("shot"), Err("shot needs a path".into()));
        assert_eq!(
            parse("wait soon"),
            Err("wait needs a millisecond count, got \"soon\"".into())
        );
        assert_eq!(parse("nope"), Err("unknown command \"nope\"".into()));
        // The old spellings are gone rather than quietly aliased, so a stale script says so.
        assert_eq!(parse("wheel 5"), Err("unknown command \"wheel\"".into()));
        assert_eq!(parse("hwheel 5"), Err("unknown command \"hwheel\"".into()));
        assert!(parse("scroll").is_err());
        assert!(parse("scroll 0").is_err());
        assert!(parse("scroll 0 nope").is_err());
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(
            parse("wait 250"),
            Ok(Some(Step::Wait(Duration::from_millis(250))))
        );
        assert_eq!(
            parse("shot /tmp/a.png"),
            Ok(Some(Step::Shot(PathBuf::from("/tmp/a.png"))))
        );
        assert_eq!(parse("quit"), Ok(Some(Step::Quit)));
        // `type` keeps every space after the first separator.
        assert_eq!(
            parse("type hello  world  "),
            Ok(Some(Step::Type("hello  world  ".into())))
        );
        assert_eq!(
            parse("type  hello  "),
            Ok(Some(Step::Type(" hello  ".into())))
        );
        let Ok(Some(Step::Keys(keys))) = parse("key ctrl-s ?") else {
            panic!("expected keystrokes");
        };
        assert_eq!(keys.len(), 2);
        assert!(keys[0].modifiers.control);
        assert_eq!(keys[0].key, "s");
        assert_eq!(keys[1].key, "?");
    }

    #[test]
    fn scroll_defaults_to_the_main_panel_and_reads_both_axes() {
        // `wheel 5` used to mean "five rows down"; its translation is `scroll 0 -5`.
        assert_eq!(
            parse("scroll 0 -5 nope nope"),
            Ok(Some(Step::Scroll {
                dx: 0.0,
                dy: -5.0,
                x: 0.75,
                y: 0.5
            }))
        );
        assert_eq!(
            parse("scroll 0 2 0.5 0.25"),
            Ok(Some(Step::Scroll {
                dx: 0.0,
                dy: 2.0,
                x: 0.5,
                y: 0.25
            }))
        );
        // `hwheel 3` used to mean "three cells right"; its translation is `scroll -3 0`.
        assert_eq!(
            parse("scroll -3 0"),
            Ok(Some(Step::Scroll {
                dx: -3.0,
                dy: 0.0,
                x: 0.75,
                y: 0.5
            }))
        );
    }

    #[test]
    fn typed_and_dispatched_keystrokes_carry_composed_characters() {
        let typed = keystroke_for('A');
        assert_eq!(typed.key, "a");
        assert!(typed.modifiers.shift);
        assert_eq!(typed.key_char.as_deref(), Some("A"));
        assert_eq!(keystroke_for(' ').key, "space");

        let colon = keystroke_for(':');
        assert_eq!(colon.key, ";");
        assert!(colon.modifiers.shift);
        assert_eq!(colon.key_char.as_deref(), Some(":"));

        let simulated = |keys| with_simulated_key_char(Keystroke::parse(keys).expect("keystroke"));
        assert_eq!(simulated("a").key_char.as_deref(), Some("a"));
        assert_eq!(simulated("A").key_char.as_deref(), Some("A"));
        assert_eq!(simulated("shift-;").key_char.as_deref(), Some(":"));
        assert_eq!(simulated("ctrl-d").key_char, None);
    }
}
