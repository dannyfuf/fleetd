//! The 44 px title bar (UX-SPEC §2.2, §3.1): where you are, a way to search or run anything, and
//! what needs you.
//!
//! Every control runs what its key runs. The section nav, the command field, Jobs, Help,
//! Settings, the update and the daemon pill dispatch the very action their key is bound to, and
//! show that key as a chip or in their tooltip, read from the live keymap. Two controls are not
//! actions: the context switcher opens a menu whose rows are the context actions (each with its
//! key), and `1 needs you` opens the waiting thread the way the palette's `AGENTS` row does.
//!
//! In the Workspace the leading region is a breadcrumb, `← Worktrees / repo / worktree ⌄`: the
//! back button is `⌃S s`, the worktree is a switcher listing every session as `⌃S W` does, and
//! the chips after it say what git knows about the worktree (`↑2 ↓0`, `3 files changed`) and
//! which pull request it has, a click away on GitHub (see [`workspace_breadcrumb`]).

use std::rc::Rc;

use fleet_core::sessions::TerminalStatus;
use fleet_core::{agents::ThreadId, ids::ContextId};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{
    ActiveTheme, Button, ButtonSize, ButtonStyle, Chip, CommandField, DaemonState,
    HarnessTargetExt, Icon, IconButton, MenuItem, PopoverMenu, PrBadgeState, Segment,
    SegmentedControl, StatusButton, StatusMark, SwitcherButton, Text, TitleBar, Tone,
};
use gpui::{Action, AnyElement, App, Entity, IntoElement, SharedString, div, prelude::*};

use crate::{
    actions::{board, daemon, fleet, hub, prefix},
    bridge::Bridge,
    dialogs,
    presentation::workspace_keys,
    screens::{
        hub::{context_board_summary, effective_context},
        workspace::host_unreachable,
    },
    shell::daemon::dot_state,
    state::{AppState, ChipCounts, DaemonLink, DaemonLossReason, HubTab, Mode, Overlay, Screen},
};

/// One context in the switcher's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextEntry {
    id: ContextId,
    name: SharedString,
}

/// The Hub's leading region: the context switcher and the section nav.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HubPlace {
    contexts: Rc<[ContextEntry]>,
    /// Index into `contexts` of the context the Hub shows.
    active: Option<usize>,
    section: HubTab,
    worktrees: usize,
    /// Pull requests waiting for your review, the count the old `⚑` chip carried.
    reviews: usize,
    board: Option<usize>,
    board_conflict: bool,
    board_loading: bool,
}

/// The Workspace's leading region: the breadcrumb and its chips.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspacePlace {
    repo: SharedString,
    worktree: SharedString,
    /// A slept session respawning its PTYs (§3.6 "Waking a slept session").
    waking: bool,
    /// The remote host, and whether it is known to be out of reach.
    host: Option<(SharedString, bool)>,
    /// `↑2 ↓0`, while either count is non-zero.
    divergence: Option<SharedString>,
    /// `3 files changed`, while the worktree is dirty.
    changed: Option<SharedString>,
    pr: Option<PrChip>,
}

/// The pull request the worktree's branch has: `#412 CI fail`, opening it on GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrChip {
    label: SharedString,
    state: PrBadgeState,
    url: SharedString,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Place {
    /// The first-run card owns the window: the bar is empty, because there is nowhere to go yet.
    #[default]
    FirstRun,
    Hub(HubPlace),
    Workspace(WorkspacePlace),
}

/// The daemon pill, shown only while the daemon is not healthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DaemonPill {
    /// The link is retrying on its own: amber.
    Reconnecting,
    /// The daemon stopped or could not be started: red.
    Down,
}

/// The jobs button: running, or failed, which replaces it rather than joining it (§1.8).
#[derive(Clone, Debug, PartialEq, Eq)]
enum JobsLabel {
    Running(SharedString),
    Failed(SharedString),
}

/// A count as its label, or nothing at zero (§1.2).
fn counted(count: usize, label: impl FnOnce(usize) -> String) -> Option<SharedString> {
    (count > 0).then(|| SharedString::from(label(count)))
}

/// Everything the title bar draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::shell) struct TitleModel {
    place: Place,
    /// `3 needs you`, while any thread does.
    needs_you: Option<SharedString>,
    /// The thread `1 needs you` opens; `None` with two or more waiting opens the agents picker.
    waiting: Option<ThreadId>,
    jobs: Option<JobsLabel>,
    /// The Jobs sheet is open, so the jobs button stays pressed.
    jobs_open: bool,
    /// `3 sleeping`, while any session is detached.
    sleeping: Option<SharedString>,
    /// `Update 0.2.0`, while an update is available.
    update: Option<SharedString>,
    daemon: Option<DaemonPill>,
    mode: Option<Mode>,
}

impl TitleModel {
    pub(super) fn build(state: &AppState) -> Self {
        let counts = state.snapshot.as_ref().map_or_else(
            || ChipCounts {
                review: state.review_pr_count,
                ..ChipCounts::default()
            },
            |snapshot| ChipCounts::from_snapshot(snapshot, state.review_pr_count),
        );
        let place = if state.is_first_run() {
            Place::FirstRun
        } else {
            match &state.screen {
                Screen::Hub { tab } => Place::Hub(hub_place(state, *tab, counts.review)),
                Screen::Workspace { session } => Place::Workspace(workspace_place(state, session)),
            }
        };
        let agents = state.agents.counts();
        Self {
            place,
            needs_you: counted(agents.needs_you, |count| format!("{count} needs you")),
            waiting: state.agents.waiting_thread(),
            jobs: counted(counts.failed, |count| format!("{count} failed"))
                .map(JobsLabel::Failed)
                .or_else(|| {
                    counted(counts.running, |count| {
                        format!("{count} {}", if count == 1 { "job" } else { "jobs" })
                    })
                    .map(JobsLabel::Running)
                }),
            jobs_open: matches!(state.overlay, Some(Overlay::Jobs)),
            sleeping: counted(counts.sleeping, |count| format!("{count} sleeping")),
            update: state
                .update_version
                .as_deref()
                .map(|version| SharedString::from(format!("Update {version}"))),
            daemon: daemon_pill(&state.daemon),
            mode: Some(state.mode()),
        }
    }
}

fn hub_place(state: &AppState, section: HubTab, reviews: usize) -> HubPlace {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return HubPlace {
            contexts: Rc::from([]),
            active: None,
            section,
            worktrees: 0,
            reviews,
            board: None,
            board_conflict: false,
            board_loading: state.board.loading,
        };
    };
    let context = effective_context(state).map(|context| context.id.clone());
    let contexts: Rc<[ContextEntry]> = snapshot
        .contexts
        .iter()
        .map(|context| ContextEntry {
            id: context.id.clone(),
            name: SharedString::from(context.name.clone()),
        })
        .collect();
    let active = context
        .as_ref()
        .and_then(|id| contexts.iter().position(|entry| &entry.id == id));
    let in_context = |repo: &fleet_core::ids::RepoId| {
        context.as_ref().is_none_or(|context| {
            snapshot
                .repos
                .iter()
                .any(|candidate| &candidate.id == repo && &candidate.context_id == context)
        })
    };
    let worktrees = snapshot
        .worktrees
        .iter()
        .filter(|worktree| in_context(&worktree.repo_id))
        .count();
    let summary = context_board_summary(&snapshot.boards, state.active_context());
    HubPlace {
        contexts,
        active,
        section,
        worktrees,
        reviews,
        board: summary.map(|summary| summary.open_count),
        board_conflict: summary.is_some_and(|summary| summary.conflict_count > 0),
        board_loading: state.board.loading,
    }
}

fn workspace_place(state: &AppState, session: &fleet_core::ids::SessionId) -> WorkspacePlace {
    let worktree = state.active_worktree().and_then(|id| {
        state
            .snapshot
            .as_ref()?
            .worktrees
            .iter()
            .find(|worktree| &worktree.id == id)
    });
    let waking = state.active_session().is_some_and(|session| {
        session.slept_at.is_some()
            && session
                .terminals
                .iter()
                .any(|terminal| terminal.status == TerminalStatus::Starting)
    });
    let Some(worktree) = worktree else {
        // A repository-level agent session has no worktree to name, and no git to report.
        return WorkspacePlace {
            repo: SharedString::default(),
            worktree: SharedString::from(session.to_string()),
            waking,
            host: None,
            divergence: None,
            changed: None,
            pr: None,
        };
    };
    let git = state
        .workspace_git
        .as_ref()
        .filter(|git| git.worktree == worktree.id);
    let divergence = git.and_then(|git| {
        let (ahead, behind) = (git.ahead.unwrap_or(0), git.behind.unwrap_or(0));
        (ahead > 0 || behind > 0)
            .then(|| SharedString::from(format!("\u{2191}{ahead} \u{2193}{behind}")))
    });
    let changed = git
        .filter(|git| git.dirty)
        .map(|git| match git.dirty_files {
            Some(1) => SharedString::new_static("1 file changed"),
            Some(count) if count > 1 => SharedString::from(format!("{count} files changed")),
            _ => SharedString::new_static("uncommitted changes"),
        });
    let pr = state
        .pr_badges
        .get(&(worktree.repo_id.clone(), worktree.branch.clone()))
        .map(|&(number, badge)| PrChip {
            label: SharedString::from(format!("#{number} {}", badge.word())),
            state: badge,
            url: SharedString::from(format!(
                "https://github.com/{}/pull/{number}",
                worktree.repo_id
            )),
        });
    WorkspacePlace {
        repo: SharedString::from(worktree.repo_id.to_string()),
        worktree: SharedString::from(worktree.slug.clone()),
        waking,
        host: worktree.host.as_ref().map(|host| {
            (
                SharedString::from(host.to_string()),
                !host_unreachable(state, host),
            )
        }),
        divergence,
        changed,
        pr,
    }
}

/// §3.12: nothing while healthy; amber while the link retries on its own; red once it stopped.
fn daemon_pill(link: &DaemonLink) -> Option<DaemonPill> {
    match (dot_state(link), link) {
        (DaemonState::Healthy, _) => None,
        (
            _,
            DaemonLink::Lost {
                reason: DaemonLossReason::Stopped,
                ..
            }
            | DaemonLink::Failed { .. },
        ) => Some(DaemonPill::Down),
        _ => Some(DaemonPill::Reconnecting),
    }
}

/// The key the Hub binds to select context `digit` (`1`–`9`).
fn select_context(digit: usize) -> Option<Box<dyn Action>> {
    Some(match digit {
        1 => Box::new(hub::SelectContext1),
        2 => Box::new(hub::SelectContext2),
        3 => Box::new(hub::SelectContext3),
        4 => Box::new(hub::SelectContext4),
        5 => Box::new(hub::SelectContext5),
        6 => Box::new(hub::SelectContext6),
        7 => Box::new(hub::SelectContext7),
        8 => Box::new(hub::SelectContext8),
        9 => Box::new(hub::SelectContext9),
        _ => return None,
    })
}

pub(super) fn render(
    model: &TitleModel,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &App,
) -> AnyElement {
    #[cfg(target_os = "macos")]
    let inset = cx.theme().metrics.traffic_light_inset;
    #[cfg(not(target_os = "macos"))]
    let inset = cx.theme().space.md;
    let bar = TitleBar::new().leading_inset(inset);
    let bar = match &model.place {
        Place::FirstRun => return bar.into_any_element(),
        Place::Hub(place) => bar
            .leading(context_switcher(place, bridge))
            .leading(section_nav(place)),
        Place::Workspace(place) => bar.leading(workspace_breadcrumb(place, state, bridge, cx)),
    };
    let workspace = matches!(model.place, Place::Workspace(_));
    let bar = bar.center(
        CommandField::new("titlebar-command", "Search or run a command")
            .action(Box::new(fleet::OpenPalette))
            .harness_target("titlebar.command"),
    );
    trailing(bar, model, workspace, state, bridge).into_any_element()
}

/// `[A] Acme ⌄`: opens every context with its digit key, then New, Edit and Delete.
fn context_switcher(place: &HubPlace, bridge: &Bridge) -> AnyElement {
    let label = place
        .active
        .and_then(|index| place.contexts.get(index))
        .map_or_else(
            || SharedString::new_static("No context"),
            |entry| entry.name.clone(),
        );
    let has_context = place.active.is_some();
    let contexts = Rc::clone(&place.contexts);
    let active = place.active;
    let bridge = bridge.clone();
    PopoverMenu::new("titlebar-context")
        .trigger_with(move |open, _, _| {
            SwitcherButton::new("titlebar-context-trigger", label)
                .map(|button| {
                    if has_context {
                        button.monogram()
                    } else {
                        button
                    }
                })
                .tooltip("Switch context")
                .selected(open)
                .harness_target("titlebar.context")
        })
        .menu(move |mut menu, _, _| {
            for (index, entry) in contexts.iter().enumerate() {
                let item = MenuItem::new(entry.name.clone()).checked(active == Some(index));
                // `1`–`9` are the Hub's keys; a context past nine has none, so its row switches
                // through the same request the palette's CONTEXT row sends.
                let item = match select_context(index + 1) {
                    Some(action) => item.action(action),
                    None => {
                        let bridge = bridge.clone();
                        let id = entry.id.clone();
                        item.on_select(move |_, _| {
                            bridge.send(RequestBody::SetActiveContext {
                                id: Some(id.clone()),
                            });
                        })
                    }
                };
                menu = menu.item(item);
            }
            menu.separator()
                .item(
                    MenuItem::new("New context")
                        .icon(Icon::Plus)
                        .action(Box::new(hub::NewContext)),
                )
                .item(
                    MenuItem::new("Edit context")
                        .icon(Icon::SquarePen)
                        .action(Box::new(hub::EditContext)),
                )
                .item(
                    MenuItem::new("Delete context")
                        .icon(Icon::Trash)
                        .destructive(true)
                        .action(Box::new(hub::DeleteContext)),
                )
        })
        .into_any_element()
}

/// Worktrees · Pull requests · Board, with counts. The segments are `hub.tab[N]`.
fn section_nav(place: &HubPlace) -> SegmentedControl {
    let count = |count: usize| (count > 0).then_some(count);
    let board_label = if place.board_conflict {
        "Board •"
    } else {
        "Board"
    };
    let active = match place.section {
        HubTab::Worktrees => 0,
        HubTab::Prs => 1,
        HubTab::Board => 2,
    };
    SegmentedControl::new(
        "hub-tabs",
        [
            Segment::new("Worktrees").count(count(place.worktrees)),
            Segment::new("Pull requests").count(count(place.reviews)),
            Segment::new(board_label)
                .count(place.board.and_then(count))
                .loading(place.board_loading),
        ],
    )
    .active(Some(active))
    .harness_segments("hub.tab")
    .on_select(|index, window, cx| {
        let action: Box<dyn Action> = match index {
            1 => Box::new(hub::GoPrs),
            2 => Box::new(board::GoBoard),
            _ => Box::new(hub::GoWorktrees),
        };
        window.dispatch_action(action, cx);
    })
}

/// `← Worktrees / acme/api / ⎇ agent ⌄  ↑2 ↓0  3 files changed  #412 CI fail`.
fn workspace_breadcrumb(
    place: &WorkspacePlace,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let keys = workspace_keys();
    let mut back = Button::new("titlebar-back", "Worktrees")
        .icon(Icon::ChevronLeft)
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .action(Box::new(prefix::GoHub))
        .tooltip("Back to the Hub; the session keeps running");
    if let Some(kbd) = keys.go_hub.clone() {
        back = back.kbd(kbd);
    }
    let separator = || Text::ui("/").faint().flex_none();
    let chip = |label: SharedString| Chip::new().text(label).filled(true);
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(theme.space.sm)
        .overflow_hidden()
        .child(
            back.harness_target("titlebar.back")
                .harness_target("workspace.back"),
        )
        .when(!place.repo.is_empty(), |el| {
            el.child(separator())
                .child(Text::ui(place.repo.clone()).muted().flex_none())
        })
        .child(separator())
        .child(worktree_switcher(place.worktree.clone(), state, bridge))
        .children(
            place
                .waking
                .then(|| Text::ui("waking\u{2026}").muted().flex_none()),
        )
        .children(place.divergence.clone().map(chip))
        .children(place.changed.clone().map(chip))
        .children(place.host.clone().map(|(host, reachable)| {
            let (icon, tone) = if reachable {
                (Icon::Cloud, Tone::Secondary)
            } else {
                (Icon::CloudOff, Tone::Warning)
            };
            Chip::labeled(icon, host).tone(tone).filled(true)
        }))
        .children(place.pr.clone().map(|pr| {
            let url = pr.url.clone();
            StatusButton::new("titlebar-pr", pr.label)
                .mark(StatusMark::Icon(pr.state.icon()))
                .tone(pr.state.tone())
                .tooltip("Open the pull request on GitHub")
                .on_click(move |_, _, cx| cx.open_url(&url))
                .harness_target("workspace.pr")
        }))
        .into_any_element()
}

/// `⎇ agent ⌄`: every session, most recently used first with its state, as `⌃S W` lists them;
/// then *Last session* (`⌃S w`) and *All sessions…* (`⌃S W`).
fn worktree_switcher(label: SharedString, state: &Entity<AppState>, bridge: &Bridge) -> AnyElement {
    let (state, bridge) = (state.clone(), bridge.clone());
    PopoverMenu::new("titlebar-worktree")
        .trigger_with(move |open, _, _| {
            SwitcherButton::new("titlebar-worktree-trigger", label.clone())
                .icon(Icon::GitBranch)
                .tooltip("Switch worktree")
                .selected(open)
                .harness_target("workspace.switcher")
        })
        .menu(move |mut menu, _, cx| {
            let keys = workspace_keys();
            // Built when the menu opens, never per frame: the rows are the palette's own.
            let app = state.read(cx);
            let current_worktree = app.active_worktree().cloned();
            let current_session = app.active_session().map(|session| session.id.clone());
            let rows = dialogs::session_rows(app);
            for row in rows {
                let current = match &row.run {
                    dialogs::PaletteRun::OpenWorktree(id) => current_worktree.as_ref() == Some(id),
                    dialogs::PaletteRun::OpenSession { session, .. } => {
                        current_session.as_ref() == Some(session)
                    }
                    _ => false,
                };
                let mut item = MenuItem::new(row.label.clone())
                    .icon(Icon::GitBranch)
                    .checked(current);
                if let Some(detail) = row.detail.clone() {
                    item = item.detail(detail);
                }
                let (state, bridge) = (state.clone(), bridge.clone());
                let run = row.run.clone();
                menu = menu.item(item.on_select(move |_, cx| {
                    if current {
                        return;
                    }
                    match &run {
                        dialogs::PaletteRun::OpenWorktree(id) => {
                            dialogs::open_worktree(id.clone(), &state, &bridge, cx);
                        }
                        dialogs::PaletteRun::OpenSession { agent, .. } => {
                            dialogs::open_agent_session(*agent, &state, &bridge, cx);
                        }
                        _ => {}
                    }
                }));
            }
            let mut last = MenuItem::new("Last session").action(Box::new(prefix::LastSession));
            if let Some(kbd) = keys.last_session.clone() {
                last = last.kbd(kbd);
            }
            let mut all = MenuItem::new("All sessions\u{2026}")
                .icon(Icon::Search)
                .action(Box::new(prefix::SessionSwitcher));
            if let Some(kbd) = keys.session_switcher.clone() {
                all = all.kbd(kbd);
            }
            menu.separator().item(last).item(all)
        })
        .into_any_element()
}

/// Needs you · jobs · sleeping · update · daemon · Help · Settings, each count only when non-zero.
fn trailing(
    mut bar: TitleBar,
    model: &TitleModel,
    workspace: bool,
    state: &Entity<AppState>,
    bridge: &Bridge,
) -> TitleBar {
    let keys = workspace.then(workspace_keys);
    if let Some(label) = model.needs_you.clone() {
        let waiting = model.waiting;
        let state = state.clone();
        let bridge = bridge.clone();
        let mut button = StatusButton::new("titlebar-needs-you", label)
            .mark(StatusMark::Dot)
            .tone(Tone::Warning)
            .tooltip(if waiting.is_some() {
                "Open the agent that needs you"
            } else {
                "Open the agents that need you"
            })
            .on_click(move |_, _, cx| match waiting {
                Some(thread) => dialogs::open_agent_thread(thread, &state, &bridge, cx),
                None => dialogs::open_agents_picker(&state, cx),
            });
        if let Some(kbd) = keys.and_then(|keys| keys.agents.clone()) {
            button = button.kbd(kbd);
        }
        bar = bar.trailing(button.harness_target("titlebar.needs_you"));
    }
    if let Some(jobs) = &model.jobs {
        let button = match jobs {
            JobsLabel::Failed(label) => StatusButton::new("titlebar-jobs", label.clone())
                .mark(StatusMark::Icon(Icon::TriangleAlert))
                .tone(Tone::Danger),
            JobsLabel::Running(label) => {
                StatusButton::new("titlebar-jobs", label.clone()).mark(StatusMark::Spinner)
            }
        };
        let mut button = button
            .tooltip("Open jobs")
            .selected(model.jobs_open)
            .action(Box::new(fleet::OpenJobs));
        if let Some(kbd) = keys.and_then(|keys| keys.jobs.clone()) {
            button = button.kbd(kbd);
        }
        bar = bar.trailing(button.harness_target("titlebar.jobs"));
    }
    // The Worktrees subtitle takes the session counts over; until it does, the one that says
    // work is parked stays here, muted and inert — it counts nothing a click could open.
    if let Some(sleeping) = model.sleeping.clone().filter(|_| !workspace) {
        bar = bar.trailing(div().flex_none().child(Text::ui(sleeping).muted()));
    }
    // `U` is a Hub key; over a terminal the update waits for the Hub.
    if let Some(label) = model.update.clone().filter(|_| !workspace) {
        bar = bar.trailing(
            StatusButton::new("titlebar-update", label)
                .mark(StatusMark::Icon(Icon::CircleArrowUp))
                .tooltip("Update Fleet")
                .action(Box::new(fleet::UpdateFleet))
                .harness_target("titlebar.update"),
        );
    }
    if let Some(pill) = model.daemon {
        let (label, tone) = match pill {
            DaemonPill::Reconnecting => ("Reconnecting\u{2026}", Tone::Warning),
            DaemonPill::Down => ("fleetd down", Tone::Danger),
        };
        bar = bar.trailing(
            StatusButton::new("titlebar-daemon", label)
                .mark(StatusMark::Dot)
                .tone(tone)
                .tooltip("Open the fleetd log")
                .action(Box::new(daemon::OpenLog))
                .harness_target("titlebar.daemon"),
        );
    }
    let mut help = IconButton::new(
        "titlebar-help",
        Icon::CircleQuestionMark,
        "Help and shortcuts",
    )
    .size(ButtonSize::Compact)
    .action(Box::new(fleet::OpenHelp));
    if let Some(kbd) = keys.and_then(|keys| keys.help.clone()) {
        help = help.kbd(kbd);
    }
    bar.trailing(help.harness_target("titlebar.help")).trailing(
        IconButton::new("titlebar-settings", Icon::Settings2, "Settings")
            .size(ButtonSize::Compact)
            .action(Box::new(fleet::OpenSettings))
            .harness_target("titlebar.settings"),
    )
}

#[cfg(test)]
mod tests;
