# Phase 3 — The gesture in dialog text inputs — Plan

> Tracker: ./paste-drop-media-2026-09-10-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
>
> **Created 2026-09-24**, split out of the original phase 2 when the composer moved to real
> attachments. References are verified at `fa31272`.

## Summary

Dialog fields where a file path makes sense accept a pasted or dropped file, and insert its
**local** path at the caret. A dialog is a Fleet-side form, so nothing about it belongs on a remote
host. Separately, every text input stops pasting a copied file list as one concatenated string,
which is what GPUI's `ClipboardItem::text()` produces today.

## Sizing call

**Phased**: phase 3 of 3; see ./paste-drop-media-2026-09-10-roadmap.md. Small. There is no wire
work, no daemon work and no new kit event. It reuses phase 2's `TextInput` media mode and phase 1's
`media::stage` with `MediaAnchor::Local`.

## Repository context

- ADR 0020 made `TextInput` the one live editor (`docs/decisions/0020-single-text-input.md`).
  `TextField` no longer exists. About 26 files under `crates/fleet-app/src` own or build a
  `TextInput`.
- Paste is `fleet-ui-kit/src/components/input/handlers.rs:254-267`. It is text-only, and it takes
  GPUI's flattened `ExternalPaths` text as is (pinned `gpui/src/platform.rs:2403-2417`: paths are
  joined with no separator).
- `TextInput::insert` (`components/input.rs:168-180`) inserts at the caret, replaces a selection as
  one undoable edit, and applies the input's filter. Use it for delivery.
- Phase 2 added `accepts_media(bool)` and a `TextInputMedia` event on `TextInput`.
- Phase 1's `media::stage` with `MediaAnchor::Local` and `force_copy = false` hands back an
  existing local file's or folder's own path without copying. A clipboard image is staged into the
  local `~/Downloads/fleet/`.

## Assumptions

- Phases 1 and 2 have shipped.
- A dialog path is inserted bare, or shell-quoted only in a field whose value is later run by a
  shell. Choose per field and record it. Several paths are joined with a single space.

## Out of scope

- The composer (phase 2), terminals (phase 1), and any remote anchor.
- Drop onto non-input surfaces.

## Tasks

### P3-T01 — Stop pasting concatenated paths

**Intent:** fix the flattening bug in every input, with the kit still knowing nothing about Fleet.

**Touches:** `fleet-ui-kit/src/components/input/handlers.rs:254-267`.

**Steps:**
- With media mode off, when the clipboard item has no `String` entry but does have
  `ExternalPaths`, insert the paths joined by a single space (a newline in a logical multi-line
  input), instead of `item.text()`'s separator-less string. No quoting: quoting is shell-specific
  and belongs to the app.
- Plain text paste stays byte-identical.

**Verification:** `cargo test -p fleet-ui-kit` (a paths-only item with two paths inserts them with
a separator); `make lint`.

**Done when:** no input anywhere pastes two copied files as one string.

### P3-T02 — Inventory the dialog fields, and wire the ones where a path belongs

**Intent:** the gesture appears only where it means something.

**Touches:** the dialog and screen hosts under `crates/fleet-app/src/dialogs/` and
`crates/fleet-app/src/screens/` that own a `TextInput`.

**Steps:**
- Load `gpui-app-shell` and `gpui-state-and-memory`.
- List every `TextInput` owner in the tracker table: dialog, field, wired or not, and the reason. A
  path in a branch-name field is nonsense. Enabling it there is worse than leaving it off, and
  saying so is part of the task.
- For each wired field:
  - turn on `accepts_media`;
  - handle `TextInputMedia` by converting it with the phase-1 helpers;
  - call `media::stage` with `MediaAnchor::Local`;
  - insert the result with `TextInput::insert`, quoted or bare as recorded;
  - follow phase 1's spawn shape.

**Verification:** `cargo test -p fleet-app`; `make lint`; `make test`; `make harness`; a manual
check of each wired field on macOS and Linux.

**Done when:** the table is complete, every wired field works, and every unwired field has a
reason.

### P3-T03 — Move the docs with the code

**Touches:** `docs/DESIGN-SYSTEM.md`, `docs/UX-SPEC.md`.

**Steps:**
- `DESIGN-SYSTEM.md`: the multi-path paste rule for `TextInput`.
- `UX-SPEC.md`: which dialog fields accept a file, and how paths are inserted.

**Verification:** `make lint`; re-read against the code.

**Done when:** no edited doc contradicts the implementation.

## Definition of done

- [ ] Every P3 task is ticked in the tracker, with verification beside it.
- [ ] `make lint`, `make test`, `make check` and `make harness` pass.
- [ ] `fleet-ui-kit/Cargo.toml` is unchanged.
- [ ] The dialog table is complete.
- [ ] `zed-quality-review` has been run over the diff.

## Risks and rollback

- **Gesture where it does not belong.** Mitigated by the explicit table.
- **Rollback.** Revert the app commits to un-wire the dialogs. P3-T01 is an independent kit fix and
  can stay.
