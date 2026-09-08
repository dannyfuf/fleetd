//! Session identity, terminal modes, and activity composed above the Workspace tabs.

use fleet_ui_kit::{
    ActiveTheme, Chip, Icon, IconSize, KeepAliveChips, KeepAliveLabel, PrBadge, PrBadgeState,
    StatusGlyph, StatusKind, TerminalMode, TerminalModes, Text, Tone, Truncate,
};
use gpui::{App, SharedString, Window, div, prelude::*};

/// How many characters of the branch name survive before it is ellipsized.
///
/// The branch is the header's identity: it gets the widest budget on the line, and the
/// repository beside it is truncated first.
const BRANCH_BUDGET: usize = 44;
/// How many characters of the `owner/name` repository id survive.
const REPO_BUDGET: usize = 28;
/// How many keep-alive labels the header lists before it collapses the rest into `+n`.
const KEEP_ALIVE_VISIBLE: usize = 3;

/// The session header.
///
/// Everything is optional except the branch, because a session can exist before its status,
/// its pull request or its jobs are known, and a header that appears one field at a time is
/// less useful than one that reserves the space and fills it in.
#[derive(IntoElement)]
pub(crate) struct WorkspaceHeader {
    branch: SharedString,
    repo: Option<SharedString>,
    host: Option<SharedString>,
    host_reachable: bool,
    pr: Option<(u64, PrBadgeState)>,
    status: StatusKind,
    keep_alive: Vec<SharedString>,
    running_jobs: usize,
    failed_jobs: usize,
    waking: bool,
    modes: Vec<TerminalMode>,
    agent: Option<SharedString>,
}

impl WorkspaceHeader {
    /// A header for a branch.
    #[must_use]
    pub(crate) fn new(branch: impl Into<SharedString>) -> Self {
        Self {
            branch: branch.into(),
            repo: None,
            host: None,
            host_reachable: true,
            pr: None,
            status: StatusKind::Attached,
            keep_alive: Vec::new(),
            running_jobs: 0,
            failed_jobs: 0,
            waking: false,
            modes: Vec::new(),
            agent: None,
        }
    }

    /// The active agent thread's attention word: `working`, `needs you`, `idle` or `failed`.
    ///
    /// It sits beside the session's own status because the two are orthogonal: the session can
    /// be attached while its agent thread is the thing that needs the user (§3.3).
    #[must_use]
    pub(crate) fn agent(mut self, word: impl Into<SharedString>) -> Self {
        self.agent = Some(word.into());
        self
    }

    /// The `owner/name` the worktree belongs to.
    #[must_use]
    pub(crate) fn repo(mut self, repo: impl Into<SharedString>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// The remote host the worktree lives on, and whether the last probe reached it.
    #[must_use]
    pub(crate) fn host(mut self, host: impl Into<SharedString>, reachable: bool) -> Self {
        self.host = Some(host.into());
        self.host_reachable = reachable;
        self
    }

    /// The pull request this branch has, when one is known.
    #[must_use]
    pub(crate) fn pr(mut self, number: u64, state: PrBadgeState) -> Self {
        self.pr = Some((number, state));
        self
    }

    /// The session's status glyph, identical to the one the Hub row shows.
    #[must_use]
    pub(crate) fn status(mut self, status: StatusKind) -> Self {
        self.status = status;
        self
    }

    /// The keep-alive labels `sleep` would preserve.
    #[must_use]
    pub(crate) fn keep_alive(mut self, labels: impl IntoIterator<Item = SharedString>) -> Self {
        self.keep_alive = labels.into_iter().collect();
        self
    }

    /// How many jobs are running and how many failed, for the far-right chip.
    #[must_use]
    pub(crate) fn jobs(mut self, running: usize, failed: usize) -> Self {
        self.running_jobs = running;
        self.failed_jobs = failed;
        self
    }

    /// The VT modes the active terminal's last frame reported.
    ///
    /// They live here and never over the grid: the cells are live output and §3.6 allows only
    /// the scroll overlays and the prefix hint on top of them.
    #[must_use]
    pub(crate) fn modes(mut self, modes: impl IntoIterator<Item = TerminalMode>) -> Self {
        self.modes = modes.into_iter().collect();
        self
    }

    /// Replace the branch line with `waking…` while a slept session's PTYs respawn.
    #[must_use]
    pub(crate) fn waking(mut self, waking: bool) -> Self {
        self.waking = waking;
        self
    }
}

impl RenderOnce for WorkspaceHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let host_icon = if self.host_reachable {
            Icon::Cloud
        } else {
            Icon::CloudOff
        };
        let host_tone = if self.host_reachable {
            Tone::Secondary
        } else {
            Tone::Warning
        };
        // §2.3: a failed job recolors the one jobs chip; it is never a second chip.
        let jobs_chip = if self.failed_jobs > 0 {
            Chip::counter(Icon::TriangleAlert, self.failed_jobs).tone(Tone::Danger)
        } else {
            Chip::counter(Icon::LoaderCircle, self.running_jobs)
                .tone(Tone::Warning)
                .spinning(true)
                .id("workspace-header-jobs")
        };

        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.pane_header_h)
            .w_full()
            .flex_none()
            .px(theme.space.md)
            .bg(theme.colors.bg)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                Icon::GitBranch
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.text_secondary),
            )
            .child(if self.waking {
                Text::ui("waking\u{2026}").tone(Tone::Secondary)
            } else {
                Text::data(self.branch).truncate_at(BRANCH_BUDGET, Truncate::Middle)
            })
            .children(self.repo.map(|repo| {
                Text::ui(repo)
                    .muted()
                    .truncate_at(REPO_BUDGET, Truncate::Middle)
            }))
            .children(self.host.map(|host| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xxs)
                    .child(
                        host_icon
                            .el()
                            .size(IconSize::Small)
                            .color(host_tone.color(theme)),
                    )
                    .child(Text::hint(host).tone(host_tone))
            }))
            .children(self.pr.map(|(number, state)| PrBadge::new(number, state)))
            .children(self.agent.map(|word| {
                let tone = if word == "needs you" || word == "failed" {
                    Tone::Warning
                } else {
                    Tone::Secondary
                };
                Text::ui(word).tone(tone)
            }))
            // Everything after this spacer is right-aligned, in the §3.6 order:
            // VT modes · status glyph · keep-alive · jobs.
            .child(div().flex_1())
            .child(TerminalModes::new(self.modes).glyphs_only())
            .child(StatusGlyph::new(self.status).id("workspace-header-status"))
            .child(
                KeepAliveChips::new(self.keep_alive.into_iter().map(|label| {
                    let icon = crate::presentation::keep_alive_icon(
                        &label,
                        crate::presentation::KeepAliveStyle::Terminal,
                    );
                    KeepAliveLabel::with_icon(label, icon)
                }))
                .max_visible(KEEP_ALIVE_VISIBLE),
            )
            .child(jobs_chip)
    }
}
