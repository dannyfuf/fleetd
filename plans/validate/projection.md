# Adversarial validation — harness projection (C19–C24)

Base `881162c5c283cefb4319bbfdb79629ef31960745`, branch `test-harness`.
Method: for each candidate, refute first (§11 entry? a test? a caller that makes it unreachable?
another path in the predicate engine?), then prove only what survives. Every "never written /
never called" claim was re-searched from scratch.

---

### C19 (F1-1 + F2-5) — verdict: SURVIVES NARROWED
- **Confidence**: high

**Refutation attempt.** Three angles, one of them succeeded partially.

1. *Is it in §11?* I read `docs/TESTING-HARNESS.md:543-583` in full and grepped the whole doc for
   `fields|message`. §11 has seven bullets — virtual-lane lock screens, no baseline recorded,
   clipboard, the Codex gate, `advance`, run-dir pruning, five oversized files. **None mentions
   `dialog`.** The only places the limit is written down are a Rust doc comment
   (`projection.rs:363-365`, "Fields, buttons and the message body belong to the dialog host entity
   and are not mirrored into `AppState`, so they are reported empty rather than guessed") and a
   *scenario comment* (`scenarios/hub/settings.scenario:6`). Neither is authoritative — `docs/` is.
   Refutation **fails** for `fields` and `message`.
2. *Is `buttons` also a gap?* **No — this half is refuted.** §3's own target-names section
   (`docs/TESTING-HARNESS.md:262-265`) already states: "`dialog.button[N]` is part of the vocabulary
   but is painted nowhere: Fleet's dialogs are confirmed from the keyboard and `Dialog::primary`
   renders text, not a control." An empty `buttons` array is therefore *accurate*, not drift.
3. *Does the predicate engine flag the missing path?* No. §2 (`:169-171`) freezes "`absent` holds for
   a path that is missing *or* JSON `null`". So `assert dialog.fields[0] absent` and
   `assert dialog.message absent` are unconditionally true. The dangerous-half claim stands.

**Reasoning.** `fields` and `message` are real unrecorded gaps; `buttons` is not.
- Six dialogs paint `dialog.field[N]` — verified by grep, not taken on trust:
  `edit_hooks.rs` (×2), `create_worktree.rs` (×3), `context.rs` (×2), `rename_terminal.rs` (×1),
  `clone_repo.rs` (×1), `card_create.rs` (×2). §3:256-261 spells out the per-dialog tab-cycle index
  for each of them.
- `message` is not vacuous either: `dialogs/rename_terminal.rs:161` does `dialog = dialog.error(error)`
  on a failed rename, i.e. a dialog that *is* showing an error body while the snapshot reports
  `message: null`.
- Mitigating fact I checked: no corpus scenario asserts on `dialog.fields`/`dialog.message` today
  (`rg 'dialog\.' scenarios/` returns only `dialog.name` assertions). So this is a loaded gun, not a
  currently-green broken test.

**Evidence**
```rust
// crates/fleet-app/src/state/harness/projection.rs:366-374
fn dialog_snapshot(&self) -> Option<DialogSnapshot> {
    let Some(Overlay::Dialog(dialog)) = self.overlay.as_ref() else { return None; };
    Some(DialogSnapshot {
        name: dialog.context_name().to_owned(),
        fields: Vec::new(),
        buttons: Vec::new(),
        message: None,
    })
}
```
`docs/TESTING-HARNESS.md:197` — `| `dialog` | `{name,fields:[{name,value,focused}],buttons:[string],message:string|null}` or null |`

**Which side moves**: the **doc**, now — and optionally the code later. The emptiness is not an
oversight but an architectural boundary (field state lives in the `DialogHost` entity, not in
`AppState`, which is the only thing the projection can read). The cheap correct move is a §11 entry
saying `dialog.fields` and `dialog.message` are always empty today and that a dialog's content is
asserted through `targets["dialog.field[N]"]` plus keystrokes, and a pointer on the §3 row. Mirroring
the host into `AppState` (and into `ProjectionKey`) is the larger fix and can land later. Do **not**
write a §11 entry for `buttons` — §3 already covers it correctly.

**Reduced claim that holds**: `dialog.fields` and `dialog.message` are structurally always empty
against a frozen §3 that promises them, §11 does not record it, and `absent` over either is an
unconditional pass. `buttons` is correctly empty and is already documented as such.

---

### C20 (F2-3) — verdict: SURVIVES
- **Confidence**: high

**Refutation attempt.** Four angles, all failed.

1. *Verify the "no writer" claim myself.* `rg 'cursors\s*\.' crates/ --glob '*.rs' | grep -i job`
   returns exactly three lines workspace-wide:
   - `state/harness/projection.rs:319` (read)
   - `state/harness/projection.rs:433` (read)
   - `state/snapshot.rs:87` (the clamp)

   And the clamp is `self.cursors.jobs = clamp_cursor(self.cursors.jobs, snapshot.jobs.len())` with
   `clamp_cursor(current, len) = if len == 0 { 0 } else { current.min(len - 1) }`
   (`state/navigation.rs:252-254`) — a **monotonically non-increasing** function. `Cursors` derives
   `Default` (`navigation.rs:226-237`), so `cursors.jobs` starts at `0` and can never leave it.
   The field is *provably* pinned at 0 for the lifetime of the app. The claim is stronger than the
   reviewer stated.
2. *Does the panel move it another way?* No. `screens/jobs/actions.rs:266-305` moves
   `panel.cursor` (`PanelState::cursor`), never `AppState::cursors`. So do
   `PanelState::clamp` (`screens/jobs.rs:87`) and `PanelState::focus_job` (`screens/jobs.rs:92-105`),
   the latter being the `!` sticky-error path that deliberately lands the cursor on a **non-zero**
   row.
3. *Is there a test that would catch it?* No. `screens/jobs/tests.rs:120`
   (`the_cursor_indexes_the_visible_rows_not_the_daemons_list`) and `:143` test `PanelState` in
   isolation and never touch the projection. `state/harness/tests.rs` has no jobs-overlay test at all
   — `focused` is only asserted for the Hub and the dialog case (`tests.rs:227`).
4. *Are the scenario lines really vacuous?* Yes, verified. `scenarios/hub/jobs-panel.scenario` never
   sends `j`/`k`, so `focused == jobs.row[0]` at `:15` and `:29` is true by construction on both the
   real and the broken reading. Line `:29`'s stated purpose ("the line that exists to prove `Esc`
   collapsed a log without closing the panel") is served by the `overlay == Jobs` clause beside it,
   not by the `focused` clause.

**Reasoning — second half (filter divergence) also confirmed.**
- `lists.jobs` is built from the *unfiltered* daemon list: `job_rows` iterates `snapshot.jobs`
  directly (`projection.rs:581-601`) with no `JobFilter` anywhere in the function.
- `jobs.row[N]` is painted over the *filtered* list: `screens/jobs/presentation.rs:241-250` builds
  `gpui::list(...)` over `rows` = `panel.prepared.rows`, which `presentation::update` populates via
  `panel.prepared.update(jobs, panel.filter, home, scroll)` (`presentation.rs:214`).
- So with `f` cycled to e.g. `Running`, `targets["jobs.row[0]"]` is the first *running* job while
  `lists.jobs.rows[0]` is the first *daemon* job. Click target and oracle address different rows.

**Evidence**
```rust
// crates/fleet-app/src/state/harness/projection.rs:319
Overlay::Jobs => format!("jobs.row[{}]", self.cursors.jobs),
// crates/fleet-app/src/state/harness/projection.rs:433
list(self.job_rows(snapshot), self.cursors.jobs, String::new()),
// crates/fleet-app/src/state/snapshot.rs:87  — the only write, and it only shrinks
self.cursors.jobs = clamp_cursor(self.cursors.jobs, snapshot.jobs.len());
// crates/fleet-app/src/screens/jobs/actions.rs:300 — what actually moves
panel.cursor = panel.cursor.saturating_sub(1);
```

**Which side moves**: the **code**. §3 freezes `jobs.row[N]` in the `focused` vocabulary (`:219`) and
freezes `lists` as "map name → `{rows,selected,filter}`" (`:196`). Reporting a hard-coded 0 and an
unfiltered row set is the projection lying about a surface the doc defines correctly. The
`projection.rs` module header's own claim — "Nothing here is a second source of truth" — is violated.
Fix per the reviewer: mirror the panel's visible-row cursor (or its selected `JobId`) into `AppState`
on every move/filter change, build `lists.jobs` from the same filtered rows the panel renders, and add
the mirrored value to `ProjectionKey`. Note the `filter` field of `lists.jobs` is also hard-coded
`String::new()` while the panel has a live `JobFilter` — same root cause, worth fixing in the same
commit.

---

### C21 (F2-4) — verdict: SURVIVES
- **Confidence**: high

**Refutation attempt.** Two angles, both failed.

1. *Does the quoted-bracket form actually work?* Yes — I read the parser rather than trusting the
   claim. `crates/fleet-drive/src/predicate.rs:413-447` (`parse_bracket`): on `[` it peeks for `"`,
   then accumulates the key with `\"`/`\\` as the only escapes, requires the closing `"]`, rejects an
   empty key, and returns `Segment::Key(key)`. Its doc comment at `:356-358` names exactly this use:
   "the two maps whose keys are not bare names: `lists["board.cards"]` carries a dot and …". §2
   freezes the form at `docs/TESTING-HARNESS.md:162-166`.
2. *Is it only theoretical?* No — it is already in the corpus.
   `scenarios/hub/pointer-tabs.scenario:13`:
   `assert targets["hub.tab[0]"] exists && targets["repos.rail"].w == 240`.
   `scenarios/agents/README.md:57` relies on the same form.

So the premise in the header comment is simply false.

**Reasoning — the oracle is readable in both states.** `views/repos_rail.rs:326-341`:
```rust
let width = if collapsed { px(COLLAPSED_WIDTH) } else { cx.theme().metrics.rail_w };
let mut pane = Pane::fixed(width) …;
// `H` (KEYMAP A22) is the rail's only collapse affordance …
pane.harness_target("repos.rail").into_any_element()
```
`COLLAPSED_WIDTH: f32 = 44.0` (`repos_rail.rs:19`). The target is painted unconditionally on both
branches, so `targets["repos.rail"].w` is 240 expanded and 44 collapsed — exactly the oracle §3's
target table names ("The rail's own `w` is the collapse oracle — 240 expanded, 44 collapsed").

**Impact confirmed.** `scenarios/hub/rail-collapse.scenario` asserts, after each `key H`, only
`hub_pane`, `focused`, `key_contexts[1]`, `lists.repos.rows[2] exists` and
`lists.repos.selected.label` (`:23-24`, `:31`, `:36`). Every one of those is preserved if `H` stops
collapsing the rail altogether — the scenario would stay green. The remaining evidence is `shot
collapsed` (`:25`), and §11's second bullet ("**No baseline has ever been recorded.**
`scenarios/baselines/virtual/` holds a `README.md` and nothing else") means that screenshot is
compared against nothing. Verified in §11 directly. `H` is untested.

**Which side moves**: neither the doc nor the projection — the **scenario**
(`scenarios/hub/rail-collapse.scenario`, owned by `corpus-hub` per §10). Add
`await targets["repos.rail"].w == 44` after the first `key H` (line 21-22) and
`await targets["repos.rail"].w == 240` after the second (line 34-35), and delete lines 6-8 of the
header comment, which state a false limitation of the predicate grammar.

---

### C22 (F2-13) — verdict: SURVIVES NARROWED
- **Confidence**: medium-high

**Refutation attempt.** Two angles; one partially succeeded (on the wording), one failed (on the
substance).

1. *Which index does the list primitive pass?* Verified. `ListView::render`
   (`fleet-ui-kit/src/components/list_view.rs:385-389`) is
   `uniform_list(self.id, self.item_count, move |range, window, cx| range.map(|ix| render_row(ix, …)))`
   — `range` is the visible item range, `ix` is the **model** index into the full item count, and
   `render_row` is the closure that calls `.harness_target_indexed("worktrees.row", index)`. The jobs
   panel uses `gpui::list(scroll.clone(), move |index, _, _| …)` (`screens/jobs/presentation.rs:241`),
   same shape. Both are virtualized: only rows in the visible range are built at all. Reviewer's
   mechanical claim is correct.
2. *Is a scrolled-out row painted?* No. `fleet-ui-kit/src/harness.rs` records in `paint`, and a row
   outside the rendered range is never constructed, let alone painted — so it has no entry in the
   frame's target table and `click worktrees.row[0]` on a scrolled list fails with "unknown target".
   Confirmed.
3. *Where the refutation partly bites*: §3's sentence is "Indices describe current visual order; rows
   also carry stable domain ids in `lists`." Read in context, the contrast being drawn is
   *positional-and-current* vs *stable-domain-id*, not *on-screen-position* vs *model-position*. The
   model index **is** the current visual order of the list as a whole (post-filter, post-sort); it is
   just offset from the viewport. So "the doc is wrong" is an over-read of an ambiguous sentence, and
   the strict drift the reviewer alleges is arguable rather than plain.

**Reasoning.** What is *not* arguable, and is documented nowhere, is the consequence: a row scrolled
out of the rendered range has no target, and `worktrees.row[0]` on a scrolled list resolves to
nothing. That is a rule a scenario author needs and neither §3's target-names section nor §11 states
it. §3 *does* state the analogous rule for two other conditional names
(`docs/TESTING-HARNESS.md:266-270`, `dialog.row[N]` and `agents.approval.edit`), which shows the doc
already knows it owes the reader this kind of caveat.

**Latency of the issue.** No scenario scrolls a list far enough to hit it today:
`rg '^scroll' scenarios/` returns two lines, both `scroll 0 -2 at worktrees.row[0]`
(`pointer-basics.scenario:27`, `pointer-tabs.scenario:34`) on the `busy` fixture. So this is a trap
for the next author, not a currently-failing path.

**Evidence**
```rust
// crates/fleet-ui-kit/src/components/list_view.rs:385
let list = uniform_list(self.id, self.item_count, move |range, window, cx| {
    range.map(|ix| render_row(ix, cursor == Some(ix), window, cx)).collect::<Vec<_>>()
});
// crates/fleet-app/src/views/worktrees_list.rs:348 — `index` is uniform_list's model index
.harness_target_indexed("worktrees.row", index)
```

**Which side moves**: the **doc**, agreeing with the reviewer. The model index is the more useful key
precisely because it lines up with `lists.*.rows[N]`, which §3 wants scenarios to use as the oracle.
Changing the code to a viewport-relative index would break that alignment and make every target name
scroll-dependent. So §3's target-names paragraph should say the index is the position in the list the
snapshot reports under `lists` — not the on-screen position — and add plainly that **a row scrolled
out of view has no target**, in the same place as the two existing conditional-name caveats.

**Reduced claim that holds**: the substantive, undocumented fact is that list-row targets are only
painted for rows in the visible range and are keyed by model index, so on a scrolled list
`worktrees.row[0]` (and its peers) is an unknown target. The "§3 literally says the wrong thing"
framing is an over-read of an ambiguous sentence; the doc still needs the sentence, for the
scrolled-out-row rule if nothing else.

---

### C23 (F1-22) — verdict: SURVIVES NARROWED
- **Confidence**: high

**Refutation attempt.** Three angles; one killed half the finding.

1. *Is the code as claimed?* Yes, both trims verified at `crates/fleet-app/src/state/terminal.rs:183`
   and `:190-192`. And `viewport.rows` is the grid height, computed independently of `rows.len()`:
   `rows: grid.rows.into()` (`projection.rs:690`).
2. *Is it in §11?* No — §11 has no terminal entry, and the doc's only statement on the subject is
   §3's round-trip sentence (`docs/TESTING-HARNESS.md:210-213`).
3. *Is the reviewer's stated impact right?* **Half of it is wrong.** "A predicate that pins a
   right-aligned column … reads a missing path and evaluates false" does not follow. `harness_row`
   converts every *interior* unset cell to a space and only then trims the run of spaces *after the
   last non-space character*. A right-aligned column's own text is non-space content, so everything
   to its left — including the padding that puts it at its column — is preserved verbatim. Column
   alignment, which is what §3's sentence actually promises, **does** survive. This half is refuted.

**Reasoning — what survives.** The row-count half.
- `harness_rows` pops every trailing empty row, so `terminal.rows.len()` is not
  `terminal.viewport.rows`, and `terminal.rows[23] == ""` for a blank bottom row reads a missing path
  and evaluates false rather than true.
- The behaviour is deliberate and **pinned by a test**, which is itself the strongest evidence the
  code is not the thing that should move: `crates/fleet-app/src/state/harness/tests.rs:269-279`
  asserts `terminal.rows == vec!["漢 x"]` with the message *"the spacer is dropped, the blank cell
  survives, the padding goes"*, alongside `assert_eq!(terminal.viewport.rows, 2)` — i.e. the test
  explicitly pins `rows.len() == 1` against `viewport.rows == 2`.
- `harness_row`'s own doc comment (`terminal.rs:161-167`) states all three rules including "Trailing
  spaces then go, because a terminal pads every row to its full width and a predicate should not have
  to know that." The code documents itself; the frozen doc does not.

**Evidence**
```rust
// crates/fleet-app/src/state/terminal.rs:183
text.truncate(text.trim_end_matches(' ').len());
// crates/fleet-app/src/state/terminal.rs:190-192
let mut rows: Vec<String> = (0..self.rows).map(|row| self.harness_row(row)).collect();
while rows.last().is_some_and(String::is_empty) { rows.pop(); }
// crates/fleet-app/src/state/harness/projection.rs:688-692
viewport: ViewportSnapshot { top: …, rows: grid.rows.into(), history: … },
```
`docs/TESTING-HARNESS.md:211-213` — "`rows` drops the spacer cell that follows a wide grapheme and
renders an unset cell as a space, so column alignment survives the round trip."

**Which side moves**: the **doc**. The trims are a deliberate, commented, test-pinned design decision
(a predicate should not have to know a terminal pads to full width), and reverting them would make
every `terminal.rows[N] == "…"` carry invisible trailing padding. §3's sentence should be extended to
state both trims and to say explicitly that `rows.len()` is the number of rows up to the last
non-blank one, which is not `viewport.rows`. Optionally add a §11 line so a scenario author indexing a
bottom row knows why the path is missing.

**Reduced claim that holds**: §3 is silent on two real trims — trailing spaces per row and trailing
blank rows — with the consequence that `terminal.rows.len() != terminal.viewport.rows` and
`terminal.rows[N]` for a blank bottom row is a missing path. The "right-hand column alignment does not
survive" half of the finding is wrong and should be dropped.

---

### C24 (F1-23) — verdict: SURVIVES NARROWED
- **Confidence**: medium

**Refutation attempt — this is the one that largely succeeded.** The finding's headline fact is true
(`renamed_terminals` is read at `projection.rs:645` and is absent from `ProjectionKey`,
`projection.rs:35-56` / `:156-181`, eighteen fields, none of them it). But the stated impact — "an
`await lists.tabs.rows[0].label == …` that waits out its timeout" — does not reproduce, because
**every** mutation of the set is accompanied by a change to an input the key *does* hold. I enumerated
all three mutators with `rg 'mark_renamed|renamed_terminals' crates/`:

1. **`AppState::mark_renamed`** (`state/terminal.rs:230-232`). Exactly one caller in the workspace:
   `dialogs/rename_terminal.rs:128`, inside
   ```rust
   state.update(cx, |app, cx| {
       app.mark_renamed(terminal);
       app.close_overlay();
       cx.notify();
   });
   ```
   `close_overlay` is `self.overlay.take().is_some()` (`state/navigation.rs:637-639`), so the same
   update flips `overlay` from `Some(Overlay::Dialog(RenameTerminal))` to `None`. That moves **five**
   keyed fields at once: `derived.overlay`, `derived.dialog`, `derived.mode`, `derived.focused` and
   `derived.key_contexts` (`projection.rs:196-209`). The memo is invalidated, `build_snapshot` re-runs,
   and it reads the already-inserted `renamed_terminals`. No stale label.
2. **`renamed_terminals.clear()`** (`state/connection.rs:216`) — only in the
   `BridgeEvent::Reconnected { restarted: true }` arm, which eleven lines later does
   `self.link_generation = self.link_generation.wrapping_add(1)` (`:226`). `link_generation` is a keyed
   field (`projection.rs:41`).
3. **`renamed_terminals.retain(…)`** (`state/snapshot.rs:125`, inside `forget_vanished`) — called only
   from `apply_snapshot`, which calls `bump_snapshot_revision()` twelve lines later (`:93`), and
   `snapshot_revision` is keyed (`projection.rs:39`).

So there is no reachable path today on which the projection serves a stale tab label.

**Reasoning — the narrow residual.** Two things keep this from being a clean DROP.
- *A real, if narrow, race.* If the user presses `Esc` while the rename is in flight (the dialog
  renders `hint_row(KeyHintRow::new().key("esc", "cancel"))` at `rename_terminal.rs:155` and the
  `dialog::Cancel` handler in `dialogs/mod.rs:155-158` closes unconditionally — the `in_flight` guard
  at `:97` only blocks a second *Confirm*), then when the reply lands `close_overlay()` is a no-op and
  the key does not move on that edge. The window self-heals almost immediately, because the daemon's
  own snapshot for the rename arrives and bumps `snapshot_revision`; and in the interim the only
  difference is `terminal.title` vs the *old* `terminal.name`, since the new name only reaches
  `AppState` through that same snapshot. This is a transient, not the described hang.
- *A stated invariant is violated in the letter.* `projection.rs:31-33`: "Adding a field to the
  builder means adding its input here: a key that misses an input is a stale snapshot, which makes
  `await` hang until its timeout." The memo's correctness currently rests on a coincidence in a
  caller three modules away, not on the key. That is exactly the fragility the module header exists to
  prevent.

**Evidence**
```rust
// crates/fleet-app/src/state/harness/projection.rs:645 — the unkeyed read
let label = if self.renamed_terminals.contains(&terminal.id) { terminal.name.clone() } else { … };
// crates/fleet-app/src/dialogs/rename_terminal.rs:127-131 — the paired keyed change
app.mark_renamed(terminal);
app.close_overlay();
cx.notify();
```

**Which side moves**: the **code**, but as a one-line hardening rather than a bug fix — add
`renamed_terminals` (or its length plus a revision counter) to `ProjectionKey`. The doc needs nothing.

**Reduced claim that holds**: `ProjectionKey` omits an input the builder reads, violating the module's
own stated invariant. It produces no observable stale snapshot today, because all three mutation sites
are paired with a keyed change (`close_overlay` → `derived.overlay`; `link_generation`;
`snapshot_revision`). The reviewer's impact ("an `await` that waits out its timeout") is refuted for
every in-tree path; severity drops from P3-defect to latent-robustness. Keep the fix, drop the
narrative.

---

## Summary

| ID | Verdict | Which side moves |
| --- | --- | --- |
| C19 | SURVIVES NARROWED | doc (§11 + §3 note) for `fields`/`message`; `buttons` half dropped |
| C20 | SURVIVES | code (`projection.rs`) |
| C21 | SURVIVES | scenario (`scenarios/hub/rail-collapse.scenario`) |
| C22 | SURVIVES NARROWED | doc (§3 target names) |
| C23 | SURVIVES NARROWED | doc (§3 round-trip sentence); alignment half dropped |
| C24 | SURVIVES NARROWED | code (one-line key addition), latent only |

---

## Incidental finding (outside C19–C24, not requested but blocking)

`cargo test -p fleet-app` **does not compile at HEAD** (`0f02991`, working tree clean for
`crates/fleet-app/`):

```
error[E0063]: missing field `reattached` in initializer of `state::connection::DaemonLink`
   --> crates/fleet-app/src/state/harness/tests.rs:440:20
```

`DaemonLink::Reconnected` carries `restarted`, `reattached: usize` and `since`
(`crates/fleet-app/src/state/connection.rs:40-50`), and the test at
`crates/fleet-app/src/state/harness/tests.rs:440-443` initialises only `restarted` and `since`.
`make test` therefore cannot pass on this branch, and neither the harness projection tests nor the
jobs tests run — which also means none of the vacuous-assertion problems in C19/C20 would have been
caught by CI even if a test for them existed. One-line fix (`reattached: 0`, or whatever the
scenario means) in the same commit that touches the projection.
