//! Modal dialogs, the command palette and the filter bar (UX-SPEC §3.8–§3.10).
//!
//! # Extension point
//!
//! [`Dialogs`] is the **open dialog**, held by [`crate::state::Overlay::Dialog`]. The shell
//! owns opening and closing it, gives it the `Dialog > <name>` key context returned by
//! [`Dialogs::context_name`], and calls [`Dialogs::render`] inside the frame's overlay layer.
//!
//! # Where dialog state lives
//!
//! The variant names of [`Dialogs`] are frozen (`docs/APP-CONTRACTS.md`: `keymap.rs` binds
//! against the words they produce) and the shell constructs several of them by name, so the
//! enum stays payload-free. Everything a dialog edits — the branch being typed, the clone
//! search results, the settings draft — lives in [`DialogHost`], a gpui global owned by this
//! module. That keeps every mutation inside `dialogs/` while [`crate::state::AppState`] stays
//! exactly the shape the shell froze.
//!
//! [`DialogHost::open`] is the dialog the host is currently seeded for. A dialog is re-seeded
//! whenever it differs, and an observer on the `AppState` entity clears it the moment the
//! overlay closes, so re-opening the same dialog always starts clean.
//!
//! # Threading
//!
//! Nothing here blocks. Every daemon call goes through [`Bridge`]; answers arrive in a
//! `cx.spawn` continuation that re-checks a per-dialog sequence number before it touches the
//! host, so a late answer to a superseded request is dropped instead of overwriting fresh
//! state.

pub mod assign_repo;
pub mod clone_repo;
pub mod confirm;
pub mod context;
pub mod create_worktree;
pub mod filter;
pub mod help;
pub mod palette;
pub mod quit;
pub mod settings;

use std::time::{SystemTime, UNIX_EPOCH};

use fleet_core::ids::RepoId;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{
    AnyElement, App, Div, Entity, FocusHandle, Global, KeyDownEvent, Pixels, Subscription, Window,
    div, px,
};

use crate::{
    bridge::Bridge,
    state::{AppState, Overlay, RepoScope},
};

pub use confirm::ConfirmRequest;

/// Which dialog is open (§3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialogs {
    /// §3.8.1 Create worktree (`n` in the worktrees pane).
    CreateWorktree,
    /// §3.8.2 Clone repo (`n` in the repos pane).
    CloneRepo,
    /// §3.8.3 Confirm — delete / prune / kill / close terminal.
    Confirm,
    /// §3.8.4 New context (`N`).
    NewContext,
    /// §3.8.4 Edit context (`E`).
    EditContext,
    /// §3.8.5 Assign repo to context (`m`).
    AssignRepo,
    /// §3.8.6 Settings (`,`).
    Settings,
    /// §3.8.7 Help (`?`).
    Help,
    /// §3.8.8 Quit (`ctrl-q`) with work still running.
    Quit,
    /// §3.8.9 Quit and stop the daemon (`ctrl-shift-q`).
    QuitDaemon,
}

impl Dialogs {
    /// The second half of this dialog's key context, i.e. `Dialog > <name>`.
    ///
    /// `NewContext` and `EditContext` share `Context` because `docs/KEYMAP.md` gives them one
    /// row (`ctrl-d` deletes from either).
    #[must_use]
    pub const fn context_name(&self) -> &'static str {
        match self {
            Self::CreateWorktree => "Create",
            Self::CloneRepo => "Clone",
            Self::Confirm => "Confirm",
            Self::NewContext | Self::EditContext => "Context",
            Self::AssignRepo => "Assign",
            Self::Settings => "Settings",
            Self::Help => "Help",
            Self::Quit => "Quit",
            Self::QuitDaemon => "QuitDaemon",
        }
    }

    /// The dialog's title, used by the header and by the palette.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::CreateWorktree => "New worktree",
            Self::CloneRepo => "Clone repository",
            Self::Confirm => "Confirm",
            Self::NewContext => "New context",
            Self::EditContext => "Edit context",
            Self::AssignRepo => "Move repo to context",
            Self::Settings => "Settings",
            Self::Help => "Keymap",
            Self::Quit => "Quit Fleet?",
            Self::QuitDaemon => "Stop fleetd and quit?",
        }
    }

    /// The header glyph §3.8 assigns to this dialog.
    #[must_use]
    pub const fn icon(&self) -> Icon {
        match self {
            Self::CreateWorktree => Icon::GitBranchPlus,
            Self::CloneRepo => Icon::CloudDownload,
            Self::Confirm => Icon::TriangleAlert,
            Self::NewContext | Self::EditContext => Icon::Boxes,
            Self::AssignRepo => Icon::ArrowRightLeft,
            Self::Settings => Icon::Settings2,
            Self::Help | Self::Quit => Icon::CircleQuestionMark,
            Self::QuitDaemon => Icon::Power,
        }
    }

    /// The card width §3.8 fixes for this dialog. Confirms size themselves from their facts.
    #[must_use]
    pub fn width(&self) -> Pixels {
        match self {
            Self::CreateWorktree | Self::CloneRepo | Self::QuitDaemon => px(560.0),
            Self::Confirm => px(480.0),
            Self::NewContext | Self::EditContext => px(460.0),
            Self::AssignRepo => px(460.0),
            Self::Settings => px(720.0),
            Self::Help => px(880.0),
            Self::Quit => px(520.0),
        }
    }

    /// Renders the dialog into the frame's overlay layer.
    ///
    /// The shell has already applied the `Dialog > <name>` key context above this element and
    /// handles `Esc` (`dialog::Cancel`) by closing the dialog. The returned root element always
    /// tracks `focus`, which is what puts the `on_action` listeners below on the key-dispatch
    /// path.
    pub fn render(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        watch(state, cx);
        seed(self, state, bridge, cx);
        match self {
            Self::CreateWorktree => create_worktree::render(state, bridge, focus, window, cx),
            Self::CloneRepo => clone_repo::render(state, bridge, focus, window, cx),
            Self::Confirm => confirm::render(state, bridge, focus, window, cx),
            Self::NewContext | Self::EditContext => {
                context::render(self, state, bridge, focus, window, cx)
            }
            Self::AssignRepo => assign_repo::render(state, bridge, focus, window, cx),
            Self::Settings => settings::render(state, bridge, focus, window, cx),
            Self::Help => help::render(state, focus, window, cx),
            Self::Quit => quit::render_quit(state, focus, window, cx),
            Self::QuitDaemon => quit::render_quit_daemon(state, focus, window, cx),
        }
    }
}

// ---------------------------------------------------------------------------- the host global

/// Every dialog's mutable draft, plus the dialog the drafts were seeded for.
#[derive(Debug, Default)]
pub struct DialogHost {
    /// The dialog the drafts below belong to, or `None` when no dialog is open.
    pub open: Option<Dialogs>,
    /// Whether the palette's draft has been seeded for the currently open palette.
    pub palette_open: bool,
    /// §3.8.1.
    pub create: create_worktree::CreateState,
    /// §3.8.2.
    pub clone: clone_repo::CloneState,
    /// §3.8.3.
    pub confirm: confirm::ConfirmState,
    /// §3.8.4.
    pub context: context::ContextState,
    /// §3.8.5.
    pub assign: assign_repo::AssignState,
    /// §3.8.6.
    pub settings: settings::SettingsState,
    /// §3.9.
    pub palette: palette::PaletteState,
    /// What the next Confirm dialog asks about, published by whoever opens it.
    pub pending_confirm: Option<ConfirmRequest>,
}

impl Global for DialogHost {}

/// Keeps the `AppState` observer alive for the lifetime of the app.
#[derive(Default)]
struct DialogWatch {
    subscription: Option<Subscription>,
}

impl Global for DialogWatch {}

/// Runs `edit` against the dialog host.
///
/// Mutating a global notifies nothing, so a listener that changes the host must also call
/// [`notify`] to repaint the frame.
pub fn with_host<R>(cx: &mut App, edit: impl FnOnce(&mut DialogHost) -> R) -> R {
    edit(cx.default_global::<DialogHost>())
}

/// Repaints the frame after a host change.
pub fn notify(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |_, cx| cx.notify());
}

/// Publishes what the next [`Dialogs::Confirm`] asks about.
///
/// Call this immediately before opening the Confirm dialog; §3.8.3's fact list, escalation key
/// and consequence sentence are all derived from the request.
pub fn request_confirm(cx: &mut App, request: ConfirmRequest) {
    with_host(cx, |host| {
        host.pending_confirm = Some(request);
        host.open = None;
    });
}

/// Subscribes to the `AppState` entity once, so a closing overlay invalidates its draft.
fn watch(state: &Entity<AppState>, cx: &mut App) {
    if cx.default_global::<DialogWatch>().subscription.is_some() {
        return;
    }
    let subscription = cx.observe(state, |entity, cx| {
        let overlay = entity.read(cx).overlay.clone();
        let host = cx.default_global::<DialogHost>();
        if !matches!(overlay, Some(Overlay::Dialog(_))) {
            host.open = None;
        }
        if !matches!(overlay, Some(Overlay::Palette)) {
            host.palette_open = false;
        }
    });
    cx.default_global::<DialogWatch>().subscription = Some(subscription);
}

/// Seeds the open dialog's draft the first time it is rendered.
fn seed(dialog: &Dialogs, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if with_host(cx, |host| host.open.as_ref() == Some(dialog)) {
        return;
    }
    with_host(cx, |host| host.open = Some(dialog.clone()));
    match dialog {
        Dialogs::CreateWorktree => create_worktree::seed(state, bridge, cx),
        Dialogs::CloneRepo => clone_repo::seed(state, cx),
        Dialogs::Confirm => confirm::seed(state, bridge, cx),
        Dialogs::NewContext => context::seed(state, cx, false),
        Dialogs::EditContext => context::seed(state, cx, true),
        Dialogs::AssignRepo => assign_repo::seed(state, cx),
        Dialogs::Settings => settings::seed(state, bridge, cx),
        Dialogs::Help | Dialogs::Quit | Dialogs::QuitDaemon => {}
    }
}

/// The root element every dialog returns: focus-tracking and full-window, so the card's own
/// scrim covers the screen behind it.
pub(crate) fn root(focus: &FocusHandle) -> Div {
    div().track_focus(focus).size_full()
}

// ---------------------------------------------------------------------------- text input

/// A single-line text buffer with a caret, edited by the keys `docs/KEYMAP.md` lists for
/// dialog inputs.
///
/// The caret is a **character** offset, never a byte offset, so multi-byte input cannot split
/// a grapheme in half.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    value: String,
    caret: usize,
}

impl TextInput {
    /// A buffer holding `value`, with the caret at its end.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let caret = value.chars().count();
        Self { value, caret }
    }

    /// The current text.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The caret's character offset.
    #[must_use]
    pub const fn caret(&self) -> usize {
        self.caret
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Replaces the text, putting the caret at the end.
    pub fn set(&mut self, value: impl Into<String>) {
        *self = Self::new(value);
    }

    /// Inserts `text` at the caret.
    pub fn insert(&mut self, text: &str) {
        let at = self.byte_offset(self.caret);
        self.value.insert_str(at, text);
        self.caret += text.chars().count();
    }

    /// Deletes the character before the caret (`Backspace`).
    pub fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let end = self.byte_offset(self.caret);
        let start = self.byte_offset(self.caret - 1);
        self.value.replace_range(start..end, "");
        self.caret -= 1;
        true
    }

    /// Deletes the word before the caret (`ctrl-w`): trailing spaces, then non-spaces.
    pub fn delete_word(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut start = self.caret;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let from = self.byte_offset(start);
        let to = self.byte_offset(self.caret);
        self.value.replace_range(from..to, "");
        self.caret = start;
        true
    }

    /// Clears the buffer (`ctrl-u`).
    pub fn clear(&mut self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value.clear();
        self.caret = 0;
        true
    }

    /// Moves the caret to the start (`ctrl-a`).
    pub fn home(&mut self) {
        self.caret = 0;
    }

    /// Moves the caret to the end (`ctrl-e`).
    pub fn end(&mut self) {
        self.caret = self.value.chars().count();
    }

    /// Moves the caret one character left (`←`).
    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    /// Moves the caret one character right (`→`).
    pub fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.value.chars().count());
    }

    fn byte_offset(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(index, _)| index)
    }
}

/// The printable character a keystroke types, or `None` when it is not text input.
///
/// gpui dispatches key **bindings** before `on_key_down`, so a bound key never reaches this;
/// what arrives is exactly the printable set `docs/KEYMAP.md` gives to text inputs.
#[must_use]
pub fn typed_char(event: &KeyDownEvent) -> Option<String> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let text = event.keystroke.key_char.as_deref()?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_owned())
}

/// Types a printable keystroke into `input`. Returns whether the buffer changed.
pub fn type_into(input: &mut TextInput, event: &KeyDownEvent) -> bool {
    match typed_char(event) {
        Some(text) => {
            input.insert(&text);
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------- shared helpers

/// Moves a list selection by `delta`, clamped to `len` (never wraps: §5.11).
#[must_use]
pub fn step(cursor: usize, delta: isize, len: usize) -> usize {
    crate::state::move_cursor(cursor, delta, len)
}

/// The repository the Hub's cursor is on, if any.
///
/// §3.8.1: "the rail selection is the repo; from `All` it is the repo of the highlighted
/// worktree".
#[must_use]
pub fn focused_repo(state: &AppState) -> Option<RepoId> {
    let snapshot = state.snapshot.as_ref()?;
    match &state.scope {
        RepoScope::Repo(repo) => Some(repo.clone()),
        RepoScope::All => {
            let worktree = snapshot.worktrees.get(state.cursors.worktrees);
            worktree
                .map(|worktree| worktree.repo_id.clone())
                .or_else(|| {
                    snapshot
                        .repos
                        .get(state.cursors.repos)
                        .map(|r| r.id.clone())
                })
        }
    }
}

/// Seconds since the Unix epoch, or `0` when the clock is before it.
#[must_use]
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Parses the `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` timestamps the daemon emits.
///
/// The wire format is always RFC 3339, and the app needs exactly one number out of it — the
/// age of a fact — so this is a parser for that shape rather than a date library.
#[must_use]
pub fn epoch_seconds(timestamp: &str) -> Option<i64> {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let number = |from: usize, to: usize| timestamp.get(from..to)?.parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let mut epoch = days * 86_400 + hour * 3_600 + minute * 60 + second;
    // Trailing offset, when it is not `Z`.
    let rest = &timestamp[19..];
    if let Some(sign_at) = rest.find(['+', '-']) {
        let offset = &rest[sign_at..];
        if offset.len() >= 6 {
            let hours = offset.get(1..3).and_then(|v| v.parse::<i64>().ok())?;
            let minutes = offset.get(4..6).and_then(|v| v.parse::<i64>().ok())?;
            let total = hours * 3_600 + minutes * 60;
            if offset.starts_with('+') {
                epoch -= total;
            } else {
                epoch += total;
            }
        }
    }
    Some(epoch)
}

/// Days from `1970-01-01` to a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The age of an ISO-8601 timestamp in seconds, clamped at zero.
#[must_use]
pub fn age_secs(timestamp: &str, now: i64) -> Option<i64> {
    epoch_seconds(timestamp).map(|then| (now - then).max(0))
}

/// A relative age (`2d`), or an en dash when the timestamp cannot be read.
#[must_use]
pub fn age_label(timestamp: &str, now: i64) -> String {
    age_secs(timestamp, now).map_or_else(|| "\u{2013}".to_owned(), fleet_ui_kit::format_age)
}

/// A duration in seconds as a coarse uptime (`3h`, `2d`), for the About and Help footers.
#[must_use]
pub fn uptime_label(seconds: i64) -> String {
    fleet_ui_kit::format_age(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialog_has_a_key_context_word() {
        for dialog in [
            Dialogs::CreateWorktree,
            Dialogs::CloneRepo,
            Dialogs::Confirm,
            Dialogs::NewContext,
            Dialogs::EditContext,
            Dialogs::AssignRepo,
            Dialogs::Settings,
            Dialogs::Help,
            Dialogs::Quit,
            Dialogs::QuitDaemon,
        ] {
            assert!(!dialog.context_name().is_empty());
            assert!(!dialog.title().is_empty());
            assert!(dialog.width() > px(0.0));
        }
        assert_eq!(
            Dialogs::NewContext.context_name(),
            Dialogs::EditContext.context_name(),
            "KEYMAP gives both context dialogs one row"
        );
    }

    #[test]
    fn dialog_widths_match_the_spec_table() {
        assert_eq!(Dialogs::CreateWorktree.width(), px(560.0));
        assert_eq!(Dialogs::Settings.width(), px(720.0));
        assert_eq!(Dialogs::Help.width(), px(880.0));
        assert_eq!(Dialogs::Quit.width(), px(520.0));
    }

    #[test]
    fn text_input_edits_by_character_not_byte() {
        let mut input = TextInput::new("héllo");
        assert_eq!(input.caret(), 5);
        input.left();
        input.backspace();
        assert_eq!(input.value(), "hélo");
        input.home();
        input.insert("x");
        assert_eq!(input.value(), "xhélo");
        assert_eq!(input.caret(), 1);
    }

    #[test]
    fn delete_word_eats_trailing_space_then_the_word() {
        let mut input = TextInput::new("feat/rut validator ");
        assert!(input.delete_word());
        assert_eq!(input.value(), "feat/rut ");
        assert!(input.delete_word());
        assert_eq!(input.value(), "");
        assert!(!input.delete_word());
    }

    #[test]
    fn clear_and_caret_moves_stay_in_range() {
        let mut input = TextInput::new("abc");
        input.right();
        assert_eq!(input.caret(), 3);
        input.home();
        input.left();
        assert_eq!(input.caret(), 0);
        assert!(input.clear());
        assert!(!input.clear());
        assert!(input.is_empty());
    }

    #[test]
    fn rfc3339_timestamps_become_epoch_seconds() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("2026-09-04T12:00:00Z"), Some(1_788_523_200));
        assert_eq!(
            epoch_seconds("2026-09-04T12:00:00+02:00"),
            Some(1_788_516_000)
        );
        assert_eq!(epoch_seconds("not a timestamp"), None);
        assert_eq!(epoch_seconds(""), None);
    }

    #[test]
    fn ages_never_go_negative_and_fall_back_to_a_dash() {
        let now = 1_788_523_200;
        assert_eq!(age_secs("2026-09-04T11:59:00Z", now), Some(60));
        assert_eq!(age_secs("2026-09-04T12:01:00Z", now), Some(0));
        assert_eq!(age_label("2026-09-02T12:00:00Z", now), "2d");
        assert_eq!(age_label("garbage", now), "\u{2013}");
    }

    #[test]
    fn step_clamps_at_both_ends() {
        assert_eq!(step(0, -1, 3), 0);
        assert_eq!(step(2, 1, 3), 2);
        assert_eq!(step(0, 1, 3), 1);
        assert_eq!(step(0, 1, 0), 0);
    }
}
