//! Defaults for local boards scoped to a context or worktree.

use super::{
    model::{
        Action, ActionKind, BackendRef, Board, BoardKind, BoardSettings, Card, ColumnAgentPrefs,
        ColumnAutomation, Label, RunLocation, Status, StatusCategory, SyncState,
    },
    ops::normalise_automation,
};
use crate::{
    ids::{BoardId, ContextId, LabelId, StatusId, WorktreeId},
    model::{Context, Worktree},
    slug::slugify,
};

/// Maximum byte length of a derived worktree board id.
pub const BOARD_ID_MAX_LEN: usize = 64;

/// What the preset's implementation column tells a run to do.
pub const PRESET_INSTRUCTIONS_IMPLEMENT: &str =
    "Implement this card in the current worktree. Do not commit.";
/// What the preset's implementation column expects back.
pub const PRESET_EXPECT_IMPLEMENT: &str = "make lint and make test pass";
/// What the preset's review column expects back.
pub const PRESET_EXPECT_REVIEW: &str = "the review finds no blocking issue";
/// The skill the preset's review column invokes.
pub const PRESET_REVIEW_SKILL: &str = "deep-review";

/// What the reviews preset's Reviewing column tells a run to do.
pub const PRESET_INSTRUCTIONS_REVIEW_PR: &str = "Review the pull request {pr_url} ({pr_repo}#{pr_number}). This worktree is checked out at the pull request's head. Read the description and the diff with gh (gh pr view {pr_url}, gh pr diff {pr_url}) and read the surrounding code in this checkout. Your report is the review: first a one-line verdict (approve, request changes, or comment), then a short summary, then numbered findings, each with file:line, severity (blocker, major, minor, nit) and a concrete suggested fix. Do not post anything to GitHub, do not commit, and do not push.";
/// What the reviews preset's Reviewing column expects back.
pub const PRESET_EXPECT_REVIEW_PR: &str =
    "a review report: a verdict line, a summary, and numbered findings with file:line";
/// What the reviews preset's Review published column tells a run to do.
pub const PRESET_INSTRUCTIONS_PUBLISH_REVIEW: &str = "Publish the review of {pr_url}. The review is the newest succeeded report under \"Previous run reports\"; apply every note under \"Notes from you\" before publishing, and drop any finding a note rejects. Post it with gh as one review: the verdict maps to gh pr review --approve, --request-changes or --comment, the summary is the review body, and each finding with a file:line becomes an inline comment through gh api repos/{pr_repo}/pulls/{pr_number}/reviews. Do not change any code. Report the URL of the published review.";
/// What the reviews preset's Review published column expects back.
pub const PRESET_EXPECT_PUBLISH_REVIEW: &str =
    "the URL of the review now visible on the pull request";

/// Creates the five initial ordered status columns.
#[must_use]
pub fn default_statuses() -> Vec<Status> {
    [
        ("backlog", "Backlog", StatusCategory::Backlog),
        ("todo", "Todo", StatusCategory::Unstarted),
        ("in-progress", "In Progress", StatusCategory::Started),
        ("done", "Done", StatusCategory::Completed),
        ("canceled", "Canceled", StatusCategory::Canceled),
    ]
    .into_iter()
    .map(|(id, name, category)| Status {
        id: StatusId::try_from(id).expect("static status slug is valid"),
        name: name.into(),
        category,
        color: None,
        automation: None,
    })
    .collect()
}

/// The seven-column pipeline a board opts into: Backlog, Todo, Ready, In Progress, In review,
/// Done, Canceled.
///
/// Todo stays human on purpose. Ready is the routing column — a card is put there once it is
/// meant to run, and `advance_when_unblocked` releases it into In Progress as soon as every
/// card blocking it is done — so nothing a person leaves in Todo can start itself.
///
/// The preset sets no `max_live_runs`, which leaves the board at one live run: every card on a
/// worktree board edits the same checkout, and a second concurrent run is a decision its owner
/// makes deliberately.
#[must_use]
pub fn workflow_preset() -> Vec<Status> {
    let status = |id: &str, name: &str, category, automation| Status {
        id: StatusId::try_from(id).expect("static status slug is valid"),
        name: name.into(),
        category,
        color: None,
        automation,
    };
    let route = |id: &str| StatusId::try_from(id).expect("static status slug is valid");
    vec![
        status("backlog", "Backlog", StatusCategory::Backlog, None),
        status("todo", "Todo", StatusCategory::Unstarted, None),
        status(
            "ready",
            "Ready",
            StatusCategory::Unstarted,
            Some(ColumnAutomation {
                on_enter: None,
                on_success: None,
                advance_when_unblocked: Some(route("in-progress")),
            }),
        ),
        status(
            "in-progress",
            "In Progress",
            StatusCategory::Started,
            Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Prompt,
                    instructions: PRESET_INSTRUCTIONS_IMPLEMENT.into(),
                    expect: PRESET_EXPECT_IMPLEMENT.into(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                on_success: Some(route("in-review")),
                advance_when_unblocked: None,
            }),
        ),
        status(
            "in-review",
            "In review",
            StatusCategory::Started,
            Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Skill {
                        name: PRESET_REVIEW_SKILL.into(),
                        args: String::new(),
                    },
                    instructions: String::new(),
                    expect: PRESET_EXPECT_REVIEW.into(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                on_success: Some(route("done")),
                advance_when_unblocked: None,
            }),
        ),
        status("done", "Done", StatusCategory::Completed, None),
        status("canceled", "Canceled", StatusCategory::Canceled, None),
    ]
}

/// Adds the preset columns this board is missing and reports whether it changed anything.
///
/// Matching is by [`StatusId`], and an existing column is never touched: its name, category,
/// colour and automation are its owner's, and a board that already renamed `in-progress` or
/// wired its own action keeps both. A missing column lands in preset order relative to the
/// neighbours already present, so applying this to the default five inserts Ready before
/// In Progress and In review after it.
pub fn apply_workflow_preset(board: &mut Board) -> bool {
    let mut cursor = 0;
    let mut added = false;
    for status in workflow_preset() {
        match board.statuses.iter().position(|s| s.id == status.id) {
            Some(position) => cursor = position + 1,
            None => {
                let at = cursor.min(board.statuses.len());
                board.statuses.insert(at, status);
                cursor = at + 1;
                added = true;
            }
        }
    }
    normalise_automation(&mut board.statuses);
    added
}

/// Substitutes `{key}` and `{title}` in a column's instructions or an env value.
///
/// Anything else in braces is left alone: a column's instructions are markdown a person wrote,
/// and a JSON snippet or a shell brace expansion in them is text, not a placeholder.
#[must_use]
pub fn render_template(text: &str, key: &str, title: &str) -> String {
    text.replace("{key}", key).replace("{title}", title)
}

/// Substitutes `{key}`, `{title}` and, when the card has a pull request, `{pr_url}`,
/// `{pr_repo}` and `{pr_number}`.
///
/// Without a pull request the three PR placeholders are left as written: a Tasks board may
/// legitimately print `{pr_url}` in its instructions.
#[must_use]
pub fn render_card_template(text: &str, key: &str, card: &Card) -> String {
    let rendered = render_template(text, key, &card.title);
    match &card.pull_request {
        Some(pull_request) => rendered
            .replace("{pr_url}", &pull_request.url)
            .replace("{pr_repo}", pull_request.repo.as_str())
            .replace("{pr_number}", &pull_request.number.to_string()),
        None => rendered,
    }
}

/// First three ASCII alphanumeric name characters, uppercased; FLT when absent.
#[must_use]
pub fn default_prefix(context: &Context) -> String {
    let prefix: String = context
        .name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(3)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if prefix.is_empty() {
        "FLT".into()
    } else {
        prefix
    }
}

/// Creates an empty local board with the context's identity and current timestamp.
#[must_use]
pub fn new_board(context: &Context, now: &str) -> Board {
    Board {
        id: context.id.clone().into(),
        context_id: context.id.clone(),
        worktree_id: None,
        kind: BoardKind::Tasks,
        name: context.name.clone(),
        prefix: default_prefix(context),
        next_number: 1,
        backend: BackendRef::default(),
        statuses: default_statuses(),
        labels: Vec::new(),
        properties: Vec::new(),
        default_repo_id: None,
        settings: BoardSettings::default(),
        sync: SyncState::default(),
        created_at: now.into(),
        updated_at: now.into(),
    }
}

/// Derives a stable board id from all three worktree-id parts.
#[must_use]
pub fn worktree_board_id(worktree: &WorktreeId) -> BoardId {
    let (owner, name) = worktree
        .repo()
        .split_once('/')
        .expect("a validated worktree id always contains an owner/name repository");
    let parts = [owner, name, worktree.slug()].map(board_id_part).join("-");
    let mut id = format!("wt-{parts}");
    id.truncate(BOARD_ID_MAX_LEN);
    while id.ends_with('-') {
        id.pop();
    }
    BoardId::try_from(id).expect("a derived worktree board id is always a valid board slug")
}

/// Creates an empty local board scoped to one worktree.
#[must_use]
pub fn new_worktree_board(context: &Context, worktree: &Worktree, now: &str) -> Board {
    let mut board = new_board(context, now);
    board.id = worktree_board_id(&worktree.id);
    board.worktree_id = Some(worktree.id.clone());
    board.name = worktree.slug.clone();
    board.prefix = worktree_prefix(&worktree.slug);
    board.default_repo_id = Some(worktree.repo_id.clone());
    board
}

/// The five-column pipeline of a Reviews board: Pending review, Reviewing, Reviewed, Review
/// published, Dismissed.
///
/// Pending review is the routing column: a card waits there until nothing blocks it and the
/// board has room, then moves to Reviewing, whose run writes the review without posting it.
/// Reviewed is where a person reads the report and leaves notes; moving the card to Review
/// published is the deliberate step that posts it. No column names an agent provider, so the
/// daemon's default applies.
#[must_use]
pub fn reviews_preset() -> Vec<Status> {
    let status = |id: &str, name: &str, category, automation| Status {
        id: StatusId::try_from(id).expect("static status slug is valid"),
        name: name.into(),
        category,
        color: None,
        automation,
    };
    let route = |id: &str| StatusId::try_from(id).expect("static status slug is valid");
    let prompt = |instructions: &str, expect: &str| Action {
        kind: ActionKind::Prompt,
        instructions: instructions.into(),
        expect: expect.into(),
        agent: ColumnAgentPrefs::default(),
        env: Vec::new(),
    };
    vec![
        status(
            "pending",
            "Pending review",
            StatusCategory::Unstarted,
            Some(ColumnAutomation {
                on_enter: None,
                on_success: None,
                advance_when_unblocked: Some(route("reviewing")),
            }),
        ),
        status(
            "reviewing",
            "Reviewing",
            StatusCategory::Started,
            Some(ColumnAutomation {
                on_enter: Some(prompt(
                    PRESET_INSTRUCTIONS_REVIEW_PR,
                    PRESET_EXPECT_REVIEW_PR,
                )),
                on_success: Some(route("reviewed")),
                advance_when_unblocked: None,
            }),
        ),
        status("reviewed", "Reviewed", StatusCategory::Started, None),
        status(
            "published",
            "Review published",
            StatusCategory::Completed,
            Some(ColumnAutomation {
                on_enter: Some(prompt(
                    PRESET_INSTRUCTIONS_PUBLISH_REVIEW,
                    PRESET_EXPECT_PUBLISH_REVIEW,
                )),
                on_success: None,
                advance_when_unblocked: None,
            }),
        ),
        status("dismissed", "Dismissed", StatusCategory::Canceled, None),
    ]
}

/// Derives the id of a context's Reviews board, `reviews-<context>`.
///
/// The result is truncated to [`BOARD_ID_MAX_LEN`] with any trailing `-` trimmed, the same way
/// [`worktree_board_id`] bounds its ids.
#[must_use]
pub fn reviews_board_id(context: &ContextId) -> BoardId {
    let mut id = format!("reviews-{context}");
    id.truncate(BOARD_ID_MAX_LEN);
    while id.ends_with('-') {
        id.pop();
    }
    BoardId::try_from(id).expect(
        "a context id is a validated slug, so `reviews-` plus its truncated ASCII form is one too",
    )
}

/// Creates a context's Reviews board: the reviews preset, runs in each card's own worktree,
/// two at a time.
///
/// It ships with the two source labels a review request can carry, `github` and `chat`, and
/// never starts a worktree for a card on its own: the card's pull request decides the checkout.
#[must_use]
pub fn new_reviews_board(context: &Context, now: &str) -> Board {
    let mut board = new_board(context, now);
    board.id = reviews_board_id(&context.id);
    board.name = "Reviews".into();
    board.prefix = "REV".into();
    board.kind = BoardKind::Reviews;
    board.statuses = reviews_preset();
    board.labels = [("github", "accent"), ("chat", "info")]
        .into_iter()
        .map(|(id, color)| Label {
            id: LabelId::try_from(id).expect("static label slug is valid"),
            name: id.into(),
            color: Some(color.into()),
        })
        .collect();
    board.settings.run_location = RunLocation::CardWorktree;
    board.settings.max_live_runs = Some(2);
    board.settings.start_on_worktree = false;
    board
}

fn board_id_part(value: &str) -> String {
    let part = slugify(&slugify(value).replace(['.', '_'], "-"));
    if part.is_empty() { "x".into() } else { part }
}

fn worktree_prefix(slug: &str) -> String {
    let prefix: String = slug
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(3)
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if prefix.is_empty() {
        "WT".into()
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests;
