# 0007 — Reproducible GUI smoke procedure

**Adopted.** The GUI is exercised through the scripted-input driver built into `fleet`, not
through `osascript` keystrokes (which need macOS Accessibility permission) and not through a
separate GPUI probe crate.

The driver is off unless `FLEET_DRIVE` names a file. It polls that file every 100 ms and runs the
lines appended since the last poll, so a script can be written while the app runs; every executed
line and every finished screenshot is logged to `$FLEET_DRIVE.log`, which is what a runner waits
on instead of sleeping. Keystrokes go through `Window::dispatch_keystroke`, so bindings resolve
against the same focus chain as real input. `docs/DEVELOPMENT.md` documents the line grammar.

The procedure is reproducible because the environment is pinned: the GPUI tag, the Rust
toolchain, the Zig version and the `FLEET_HOME` the run uses are all fixed
(`0001-gpui-and-toolchain.md`, `docs/DEVELOPMENT.md`). A smoke run states the pinned versions it
ran against.

The earlier standalone smoke crate — a two-file binary against crates.io `gpui 0.2.2` — is
superseded: it verified an API Fleet does not use.

Provenance (removed from the tree; read them in git history): `docs/research/gpui-smoke-recipe.md`
@ 79d7574, `docs/research/gpui.md` @ b5741b7.
