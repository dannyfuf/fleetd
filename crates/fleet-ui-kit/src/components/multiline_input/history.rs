use super::HISTORY_LIMIT;

/// The submitted prompts `↑` and `↓` walk, plus the draft they were opened from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: Option<String>,
}

impl PromptHistory {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded prompts, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Whether the composer is currently showing a recalled prompt.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.cursor.is_some()
    }

    /// Record a submitted prompt and leave navigation at the live draft.
    pub fn push(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.reset();
        if text.trim().is_empty() || self.entries.last() == Some(&text) {
            return;
        }
        self.entries.push(text);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
    }

    /// Forget where the walk was; the next `↑` starts from the newest entry again.
    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft = None;
    }

    /// Return the previous prompt, remembering `current` as the initial draft.
    pub fn previous(&mut self, current: &str) -> Option<String> {
        let index = match self.cursor {
            None => {
                self.draft = Some(current.to_owned());
                self.entries.len().checked_sub(1)?
            }
            Some(0) => return None,
            Some(index) => index - 1,
        };
        self.cursor = Some(index);
        Some(self.entries[index].clone())
    }

    /// Return the next prompt, or restore the draft that opened the walk.
    pub fn newer(&mut self) -> Option<String> {
        let index = self.cursor?;
        if index + 1 < self.entries.len() {
            self.cursor = Some(index + 1);
            return Some(self.entries[index + 1].clone());
        }
        self.cursor = None;
        Some(self.draft.take().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_drops_blanks_and_adjacent_duplicates() {
        let mut history = PromptHistory::new();
        history.push("fix the rounding");
        history.push("fix the rounding");
        history.push("   ");
        history.push("");
        history.push("now the tz shifts");
        assert_eq!(history.entries(), ["fix the rounding", "now the tz shifts"]);
        assert!(!history.is_active());
    }

    #[test]
    fn history_keeps_the_last_hundred_prompts() {
        let mut history = PromptHistory::new();
        for index in 0..HISTORY_LIMIT + 5 {
            history.push(format!("prompt {index}"));
        }
        assert_eq!(history.entries().len(), HISTORY_LIMIT);
        assert_eq!(history.entries()[0], "prompt 5");
        assert_eq!(history.entries()[HISTORY_LIMIT - 1], "prompt 104");
    }

    #[test]
    fn history_walks_back_then_restores_the_draft() {
        let mut history = PromptHistory::new();
        history.push("first");
        history.push("second");

        assert_eq!(history.previous("draft").as_deref(), Some("second"));
        assert_eq!(history.previous("ignored").as_deref(), Some("first"));
        assert_eq!(history.previous("ignored"), None);
        assert_eq!(history.newer().as_deref(), Some("second"));
        assert_eq!(history.newer().as_deref(), Some("draft"));
        assert!(!history.is_active());
    }

    #[test]
    fn history_walk_on_an_empty_list_does_nothing() {
        let mut history = PromptHistory::new();
        assert_eq!(history.previous("draft"), None);
        assert!(!history.is_active());
    }
}
