# Phase 3 — The gesture in dialog text inputs — Tracker

> Plan: ./paste-drop-media-2026-09-10-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.

Repo-specific: load the skill `CLAUDE.md` names for the area; commit as `ui-kit: …` / `app: …` / `docs: …`; `fleet-ui-kit` depends only on `gpui`.

## Kickoff
- [x] Phase 1 and phase 2 have shipped (`media::stage`, `MediaAnchor::Local`, `TextInput::accepts_media` + `TextInputMedia`).
- [x] I have read the plan end to end.
- [x] Green baseline: `make lint`, `make test`, `make check`, `make harness`. — verified by the orchestrator on the committed phase-2 tree (2026-09-26): fmt + clippy clean, `make test` 4365 green, `make harness` 104/104

## Tasks
- [x] P3-T01 — Stop pasting concatenated paths — verified: `RUSTC_WRAPPER="" cargo test -p fleet-ui-kit` (415 passed; 5 doc tests, 3 ignored)
- [x] P3-T02 — Inventory the dialog fields, and wire the ones where a path belongs — verified: 48 media-focused app tests passed; the exact full app suite passed 1,251 tests and reached only 14 sandbox-denied Unix-socket failures
- [x] P3-T03 — Move the docs with the code — verified: re-read `DESIGN-SYSTEM.md` TextInput and `UX-SPEC.md` dialogs against the implementation

## Dialog fields (P3-T02)

Fill this in completely, including the deliberate noes.

| File | Field | Wired? | Quoting | Reason |
| --- | --- | --- | --- | --- |
| `dialogs/board_settings/keys.rs` | General: board name, prefix | No | — | Board identity text, not a filesystem path. |
| `dialogs/board_settings/keys.rs` | Backend setting values | No | — | Provider configuration scalars, not local filesystem paths. |
| `dialogs/board_settings/keys.rs` | Column: name, on-enter action, model, effort, instructions, expectation, env | No | — | Structured board automation or prose; none is a local path field or a shell command line. |
| `dialogs/board_settings/keys.rs` | Schedule: name, model, effort, cadence values, prompt | No | — | Schedule configuration and prose, not local filesystem paths. |
| `dialogs/board_settings/view.rs` | General board-settings editor consumer | No | — | Renders the row-scoped editor built and classified in `keys.rs`; owns no additional input. |
| `dialogs/board_settings/view/column_pane.rs` | Column editor consumer | No | — | Receives the row-scoped editor from the shell and passes it to the shared row renderer; owns no input. |
| `dialogs/board_settings/view/row.rs` | Shared board-settings `ValueBox` editor consumer | No | — | Draws a caller-owned editor inside the value box; construction and field meaning remain in `keys.rs`. |
| `dialogs/board_settings/schedules/view.rs` | Schedule editor consumer | No | — | Renders the same row-scoped editor built and classified in `keys.rs`; owns no additional input. |
| `dialogs/card_create.rs` | New-card title | No | — | Card title text. |
| `dialogs/card_create.rs` | New-card description | No | — | Markdown prose, not a path field. |
| `dialogs/card_detail/draft.rs` | Card title | No | — | Card title text. |
| `dialogs/card_detail/draft.rs` | Card description | No | — | Markdown prose, not a path field. |
| `dialogs/card_detail/draft.rs` | New comment | No | — | Markdown prose, not a path field. |
| `dialogs/card_detail/view.rs` | Card-detail editor consumer | No | — | Renders the shared editor built in `draft.rs`; owns no additional input. |
| `dialogs/card_picker/lifecycle.rs` | Property query / custom value | No | — | Picker query or model/effort value, not a filesystem path. |
| `dialogs/clone_repo.rs` | Repository search | No | — | Search query / owner-name or URL lookup, not a local path field. |
| `dialogs/context.rs` | Context name | No | — | Context identity text. |
| `dialogs/context.rs` | GitHub owners | No | — | Comma-separated organization/user names. |
| `dialogs/create_worktree/branch.rs` | Branch / base-ref filter | No | — | Git ref and list filter; a filesystem path is invalid here. |
| `dialogs/edit_hooks.rs` | Prepare commands | Yes | Shell-quoted | The value is a shell command; local paths are valid command words. |
| `dialogs/edit_hooks.rs` | Post-create commands | Yes | Shell-quoted | The value is a shell command; local paths are valid command words. |
| `dialogs/filter.rs` | Hub filter consumer | No | — | Borrows the Hub query editor; filtering is inventoried at `screens/hub.rs`. |
| `dialogs/help.rs` | Help search | No | — | Search query. |
| `dialogs/host.rs` | Dialog input entity slots | No | — | Lifetime owner only; every semantic field stored here is classified in its constructing module. |
| `dialogs/palette.rs` | Command-palette query | No | — | Search / command selection query, not a command line executed by a shell. |
| `dialogs/rename_terminal.rs` | Terminal name | No | — | Display title, not a filesystem path. |
| `dialogs/settings/interaction.rs` | Settings search | No | — | Search query. |
| `dialogs/settings/draft.rs` | Claude/Codex terminal command | Yes | Shell-quoted | Fleet types the value into a terminal shell; inserted paths must be independent shell words. |
| `dialogs/settings/draft.rs` | Claude/Codex binary for threads | Yes | Bare | Fleet executes the binary directly without a shell; the field's meaning includes an executable path. |
| `dialogs/settings/draft.rs` | Claude/Codex default model | No | — | Provider model identifier, not a filesystem path. |
| `dialogs/settings/draft.rs` | Numeric settings | No | — | Digit-filtered durations and intervals. |
| `dialogs/settings/choice.rs` | Settings choices and custom model option | No | — | Defines model/effort choices; the custom model editor is built in `draft.rs` and is an identifier, not a path. |
| `dialogs/settings/schema.rs` | Settings row schema | No | — | Defines semantic row ids and kinds but creates no input; path classification is applied when `draft.rs` materializes an editor. |
| `dialogs/settings/view.rs` | Settings `SearchField` / `ValueBox` consumers | No | — | Renders caller-owned inputs built in `interaction.rs` and `draft.rs`; owns no additional input. |
| `fleet-ui-kit/components/search_field.rs` | Caller-owned search input | No | — | The component retains and renders the input supplied by the app; it does not decide field semantics or enable media. |
| `fleet-ui-kit/components/value_box.rs` | Caller-owned inline editor | No | — | The component optionally retains and renders an editor supplied by a settings shell; the app classifies that editor before passing it in. |
| `fleet-ui-kit/components/settings_row.rs` | Settings-row layout | No | — | Owns row chrome and controls but no text input. |
| `screens/agent_thread/mod.rs` | Native-agent composer | No | — | Phase 2 owns composer attachments; phase 3 must not rewire it as path text. |
| `screens/agent_thread/actions.rs` | Composer editor actions | No | — | Operates the composer owned in `mod.rs`; owns no additional input. |
| `screens/board.rs` | Board filter | No | — | Filter query. |
| `screens/hub.rs` | Hub/worktree filter | No | — | Filter query. |
| `views/board_screen.rs` | Board-filter consumer | No | — | Borrows `BoardScreen`'s filter input; owns no additional input. |
| `views/worktrees_list.rs` | Worktree-filter consumer | No | — | Borrows `HubScreen`'s filter input; owns no additional input. |

## Notes / decisions log
(Append-only. Date-stamp entries.)

- 2026-09-24 — Phase created from the dialog half of the original phase 2. `TextField` no longer exists (ADR 0020); every field is a `TextInput`, so there is no longer a paste/drop asymmetry between two input kinds to document.
- 2026-09-26 — Inventory found three path-bearing field kinds: hook commands and Settings terminal commands are shell-quoted; Settings thread binaries are bare. All stage locally with `force_copy = false`, preserve source order, and join several successful paths with one space.
- 2026-09-26 — The plan's baseline and definition still name `make harness`; this task explicitly delegates harness/restart to the orchestrator, so the required four direct Cargo/fmt commands are recorded here instead and the kickoff baseline stays unchecked.
- 2026-09-26 — Verification: `cargo test -p fleet-ui-kit` passed 415 unit and 2 doc tests (3 doc tests ignored); `cargo test -p fleet-app media` passed all 48 focused tests; workspace clippy with `-D warnings`, fmt-check, and `make lint` passed. The exact `cargo test -p fleet-app` and review-required `make test` each passed 1,251 tests before the sandbox denied Unix-domain socket binding in 13 bridge tests and one drive test (`Operation not permitted`); all new dialog-media tests passed. `zed-quality-review` found no diff finding; its gate remains environmentally incomplete for the same socket failures.
- 2026-09-27 — Rebased the inventory onto PR #53's one-shell dialogs. The new `SearchField`, `SettingsRow` and `ValueBox` components only render caller-owned inputs; board-settings row consumers still receive editors created in `keys.rs`; Settings path semantics still live at editor materialization in `draft.rs`.
- 2026-09-27 — Merge-resolution verification: ui-kit passed 435 unit tests plus 2 doc tests (3 ignored); the app dialog filter passed 312 tests; the media filter passed 48 tests, including all four dialog-media cases; workspace clippy and fmt-check passed. The review-only full `make test` reached 1,282 passing tests, then the sandbox denied Unix-socket binding in 13 bridge tests and the drive socket test (14 environmental failures, all outside this diff).
- 2026-09-27 — This agent's filesystem sandbox exposes `.git` read-only. The five resolved files contain no conflict markers, but `git add` cannot create `.git/index.lock`; the orchestrator must stage them to clear the index's `UU` entries.

## Follow-ups
- 2026-09-26 — Orchestrator gate on the integrated phase-3 tree: fmt + workspace clippy clean, `make test` 4371 green. `make harness` stopped at `scenarios/agents/thread-streaming.scenario:32`: the thread reached `idle` and the toast assert ran 88 ms later, before the "agent finished" toast was applied. Phase 3 touches no toast, thread or streaming code and the same corpus passed 104/104 one hour earlier on the phase-2 tree; the scenario asserts a toast right after a state `await` without awaiting the toast itself, so this is a pre-existing ordering race. Recorded as a follow-up; the scenario was rerun alone and the full corpus rerun to close the gate.

## Follow-ups (integration)
- `scenarios/agents/thread-streaming.scenario:32` should `await toasts[0].text ~= "agent finished"` instead of asserting it immediately after the `idle` await; the toast and the state change are separate events and can arrive in either order.
