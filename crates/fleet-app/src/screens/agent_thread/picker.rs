//! The three small keyboard-only completion surfaces of the composer (§9).
//!
//! A picker is deliberately not a dialog: it never takes the keyboard away from the composer.
//! The trigger key opens it and advances the highlight, the composer's text after the trigger
//! filters it, `⏎` accepts the highlighted row and `esc` closes it. That is the whole surface,
//! which is why it needs no key context of its own.

use gpui::SharedString;

/// What a picker is completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerKind {
    /// `@` — a worktree file path.
    Files,
    /// `/` — a harness slash command. Offered at **line start only**: a harness expands a slash
    /// command only when it opens the whole message, and anywhere else it reaches the model as
    /// literal text, which is a whole class of "why didn't my command run?" bugs.
    Commands,
    /// `$` — a skill. Selecting one always inserts `$name`, never `/name`; the rewrite to the
    /// harness's own invocation happens at the daemon's adapter boundary.
    Skills,
    /// `^s m` — a model and effort.
    Models,
    /// `^s e` — the harness-declared traits of the selected model.
    Traits,
    /// `^s t` — the access ladder.
    Access,
}

impl PickerKind {
    /// The character the composer inserts before an accepted row, if any.
    pub(crate) const fn prefix(self) -> Option<char> {
        match self {
            Self::Files => Some('@'),
            Self::Commands => Some('/'),
            Self::Skills => Some('$'),
            Self::Models | Self::Traits | Self::Access => None,
        }
    }

    /// The picker opened by one composer trigger character.
    pub(crate) const fn for_trigger(symbol: char) -> Option<Self> {
        match symbol {
            '@' => Some(Self::Files),
            '/' => Some(Self::Commands),
            '$' => Some(Self::Skills),
            _ => None,
        }
    }

    /// Whether an accepted row replaces text in the composer rather than dispatching a control.
    pub(crate) const fn completes_text(self) -> bool {
        self.prefix().is_some()
    }

    /// The picker's heading.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Commands => "commands",
            Self::Skills => "skills",
            Self::Models => "model",
            Self::Traits => "traits",
            Self::Access => "access mode",
        }
    }
}

/// How many rows a picker shows, which is what keeps it small enough to sit over the composer.
pub(crate) const VISIBLE: usize = 6;

/// One open completion surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picker {
    /// What is being completed.
    pub(crate) kind: PickerKind,
    /// Every candidate, unfiltered.
    candidates: Vec<String>,
    /// The highlighted row within the filtered list.
    highlight: usize,
    /// The substring typed after the trigger.
    query: String,
}

impl Picker {
    /// Opens a picker over `candidates`.
    pub(crate) fn new(kind: PickerKind, candidates: Vec<String>) -> Self {
        Self {
            kind,
            candidates,
            highlight: 0,
            query: String::new(),
        }
    }

    /// Narrows the picker with the text typed after the trigger.
    pub(crate) fn filter(&mut self, query: &str) {
        if self.query != query {
            self.query = query.to_owned();
            self.highlight = 0;
        }
    }

    /// The rows currently shown, longest-prefix matches first.
    pub(crate) fn matches(&self) -> Vec<&str> {
        let query = self.query.to_ascii_lowercase();
        self.candidates
            .iter()
            .filter(|candidate| {
                query.is_empty() || candidate.to_ascii_lowercase().contains(query.as_str())
            })
            .map(String::as_str)
            .take(VISIBLE)
            .collect()
    }

    /// Pressing the trigger again moves to the next row, wrapping.
    pub(crate) fn advance(&mut self) {
        let len = self.matches().len();
        if len == 0 {
            self.highlight = 0;
            return;
        }
        self.highlight = (self.highlight + 1) % len;
    }

    /// `↑` / `ctrl-p`: the previous row, wrapping.
    ///
    /// DESIGN-SYSTEM §4: "a list **under a text field** moves with `ctrl-n`/`ctrl-p` or
    /// `↓`/`↑`". Both directions have to exist, or `↑` reads as `↓`.
    pub(crate) fn retreat(&mut self) {
        let len = self.matches().len();
        if len == 0 {
            self.highlight = 0;
            return;
        }
        self.highlight = (self.highlight + len - 1) % len;
    }

    /// The highlighted row index.
    pub(crate) const fn highlight(&self) -> usize {
        self.highlight
    }

    /// The row `⏎` accepts.
    pub(crate) fn accepted(&self) -> Option<SharedString> {
        self.matches()
            .get(self.highlight)
            .map(|row| SharedString::from((*row).to_owned()))
    }

    /// Whether the picker has anything to show.
    pub(crate) fn is_empty(&self) -> bool {
        self.matches().is_empty()
    }
}
