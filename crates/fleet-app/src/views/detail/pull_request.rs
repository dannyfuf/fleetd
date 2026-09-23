use super::*;

use fleet_core::ids::HostId;
use fleet_core::slug::slugify;
use fleet_proto::snapshot::LinkState;
use fleet_ui_kit::{
    Button, ButtonStyle, Chip, IconButton, InfoCard, MenuAnchor, PopoverMenu, StatusGlyph,
};

use crate::{
    actions::{hub as hub_actions, prs as pr_actions},
    views::{
        prs_screen::{self, Checks, ChecksKind, ReviewState, state_chip},
        worktrees_list::OPEN_KEY,
    },
};

/// What `Enter` on a PR with no local worktree is about to create (§3.5 [D-6]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WillCreate {
    /// The proposed worktree, `repo/slug`.
    worktree: String,
    /// The local branch: `headRefName`, or `pr/<n>` across repositories.
    branch: String,
    /// The base ref persisted on the worktree.
    base: String,
    /// `dannyfuf/payroll → pr/412`, only for a cross-repository PR.
    fork: Option<String>,
}

/// Derives what the §3.5 worktree card says opening will create, for one pull request.
fn will_create(pr: &PullRequest) -> WillCreate {
    let branch = local_branch_for_pr(pr);
    WillCreate {
        worktree: format!("{}/{}", pr.repo_id.name(), slugify(&branch)),
        branch: branch.clone(),
        base: format!("pull/{}/head", pr.number),
        fork: pr
            .head_repo
            .as_ref()
            .filter(|_| pr.is_cross_repository)
            .map(|head| format!("{head} \u{2192} {branch}")),
    }
}

/// Everything the pull-request variant needs.
#[derive(Clone, Copy)]
pub struct PrProps<'a> {
    /// The pull request under the cursor.
    pub pr: &'a PullRequest,
    /// The local worktree matching it, when one exists.
    pub local: Option<&'a Worktree>,
    /// Machine owning the matching worktree, when remote.
    pub host: Option<&'a HostId>,
    /// Current link state of that machine.
    pub host_link: Option<LinkState>,
    /// That worktree's runtime status.
    pub status: Option<&'a WorktreeStatus>,
    /// Whether `Enter` / `c` is creating its worktree right now.
    pub creating: bool,
    /// `$HOME`, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// The primary button when the worktree exists. The card's copy, not the catalogue's
/// "Open the pull request's worktree": the panel already says which pull request.
const OPEN_WORKTREE: &str = "Open worktree";
/// The primary button when it does not: `c`, which creates and stays here.
const CREATE_WORKTREE: &str = "Create worktree";
/// The primary button while that worktree is being created.
const CREATING_WORKTREE: &str = "Creating worktree\u{2026}";
/// The browser button: it names the destination; its tooltip carries the catalogue label.
const GITHUB: &str = "GitHub";
/// The worktree card's title.
const WORKTREE: &str = "Worktree";

/// §3.5's PR detail: title, head → base, the primary action, the facts, and the worktree card.
#[must_use]
pub fn pull_request(props: PrProps<'_>, cx: &App) -> AnyElement {
    pull_request_with_status(props, None, cx)
}

/// Renders PR detail with the exact local-worktree status already resolved for its PR row.
#[must_use]
pub fn pull_request_with_status(
    props: PrProps<'_>,
    resolved_status: Option<StatusKind>,
    cx: &App,
) -> AnyElement {
    let PrProps {
        pr,
        local,
        creating,
        now,
        ..
    } = props;
    let theme = cx.theme();
    let review = ReviewState::of(pr.is_draft, pr.review_decision);
    let checks = Checks::of(pr.checks, pr.checks_passed, pr.checks_total);
    let updated = age_secs(&pr.updated_at, now)
        .map(|age| format!("{} ago", format_age(age)))
        .unwrap_or_else(|| "\u{2014}".to_owned());

    let head = div()
        .flex()
        .flex_col()
        .gap(theme.space.xs)
        .child(
            Text::data_small(format!(
                "{} #{} \u{00b7} by {}",
                pr.repo_id, pr.number, pr.author
            ))
            .muted()
            .ellipsize(),
        )
        .child(Text::section_title(pr.title.clone()))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.space.xs)
                .pt(theme.space.xs)
                .child(state_chip(review))
                .child(
                    Chip::new()
                        .text(format!(
                            "{} \u{2192} {}",
                            pr.head_ref_name, pr.base_ref_name
                        ))
                        .tone(Tone::Secondary)
                        .filled(true),
                ),
        );

    let primary = match (local.is_some(), creating) {
        (true, _) => Button::new("pr-detail-primary", OPEN_WORKTREE)
            .style(ButtonStyle::Primary)
            .prefer_key(OPEN_KEY)
            .tooltip(catalogue_label(&pr_actions::Open))
            .action(Box::new(pr_actions::Open)),
        (false, true) => Button::new("pr-detail-primary", CREATING_WORKTREE)
            .style(ButtonStyle::Primary)
            .disabled(true),
        (false, false) => Button::new("pr-detail-primary", CREATE_WORKTREE)
            .style(ButtonStyle::Primary)
            .tooltip(catalogue_label(&pr_actions::CreateWithoutOpening))
            .action(Box::new(pr_actions::CreateWithoutOpening)),
    };
    let actions = div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .child(div().flex_1().min_w_0().child(primary.full_width()))
        .child(
            Button::new("pr-detail-github", GITHUB)
                .tooltip(catalogue_label(&hub_actions::OpenInBrowser))
                .action(Box::new(hub_actions::OpenInBrowser)),
        )
        .child(
            PopoverMenu::new("pr-detail-more")
                .anchor(MenuAnchor::BottomRight)
                .trigger_with(|open, _, _| {
                    IconButton::new("pr-detail-more-trigger", Icon::Ellipsis, "More actions")
                        .selected(open)
                })
                .menu(prs_screen::menu_builder(local.is_some(), creating)),
        );

    let changes = div()
        .flex()
        .items_center()
        .gap(theme.space.xs)
        .child(Text::data_small(format!("+{}", pr.additions)).tone(Tone::Success))
        .child(Text::data_small(format!("\u{2212}{}", pr.deletions)).tone(Tone::Danger));
    let labels = if pr.labels.is_empty() {
        Text::ui("\u{2014}").faint().into_any_element()
    } else {
        div()
            .flex()
            .flex_wrap()
            .gap(theme.space.xxs)
            .children(pr.labels.iter().map(|label| {
                Chip::new()
                    .text(label.clone())
                    .tone(Tone::Secondary)
                    .filled(true)
            }))
            .into_any_element()
    };
    let facts = div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .child(fact("Changes", changes, cx))
        .child(fact(
            "Checks",
            Text::ui(checks.sentence.clone()).tone(match checks.kind {
                ChecksKind::Running => Tone::Default,
                _ => checks.tone(),
            }),
            cx,
        ))
        .child(fact(
            "Review",
            Text::ui(ReviewState::sentence(pr.review_decision)),
            cx,
        ))
        .child(fact("Labels", labels, cx))
        .child(fact("Updated", Text::ui(updated), cx));

    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_none()
        .gap(theme.space.lg)
        .p(theme.space.lg)
        .child(head)
        .child(actions)
        .child(facts)
        .child(worktree_card(props, resolved_status, cx))
        .into_any_element()
}

/// The catalogue's full label: a button's tooltip when its face carries shorter copy.
fn catalogue_label(action: &dyn gpui::Action) -> &'static str {
    crate::action_catalogue::info(action.name()).map_or("", |info| info.label)
}

/// One fact: a muted label in the fixed label column, then its value.
fn fact(label: &'static str, value: impl IntoElement, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_start()
        .gap(theme.space.sm)
        .child(
            div()
                .flex_none()
                .w(theme.metrics.fact_label_w)
                .child(Text::ui(label).muted()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

/// The `Worktree` card: the local worktree and its session, or what opening will create.
fn worktree_card(props: PrProps<'_>, resolved_status: Option<StatusKind>, cx: &App) -> AnyElement {
    let PrProps {
        pr,
        local,
        host,
        host_link,
        status,
        home,
        ..
    } = props;
    let theme = cx.theme();
    let card = InfoCard::new().title(WORKTREE);
    match local {
        Some(worktree) => {
            let glyph = resolved_status.unwrap_or_else(|| {
                resolved_worktree_status(status, false, worktree.degraded.is_some(), false, false)
            });
            let place = match host {
                Some(host) => {
                    let link = host_link.map_or("unknown", |link| match link {
                        LinkState::Connecting => "connecting",
                        LinkState::Ready => "ready",
                        LinkState::Down => "down",
                        LinkState::Legacy => "legacy",
                    });
                    format!("on {host} \u{00b7} {link}")
                }
                None => "on this Mac".to_owned(),
            };
            let running = status
                .map(|status| status.running.join(", "))
                .filter(|labels| !labels.is_empty());
            let session = match running {
                Some(labels) => format!("{} \u{00b7} {labels}", glyph.detail_word()),
                None => glyph.detail_word().to_owned(),
            };
            let path = qualified_path(host, &worktree.path);
            let path = crate::presentation::tilde(&path, Some(std::path::Path::new(home)));
            card.trailing(Text::ui(place).muted())
                .line(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .child(Icon::GitBranch.el().size(IconSize::Small).tone(Tone::Info))
                        .child(
                            div().flex_1().min_w_0().child(
                                Text::ui_strong(format!(
                                    "{} / {}",
                                    pr.repo_id.name(),
                                    worktree.branch
                                ))
                                .ellipsize(),
                            ),
                        ),
                )
                .line(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .child(StatusGlyph::new(glyph).id("pr-detail-session"))
                        .child(Text::ui(session).muted().ellipsize()),
                )
                .line(
                    Text::data_small(truncate(&path, line_budget(cx), Truncate::Middle))
                        .faint()
                        .ellipsize(),
                )
                .into_any_element()
        }
        None => {
            let plan = will_create(pr);
            let card = card
                .line(
                    Text::ui(format!(
                        "Opening creates {} from {}",
                        plan.worktree, plan.base
                    ))
                    .muted(),
                )
                .line(Text::ui(format!("Branch {}", plan.branch)).muted());
            match plan.fork {
                Some(fork) => card.line(Text::ui(format!("Fork {fork}")).muted()),
                None => card,
            }
            .into_any_element()
        }
    }
}

fn qualified_path(host: Option<&HostId>, path: &str) -> String {
    host.map_or_else(|| path.to_owned(), |host| format!("{host}:{path}"))
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        github::{PrChecks, PrReviewDecision},
        ids::RepoId,
    };

    use super::*;

    #[test]
    fn remote_worktree_paths_are_qualified_with_their_host() {
        let host = HostId::try_from("dev-box").expect("host");
        assert_eq!(
            qualified_path(Some(&host), "/srv/fleet/worktree"),
            "dev-box:/srv/fleet/worktree"
        );
        assert_eq!(qualified_path(None, "/tmp/local"), "/tmp/local");
    }

    fn pull_request(cross: bool) -> PullRequest {
        PullRequest {
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            number: 412,
            title: "Fix RUT validation".to_owned(),
            url: "https://github.com/buk/payroll/pull/412".to_owned(),
            author: "dannyfuf".to_owned(),
            head_ref_name: "feat/rut".to_owned(),
            base_ref_name: "main".to_owned(),
            is_draft: false,
            is_cross_repository: cross,
            head_repo: cross
                .then(|| RepoId::try_from("dannyfuf/payroll").unwrap_or_else(|e| panic!("{e}"))),
            review_decision: PrReviewDecision::None,
            checks: PrChecks::Pass,
            checks_passed: Some(9),
            checks_total: Some(9),
            additions: 142,
            deletions: 18,
            labels: vec!["payroll".to_owned()],
            updated_at: "2026-09-04T10:00:00Z".to_owned(),
        }
    }

    #[test]
    fn same_repo_pull_requests_reuse_the_head_branch() {
        let plan = will_create(&pull_request(false));
        assert_eq!(plan.branch, "feat/rut");
        assert_eq!(plan.base, "pull/412/head");
        assert_eq!(plan.worktree, "payroll/feat-rut");
        assert_eq!(plan.fork, None);
    }

    #[test]
    fn cross_repository_pull_requests_get_a_pr_branch_and_a_fork_line() {
        let plan = will_create(&pull_request(true));
        assert_eq!(plan.branch, "pr/412");
        assert_eq!(
            plan.fork.as_deref(),
            Some("dannyfuf/payroll \u{2192} pr/412")
        );
    }

    #[test]
    fn preview_uses_daemon_slug_rules() {
        let mut pr = pull_request(false);
        pr.head_ref_name = "Feature/Foo Bar".to_owned();
        let plan = will_create(&pr);
        assert_eq!(plan.branch, "Feature/Foo Bar");
        assert_eq!(plan.worktree, "payroll/feature-foo-bar");
    }
}
