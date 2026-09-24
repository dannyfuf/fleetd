//! Sample title and status bars for the galleries, spelled with [`Kbd::parse`] because the
//! gallery has no keymap. The app builds the same components from its state and live key table.

use fleet_ui_kit::prelude::*;
use gpui::AnyElement;

/// What the sample title bar's leading region shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TitleSample {
    /// The Hub: context switcher and section nav.
    Hub,
    /// The Workspace: the breadcrumb.
    Workspace,
    /// The first run: nothing.
    Empty,
}

/// What the sample title bar's status cluster reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TitleStatus {
    pub needs_you: usize,
    pub running: usize,
    pub failed: usize,
    pub sleeping: usize,
    pub update: bool,
    /// `None` healthy, `Some(false)` reconnecting, `Some(true)` down.
    pub daemon_down: Option<bool>,
}

impl TitleStatus {
    /// Nothing to report: only Help and Settings.
    pub const QUIET: Self = Self {
        needs_you: 0,
        running: 0,
        failed: 0,
        sleeping: 0,
        update: false,
        daemon_down: None,
    };
}

fn kbd(keys: &str) -> Option<Kbd> {
    Kbd::parse(keys).ok()
}

/// A title bar in one of its shapes. `id` keeps the element ids of several samples apart.
pub fn title_bar(id: &'static str, sample: TitleSample, status: TitleStatus) -> AnyElement {
    let bar = TitleBar::new();
    let bar = match sample {
        TitleSample::Empty => return bar.into_any_element(),
        TitleSample::Hub => bar
            .leading(
                SwitcherButton::new((id, 0usize), "Acme")
                    .monogram()
                    .tooltip("Switch context"),
            )
            .leading(
                SegmentedControl::new(
                    (id, 1usize),
                    [
                        Segment::new("Worktrees").count(Some(4)),
                        Segment::new("Pull requests").count(Some(3)),
                        Segment::new("Board").count(Some(5)),
                    ],
                )
                .active(Some(0)),
            ),
        TitleSample::Workspace => bar.leading(
            gpui::div()
                .flex()
                .items_center()
                .gap(gpui::px(8.0))
                .child({
                    let back =
                        IconButton::new((id, 2usize), Icon::ChevronLeft, "Back to Worktrees")
                            .size(ButtonSize::Compact);
                    match kbd("ctrl-s s") {
                        Some(kbd) => back.kbd(kbd),
                        None => back,
                    }
                })
                .child(Text::ui("acme/api").muted())
                .child(Text::ui("/").faint())
                .child(Text::ui_strong("agent")),
        ),
    };
    let mut command = CommandField::new((id, 3usize), "Search or run a command");
    if let Some(kbd) = kbd("cmd-k") {
        command = command.kbd(kbd);
    }
    let mut bar = bar.command(command);
    if status.needs_you > 0 {
        bar = bar.trailing(
            StatusButton::new((id, 4usize), format!("{} needs you", status.needs_you))
                .mark(StatusMark::Dot)
                .tone(Tone::Warning),
        );
    }
    if status.failed > 0 {
        bar = bar.trailing(
            StatusButton::new((id, 5usize), format!("{} failed", status.failed))
                .mark(StatusMark::Icon(Icon::TriangleAlert))
                .tone(Tone::Danger),
        );
    } else if status.running > 0 {
        bar = bar.trailing(
            StatusButton::new((id, 5usize), format!("{} jobs", status.running))
                .mark(StatusMark::Spinner),
        );
    }
    if status.sleeping > 0 {
        bar = bar.trailing(Text::ui(format!("{} sleeping", status.sleeping)).muted());
    }
    if status.update {
        bar = bar.trailing(
            StatusButton::new((id, 6usize), "Update 0.2.0")
                .mark(StatusMark::Icon(Icon::CircleArrowUp)),
        );
    }
    if let Some(down) = status.daemon_down {
        let (label, tone) = if down {
            ("fleetd down", Tone::Danger)
        } else {
            ("Reconnecting\u{2026}", Tone::Warning)
        };
        bar = bar.trailing(
            StatusButton::new((id, 7usize), label)
                .mark(StatusMark::Dot)
                .tone(tone),
        );
    }
    let mut help = IconButton::new((id, 8usize), Icon::CircleQuestionMark, "Help and shortcuts")
        .size(ButtonSize::Compact);
    if let Some(kbd) = kbd("?") {
        help = help.kbd(kbd);
    }
    bar.trailing(help)
        .trailing(
            IconButton::new((id, 9usize), Icon::Settings2, "Settings").size(ButtonSize::Compact),
        )
        .into_any_element()
}

/// The status bar's `Shortcuts ?` button, or the Workspace's pair.
pub fn status_buttons(bar: StatusBar, id: &'static str, workspace: bool) -> StatusBar {
    let button = |index: usize, label: &'static str, keys: &str| {
        let button = Button::new((id, index), label)
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact);
        match kbd(keys) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        }
    };
    if workspace {
        bar.trailing(button(0, "Fleet commands", "ctrl-s"))
            .trailing(button(1, "Shortcuts", "ctrl-s ?"))
    } else {
        bar.trailing(button(1, "Shortcuts", "?"))
    }
}

/// The §3.12 C banner with the app's two buttons and its ✕. The app wires each button with
/// `Button::action`, so its chip is the live key; the gallery spells the chips instead.
pub fn reconnect_banner(banner: Banner) -> Banner {
    let reconnect = Button::new("banner-reconnect", "Reconnect now");
    let log = Button::new("banner-log", "Open log").style(ButtonStyle::Ghost);
    banner
        .button(match kbd("r") {
            Some(chip) => reconnect.kbd(chip),
            None => reconnect,
        })
        .button(match kbd("l") {
            Some(chip) => log.kbd(chip),
            None => log,
        })
        .on_dismiss(|_, _| {})
}
