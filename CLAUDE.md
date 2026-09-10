# Fleet — agent guide

Fleet is a macOS GPUI app (`fleet`), a long-running daemon (`fleetd`), and a CLI over one
Cargo workspace. Jobs, terminals, and worktrees belong to the daemon; the app only mirrors
it. GPUI comes from Zed tag `v1.18.1`; the engineering bar is Zed's, adapted to this repo.

## Before writing code, load the matching skill

Project skills live in `.claude/skills/`. Load the one for the area you touch before
editing, and load `zed-quality-review` before declaring a change done.

| You are about to… | Load |
| --- | --- |
| Add or change an entity, subscription, `Task` field, global, `cx.notify`, or async closure in `fleet-app` / `fleet-ui-kit` / `fleet-lazygit` | `gpui-state-and-memory` |
| Add or change a component, view, list, builder API, or the ui-kit gallery | `gpui-components` |
| Touch colors, spacing, sizes, fonts, elevation, icons, hover/focus states, or animation | `gpui-styling` |
| Touch a `render` path, a list over many rows, a per-frame projection, or anything that felt slow | `gpui-performance` |
| Spawn work, await, debounce, use channels, the app `Bridge`, tokio in the daemon, PTY or subprocess IO | `rust-async-background-work` |
| Change `fleet-proto`, `fleet-client`, the daemon server loop, reconnect, timeouts, remote bootstrap | `rust-ipc-protocol` |
| Add a crate, module, dependency, lint, error type, log line, migration, or split a large file | `rust-workspace-architecture` |
| Write or change any test, fake, fixture, or regression test | `rust-gpui-testing` |
| Add an action, keybinding, focus handle, dialog, modal, palette command, notification, window | `gpui-app-shell` |
| Review a diff or PR, or finish a task | `zed-quality-review` |

## Non-negotiables

- `docs/` is authoritative, not descriptive. `docs/README.md` assigns each document a
  domain. When code and a doc disagree, one of them is a bug; fix both in the same commit.
- Render prepares nothing: no IO, no requests, no `cx.notify`, no heavy CPU inside `render`
  (`docs/APP-CONTRACTS.md`). Precompute in update paths and memoise per revision.
- Nothing the user started is tied to a UI surface. Processes, PTYs, and jobs live in the daemon.
- No `unwrap`, `todo!`, `unimplemented!`, `dbg!`, `TODO` in production code. `expect` only
  for static invariants with a message. Never `let _ =` a fallible call; use `?`, log it,
  or match it — the one exception is a fire-and-forget channel send, which needs a comment
  saying the receiver is gone only during shutdown (`rust-workspace-architecture`).
- Components in `fleet-ui-kit` take no domain types and contain no literal colors, sizes,
  or durations; everything comes from the theme and tokens (`docs/DESIGN-SYSTEM.md`).
- Every fallible spawned task ends in `.detach_and_log_err(cx)` or is stored in a field,
  never a bare `.detach()`.

## Verify before you say it is done

```sh
make lint    # cargo fmt --check + clippy --workspace --all-targets --all-features -D warnings
make test    # builds fleetd first, then cargo test --workspace
```

Run `make restart` after changing daemon code so the running `fleetd` matches the build.

## Commits

`<area>: <imperative lowercase summary>` where area is the crate short name: `app`, `daemon`,
`core`, `ui-kit`, `proto`, `cli`, `client`, `git`, `term`, `lazygit`, `tests`, `build`, `docs`.
One logical change per commit; the doc update rides with the code it describes.
