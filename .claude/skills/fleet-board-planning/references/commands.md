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

## Card verbs

Every `<key>` accepts a display key (`FLT-12`, `PROJ-123`), a card id, or the local key of a
card with no remote link; matching is case-insensitive; an ambiguous match is refused with
`use its ID`. Every mutating verb prints the resulting card (or the JSON `card` envelope).

| Command | Notes |
| --- | --- |
| `card new <title> [fields]` | No `--status` means the first unstarted column. `--clear-*` flags are refused here. |
| `card show <key>` | Identity line, one `Field: value` line each (Status, Priority, Labels, Assignee, Estimate, Due, Parent, Repo, Worktree, Archived, Dirty, Created, Updated, Local key when unlinked, Remote/URL/Synced when linked, Conflict when present, backend properties), then `Description` and `Comments` blocks. |
| `card edit <key> [--title T] [fields] [--clear-*] [--archive [true\|false]]` | Omitted fields are unchanged. Refuses an empty patch. `--archive false` restores. |
| `card move <key> <status> [--index N]` | Status by id or name (case-insensitive). `--index` positions inside the column; default is the end. |
| `card comment <key> <body>` | Appends a comment. Author is empty for local comments. |
| `card delete <key>` | Deletes. Prints `Deleted <key>`. Prefer `edit --archive` or `move … canceled`. |
| `card worktree <key> [--repo owner/name] [--base REF] [--host H]` | Creates or adopts the worktree, links it on the card, applies `start_on_worktree`. Prints `Created <id>` or `Existing <id>`. |
| `card resolve <key> keep-local\|take-remote` | Resolves a sync conflict. |

### Card field flags (`new` and `edit`)

| Flag | Value |
| --- | --- |
| `--desc` | Markdown description. |
| `--status` | Status id or name. |
| `--priority` | `urgent`, `high`, `medium`, `low`, `none`. |
| `--label` (repeatable) | Label id or name; must exist on the board. |
| `--assignee` | Free text. |
| `--estimate` | Points, unsigned integer. |
| `--due` | `YYYY-MM-DD`; validated client-side. |
| `--repo` | `owner/name`. |
| `--clear-labels`, `--clear-assignee`, `--clear-estimate`, `--clear-due`, `--clear-repo` | `edit` only; each conflicts with its value flag. |

There is no `--parent` flag. Express sequencing in titles, priorities, or a label.

## JSON envelopes

All carry `"protocol": 1`. Fields are camelCase.

| Command | Shape |
| --- | --- |
| `show`, `create`, `set` | `{ protocol, board: {…}, cards: [{…}] }` — whole cards, including `description`, `comments`, `activity`. |
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
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Any error: bad flags, unknown key, ambiguous key, daemon refusal, failed `sync --wait`. |

## Finding ids

```sh
fleet agent list          # thread  provider  host  session  attention  WORKTREE  title
fleet list                # WORKTREE  HOST  SESSION
fleet list --json | jq -r --arg p "$PWD" '.worktrees[] | select(.path==$p) | .id'
fleet board list          # board ids and their scope
```
