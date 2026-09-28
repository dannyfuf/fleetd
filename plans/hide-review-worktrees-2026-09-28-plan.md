# Hide pull-request review worktrees from the Worktrees list — Plan
> Tracker: ./hide-review-worktrees-2026-09-28-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Every pull request the user reviews gets its own worktree: the Reviews board creates one per
review card, and the Pull requests screen's *open-or-create* does the same. Those worktrees land
in the Hub's Worktrees list next to the worktrees the user is actually working in, and with a
few reviews in flight the list stops answering its own question ("which branch is alive, which
needs attention, which can I throw away?"). This change hides review worktrees from the Worktrees
list by default, says how many are hidden in the page subtitle, and gives the user one key to
show them again. The Reviews board, the Workspace, the palette and the CLI keep seeing every
worktree; only the Hub's Worktrees list (and the counts that must agree with it) change.

## Sizing call

**Standard.** One pull request, one focused stretch (roughly two to four days). The work is
almost entirely in `fleet-app`, with a pure helper in `fleet-core`, a harness scenario and the
doc updates that ride with each commit. No wire change, no daemon change, no persisted-schema
change: a review worktree is already recognisable by its base ref. Phasing was considered and
rejected: there is no intermediate state worth shipping on its own.

## Repository context

- Rust Cargo workspace; toolchain pinned by `rust-toolchain.toml`. Crates under `crates/`:
  `fleet-app` (GPUI app), `fleet-core` (pure model), `fleet-daemon`, `fleet-proto`, `fleet-cli`,
  `fleet-harness` / `fleet-drive` (GUI harness), `fleet-ui-kit` (components and tokens).
- Lint: `make lint` (`cargo fmt --check` + `clippy --workspace --all-targets --all-features -D warnings`).
- Tests: `make test` (builds `fleetd` first, then `cargo test --workspace`).
- GUI verification: `make harness` (whole corpus, virtual lane), `make harness-headless`
  (scenarios without `shot`/`clipboard`), `make harness-one SCENARIO=scenarios/<dir>/<name>.scenario`.
- `docs/` is authoritative. A change to what a screen shows must update `docs/UX-SPEC.md` in the
  same commit; a key change updates `docs/KEYMAP.md`; a harness dump change updates
  `docs/TESTING-HARNESS.md` (frozen: read it before touching a target name or the row grammar).
- Project skills in `.claude/skills/` must be loaded before editing the matching area (table in
  `CLAUDE.md`). Load `zed-quality-review` before declaring any task done.
- Commit format: `<area>: <imperative lowercase summary>`; areas here are `core`, `app`, `tests`, `docs`.
- What already exists and is reused, not rebuilt:
  - A pull-request worktree is created by the daemon with `base_ref = "pull/<n>/head"`
    (`crates/fleet-daemon/src/services/worktrees/creation.rs`, `create_pr_local`). Both the
    Reviews board (`crates/fleet-daemon/src/services/boards/worktree.rs`,
    `ensure_pull_request_worktree`) and the app's PR screen (`crates/fleet-app/src/views/prs_screen.rs`,
    `crates/fleet-app/src/views/detail/pull_request.rs`) go through it.
  - `fleet_core::github::worktree_matches_pr` already treats `pull/<n>/head` as "this worktree is
    a pull request checkout" (`crates/fleet-core/src/github.rs`).
  - The Hub builds its model once per revision in
    `crates/fleet-app/src/screens/hub/projection.rs` (`model`, `ProjectionKey`, `HubModel`); the
    list's rows, filter and subtitle live in `crates/fleet-app/src/views/worktrees_list/model.rs`
    (`build_rows`, `matches`, `summary`) and are drawn by `crates/fleet-app/src/views/worktrees_list.rs`.
  - The rail's per-repository counts come from `repos_rail::rail_rows` over the same worktree
    slice; the context bar's `Worktrees` count comes from `hub_place` in
    `crates/fleet-app/src/shell/chrome/title.rs`.
  - Hub worktree actions live in `crates/fleet-app/src/actions.rs` (`actions!` block under
    `worktrees`), keys in `crates/fleet-app/src/keymap.rs`, labels in
    `crates/fleet-app/src/action_catalogue.rs`, the help overlay in `crates/fleet-app/src/dialogs/help/`.
  - The harness dumps the displayed worktree rows as `lists.worktrees` with `marks` (`degraded`,
    `remote`) in `crates/fleet-app/src/state/harness/projection.rs` (`worktree_rows`). Fixture
    presets are in `crates/fleet-harness/src/fixture/plan.rs`; the `busy` preset already carries
    fake pull requests served by the fake `gh` in `crates/fleet-harness/src/fixture/tools.rs`.
  - A precedent for "rows exist but are not listed": `HubModel.pr_hidden` and
    `prs_screen::hidden_rows`, drawn by `screens/hub/composition.rs`.

## Assumptions

- **A review worktree is a worktree whose `base_ref` is `pull/<n>/head`.** That is what the
  daemon writes for every worktree created from a pull request, whether the Reviews board or the
  PR screen asked for it. No new field is persisted and nothing crosses the wire. A worktree the
  user created from the PR's branch with an ordinary base (`origin/main`) is *not* a review
  worktree even if a review card later adopted it; that is the user's own branch and stays listed.
- **A worktree the user created by hand with a `pull/<n>/head` base** (the Create worktree
  dialog offers the previous base) is treated as a review worktree too. This is the accepted cost
  of using the base ref as the signal; it is a rare action and the toggle shows it again.
- **Hidden is the default on every launch.** The toggle is session state in `AppState`, not a
  persisted preference. The user's ask is about the default; persisting the choice can come later.
- **Counts agree with rows.** While review worktrees are hidden they are excluded from the
  Worktrees subtitle count, the repository rail's per-repo counts and health glyphs, the `All`
  row's total, and the context bar's `Worktrees` segment. A number that disagrees with the rows
  under it is the bug this change is trying to remove, not one to add.
- **The key is `v`** in the Hub › Worktrees pane (free in `docs/KEYMAP.md`'s Global and
  Worktrees-pane tables at the time of writing). T02 verifies this against the keymap before
  binding it and picks another free key if `v` has been taken since.
- **The filter query (`/`) narrows the visible list only.** A hidden review worktree does not
  reappear because its branch matches the query; the subtitle's hidden count still says it is there.

## Out of scope

- The CLI. `fleet list` and `fleet list --json` stay a complete inventory: scripts and agents
  depend on them, and hiding rows there would break `fleet board`, prune and delegation flows.
- The palette's `@` (Worktrees) scope, the Workspace's worktree switcher in the title bar, and
  the Reviews board's card tiles and detail. They are how a review worktree is reached, so they
  keep listing it.
- Persisting the toggle across launches (a Settings row or a per-installation preference).
- Any change to how review worktrees are created, named, pruned or deleted in the daemon.
- A separate "Reviews" section or group inside the Worktrees list.

## Affected areas

- `crates/fleet-core/src/github.rs` — the pull-request-checkout helper next to `worktree_matches_pr`.
- `crates/fleet-app/src/state.rs` (and `state/navigation.rs` if that is where Hub view state lives) — the session flag.
- `crates/fleet-app/src/actions.rs`, `keymap.rs`, `action_catalogue.rs`, `dialogs/help/` — the toggle action, its key, its label and its help row.
- `crates/fleet-app/src/screens/hub/projection.rs`, `screens/hub/composition.rs`, `screens/hub/navigation.rs` — exclusion, hidden count, cursor stability.
- `crates/fleet-app/src/views/worktrees_list/model.rs`, `views/worktrees_list.rs`, `views/worktrees_list/tests.rs` — subtitle, row marker, empty state.
- `crates/fleet-app/src/views/repos_rail.rs`, `crates/fleet-app/src/shell/chrome/title.rs` — counts that must agree with the list.
- `crates/fleet-app/src/presentation/` (`DisplayedHub`) and `crates/fleet-app/src/state/harness/projection.rs` — harness dump.
- `crates/fleet-harness/src/fixture/plan.rs`, `fixture/seed.rs`, `scenarios/hub/` — fixture and scenario.
- `docs/UX-SPEC.md` §3.3 (and §2.2 / §3.2 where the counts are described), `docs/KEYMAP.md`, `docs/TESTING-HARNESS.md`, `docs/BOARD.md` (one sentence where review worktrees are introduced).

## Tasks

### T01 — Recognise a pull-request checkout in `fleet-core`
- **Intent:** one pure, tested function says whether a worktree is a pull-request checkout and which number it checks out, and `worktree_matches_pr` uses it instead of formatting the string itself.
- **Touches:** `crates/fleet-core/src/github.rs`
- **Steps:**
  - Add a `#[must_use]` function beside `worktree_matches_pr` that parses `Worktree.base_ref` of the form `pull/<n>/head` and returns `Option<u64>` (the number), `None` for any other base ref. Document it as the definition of "review worktree" the app relies on, and note that the daemon writes this base ref in `create_pr_local`.
  - Rewrite the `pull/<n>/head` arm of `worktree_matches_pr` to call it, so there is one definition.
  - Unit tests in the existing `mod tests`: `pull/412/head` parses to 412; `origin/main`, `pull/x/head`, `pull/412/merge` and an empty string are `None`; `worktree_matches_pr` still matches by base ref and by same-repo branch as before.
- **Verification:** `cargo test -p fleet-core github` then `make lint`.
- **Done when:** the helper exists with tests and `worktree_matches_pr` has no second copy of the `pull/<n>/head` format.

### T02 — Add the session flag, the toggle action, its key, label and help row
- **Intent:** the app has one piece of state saying whether review worktrees are shown, one action that flips it, and the user can reach that action from the keyboard, the palette and the help overlay.
- **Touches:** `crates/fleet-app/src/state.rs` (or `state/navigation.rs`), `crates/fleet-app/src/actions.rs`, `crates/fleet-app/src/keymap.rs`, `crates/fleet-app/src/action_catalogue.rs`, `crates/fleet-app/src/dialogs/help/`, `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md` (only if it lists the Hub actions or key contexts by name)
- **Steps:**
  - Load `gpui-state-and-memory` and `gpui-app-shell` first.
  - Add a `bool` field on the Hub's view state (next to `hub_pane` / `scope` / `pr_tab`), default `false` ("hidden"). It is not persisted and survives screen changes within the session.
  - Add a `worktrees::ToggleReviewWorktrees` action in the `worktrees` `actions!` block with a doc line in the same style as its neighbours. Its handler flips the field and calls `cx.notify()` once, in an update path.
  - Check `docs/KEYMAP.md` (Global, Hub › Worktrees pane, "Explicitly rejected", "Conflicts noticed") and the keymap tests: bind `v` in the Hub › Worktrees pane key context if it is still free; otherwise pick another free letter and record the choice in the tracker's decisions log.
  - Give the action a catalogue label pair the palette and help can show: `Show review worktrees` when hidden, `Hide review worktrees` when shown (follow whatever the catalogue already does for state-dependent labels; if it has no such mechanism, use one label `Toggle review worktrees` and say so in the decisions log).
  - Add the row to the help overlay's Hub › Worktrees table and to `docs/KEYMAP.md`'s Hub › Worktrees pane table in the same commit.
  - Extend the keymap/catalogue tests that pin every action has a label and every documented key is bound.
- **Verification:** `cargo test -p fleet-app keymap`, `cargo test -p fleet-app action_catalogue`, `cargo test -p fleet-app help`, then `make lint`.
- **Done when:** pressing the key in the Worktrees pane flips the flag (a GPUI test asserts it), the palette lists the command, and `docs/KEYMAP.md` names the key.

### T03 — Exclude hidden review worktrees from the Hub projection and every count that must agree with it
- **Intent:** while the flag is off, review worktrees leave the Hub's scoped worktree slice before rows, glyphs, rail counts, the subtitle and the context-bar count are derived, and the model records how many were left out.
- **Touches:** `crates/fleet-app/src/screens/hub/projection.rs`, `crates/fleet-app/src/views/repos_rail.rs` (only if the count needs a parameter), `crates/fleet-app/src/shell/chrome/title.rs`, `crates/fleet-app/src/presentation/` (`DisplayedHub`, `filter_counts`)
- **Steps:**
  - Load `gpui-performance` first: this is a per-revision projection, not a render path.
  - In `model`, after the context filter builds `scoped`, partition it with the T01 helper when the flag is off. Keep the count of removed worktrees; the rail glyphs, `rail_rows`, `worktree_total`, `build_rows` and `summary` all operate on the remaining slice. Note `rail_rows` currently receives `snapshot.worktrees` directly, so the per-repo counts need the filtered slice (or a filtered clone) instead.
  - Add the flag to `ProjectionKey` so toggling rebuilds the model, and add `review_hidden: usize` to `HubModel` and to `DisplayedHub` (the harness reads `DisplayedHub`).
  - In `hub_place` (title bar), apply the same exclusion to the `worktrees` count when the flag is off, so the context-bar segment and the page subtitle say the same number.
  - `filter_counts` needs no change if `worktree_total` is already the post-exclusion total; confirm with a test.
  - Tests in `projection.rs`'s `mod tests` and in `views/worktrees_list/tests.rs`: with two ordinary worktrees and one `pull/7/head` worktree, hidden mode yields two rows, `worktree_total == 2`, `review_hidden == 1`, and the rail's repo count is 2; shown mode yields three rows and `review_hidden == 0`. A test that the title bar's count follows the same rule.
- **Verification:** `cargo test -p fleet-app hub`, `cargo test -p fleet-app worktrees_list`, `cargo test -p fleet-app title`, then `make lint`.
- **Done when:** with review worktrees hidden, no surface in the Hub (list, subtitle, rail, context bar) counts or draws them, and `HubModel.review_hidden` says how many there are.

### T04 — Say what is hidden, mark what is shown, and keep the empty state honest
- **Intent:** the page subtitle reports the hidden count and offers the toggle to the pointer, a shown review worktree is visibly a review worktree, and a scope whose only worktrees are hidden reviews does not claim "No worktrees yet".
- **Touches:** `crates/fleet-app/src/views/worktrees_list/model.rs`, `crates/fleet-app/src/views/worktrees_list.rs`, `crates/fleet-app/src/views/first_run.rs` (or wherever `EmptySurface` lives), `crates/fleet-app/src/screens/hub/composition.rs`, `crates/fleet-ui-kit` only if a needed chip variant is missing, `docs/UX-SPEC.md` §3.3
- **Steps:**
  - Load `gpui-components` and `gpui-styling` first. No literal colors, sizes or durations; everything from the theme.
  - Subtitle: `summary` (or its caller) appends `· <k> review worktree(s) hidden`, zero-suppressed, singular at one, after the existing `needs attention` segment. Draw that segment as a ghost button with a stable target name (`worktrees.reviews`) whose click dispatches the T02 action; its tooltip carries the key from the live keymap, as the toolbar buttons do. When review worktrees are shown, the segment reads `· <k> review worktree(s)` and toggles back.
  - Row marker: when shown, a review worktree's name cell carries a muted `review` chip after the branch (same slot as the host chip and `uncommitted changes`), so the user can tell it from their own work at a glance. Add a `review: bool` (or the PR number) to `WorktreeRow` from the T01 helper.
  - Empty state: add an `EmptySurface` variant for "the scope has worktrees, but all of them are hidden reviews": `No worktrees of your own yet` over the hidden-count line and a `Show review worktrees  v` button that dispatches the toggle. Keep `New worktree` available as today.
  - Update `docs/UX-SPEC.md` §3.3 in the same commit: the subtitle sentence, the new element row in the table (marker chip, subtitle button), the new state row, the keyboard line (`v`), and the "Intentionally omitted" list if the marker changes it. Add one sentence to §3.5 (Review tab) or `docs/BOARD.md` where review worktrees are introduced: they run in their own worktree, which the Worktrees list hides by default.
  - Tests in `views/worktrees_list/tests.rs`: the summary wording at 0, 1 and 3 hidden; a shown review row carries the marker; the empty surface picks the new variant only when every scoped worktree is a hidden review.
- **Verification:** `cargo test -p fleet-app worktrees_list`, `cargo test -p fleet-app hub`, `make lint`, then run the app (`make run`) with at least one review worktree and check the subtitle, the chip and the empty state by eye.
- **Done when:** the subtitle names the hidden count and toggles on click, a shown review row is marked, and an all-hidden scope explains itself instead of saying there are no worktrees.

### T05 — Keep the cursor and selection stable across the toggle
- **Intent:** hiding the row the cursor sits on, or showing rows above it, never leaves the cursor pointing at a row that is not there.
- **Touches:** `crates/fleet-app/src/screens/hub/navigation.rs`, `crates/fleet-app/src/screens/hub/` tests
- **Steps:**
  - Read the anchored-by-identity cursor logic in `navigation.rs` (the `anchor` handling at the top of the file) and confirm what happens when the anchored row disappears from the projection.
  - If the existing clamp already lands the cursor on a neighbouring row, add a GPUI test that pins it for this case: cursor on a review row, toggle to hidden, cursor lands on a valid row and the detail panel follows; toggle back, the anchor does not jump.
  - If it does not, make the toggle handler re-anchor the way a filter change does (§3.3 "Cursor stability": sort and cursor move only on explicit user action, which this is).
- **Verification:** `cargo test -p fleet-app hub::navigation` (or the file's test module name) and `make lint`.
- **Done when:** a test toggles both ways with the cursor on a review row and asserts a valid, sensible cursor after each toggle.

### T06 — Harness: a `review` mark, the new target, a fixture with a review worktree, and a scenario
- **Intent:** the GUI harness can see review worktrees and pins the default-hidden behaviour end to end.
- **Touches:** `crates/fleet-app/src/state/harness/projection.rs`, `crates/fleet-harness/src/fixture/plan.rs`, `crates/fleet-harness/src/fixture/seed.rs`, `scenarios/hub/review-worktrees.scenario`, `docs/TESTING-HARNESS.md`, `crates/fleet-app/tests/harness_headless.rs` only if the scenario list is explicit there
- **Steps:**
  - Load `rust-gpui-testing`, then read `docs/TESTING-HARNESS.md` end to end before changing anything: it is the frozen contract.
  - In `worktree_rows`, push a `review` mark for a row whose record the T01 helper recognises. Document the new mark word in `docs/TESTING-HARNESS.md` next to `degraded` and `remote`, and the `worktrees.reviews` target next to the other Worktrees page targets, in the same commit.
  - Fixture: give the harness a worktree created the production way. Preferred route: add a fixture field (or extend the `busy` preset) that seeds `refs/pull/<n>/head` into the fixture's origin repository and then sends `CreateWorktreeFromPr` for it, so the daemon's own `create_pr_local` writes the `pull/<n>/head` base ref. Fallback if the origin cannot carry the ref: create the worktree with `base: Some("pull/<n>/head")` after fetching that ref into the base clone. Record which route worked in the tracker's decisions log. Reuse one of the `busy` preset's fake pull requests so the row's PR chip resolves.
  - Scenario (`fixture: busy` or the extended preset, no `shot`, so it runs headless too): await idle; assert `lists.worktrees` has no row with mark `review` and the count of rows equals the non-review worktrees; assert `targets["worktrees.reviews"]` exists; `key v`; await a row whose marks include `review` and whose badge is the review branch; `key v`; await that row absent. Head the file with the `UX-SPEC §3.3` reference and the reasoning comment block, as the sibling scenarios do.
  - If the app's harness list of headless scenarios is enumerated anywhere, add the new file.
- **Verification:** `make harness-one SCENARIO=scenarios/hub/review-worktrees.scenario LANE=headless`, then `make harness-headless`, then `make harness` (the subtitle changed, so the `worktrees-page` baseline in `scenarios/hub/worktrees-pointer.scenario` may need `HARNESS_ARGS=--update-baselines` after a human look at the new screenshot).
- **Done when:** the new scenario passes in both lanes, the existing corpus passes, and `docs/TESTING-HARNESS.md` names the new mark and target.

### T07 — Quality gate and doc sweep
- **Intent:** the change meets the repo's bar and every document that describes a changed surface agrees with the code.
- **Touches:** whatever the review turns up; `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/TESTING-HARNESS.md`, `docs/BOARD.md`
- **Steps:**
  - Load `zed-quality-review` and run it over the branch diff. Fix every finding it ranks as must-fix.
  - Grep the docs for `Worktrees list`, `worktree count`, `across <n> repositories` and `review worktree` and confirm each sentence matches what the code now does (§2.2 context bar count, §3.2 rail counts, §3.3 list, §3.5 Review tab).
  - Confirm no `unwrap`, `todo!`, `dbg!`, `TODO`, bare `.detach()` or `let _ =` on a fallible call entered production code.
  - Confirm each commit is one logical change with its doc update riding along, in `<area>: <summary>` form.
- **Verification:** `make lint`, `make test`, `make harness`.
- **Done when:** all three commands are green on the branch tip and the review reports nothing above nit level.

## Verification

Run from the workspace root, in this order:

```sh
make lint
make test
make harness-headless
make harness
```

`make harness` is required: this change alters a screen, a key and the harness dump, none of
which `make test` covers. If the daemon binary is being run from this checkout, `make restart`
is not needed (no daemon code changes), but a running app must be relaunched to pick up the new
keymap.

## Definition of done

- [ ] Every task T01–T07 is ticked in the tracker, with its verification output or a one-line "verified: how".
- [ ] `make lint` is clean.
- [ ] `make test` passes.
- [ ] `make harness-headless` and `make harness` pass, with any baseline update reviewed by a human and committed.
- [ ] `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/TESTING-HARNESS.md` and `docs/BOARD.md` describe the shipped behaviour, each updated in the commit that changed the code it describes.
- [ ] The tracker reflects reality: decisions log filled in (key chosen, fixture route, label mechanism), follow-ups captured.
- [ ] `zed-quality-review` has been run on the final diff and its findings addressed or explicitly deferred in the tracker.

## Risks and rollback

- **A user's own worktree is hidden.** Only a `pull/<n>/head` base triggers hiding, which the user sets by hand only through the Create dialog's "previous base" option. The subtitle always states the hidden count and `v` shows everything, so nothing is lost, only out of the way. If this bites, the follow-up is to persist an "always show" preference.
- **A degraded or offline review worktree is invisible in the rail's health glyph while hidden.** Its review card on the Reviews board still shows the failed run, and the subtitle's count keeps it discoverable. Documented in §3.3; revisit if users miss failed hooks.
- **Counts drift.** Three surfaces (subtitle, rail, context bar) must exclude the same set. T03's tests pin each one; a future refactor that passes `snapshot.worktrees` straight to one of them reintroduces the drift, so the tests name the rule in their titles.
- **Cursor lands on nothing after a toggle.** T05 pins the clamp with a GPUI test.
- **Harness fixture cannot produce a `pull/<n>/head` worktree.** T06 carries a fallback route; if both fail, the scenario is replaced by a GPUI test over `DisplayedHub` and the follow-up is logged.
- **Rollback:** revert the branch's commits. No persisted data, schema version or wire capability changes, so an older app reads the same daemon state unchanged.
