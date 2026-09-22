# `fleet board` command reference

Source of truth: `crates/fleet-cli/src/args.rs` (`BoardArgs`, `BoardCommand`, `BoardSetArgs`,
`BoardCardCommand`, `BoardCardFields`) and `docs/BOARD.md` §6. When this file and the code
disagree, the code is right and this file needs the fix.

## Global flags (any position, mutually exclusive selectors)

| Flag | Meaning |
| --- | --- |
| `--board <id>` | An existing board by id. Never creates. |
| `--worktree` | The worktree owning `FLEET_SESSION` — the worktree session in a terminal, or the native agent thread with that id. Creates the board on first use. |
| `--worktree=<owner/name#slug>` | A named worktree. The `=` is mandatory so a subcommand is never eaten as the value. Creates on first use. |
| `--context <id>` | A named context. Creates on first use. |
| *(none)* | The daemon's active context. Creates on first use. Errors with `no active context` if there is none. |
| `--json` | Protocol-1 JSON envelope on stdout; errors as a JSON envelope too. |

`list` refuses `--worktree`. `create` refuses `--board`.

## Board verbs

| Command | What it does | Prints |
| --- | --- | --- |
| `fleet board show` | Columns and their cards. | Header, then `<Column> (n)` sections with `KEY  priority  title  [labels]  @assignee` rows, then `No column (n)` for orphaned cards. |
| `fleet board list [--board ID] [--context ID]` | Every board the daemon knows, or one. | Table: id, context, worktree, name, backend, cards, open, dirty, conflicts, and a trailing last-error cell when one exists. |
| `fleet board backends` | Registered backend kinds, their capabilities, their setting keys. | Table. Resolves no board. |
| `fleet board describe` | What the selected board's backend reports: statuses, labels, properties, read-only fields. | Table. On a Jira board this is where the real column names come from. |
| `fleet board create [--name N] [--prefix P] [--backend K] [--setting k=v]...` | Create a context or worktree board explicitly. Name defaults to the context name or worktree slug. `--setting` requires `--backend`. | The new board as `show` prints it. |
| `fleet board set …` | Update board properties (below). Refuses an empty patch. | The board as `show` prints it. |
| `fleet board sync [--wait] [--full]` | Start a sync job. `--wait` blocks until it ends. `--full` ignores the incremental cursor. | `Sync started <job>`; with `--wait`: `Synced <board> (<job>)`, progress, counts, and `Last error:` when the backend skipped cards. |

### `fleet board set` flags

| Flag | Effect |
| --- | --- |
| `--name N`, `--prefix P` | Rename; prefix is uppercase, 1 to 8 characters. Changing the prefix changes every local display key. |
| `--default-repo owner/name`, `--clear-default-repo` | The repo `card worktree` uses when neither the flag nor the card names one. |
| `--start-on-worktree [true\|false]` | Move an unstarted card to the first started column when a worktree is created from it. Default true. |
| `--branch-template "{key}-{slug}"` | Branch and worktree slug for `card worktree`. |
| `--conflict-policy manual\|remote_wins\|local_wins` | How sync resolves a card both sides changed. Default manual. |
| `--push-new-cards [true\|false]` | Whether local-only cards become remote issues on push. Default false. |
| `--add-label NAME` (repeatable), `--remove-label ID-or-NAME` (repeatable) | Edit the board's label set. Adding a name that exists case-insensitively is a no-op. |
| `--backend KIND [--setting k=v]...` | Switch backend; settings start from empty, so give every setting the new kind needs. |
| `--setting k=v` alone | Merge into the current backend's settings. A JSON-valid value is parsed as JSON; `null` removes a key. |
| `--max-live-runs N` | How many card runs one board may have live at once. Unset means 1, because every run of one board edits the same checkout. |

## Column verbs

Each verb reads the whole column vector, edits it, and sends one write, so a concurrent editor
loses — as `board set` already behaves. With `--json` every one of them prints the board
envelope; `board.statuses` *are* the columns.

| Command | Notes |
| --- | --- |
| `fleet board columns` | One row per column: `ID  NAME  CATEGORY  ON ENTER  ON SUCCESS  WHEN UNBLOCKED`, `—` where there is nothing. Every automation cell is written the way the flag that sets it accepts it, so a row can be typed back into `columns edit`. |
| `columns add <name> [--id ID] [--category K] [--after C\|--before C]` | `--id` defaults to a slug of the name; category defaults to `unstarted`; without a position the column goes last. |
| `columns edit <id\|name> [flags below]` | The only way to give a column an action or a route. |
| `columns move <id\|name> --after C\|--before C` | One of the two is required. A column cannot move relative to itself. |
| `columns remove <id\|name> [--move-cards-to C]` | The daemon refuses while a card still stands there; `--move-cards-to` moves them first, archived cards included. |
| `columns preset workflow` | Adds the columns the workflow preset names and never rewrites one that exists. |

### `columns edit` flags

| Flag | Effect |
| --- | --- |
| `--name N`, `--category K`, `--color C` | The column's own presentation. |
| `--on-enter none\|prompt\|skill:<name>[:<args>]` | What entering the column starts. `prompt` runs the card's own brief; `skill:` invokes that skill — and always on Claude, whatever the card asks for. `none` clears the action. |
| `--instructions T` / `--instructions-file F` | Markdown prepended to every brief this column starts. `{key}` and `{title}` are substituted. |
| `--expect T` | Completion criteria, printed in the brief's footer as `The card expects: …`. |
| `--provider`, `--model`, `--effort`, `--mode` | What the column's runs launch with. A card's own `--provider/--model/--effort` wins; `--mode` exists only here, because it is the column's policy over every card passing through. |
| `--on-success C` / `--no-on-success` | Where a card goes when its run here succeeds. |
| `--when-unblocked C` / `--no-when-unblocked` | Where a card *waiting* here goes once every card blocking it is done. |
| `--env KEY=VALUE` (repeatable) / `--clear-env` | Environment for the column's runs. `FLEET_*` and `PATH` are refused. |

The **workflow preset** is `Backlog → Todo → Ready → In Progress → In review → Done` plus
`Canceled`: `ready` routes `when-unblocked` to `in-progress`, `in-progress` runs a prompt and
succeeds into `in-review`, `in-review` runs the `deep-review` skill and succeeds into `done`.
Todo stays human on purpose. On a board that already has its own `in-progress`, the preset
leaves that column alone — give it its action with `columns edit in-progress --on-enter prompt
--on-success in-review`.

## Card verbs

Every `<key>` accepts a display key (`FLT-12`, `PROJ-123`), a card id, or the local key of a
card with no remote link; matching is case-insensitive; an ambiguous match is refused with
`use its ID`. Every mutating verb prints the resulting card (or the JSON `card` envelope).

| Command | Notes |
| --- | --- |
| `card new <title> [fields] [--blocked-by KEY]... [--blocks KEY]...` | No `--status` means the first unstarted column. `--clear-*` flags are refused here. Both link lists are resolved before the card is created, so a typo costs nothing. |
| `card show <key>` | Identity line, one `Field: value` line each (Status, Priority, Labels, Assignee, Estimate, Due, Parent, Repo, Worktree, Archived, Dirty, Created, Updated, Local key when unlinked, Remote/URL/Synced when linked, Conflict when present, backend properties), then `Description` and `Comments` blocks. |
| `card edit <key> [--title T] [fields] [--clear-*] [--archive [true\|false]] [link flags]` | Omitted fields are unchanged. Refuses an empty patch. `--archive false` restores. Link flags: `--add-blocked-by KEY`, `--remove-blocked-by KEY`, `--clear-blocked-by`, `--add-blocks KEY`, `--remove-blocks KEY` (all repeatable except the clear). `--add-blocks` edits the *named* card, so it is never an empty patch. |
| `card move <key> <status> [--index N] [--cancel-run]` | Status by id or name (case-insensitive). `--index` positions inside the column; default is the end. A card with a live run is refused unless `--cancel-run` says to stop it. |
| `card run <key>` | Starts a run for a card standing in a column with an action. Prints `run <id> started, thread <thread>` then the card, or `run pending; it starts when a run slot frees` when the board is at its ceiling. |
| `card cancel <key>` | Cancels the card's live run, or drops the slot a card is waiting for when it has only been *owed* one. Refuses a card with neither: `{KEY} has no live run`. Prints the card. |
| `card runs <key>` | One tab-separated line per run, newest last: run id, column, outcome, provider, model, effort, duration, files changed, cost, thread. `—` for what is not known yet. |
| `card attach <key>` | Prints the thread id alone — the live run's, else the newest run's — so `fleet agent tail` can follow it. |
| `card wait <key> [--timeout 540]` | Waits for the card's *newest* run. Exits 0 when it is terminal, 2 when it is still live, when no run started, or when the card is still owed one behind the board's live-run ceiling. It does not wait for a run that has not begun, so it is not a barrier for a whole chain. |
| `card comment <key> <body>` | Appends a comment. Author is empty for local comments. A run's report arrives as a comment carrying its `runId`. |
| `card delete <key>` | Deletes. Prints `Deleted <key>`. Prefer `edit --archive` or `move … canceled`. |
| `card worktree <key> [--repo owner/name] [--base REF] [--host H]` | Creates or adopts the worktree, links it on the card, applies `start_on_worktree`. Prints `Created <id>` or `Existing <id>`. |
| `card resolve <key> keep-local\|take-remote` | Resolves a sync conflict. |

### Card field flags (`new` and `edit`)

| Flag | Value |
| --- | --- |
| `--desc`, `--desc-file F` | Markdown description, inline or read from a file. Mutually exclusive. |
| `--status` | Status id or name. |
| `--priority` | `urgent`, `high`, `medium`, `low`, `none`. |
| `--label` (repeatable) | Label id or name; must exist on the board. |
| `--assignee` | Free text. |
| `--estimate` | Points, unsigned integer. |
| `--due` | `YYYY-MM-DD`; validated client-side. |
| `--repo` | `owner/name`. |
| `--provider claude\|codex`, `--model M`, `--effort E` | What *this card's* runs launch with; each wins over the column's. `--model` and `--effort` merge onto the card's existing block rather than replacing it. |
| `--clear-labels`, `--clear-assignee`, `--clear-estimate`, `--clear-due`, `--clear-repo`, `--clear-agent` | `edit` only; each conflicts with its value flag. `--clear-agent` drops the card's preferences and leaves the column's. |

There is no `--parent` flag. Express sequencing with `--blocked-by`/`--blocks`, which the
automation engine also reads, or in titles, priorities, or a label.

## JSON envelopes

All carry `"protocol": 1`. Fields are camelCase.

| Command | Shape |
| --- | --- |
| `show`, `create`, `set`, every `columns` verb | `{ protocol, board: {…}, cards: [{…}], liveRuns?: [{…}] }` — whole cards, including `description`, `comments`, `activity`, `blockedBy`, `pendingRun` and `runs`. `liveRuns` joins the delegation behind each live run and is omitted when nothing is live; it is never persisted. |
| `list` | `{ protocol, boards: [{ id, contextId, worktreeId?, name, backendKind, cardCount, openCount, dirtyCount, conflictCount, lastError? }] }` |
| `card new/show/edit/move/comment/resolve` | `{ protocol, card: {…} }` |
| `card delete` | `{ protocol, ok: true }` |
| `card worktree` | `{ protocol, created, card, worktree }` |
| `sync` | `{ protocol, jobId, job?, summary? }` — `job` and `summary` only with `--wait`. |
| `backends` | `{ protocol, backends: [...] }` |
| `describe` | `{ protocol, schema: {…} }` |
| error | `{ protocol, error: { kind, message } }` on stdout, exit 1. |

Useful filters:

```sh
fleet board show --json | jq -r '.cards[] | select(.archived|not) | "\(.statusId)\t\(.title)"'
fleet board show --json | jq -r '.board.statuses[] | "\(.id)\t\(.category)"'
fleet board card show FLT-3 --json | jq -r '.card.worktreeId // empty'
fleet board show --json | jq -r '.cards[] | select(.statusId!="done") | "\(.title)\t\(.statusId)"'
fleet board show --json | jq -r '.liveRuns[]? | "\(.cardId)\t\(.status)"'
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. For `card wait`: the card's newest run is terminal. |
| 1 | Any error: bad flags, unknown key, ambiguous key, daemon refusal, failed `sync --wait`. |
| 2 | `card wait` only: the newest run is still live, no run started before the timeout, or the card is owed one it has not been given yet. Nothing is wrong; wait again. |

## Finding ids

```sh
fleet agent list          # thread  provider  host  session  attention  WORKTREE  title
fleet list                # WORKTREE  HOST  SESSION
fleet list --json | jq -r --arg p "$PWD" '.worktrees[] | select(.path==$p) | .id'
fleet board list          # board ids and their scope
```
