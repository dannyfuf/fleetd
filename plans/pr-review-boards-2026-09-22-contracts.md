# PR review boards and scheduled agent tasks — Contracts

Every name, field, request, capability and user-visible sentence the four phases share. A card that
introduces one of these copies it from here verbatim; a card that consumes one relies on it being
exactly this. Change this file only together with every card that uses the changed item.

## C1. Board model additions (`crates/fleet-core/src/board/model.rs`)

- `pub enum BoardKind { Tasks, Reviews }` — `#[serde(rename_all = "snake_case")]`, `Default` = `Tasks`,
  `pub fn is_tasks(&self) -> bool`.
- `Board.kind: BoardKind` — `#[serde(default, skip_serializing_if = "BoardKind::is_tasks")]`.
- `pub enum RunLocation { BoardWorktree, CardWorktree }` — snake_case, `Default` = `BoardWorktree`,
  `pub fn is_board_worktree(&self) -> bool`.
- `BoardSettings.run_location: RunLocation` — `#[serde(default, skip_serializing_if = "RunLocation::is_board_worktree")]`.
  Meaning: `BoardWorktree` = every run of the board executes in `board.worktree_id` (today's
  behaviour); `CardWorktree` = each run executes in the card's own `card.worktree_id`, created from
  the card's pull request when missing.
- `pub struct PullRequestRef { pub repo: RepoId, pub number: u64, pub url: String }` — camelCase.
  - `pub fn parse(text: &str) -> Result<PullRequestRef, BoardError>` accepts
    `https://github.com/<owner>/<name>/pull/<n>` (optionally followed by `/files`, `/commits`, `#…` or `?…`)
    and `<owner>/<name>#<n>`. The URL is normalised to `https://github.com/<owner>/<name>/pull/<n>`.
    Refusal: `BoardError::Invalid { field: "pull_request", reason: "expected a GitHub pull request URL or owner/name#number" }`.
  - `pub fn key(&self) -> String` → `"<owner>/<name>#<n>"`.
- `Card.pull_request: Option<PullRequestRef>` — `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- `CardDraft.pull_request: Option<PullRequestRef>` (in `board/ops.rs`). `CardPatch` does **not** gain it:
  a card's pull request never changes after creation.
- `CardRun.worktree_id: Option<WorktreeId>` — `#[serde(default, skip_serializing_if = "Option::is_none")]`,
  the worktree that run executed in. Written for every new run; `None` on runs recorded before this feature.
- `BOARD_DOCUMENT_VERSION` becomes `3`. `document_version(board, cards)` returns `3` when
  `!board.kind.is_tasks() || !board.settings.run_location.is_board_worktree() || cards.iter().any(|c| c.pull_request.is_some() || c.runs.iter().any(|r| r.worktree_id.is_some()))`,
  else the existing `2`/`1` logic. `BOARD_DOCUMENT_MIN_VERSION` stays `1`.

## C2. Reviews board defaults (`crates/fleet-core/src/board/defaults.rs`)

- `pub fn reviews_board_id(context: &ContextId) -> BoardId` → `"reviews-" + context id`, truncated to
  `BOARD_ID_MAX_LEN` (64) with trailing `-` trimmed.
- `pub fn new_reviews_board(context: &Context, now: &str) -> Board` — `new_board(context, now)` then:
  id `reviews_board_id(&context.id)`, name `"Reviews"`, prefix `"REV"`, `kind = Reviews`,
  `statuses = reviews_preset()`, `settings.run_location = CardWorktree`, `settings.max_live_runs = Some(2)`,
  `settings.start_on_worktree = false`.
- `pub fn reviews_preset() -> Vec<Status>`:

| id | Name | Category | On enter | On success | When unblocked |
| --- | --- | --- | --- | --- | --- |
| `pending` | Pending review | Unstarted | — | — | `reviewing` |
| `reviewing` | Reviewing | Started | prompt, `PRESET_INSTRUCTIONS_REVIEW_PR`, expects `PRESET_EXPECT_REVIEW_PR` | `reviewed` | — |
| `reviewed` | Reviewed | Started | — | — | — |
| `published` | Review published | Completed | prompt, `PRESET_INSTRUCTIONS_PUBLISH_REVIEW`, expects `PRESET_EXPECT_PUBLISH_REVIEW` | — | — |
| `dismissed` | Dismissed | Canceled | — | — | — |

- Constants (exact text):
  - `PRESET_INSTRUCTIONS_REVIEW_PR`:
    `Review the pull request {pr_url} ({pr_repo}#{pr_number}). This worktree is checked out at the pull request's head. Read the description and the diff with gh (gh pr view {pr_url}, gh pr diff {pr_url}) and read the surrounding code in this checkout. Your report is the review: first a one-line verdict (approve, request changes, or comment), then a short summary, then numbered findings, each with file:line, severity (blocker, major, minor, nit) and a concrete suggested fix. Do not post anything to GitHub, do not commit, and do not push.`
  - `PRESET_EXPECT_REVIEW_PR`: `a review report: a verdict line, a summary, and numbered findings with file:line`
  - `PRESET_INSTRUCTIONS_PUBLISH_REVIEW`:
    `Publish the review of {pr_url}. The review is the newest succeeded report under "Previous run reports"; apply every note under "Notes from you" before publishing, and drop any finding a note rejects. Post it with gh as one review: the verdict maps to gh pr review --approve, --request-changes or --comment, the summary is the review body, and each finding with a file:line becomes an inline comment through gh api repos/{pr_repo}/pulls/{pr_number}/reviews. Do not change any code. Report the URL of the published review.`
  - `PRESET_EXPECT_PUBLISH_REVIEW`: `the URL of the review now visible on the pull request`

## C3. Templates and the brief (`crates/fleet-core/src/board/defaults.rs`, `board/automation.rs`)

- `render_template(text, key, title)` keeps its signature. A new
  `pub fn render_card_template(text: &str, key: &str, card: &Card) -> String` substitutes `{key}`,
  `{title}`, and — when `card.pull_request` is `Some` — `{pr_url}`, `{pr_repo}`, `{pr_number}`. With
  no pull request the three PR placeholders are left as written. `brief()` and the column `env`
  rendering switch to it.
- `brief()` gains two sections, in this order, after the card description and before
  `## Previous run reports`:
  - `## Pull request` — one line `{pr_repo}#{pr_number} · {pr_url}`, only when the card has one.
  - `## Notes from you` — every comment with `run_id == None` created after the newest
    **succeeded** run's `started_at` (every such comment when no run of the card has succeeded),
    oldest first, each as `- {author}: {body}` (author `you` when absent). Omitted when empty.
    (Amended in hardening round 2: a failed publish must not swallow the note its retry applies.
    Amended in hardening round 3: `started_at`, not `ended_at`, so a note written while the review
    ran reaches the publish run.)
- Each report under `## Previous run reports` is headed `### Report from {created_at} · {outcome
  word}`, the outcome of the run that wrote it (no suffix when that run is no longer on the card),
  so "the newest succeeded report" is unambiguous after a failed run left a report.

## C4. Routing columns queue (`crates/fleet-core/src/board/automation.rs`, `ops/query.rs`)

- **Entry advance (rule 0).** When a card *enters* (a move, a creation, or a cascade move) a column
  whose automation has `advance_when_unblocked = Some(T)` and every blocker of the card is satisfied
  (a card with no blockers qualifies), the walk tries to advance it to `T`:
  - if `T` has an `on_enter` action and the board is at its ceiling
    (`live.len() + in_flight.len() >= settings.max_live_runs()`), the card **stays** where it is and
    gets `pending_run = PendingRun { status_id: T, since }` (keeping an existing `since` for the same
    `T`); nothing else happens;
  - otherwise the card is moved to `T` as `AutoMoved` with message `Moved to {T name}: nothing blocks it`
    and queued, so rule 1 starts it.
- **Slot release.** `next_pending` already returns the oldest `pending_run`. When the returned card's
  `status_id != pending_run.status_id` (a queued card), the walk moves it to `pending_run.status_id`
  as `AutoMoved` `Moved to {T name}: a run slot freed`, clears the marker, and starts it.
- **A start that ends without a run** (a recorded failed start, or one abandoned because the card
  left its column first) hands its slot on at once through the same slot release, and an abandoned
  start's card is re-evaluated as an entry into the column it stands in.
- **Queued is not attention.** `pub fn queued(card: &Card) -> bool` = `pending_run` is `Some` and its
  `status_id != card.status_id`. `attention()` ignores a queued card's `pending_run` age.
- The workflow preset's `Ready` column inherits this: an unblocked card moved into Ready now starts at
  once. The `fleet-board-planning` skill's sentence "Leaving it in Ready waits forever" is replaced by
  "A card moved into Ready with nothing blocking it starts at once, or waits in Ready for a free slot."

## C5. Refusal and outcome sentences (every surface prints them verbatim)

| Where | Sentence |
| --- | --- |
| `require_automatable`, board-worktree board with no worktree | `automation is available on worktree boards only` (unchanged; now raised only when `run_location` is `BoardWorktree`) |
| start, card-worktree board, card has neither worktree nor pull request | `{KEY} has no worktree to run in; link a pull request or create its worktree first` |
| start, card's PR repository is not a Fleet repository | `{owner/name} is not a Fleet repository; clone it into this context first` |
| start, card's worktree lives on another host | `automation is unavailable on a worktree owned by host {host}` (unchanged text) |
| start in the board's first running column, card's existing or adopted worktree cannot follow the pull request's current head (changes to tracked files, or commits Fleet did not place that the head lacks) | `The worktree for {pr_key} has local changes; commit or discard them before the review runs.` (`{pr_key}` = `{owner/name}#{number}`). (Amended in hardening round 3: a clean worktree still at the head Fleet last placed follows a force-pushed head; untracked files are not local changes; a later column — Review published — never moves the checkout.) |
| `UpsertPullRequestCard` first line, CLI | `Created {KEY}` · `Existing {KEY}` · `Reopened {KEY}` |
| Activity on reopen | `Review re-requested` (kind `Updated`) |

## C6. Wire (`crates/fleet-proto`)

- Capability `pub const BOARD_REVIEWS_CAPABILITY: &str = "board.reviews";` (in `response.rs`, next to
  `BOARD_AUTOMATION_CAPABILITY`).
- `RequestBody::EnsureReviewsBoard { context_id: ContextId }` → `ResponseBody::Board(BoardView)`.
- `RequestBody::UpsertPullRequestCard { board_id: BoardId, draft: CardDraft, #[serde(default, skip_serializing_if = "Option::is_none")] requested_at: Option<String> }`
  → `ResponseBody::CardUpsert { card: Card, outcome: UpsertOutcome }`, where
  `pub enum UpsertOutcome { Created, Existing, Reopened }` (snake_case). `draft.pull_request` must be `Some`.
- Capability `pub const SCHEDULES_CAPABILITY: &str = "schedules";`.
- `RequestBody::ListSchedules { #[serde(default, skip_serializing_if = "Option::is_none")] board_id: Option<BoardId> }` → `ResponseBody::Schedules(Vec<Schedule>)`.
- `RequestBody::CreateSchedule { draft: ScheduleDraft }` → `ResponseBody::Schedule(Schedule)`.
- `RequestBody::UpdateSchedule { id: ScheduleId, patch: SchedulePatch }` → `ResponseBody::Schedule(Schedule)`.
- `RequestBody::DeleteSchedule { id: ScheduleId }` → `ResponseBody::Ok` (or the crate's existing unit response; use whatever `DeleteBoard` answers).
- `RequestBody::RunScheduleNow { id: ScheduleId }` → `ResponseBody::Schedule(Schedule)` (the run is recorded as started; the answer's `runs` end at the run this request recorded, so a concurrent fire never takes its place).
- `BoardSummary` gains `#[serde(default, skip_serializing_if = "is_zero")] idle_started: u32`
  (`idleStarted`): cards in a `Started`-category column with no live or owed run and not counted by
  `attention_count`, so the two add up to the cards waiting on a person.
- `Event::SchedulesChanged { board_id: BoardId }` with `EventKind::SchedulesChanged`.
- `JobKind::ScheduledTask` — the job a schedule run is.
- Client gate: `required_capability` maps the first two requests to `board.reviews` and the five
  schedule requests to `schedules`.

## C7. Schedule model (`crates/fleet-core/src/schedule.rs`, new module)

- `string_id!(ScheduleId, "schedule", |value| validate_slug("schedule id", value))` in `ids.rs`;
  generated as `sch-` + 8 lowercase hex. A malformed id is refused as `invalid schedule id
  `{value}`: …`. (Amended in hardening round 3: `validate_slug` names the id family it parses,
  so board, status and label ids no longer say `context id` either.)
- `pub struct Schedule { id, board_id: BoardId, name: String, prompt: String, cadence: Cadence, agent: ScheduleAgent, enabled: bool, timeout_minutes: u32, created_at: String, updated_at: String, runs: Vec<ScheduleRun>, next_run_at: Option<String> }` (camelCase).
- `pub enum Cadence { Every { minutes: u32 }, Once { at: String } }` — `#[serde(tag = "kind", rename_all = "snake_case")]`.
- `pub struct ScheduleAgent { provider: AgentKind, model: Option<String>, effort: Option<String>, mode: PermissionMode }` — default Claude, none, none, `FullAccess`.
- `pub struct ScheduleRun { job_id: Option<JobId>, started_at: String, ended_at: Option<String>, outcome: Option<ScheduleOutcome>, summary: Option<String>, cost_usd: Option<f64>, log_path: Option<String> }`.
- `pub enum ScheduleOutcome { Succeeded, Failed, TimedOut, Skipped }` (snake_case).
- `pub struct ScheduleDraft { board_id, name, prompt, cadence, agent: Option<ScheduleAgent>, enabled: Option<bool>, timeout_minutes: Option<u32> }` and `pub struct SchedulePatch` with every field but `board_id` optional.
- Limits: `SCHEDULE_MIN_EVERY_MINUTES = 5`, `SCHEDULE_MAX_EVERY_MINUTES = 1440`, `SCHEDULE_DEFAULT_TIMEOUT_MINUTES = 20`, `SCHEDULE_MAX_TIMEOUT_MINUTES = 120`, `MAX_RUNS_PER_SCHEDULE = 20`, `SCHEDULE_PROMPT_MAX_BYTES = 16 * 1024`, `SCHEDULE_NAME_MAX_CHARS = 80`, `SCHEDULE_SUMMARY_MAX_CHARS = 280`.
- Refusals (`ScheduleError::Invalid { field, reason }`): `name` `must not be empty` / `must not contain a NUL byte` / `must be at most 80 characters`; `prompt` `must not be empty` / `must not contain a NUL byte` / `must be at most 16 KiB`; `cadence` `every must be between 5 and 1440 minutes` / `once needs an RFC 3339 time`; `timeout_minutes` `must be between 1 and 120`; `model` `must not be blank`; `effort` `must not be blank`; `mode` `{mode} is not supported by {provider}`; `board_id` (daemon) `no board {id}`. (Amended in hardening round 3 to record the round-2 refusals: a NUL byte cannot reach a process argument, and a blank model or effort is a flag the agent refuses. A stored document written before these refusals existed is repaired on load: a blank `model` or `effort` reads as `None`.)
- `pub fn next_run_at(schedule: &Schedule, now: DateTime<Utc>) -> Option<DateTime<Utc>>`:
  disabled → `None`; `Once { at }` → `Some(at)` when no run (of any outcome) has a `started_at` at
  or after `at`, else `None` — so a "Run now" before `at` does not cancel the real fire, and moving
  a fired schedule to a new future `at` makes it due again;
  `Every { minutes }` → `min(last started_at, now) + minutes` (a clock stepped backwards does not
  silence it), or `now` when the schedule has never run; a result earlier than `now` becomes `now`
  (one catch-up run after downtime, never several). `Once.at` is stored normalised to UTC to the
  second.
- `SCHEDULES_DOCUMENT_VERSION = 1`; document `SchedulesDocument { version: u32, schedules: Vec<Schedule> }`
  stored at `<FLEET_HOME>/schedules.json` (`FleetHome::schedules_path()`).
- Per-schedule directories: `<FLEET_HOME>/schedules/<id>/work` (the run's cwd) and
  `<FLEET_HOME>/schedules/<id>/logs/<started_at compact>.log`.
- Prompt placeholders rendered at fire time: `{board}` (board id), `{last_run_at}` (RFC 3339 of the
  previous run's `started_at`, passing over `Skipped` fires, or `never`), `{now}`. (Amended in
  hardening round 3 to record the round-1 fix: a skipped fire launched no agent and read no
  source, so it is not a previous run.)
- `pub const SCHEDULE_FOOTER_TEMPLATE` appended after the rendered prompt (exact text, `{fleet}` is the
  quoted absolute path to the `fleet` binary or `fleet`):

  ```
  --- Fleet scheduled task "{name}" for board {board} ---
  Record every pull request you are asked to review as a card on this board with:
    {fleet} board --board {board} card new "<pull request title>" --pr <pull request URL> --requested-at <RFC 3339 time the review was requested> --label <source>
  <source> is one of the board's labels; run `{fleet} board --board {board} describe` to list them.
  The command prints "Created <KEY>", "Existing <KEY>" or "Reopened <KEY>" first; a pull request already on the board is never duplicated, so run it for every request you find.
  Only consider requests made after {last_run_at} when a source keeps old messages (chat channels, email).
  Do not review, comment on, or change any pull request. Do not edit files.
  End your reply with exactly one line: SUMMARY: <n> created, <n> existing, <n> reopened, <anything the user must know>
  ```
- `pub const STARTER_PROMPT_GITHUB_REVIEWS`:
  `List every open pull request where my review is requested, directly or through one of my teams, with gh search prs --review-requested=@me --state=open --json url,title,repository,updatedAt. For each one, use its updatedAt as the requested time and github as the source label.`
- Labels a fresh Reviews board ships with: `github`, `chat` (ids `github`, `chat`).

## C8. CLI (`crates/fleet-cli`)

- Global board selector `--reviews` (boolean): the Reviews board of `--context <id>` or of the active
  context; conflicts with `--board` and `--worktree`; ensures the board exists (like `--context`).
- `fleet board card new … --pr <ref> [--requested-at <rfc3339>]` sends `UpsertPullRequestCard` and
  prints `Created|Existing|Reopened <KEY>` then the card as today. Without `--pr`, `card new` is unchanged
  and `--requested-at` is refused: `--requested-at needs --pr`.
- `fleet schedule list|show <id>|new|edit <id>|rm <id>|run <id>|runs <id>` with the same board
  selectors (`--board`, `--worktree`, `--context`, `--reviews`) and `--json`.
  `new` flags: `--name`, `--prompt` | `--prompt-file` | `--starter github-reviews`, `--every <minutes>` | `--once <rfc3339>`,
  `--provider`, `--model`, `--effort`, `--mode`, `--timeout <minutes>`, `--disabled`.
