# 0004 — A native git UI instead of embedding lazygit

**Adopted.** `crates/fleet-lazygit` draws Fleet's git UI in GPUI. It runs two ways from one
crate: as the `fleet://lazygit` native tab inside `fleet-app` (no PTY, no process — see
`docs/ARCHITECTURE.md`, "Native tabs"), and as a standalone `fleet-lazygit` binary against a
repository path.

- **Layout and key vocabulary mirror lazygit**: the side column of panel groups, main and
  secondary panels, screen modes (`+` / `_`), per-panel tabs (`]` / `[`), the status line and the
  semantic color palette. Behavior that lazygit made user-configurable is fixed here.
- **Key contexts must not collide with the host.** GPUI matches a key context as a *subsequence*
  of the rendered chain, and actions are process-wide, so an embedded view may not reuse a word
  the app already uses. The pane renames its overlay words to `LgDialog` / `LgConfirm` / `LgHelp`
  and its action namespaces to `lg_confirm` / `lg_help`. `keymap.rs` has tests that keep both
  true; `App::load_actions` panics on a duplicate action name.
- **Git plumbing is `fleet-git`**: a command runner over the real `git` binary, with the exact
  argv lazygit uses for status, log, `for-each-ref`, stash, reflog and diffs. No libgit2.
- **Architecture is the app's**: one Tokio thread bridged into GPUI with `async_channel`, a
  central state entity with pure reducers, and tests over the reducers rather than over rendered
  strings.

Provenance (removed from the tree; read them in git history): `docs/research/lazygit-native-
brief.md` @ d5bc677, `docs/research/lazygit-reference.md` @ d5bc677.
