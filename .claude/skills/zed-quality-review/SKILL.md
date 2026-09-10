---
name: zed-quality-review
description: Review a diff, branch, or PR in the fleetd workspace against the Zed-derived engineering bar, and gate a task as done. Load it when asked to review, audit, or check a change, before opening a PR, or as the last step of any implementation task in this repo. It routes the touched files to the area checklists of the other project skills, runs make lint and make test, and reports ranked findings with file:line and the rule each one violates.
---

# Zed-bar quality review for fleetd

This skill turns the other project skills into a review. Each of them ships a
`references/checklist.md`; this skill decides which checklists apply to a change, applies
them, runs the mechanical gates, and reports. Patterns are verified against Zed v1.18.1,
the GPUI tag fleetd depends on, and adapted to fleetd's architecture (one `Entity<AppState>`,
free render functions, daemon-owned work, `docs/` authoritative).

## When to use

- The user asks to review, audit, sanity-check, or "look over" a diff, branch, or PR.
- You are about to open or update a PR from this repo.
- You finished an implementation task and need to declare it done.
- A change touches more than one crate or any of: `render` paths, `fleet-proto`,
  `fleet-ui-kit`, `keymap.rs`, `dialogs/`, `bridge.rs`, daemon `server/`.

## When not to

- Pure doc typo fixes with no code change (still check the doc is not authoritative
  text that now contradicts code).
- The user explicitly asked for a narrow single-rule check; apply only that checklist.

## Procedure

1. **Scope the change.** Prefer the exact set the user named. Otherwise:
   `git diff --name-only main...HEAD` for a branch, `git diff --name-only` plus
   `git status --porcelain` for the working tree, `gh pr diff <n> --name-only` for a PR.
   Delegate reading the full diff to a subagent when it exceeds ~800 lines; keep only the
   file list and the subagent's per-file summary in your own context.
2. **Map files to areas** with the table below and load each matching
   `references/checklist.md`. Load at most four; when more match, pick the four whose
   files changed most and say which you skipped.
3. **Run the gates** and paste failures verbatim:
   ```sh
   make lint
   make test
   ```
   `make test` builds `fleetd` first because app socket tests launch the real binary.
   If a daemon crate changed, also confirm the user ran `make restart` before any manual
   smoke claims. Never report green on inference; if a gate was not run, say so first.
4. **Apply the checklists** to the diff only, plus the immediate callers of any changed
   public function. Each finding needs: file:line, the checklist question it fails, why it
   matters in one sentence, and the concrete fix. Confirm the line exists before reporting.
5. **Check the docs contract.** `docs/README.md` maps each doc to its domain. If the diff
   changes behavior a doc governs (keymap, design tokens, protocol, remote machines,
   app contracts, board model, native agents), the same change must update that doc.
   A missing doc update is a finding, not a nit.
6. **Check what the repo already does well** (below) and do not report those as findings.
7. **Report** in the format at the end, most severe first, and state the verdict.

## File-to-area routing

| Changed path matches | Checklist to load |
| --- | --- |
| `crates/fleet-app/src/**` state, `Entity<`, `subscribe`, `observe`, `Task<`, `Global`, `cx.spawn` | `.claude/skills/gpui-state-and-memory/references/checklist.md` |
| `crates/fleet-ui-kit/src/components/**`, `examples/kit_gallery.rs`, any `RenderOnce`/`Render` impl | `.claude/skills/gpui-components/references/checklist.md` |
| `crates/fleet-ui-kit/src/theme/**`, any `.bg(`, `.text_color(`, `px(`, `rems(`, `.gap_`, `.p_` in a diff | `.claude/skills/gpui-styling/references/checklist.md` |
| Any `fn render`, `uniform_list`, `list(`, projections/`model.rs`, `to_lowercase`/`clone` inside render | `.claude/skills/gpui-performance/references/checklist.md` |
| `bridge.rs`, `background_executor`, `spawn`, `tokio::`, `async_channel`, `timer(`, PTY/subprocess code | `.claude/skills/rust-async-background-work/references/checklist.md` |
| `crates/fleet-proto/**`, `crates/fleet-client/**`, `crates/fleet-daemon/src/server/**`, remote/bootstrap code | `.claude/skills/rust-ipc-protocol/references/checklist.md` |
| Any `Cargo.toml`, `lib.rs`/`main.rs`, new module or crate, `log::`/`tracing::`, migrations, files > 900 lines | `.claude/skills/rust-workspace-architecture/references/checklist.md` |
| Any `#[test]`, `#[gpui::test]`, `#[tokio::test]`, `tests/**`, fakes, `test-support` | `.claude/skills/rust-gpui-testing/references/checklist.md` |
| `actions.rs`, `keymap.rs`, `dialogs/**`, `shell/root/focus.rs`, notifications, windows | `.claude/skills/gpui-app-shell/references/checklist.md` |

## Always-on checks (apply to every diff, no checklist needed)

- No `unwrap()`, `todo!`, `unimplemented!`, `dbg!`, `TODO`/`FIXME` in production code.
  `expect("…")` only for static invariants with a message. Workspace lints deny `todo!`,
  `unimplemented!` and `dbg!` only; `unwrap()` and `TODO`/`FIXME` must be caught by review.
  A new `#[allow]` is a finding.
- No `let _ =` on a fallible call. Use `?`, `.log_err()`-style logging, or an explicit `match`.
  A fire-and-forget channel send may use it only with a comment saying why the failure is expected.
- No bare `.detach()` on a `Task<Result<_>>`; use `.detach_and_log_err(cx)` or store the task.
- No IO, daemon request, `cx.notify`, or entity update inside a `render` function
  (`docs/APP-CONTRACTS.md`, "Render prepares nothing").
- No literal color, size, or duration inside `fleet-ui-kit` components; no domain types in ui-kit.
- Nothing durable (process, PTY, job, timer that must survive the window) owned by a view.
- Commit messages follow `<area>: <imperative lowercase summary>`.
- Behavior governed by a doc changed the doc in the same commit.

## Already at or above Zed's bar (do not report as findings)

Effectively zero production `unwrap`; `fleet-ui-kit` with 70+ `RenderOnce` components and
zero color literals outside `theme/`; background executor for every filesystem walk and
subprocess in the app; `_subscriptions` / `_tasks` retention fields in the shell; the pure
agent reducer shared by daemon and client; ports-and-adapters in the daemon with
`#[async_trait]` reserved for adapters; byte-exact protocol goldens in
`crates/fleet-proto/tests/compatibility.rs`; `test-support` feature gating; namespaced
actions with keystroke doc comments; the docs-first contract itself.

## Severity scale

| Level | Meaning | Examples |
| --- | --- | --- |
| P0 | Blocks merge: correctness, data loss, panic path, protocol break | production `unwrap`, wire-incompatible proto change without golden update, blocking call on the GPUI thread |
| P1 | Must fix before merge: violates a documented contract | IO in render, doc not updated, bare `.detach()` on fallible task, literal color in ui-kit |
| P2 | Should fix: erodes the bar | per-frame recomputation without a revision cache, new 1000-line mixed file, `let _ =`, missing test for new behavior |
| P3 | Nit: style and naming | non-fluent `if` blocks producing `AnyElement`, non-qualified log macro, doc comment missing on a pub seam |

## Report format

```
Verdict: <ready | ready after P2s | not ready>
Gates: make lint <pass|fail|not run>, make test <pass|fail|not run>
Checklists applied: <names>; skipped: <names or none>

P0 <file>:<line> — <rule> — <why> — Fix: <what>
P1 …
P2 …
P3 …

Docs: <which docs the change should have touched, and whether it did>
Kept the bar: <one line on what the change did well, only if true>
```

Deliver the report in the chat as the final message. Do not post it to GitHub unless the
user asked; when they did, post one review comment per finding with the same fields.

## Related skills

Every checklist this skill loads comes from: `gpui-state-and-memory`, `gpui-components`,
`gpui-styling`, `gpui-performance`, `rust-async-background-work`, `rust-ipc-protocol`,
`rust-workspace-architecture`, `rust-gpui-testing`, `gpui-app-shell`.
