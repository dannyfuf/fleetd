# Board — Jira backend (via `acli`)

Status: **implemented** (authoritative for the Jira milestone). Builds on `docs/BOARD.md`; every signature
there stays valid. This file was reconciled against the code at integration: where the implementation
refined the contract, the text below is the refined rule, and the change is called out inline. When code
and this doc disagree from here on, fix the code.

## 0. Goal and constraints

A `BoardBackend` of kind `"jira"` that mirrors one Jira project (optionally narrowed by JQL) into a context's
board, using **only the Atlassian CLI (`acli`)** through the daemon's `Shell` adapter — no HTTP client, no
API token. Verified against `acli` 1.3.18 (`docs/BOARD-JIRA.md` §7 lists the exact commands):

- `search` returns only `key, summary, status, assignee, priority, issuetype, description, labels`; everything
  else (updated, duedate, parent, comments, custom fields) needs one `view --fields … --json` **per key**.
- `edit` can change **summary, description, assignee, labels** only. Priority, due date, parent and story
  points are **not writable** post-create. `transition --status <name>` moves by **status name**; transitions
  cannot be listed. `comment create` adds comments.
- Descriptions and comment bodies are **ADF** (Atlassian Document Format) JSON, not text.
- No cursor pagination, no page metadata; incremental sync is JQL `updated >= "-<n>m"`.
- One active `acli` account per machine (`acli jira auth status`), OAuth session; failures come back as text.

Consequences encoded below: per-key fetch with bounded concurrency, name-keyed statuses, a pure ADF⇄markdown
converter, backend-declared **read-only fields** enforced by the core, and a **settings schema** so the app
renders backend settings generically (nothing Jira-specific in `fleet-app`).

## 1. Ownership

| Piece | Path | Stage |
|---|---|---|
| Core additions (§2) | `crates/fleet-core/src/board/{sync,ops,model}.rs` | contracts (types) / service (logic+tests) |
| Proto additions (§2) | `crates/fleet-proto/src/{request,response}.rs`, `crates/fleet-client/src/api.rs` | contracts |
| Trait/registry changes (§3) | `crates/fleet-daemon/src/adapters/board/{mod,local}.rs`, `adapters/mod.rs`, `testing/fakes.rs` | contracts |
| Jira backend | `crates/fleet-daemon/src/adapters/board/jira/{mod,settings,acli,adf,map,users}.rs` | contracts (skeleton) → jira-adf (adf.rs) + jira-backend (rest) |
| Service changes (§4) | `crates/fleet-daemon/src/services/boards.rs`, `services/mod.rs` dispatch arms | contracts (arms/signatures) → service (logic) |
| Daemon tests | `crates/fleet-daemon/tests/boards_jira.rs`, `tests/fixtures/jira/*.json` | jira-backend |
| CLI (§5) | `crates/fleet-cli/src/{args.rs,commands/board.rs,envelope.rs,human.rs}`, `tests/board_cli.rs` | cli |
| App (§6) | `crates/fleet-app/src/dialogs/board_settings.rs`, `dialogs/card_picker.rs`, `views/board_card_detail.rs`, `dialogs/card_detail.rs`, `state.rs`, `keymap.rs`, `actions.rs`, `dialogs/palette.rs`, `docs/KEYMAP.md` | app |
| Docs | this file, `docs/BOARD.md` §10, `docs/ARCHITECTURE.md`, `README.md` | each stage its surface; integrate consolidates |

Rules as in `docs/BOARD.md` §1: disjoint ownership, no dependency changes (everything needed exists:
`serde_json`, `chrono`, `regex`, `tokio` process, `async-trait`), no commits, RFC3339 timestamps.

## 2. Core and protocol additions (all additive, `#[serde(default)]`)

```rust
// fleet_core::board::sync
pub struct BackendSchema { …existing…, 
    /// Standard card fields the backend cannot write back; the core rejects local edits to them.
    /// Values: "title" "description" "status_id" "priority" "labels" "assignee" "estimate" "due_date" "parent_id".
    #[serde(default)] pub readonly_fields: Vec<String> }
pub struct SyncState { …existing…, #[serde(default)] pub readonly_fields: Vec<String> }
// `adopt_schema` **replaces** `sync.readonly_fields` with the schema's list (it never merges), so a backend
// that stops refusing a field stops refusing it — and every later `adopt_schema` call must pass the list again.
/// What a backend kind looks like to clients (for `fleet board set --backend` and the settings dialog).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDescriptor { pub kind: String, pub label: String, pub capabilities: BackendCapabilities,
    /// Settings rendered generically: key = JSON key in `BackendRef.settings`; kinds Text/Number/Bool/Select/MultiSelect.
    pub settings_schema: Vec<PropertySchema> }

// fleet_core::board::ops
/// NEW rule: `apply_card_patch` returns `BoardError::ReadOnlyField(field)` when the patch **changes** a field
/// listed in `board.sync.readonly_fields` (only relevant on non-local boards). A patch that restates the
/// current value changes nothing and is not an edit, so it passes. `move_card` likewise if "status_id" is
/// read-only — a same-column reorder still works, because it changes no status.
pub enum BoardError { …existing…, #[error("{0} is read-only on this board's backend")] ReadOnlyField(String) }
/// Helper for CLI/app: parse `k=v` settings (value parsed as JSON when valid, else string) and merge into a settings object.
pub fn merge_settings(base: &serde_json::Value, pairs: &[(String, String)]) -> Result<serde_json::Value, BoardError>;

// fleet_proto
RequestBody::SyncBoard { board_id, #[serde(default)] full: bool }          // full = ignore the cursor
RequestBody::ListBoardBackends {}  → ResponseBody::BoardBackends(Vec<BackendDescriptor>)
// client: sync_board(board_id, full), list_board_backends()
```

## 3. Backend trait and registry changes

```rust
// adapters/board/mod.rs
pub trait BoardBackend: Send + Sync {
    …existing…,
    fn label(&self) -> &'static str;                       // "Local", "Jira (acli)"
    fn settings_schema(&self) -> Vec<PropertySchema>;      // for BackendDescriptor
}
impl BoardBackends {
    pub fn system(shell: Arc<dyn Shell>, clock: Arc<dyn Clock>) -> Self;   // [LocalBackend, JiraBackend::new(shell, clock)]
    pub fn descriptors(&self) -> Vec<BackendDescriptor>;   // sorted by kind, "local" first
}
// adapters/mod.rs: Adapters::system(files) passes its `shell` (and a SystemClock) into BoardBackends::system.
// testing/fakes.rs: FakeBackend implements the two new methods (label "Fake", empty schema).
```

## 4. Service changes (`services/boards.rs`)

- `sync(&self, id, full: bool)`; `full` → pull with `cursor = None` and **clear** `sync.cursor` first (under the
  mutation gate, before the job starts, so a failed full sync still leaves the cursor cleared).
- After `pull`, before `reconcile`: collect the distinct `RemoteStatus`es of pulled cards that are not yet in
  `status_map.remote_to_local` (or whose mapping points at a dead column). For each, **insert a real column**
  first — after the last column of its own `StatusCategory` or earlier, so a status a project gained between two
  syncs does not land a *started* column to the right of the completed one; a status whose category the remote
  did not declare is a guess either way and goes at the end — `adopt_schema` alone only matches an already-mapped board by name/category, it does not invent
  columns — then call `adopt_schema` again so the map picks them up, so newly seen Jira statuses become columns
  instead of "unmapped" activity noise. The schema handed to that second call is the **described** schema minus
  any status the pull re-describes, plus the pulled ones: `adopt_schema` replaces `status_map`, `properties`
  and `readonly_fields` wholesale, so a statuses-only schema would erase every mapping and property `describe`
  named and silently disarm the read-only guard. Skipped entirely while the board still carries the untouched
  default columns with nothing mapped, where a wholesale adoption would trade them for the handful this pull
  mentioned. A pulled status whose name already matches a column is mapped, never duplicated; one with neither
  id nor name is ignored.
- `list_backends(&self) -> Vec<BackendDescriptor>`; dispatch arm for `ListBoardBackends`.
- `update`: when `patch.backend` changes **kind**, `settings` must come from the patch (never carried over): the
  stored settings are dropped before `apply_board_patch`, and `sync` (cursor, status map, read-only fields) is
  reset — they all describe the remote the board is leaving.
- `update_card`/`move_card` propagate `BoardError::ReadOnlyField` as `DaemonError::Validation`
  (`ErrorKind::Validation` on the wire, the same class as `BoardError::Invalid`) with the message intact — the
  CLI and app show it verbatim.
- `delete_card` clears a child's `parent_id` **directly** (plus the usual activity, never dirtying the card)
  rather than through `apply_card_patch`, which would refuse it on a backend where `parent_id` is read-only.
- `describe_backend` works on a `local` board too — `LocalBackend::describe` answers with an empty
  `BackendSchema`, so `fleet board describe` never fails for lack of a remote.

## 5. Jira backend (`adapters/board/jira/`)

```rust
// settings.rs — deserialized from BackendRef.settings (unknown keys rejected with a clear message)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JiraSettings {
    #[serde(default)] pub project: String,                // required, e.g. "SP"; defaulted so the
        // empty case is refused by name ("a Jira project key is required") and not by serde's
        // "missing field", which is the message `fleet board set --backend jira` would print.
        // Shape-checked against `^[A-Za-z][A-Za-z0-9_]*$`: it is the one JQL fragment that is
        // interpolated rather than bound, so `SP OR project = OTHER` would widen the board onto
        // another project. `base_jql` quotes it anyway, for a key that is a reserved word.
    #[serde(default)] pub site: Option<String>,           // e.g. "buk.atlassian.net"; checked against `auth status`; also used for URLs
    #[serde(default)] pub jql: Option<String>,            // extra filter ANDed with `project = "X"`;
        // a trailing `ORDER BY …` is stripped by `parse` — `base_jql` parenthesizes the filter and
        // every search appends its own ordering, and Jira rejects `(… ORDER BY rank) ORDER BY …`.
        // Only the rightmost clause that is outside every quoted literal and has no bracket after
        // it: `summary ~ "order by date"` is search text, and cutting it unbalances the quote.
        // `normalize` stores the stripped form, so no surface shows a clause the searches ignore.
        // Balance-checked too: the filter is interpolated inside `( … )`, so `1=1) OR (project =
        // "OTHER"` would close the group and widen the board past the project it names — the very
        // thing `project`'s regex refuses — and an unclosed quote fails every search
    #[serde(default)] pub issue_type: Option<String>,     // default type for created issues (required when pushing new cards)
    #[serde(default)] pub story_points_field: Option<String>, // e.g. "customfield_10102" → Card.estimate
    #[serde(default)] pub statuses: Vec<String>,          // explicit column order by Jira status name; else sampled.
        // Refused blank or repeated (case-insensitively): a blank name becomes a column `validate_board` then
        // rejects, wedging every later sync with a message that names no setting.
        // The order is adopted once, on a board that still carries its default columns: `adopt_schema` never
        // reorders columns a board has already synced, and a status a later pull brings back is inserted by
        // its declared category
    #[serde(default)] pub extra_fields: Vec<ExtraField>,  // additional read-only fields surfaced as card properties
    #[serde(default = "default_overlap")] pub overlap_minutes: u32,       // 10 (1..=1440: Jira refuses a wider
        // window, and `0` removes the protection the setting exists for — `updated` has minute
        // resolution and the `-Nm` window is evaluated on the site's clock, so a window starting
        // exactly at our watermark hides every issue edited inside the previous pull's minute)
    #[serde(default = "default_concurrency")] pub max_concurrency: usize, // 4 (1..=8)
    #[serde(default = "default_full_every")] pub full_sync_every: u32,    // 10 (>= 1): every Nth pull is full (catches deletions/filter exits); `0` would make every pull full
    #[serde(default = "default_sample")] pub sample_limit: u32,           // 200 issues sampled by describe()
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraField { pub id: String /* customfield_10000 */, pub name: String, #[serde(default)] pub kind: PropertyKind /* Text */ }
impl JiraSettings { pub fn parse(v: &serde_json::Value) -> Result<Self, BoardError>; pub fn base_jql(&self) -> String; pub fn browse_url(&self, key: &str) -> Option<String>; }
// `BoardBackend::normalize(settings) -> Value` (default: identity) is what the service stores after `validate`.
// Jira's rewrites `jql` only, and only when the caller supplied it: filling the optional defaults in would turn
// every unset row of the settings dialog into a value the user never chose.
pub fn settings_schema() -> Vec<PropertySchema>;   // project, site, jql, issueType, storyPointsField, statuses (MultiSelect free), overlapMinutes, maxConcurrency, fullSyncEvery — all `source: Backend`, `editable: true`
// `PropertySchema` has no `required` flag, so a required setting says so in its human name: "Project key (required)".
// The trailing "(required)" marker is the convention the app reads to star and enforce the row; the hard refusal
// stays in `JiraSettings::parse`. Human names: "Project key (required)", "Site", "Extra JQL", "Issue type",
// "Story points field", "Columns", "Overlap (minutes)", "Max concurrency", "Full sync every".
// `sampleLimit` and `extraFields` parse but are deliberately absent from the dialog schema (free-form keys,
// still writable through `fleet board set --setting`).

// acli.rs — the only place that spawns `acli`
pub struct Acli { shell: Arc<dyn Shell>, permits: Arc<tokio::sync::Semaphore>, program: String /* "acli" */ }
impl Acli {
    pub fn new(shell: Arc<dyn Shell>, max_concurrency: usize) -> Self;
    /// Runs `acli jira <args>` with a 90 s timeout; parses stdout as JSON. Retries 3× with backoff (1s,3s,9s) on
    /// rate-limit/transient failures; maps auth failures to BoardError::Backend("acli is not authenticated (run `acli jira auth login`)").
    pub async fn json(&self, args: &[&str]) -> Result<serde_json::Value, BoardError>;
    /// As `json`, with no retry: for the two calls that are not idempotent (`workitem create`,
    /// `workitem comment create`). A write Jira accepted and then failed to answer — a gateway
    /// 504, a killed 90 s call — is classified transient like any other, and retrying it files a
    /// second issue or a second comment. A lost write is a failed push the next sync repeats.
    pub async fn json_once(&self, args: &[&str]) -> Result<serde_json::Value, BoardError>;
    /// As `json`, but a `NotFound` failure is `Ok(None)`: `pull` needs the 404 *class* at the call site,
    /// which `json` collapses into a message.
    pub async fn json_optional(&self, args: &[&str]) -> Result<Option<serde_json::Value>, BoardError>;
    /// As `json`, keeping *every* JSON document stdout carried instead of only the first. The one
    /// caller is the paginated `workitem search` of a pull: `--paginate` is documented to answer one
    /// merged array, but a CLI that answers one document per page would otherwise have every key past
    /// page one dropped, and a full pull reports a key it never listed as deleted and archives its card.
    /// The keys are deduplicated by the caller, so overlapping pages cost no extra `view`.
    pub async fn json_all(&self, args: &[&str]) -> Result<Vec<serde_json::Value>, BoardError>;
    pub async fn text(&self, args: &[&str]) -> Result<String, BoardError>;
    /// Parses "Authenticated / Site: … / Email: …". A CLI that answers "not authenticated" is
    /// `Ok(AuthStatus::default())`, not an error — it answered the question; `validate` is what refuses.
    pub async fn auth_status(&self) -> Result<AuthStatus, BoardError>;
    pub async fn version(&self) -> Result<String, BoardError>;           // `acli --version`
}
pub const TIMEOUT: Duration; pub const BACKOFF: [Duration; 3];           // the policy above, named once
pub const NOT_AUTHENTICATED: &str;  pub const NOT_INSTALLED: &str;       // the two verbatim messages, once
pub struct AuthStatus { pub authenticated: bool, pub site: Option<String>, pub email: Option<String> }
/// Classifies stderr/stdout text: NotAuthenticated | RateLimited | NotFound | Transition(msg) | Other(msg).
/// `NotFound` is decided by the *subject* of the sentence, never by "does not exist" alone: Jira says
/// exactly that about a field the account cannot read ("Field 'customfield_10102' does not exist or you
/// do not have permission to view it"), and reading that as a missing issue reports every key of a pull
/// as deleted and archives the whole board while calling the sync a success.
pub fn classify_failure(status: i32, stdout: &str, stderr: &str) -> AcliFailure;

// adf.rs — pure, unit-tested both directions
pub fn adf_to_markdown(doc: &serde_json::Value) -> String;
pub fn markdown_to_adf(text: &str) -> serde_json::Value;
pub fn adf_plain_text(doc: &serde_json::Value) -> String;
// Node coverage: doc, paragraph, heading 1–6, bulletList/orderedList/listItem (nested), codeBlock(language), blockquote,
// rule, hardBreak, text + marks strong/em/code/link/strike/underline, mention (@name), inlineCard/embedCard (url),
// emoji (shortName), panel (→ blockquote), table (→ pipe table, best effort), mediaSingle/media (→ "[attachment]").
// Unknown nodes: recurse into `content`, else drop. markdown_to_adf covers what `parse_markdown` in fleet-ui-kit covers
// (headings, paragraphs, lists, fenced code, inline code, bold, links) — plain text otherwise. Round-trip tests.
// Markdown has no underline, so `underline` renders as `_text_` and `strike` as `~~text~~`. A `link` whose text is
// its href renders bare; `[text](url)` renders *and* re-parses as a link mark, marks inside the label included —
// a scanner that could not read its own output turned every labelled link of a pulled description into literal
// brackets around a link node whose text was the bare href, from an edit to some other line. A bracket pair that
// is not a link (no `](`, a href carrying whitespace, an empty half) stays the text it was. Ordered lists carry `attrs.order` and are renumbered sequentially from it (ADF numbers items itself).
// Round-trip identity is for *canonical* markdown (blank-line-separated blocks); soft-wrapped paragraph lines join
// with a space. `adf_plain_text` collapses whitespace and joins blocks with one space (it feeds single-line fields).
// `adf_to_markdown` accepts a bare block node as well as a `doc` — comment bodies arrive both ways.

// map.rs — Jira JSON ⇄ core types (pure)
pub const VIEW_FIELDS: &[&str] = &["summary","description","status","priority","labels","assignee","duedate","parent","updated","created","comment","issuetype"];
pub fn issue_to_remote_card(issue: &serde_json::Value, settings: &JiraSettings) -> Result<RemoteCard, BoardError>;
//   key; url = settings.browse_url(key); version = fields.updated; updated_at = fields.updated → RFC3339 (chrono, keep offset);
//   an `updated` that is present and not a timestamp fails the key (named), because it is stored as the link's
//   `version`/`remoteUpdatedAt` and published by `board show --json`, where the contract says RFC3339;
//   title = summary; description = adf_to_markdown(description) ("" when null); status = RemoteStatus { id: status.name, name: status.name, category: status_category(statusCategory.key, name) };
//   priority: Highest→Urgent, High→High, Medium→Medium, Low→Low, Lowest→Low, null→None; labels; assignee = assignee.displayName;
//   estimate = round(fields[story_points_field]) when set; due_date = duedate; parent_key = parent.key;
//   properties: "jira.issue_type" = Select(issuetype.name), "jira.created" = Date(created[..10]), each extra_field → its kind (Text via adf_plain_text when the value is an ADF object; Select via .value/.name; Number; Date);
//   comments = comment.comments[] → RemoteComment { id, author: author.displayName, body: adf_to_markdown(body), created_at: RFC3339 }.
pub fn status_category(key: &str, name: &str) -> StatusCategory;  // "new" → Backlog if name matches /backlog/i else Unstarted; "indeterminate" → Started; "done" → Canceled if name matches /cancel|descart|won'?t|rechaz/i else Completed
pub fn priority_to_jira(p: Priority) -> &'static str;             // inverse mapping (Urgent→Highest, …, None→Medium)
pub fn build_search_jql(settings: &JiraSettings, since_minutes: Option<u32>) -> String;  // `project = "X" AND (jql) AND updated >= "-Nm" ORDER BY updated ASC`
pub fn parse_cursor(cursor: Option<&str>) -> Cursor; pub fn render_cursor(c: &Cursor) -> String;  // Cursor { watermark: DateTime<Utc>, pulls_since_full: u32 } as "<rfc3339>|<n>"
//   `parse_cursor(None | unreadable)` yields `pulls_since_full = u32::MAX`, so an unresumable cursor forces a
//   full pull instead of an incremental one reaching back to the epoch.
//   Property keys: every backend property is `jira.<id>` — `jira.issue_type`, `jira.created`, and one per extra field.

// users.rs — displayName → accountId/email cache, in-memory per daemon, refilled by every pull
// and by `describe`'s sample, which runs on **every** sync — an explicit column list skips the status
// half of it, never the people half, or a restarted daemon could not address anyone whose issues fall
// outside the first incremental window, and one such push failure fails the whole sync.
pub struct UserCache { inner: RwLock<HashMap<String /* site or project */, HashMap<String, JiraUser>>> }
// `users::scope` is the site when the settings name one and the project key otherwise, so every caller must
// compute it from the **located** settings (`site` defaulted to the signed-in one). `describe` computing it from
// the raw ones filed the whole sample under a scope no push ever read on any board created without `site`.
pub struct JiraUser { pub account_id: String, pub email: Option<String>, pub display_name: String }
impl UserCache { pub fn remember(&self, scope: &str, user: JiraUser); pub fn resolve(&self, scope: &str, display_name: &str) -> Option<JiraUser>; }

// mod.rs
pub struct JiraBackend { shell: Arc<dyn Shell>, clock: Arc<dyn Clock>, users: Arc<UserCache> }
impl JiraBackend { pub fn new(shell: Arc<dyn Shell>, clock: Arc<dyn Clock>) -> Self; }
#[async_trait] impl BoardBackend for JiraBackend {
    fn kind() = "jira"; fn label() = "Jira (acli)";
    fn capabilities() = { pull: true, push_updates: true, push_create: true, transitions: true, comments: true, custom_properties: true, incremental: true }
    async fn validate(settings): JiraSettings::parse; `acli --version` (Backend("acli not found: install …") when status 127); auth_status().authenticated; site match when both known.
    async fn describe(board): statuses = settings.statuses (category guessed by name) — an explicit column
       list costs no status sample, but still costs the people one (`--fields labels,assignee`), because the
       account map is the only thing a push can address an assignee with; a failing sample is a `tracing::warn`
       — else sampled: search `base_jql ORDER BY updated DESC --fields status,labels,assignee --limit sample_limit --json` → unique statuses ordered by (category, first-seen), plus labels and assignee display names from the sample (and remember users); properties = [jira.issue_type (Select, options from `project view --key X --json` issueTypes, editable false, source Backend), jira.created (Date, readonly, Backend), extra_fields…]; assignees; key_prefix = project; readonly_fields = `jira::READONLY_FIELDS` = ["priority","estimate","due_date","parent_id"] (public, so any second `adopt_schema` can re-state them). A failing `project view` is a `tracing::warn` and empty issue-type options, not a failed sync: the options are labels, not data.
    async fn pull(board, cursor): Cursor c = parse_cursor; full = cursor.is_none() || c.pulls_since_full + 1 >= full_sync_every (the *incremental* pulls since the last full one, so `fullSyncEvery = N` is every Nth pull and not every N+1st); keys = search (full: base_jql; incremental: since = now - watermark + overlap) `--fields summary --paginate --json`, read with `json_all` and deduplicated, so a CLI answering one document per page loses no key → .[].key; for each key (bounded by max_concurrency): view `--fields VIEW_FIELDS+story_points+extra --json` → issue_to_remote_card (remember users); a key that 404s between search and view is skipped and reported in `deleted_keys` — but a pull whose
       *every* key came back missing (2 or more) is a failure, not a board-wide deletion, and a watermark in the
       future forces `full` instead of collapsing the window to the overlap; any other view failure names its key
       in `failed_keys` and keeps the rest of the pull — `reconcile` neither updates nor archives an unread key,
       so nothing is archived on a permission error, and one throttled issue no longer discards a whole pull. A
       pull with any failed key keeps the *watermark* it was given, so the next one offers the same window again,
       while `pulls_since_full` still advances — rewinding both froze the counter, so one permanently unreadable
       key meant no full pull ever came due again and deletions stopped being noticed. A cursor that was absent
       or unreadable has no watermark to keep, and answering with the invented epoch one would send an unbounded
       window: it stays `None`. One whose every key failed (nothing read, nothing deleted) is still a failure.
       PullResult { cards, deleted_keys, failed_keys, cursor: failed_keys.is_empty()
       ? render_cursor(watermark = pull start time, pulls_since_full = full ? 0 : n+1)
       : (resumable ? render_cursor(watermark = previous, pulls_since_full = full ? 0 : n+1) : None), full }.
       The *service* rewinds the whole stored cursor only for a card **it** could not import; a key the backend
       reported in `failed_keys` keeps the cursor the backend chose.
    async fn push(board, cards, ops): per op, sequentially (writes are never parallel):
       Update { fields }: view current → diff labels (add/remove), then `workitem edit --key K [--summary] [--description-file <tmp ADF>] [--labels a,b] [--remove-labels c] [--assignee <accountId|email>|--remove-assignee] --yes --json`; unsupported fields in `fields` are skipped and logged via tracing::warn; then view again → ack { key, url, version: updated }.
       Transition { remote_status }: `workitem transition --key K --status "<remote_status>" --yes --json`; failure text → PushFailure with the Jira message ("no allowed transitions…") — except when a re-read of `view --fields status` shows the issue is already in the requested status, which is what a move Jira applied and then failed to answer for (a 504, a killed call, the retry of either) looks like: that is an applied op, not a failure that abandons the rest of the card's batch. A transition that follows a `Create` for the same card in this batch first reads `view --fields status`: a create carries no status, so `plan_push` always queues the column the card was born in, and Jira has already filed the issue somewhere — transitioning to the status it is in would fail a push with nothing left to do.
       Create: `workitem create --from-json <tmp>` with { projectKey, type: issue_type (Backend error if missing), summary, description ADF, labels, assignee, parentIssueId (parent's remote key, **or** the key this same batch just minted for it — `cards` is the pre-push snapshot, so a parent created in this batch still reads as unlinked there; groups are ordered parents-first) } → key from output → view → ack.
       AddComment { comment_id }: `workitem comment create --key K --body-file <tmp ADF> --json` → remote comment id (from output; if absent, `view --fields comment` and match the newest by body) → ack.comment_ids.
       Temp files under std::env::temp_dir(), removed after use. Assignee that cannot be resolved via UserCache (display name, email, or account id typed by the user) → PushFailure "unknown assignee…".
       Transitions and comments produce a `PushAck` too — without one a transitioned card stays `dirty` forever —
       and a card's ops are grouped so a single closing `view --fields updated` acks all of them instead of N.
       A batch that fails halfway still acknowledges the prefix that landed, *alongside* the `PushFailure`
       (`sync.rs` already expects both for one card): dropping the ack would post the comment that succeeded a
       second time on the next sync and leave `link.version` behind the edit that did apply, turning the user's
       own change into a conflict. A batch where nothing landed acks nothing.
       An assignee is forwarded only when it is an email or an Atlassian account id; any other unresolved word is
       the documented `PushFailure`, because Jira either refuses the edit or drops it silently.
}
```

## 6. CLI and app

CLI additions (`fleet board …`):
```
fleet board backends                                   # descriptors: kind, label, capabilities, settings keys
fleet board describe [--board|--context]               # BackendSchema: statuses (with category), labels, properties, readonly fields, assignees
fleet board set --backend jira --setting project=SP --setting jql='sprint in openSprints()' [--setting k=v ...]
      # --setting is repeatable; values parsed as JSON when valid ("10", "true", '["a","b"]'), else strings; merged with ops::merge_settings.
      # Naming --backend at all starts from empty settings; --setting alone keeps the kind and merges into the
      # stored object (a `null` value removes a key). The reset keys off the *presence of the flag*, not off an
      # actual kind change: a flag whose meaning depends on the board's current value is not predictable.
fleet board create --backend jira --setting project=SP ...    # --setting requires --backend (clap-enforced)
fleet board sync [--wait] [--full]
fleet board card edit … on a read-only field → exit 1 with the daemon's message
```
`board show`'s human header prints the descriptor **label**, not the raw kind:
`backend: Jira (acli) · project SP · synced 3m ago · 2 dirty · 1 conflict` (`site` after `project` when set,
raw kind when `ListBoardBackends` fails or lists no such kind — a failed lookup never fails the command).
The two settings named there are the **first two of the descriptor's own `settings_schema`** that the board has
set, never a key list in the printer: `project` and `site` are simply Jira's first two rows, and a second
backend's identity settings reach the header without a client change (BOARD §10). Without a descriptor the
header names the kind and stops, rather than guessing which keys identify a board.
A local board prints `backend: Local` alone: no sync tail for a board with no remote. `--json` carries the board
verbatim and never issues `ListBoardBackends`. Envelopes: `{"protocol":1,"backends":[BackendDescriptor…]}` and
`{"protocol":1,"schema":BackendSchema}`. `fleet board backends` resolves no board.

App (generic, no Jira strings in `fleet-app`):
- Board settings dialog: a **Backend** row cycling over `ListBoardBackends` kinds (h/l), then one row per
  `settings_schema` entry (Text → inline text field, Bool → toggle, Number → number field, Select → cycler,
  MultiSelect → comma-separated text); Save sends `BoardPatch { backend: Some(BackendRef { kind, settings }) }`
  and shows the daemon's validation message on failure (dialog stays open). Changing kind on a linked board shows the
  daemon's error verbatim.
  A setting whose schema `name` ends in `(required)` is starred and refuses to save empty; the marker is stripped
  from the row label. Number rows accept digits only (so `h`/`l` keep stepping them), `Select` rows cycle, and the
  row list scrolls — a nine-row backend form is taller than the card ever gets.
- Read-only fields: pickers for a field in `board.sync.readonly_fields` do not open; the refusal reads "<field> is
  read-only on <backend label> boards"; detail rows render in the secondary tone with a lock glyph. On the **board**
  the refusal is a toast; inside the **card detail** it is the dialog's own error line — `shell/root.rs` puts the
  toast stack in `body_overlay`, and a dialog's scrim would make a toast there unreadable. Same sentence either way.
- Card detail key `x` (context `Dialog > CardDetail`): open the remote issue URL in the browser (`cx.open_url`);
  Board key `x` does the same for the focused card. Both in `docs/KEYMAP.md`, palette, help. A card with no remote
  link says so, with the same toast/error-line split.
- Header shows backend label (from descriptors) instead of the raw kind. Descriptors are fetched once per
  connection on **board render** (the header needs the label too), not on dialog open; a dialog already seeded
  adopts a late schema.
- Full sync: the palette gets "Board: Full sync" issuing `SyncBoard { full: true }`. The palette's
  `every_command_has_a_label_and_a_bound_key` invariant refuses a keyless command, so it is backed by a real
  action `board::FullSync` bound to `F` in `Hub > Board` (in `docs/KEYMAP.md`) rather than being CLI-only.

## 7. `acli` command reference (verified 1.3.18)

```
acli jira auth status
acli jira workitem search --jql "<jql>" --fields summary,status,labels,assignee --limit 200 --json
acli jira workitem search --jql "<jql>" --fields summary --paginate --json
acli jira workitem view <KEY> --fields summary,description,status,priority,labels,assignee,duedate,parent,updated,created,comment,issuetype[,customfield_10102] --json
acli jira project view --key <P> --json                      # issueTypes[]
acli jira workitem edit --key <KEY> [--summary S] [--description-file f.json] [--labels a,b] [--remove-labels c] [--assignee A | --remove-assignee] --yes --json
acli jira workitem transition --key <KEY> --status "<Status Name>" --yes --json
acli jira workitem create --from-json f.json --json           # {"projectKey","type","summary","description":{ADF},"labels":[],"assignee","parentIssueId"}
acli jira workitem comment create --key <KEY> --body-file c.json --json
```
Failure texts to recognize: `✗ Error: …`, `can't be transitioned: No allowed transitions found`, `not authenticated`,
`unauthorized`, `429`/`rate`, `usage limit`, `Issue does not exist`. Always drive writes by explicit `--key`, never `--jql`.

## 8. Tests

- Unit: settings parse/defaults/deny-unknown; `build_search_jql`; cursor round-trip; `status_category`; priority maps;
  `issue_to_remote_card` on a realistic fixture (`tests/fixtures/jira/issue.json`, ADF description, comments, parent,
  story points); ADF⇄markdown (≥ 25 cases incl. nested lists, code blocks, links, mentions, unknown nodes, round-trip).
- Backend with `FakeShell`: describe (sampled statuses ordered by category), full pull (search + N views, concurrency
  ≤ max), incremental pull (JQL contains `updated >= "-Nm"`, cursor advanced, `pulls_since_full` counts, full every N),
  view 404 → deleted_keys, push Update (label diff → correct add/remove flags; unsupported field skipped), Transition
  failure → PushFailure, Create → key + url + version, AddComment; auth failure → clear error; rate limit → retried.
- Service integration (`tests/boards_jira.rs`): a board with `backend: jira` + `FakeShell` scripted acli → `sync`
  creates cards in mapped columns, a second sync with a changed remote and a dirty local card yields a conflict, a
  local title edit is pushed via `edit`, a status move via `transition`, a comment via `comment create`; editing
  priority on the Jira board is rejected with `ReadOnlyField`; `sync(full=true)` clears the cursor.
- CLI tests for the new flags/commands; app unit tests for the generic settings rows and readonly picker guard.
- The retry policy's *exhaustion* path (3 steps, 13 s of real sleep) is exercised with a single 1 s step: skipping
  the sleep would need `tokio`'s `test-util` feature, and this milestone adds no dependencies.
- Real-Jira smoke (read-only, **not part of `make ci`**; run by hand against a live site): throwaway FLEET_HOME, `fleet board set --backend jira --setting project=SP
  --setting jql="assignee = currentUser()"`, `fleet board sync --wait`, `fleet board show`; screenshots of the app.
  **No push/create/edit/transition/comment against the real site, ever.**
