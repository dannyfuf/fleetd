use gpui::SharedString;

use crate::InputBuffer;

/// A completion surface the composer asked for, and what has been typed into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// Which surface: `@` files, `$` skills, `/` commands.
    pub symbol: char,
    /// The byte offset of the symbol itself.
    pub at: usize,
    /// Everything typed between the symbol and the caret.
    pub query: SharedString,
}

/// The completion token containing the caret.
pub(super) fn active_trigger(buffer: &InputBuffer) -> Option<Trigger> {
    let caret = buffer.caret();
    let line_start = buffer.line_start(caret);
    let token_start = buffer.text()[line_start..caret]
        .rfind(char::is_whitespace)
        .map_or(line_start, |index| line_start + index + 1);
    let token = &buffer.text()[token_start..caret];
    let symbol = token.chars().next()?;
    if !matches!(symbol, '@' | '/' | '$') || (symbol == '/' && token_start != line_start) {
        return None;
    }
    Some(Trigger {
        symbol,
        at: token_start,
        query: SharedString::from(token[symbol.len_utf8()..].to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputMode;

    fn buffer(text: &str) -> InputBuffer {
        InputBuffer::from_text(
            InputMode::Multiline {
                min_rows: 1,
                max_rows: 8,
            },
            text,
        )
    }

    #[test]
    fn active_triggers_follow_token_and_line_rules() {
        let trigger = active_trigger(&buffer("read @src/li")).expect("inside an @ token");
        assert_eq!(trigger.symbol, '@');
        assert_eq!(trigger.query, "src/li");
        assert_eq!(trigger.at, "read ".len());

        assert!(active_trigger(&buffer("read @src/lib.rs and")).is_none());
        assert_eq!(
            active_trigger(&buffer("$rev")).map(|trigger| trigger.query),
            Some(SharedString::from("rev"))
        );
        assert_eq!(
            active_trigger(&buffer("/mod")).map(|trigger| trigger.query),
            Some(SharedString::from("mod"))
        );
        assert!(active_trigger(&buffer("cd /mod")).is_none());
        assert!(active_trigger(&buffer("plain")).is_none());
    }
}
