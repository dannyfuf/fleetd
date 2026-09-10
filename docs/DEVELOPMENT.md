# Fleet development

Fleet is a Cargo workspace built with Rust 1.97.1 and edition 2024. The repository's
`rust-toolchain.toml` selects the minimal 1.97.1 toolchain and installs `rustfmt` and `clippy`, so
ordinary `cargo` commands use the correct compiler automatically.

## Common commands

Run the project commands through the checked-in `Makefile` (`make help` lists them):

```sh
make run         # build the workspace and open Fleet (ARGS="..." is forwarded)
make run-release # the same with the release profile
make restart     # restart fleetd from this build — run it after changing daemon code
make daemon      # restart fleetd and follow its log
make build       # build the workspace
make check       # cargo check --workspace --all-targets
make test        # build fleetd, then cargo test --workspace against it
make fmt         # cargo fmt --all
make lint        # fmt --check plus Clippy with warnings denied
make doctor      # build Fleet and run its diagnostics
make bootstrap   # install and verify the pinned Zig toolchain
make prune       # drop build artifacts unused for SWEEP_DAYS (7) days; runs before every build
make fresh       # cargo clean, then rebuild from scratch (RELEASE=1 supported)
```

## Keeping `target/` small

A debug build of this workspace used to weigh ~19 GB, mostly per-crate debuginfo objects for
dependencies (macOS keeps them unpacked next to the `.rlib`s) plus `incremental/` state. Cargo
also never garbage-collects `target/`, so dependency bumps and toolchain updates accumulate stale
artifacts on top. Two things keep it in check:

- The dev profile disables debuginfo for dependencies (`[profile.dev.package."*"]` in
  `Cargo.toml`). They are already optimized, so that debuginfo was mostly dead weight; workspace
  crates keep full debuginfo.
- `make build` (and therefore `make run`) runs `make prune` first, which uses
  [`cargo-sweep`](https://github.com/holmgr/cargo-sweep) (installed on first use) to delete
  artifacts not touched in `SWEEP_DAYS` days or built by toolchains no longer installed. Anything
  it removes is simply rebuilt on demand.

When `target/` still grows past what you want, `make fresh` wipes it and rebuilds from scratch.

`make run` does **not** restart the daemon: an already-running `fleetd` keeps its terminal
sessions across an app rebuild. Restarting is explicit, through `make restart` or:

```sh
fleet daemon restart
```

That command requests a graceful shutdown, falls back to `SIGTERM` when the daemon cannot answer,
waits for its socket to disappear, and starts the newly built sibling `fleetd` binary. PTYs do not
survive it.

`make test` and `make ci` build `fleetd` before running workspace tests because app
integration tests launch the ordinary daemon binary. These targets set `FLEET_DAEMON`
to that freshly built workspace binary, overriding inherited paths to other checkouts.
`fleet-daemon`'s own tests ignore `FLEET_DAEMON` and always launch the binary Cargo built
for them, so both halves of a loopback link are the same build.
`cargo test` alone builds its test
harness and can leave an older `target/debug/fleetd` in place. Before running app tests
directly, run `cargo build -p fleet-daemon`, then
`FLEET_DAEMON="$PWD/target/debug/fleetd" cargo test -p fleet-app`.

Direct Cargo equivalents work as usual. Build artifacts use Cargo's default target directory,
`target/` inside this repository. Sharing that repository-local directory between commands in
the same worktree is supported. Do not point multiple worktrees at one external
`CARGO_TARGET_DIR`: Cargo's relative dep-info paths can make one worktree accept another's stale
artifacts. Give parallel worktrees separate target directories when an override is necessary.

## Lints and formatting

`rustfmt.toml` selects edition 2024 and its style edition, so `cargo fmt` is the only formatter.
`Cargo.toml` inherits a hazard-only Clippy list into every crate (`dbg_macro`, `todo`,
`unimplemented`, `declare_interior_mutable_const`, `disallowed_methods`); Clippy's `style`,
`complexity`, `perf` and `correctness` groups stay at warn and are enforced by `-D warnings` in
`make clippy`. `clippy.toml` holds the disallowed-method list. Nothing in that policy grants an
allowance — it only raises what a warning would let slip.

Daemon-discovered subagent watches are configured by `discoveredWatches` in
`config.json`: `enabled` defaults true, `intervalMs` defaults 2000, and `processes`
contains configurable `{id, pattern, enabled}` candidate rules. See
`docs/APP-CONTRACTS.md` for the exact defaults, exclusions, and read-only lifecycle.

## Zig

Building `fleet-term` with its default `ghostty` feature requires Zig **0.15.2**. The pinned
`libghostty-vt 0.2.1` dependency builds its vendored Ghostty commit with that exact minimum
version. On Apple Silicon macOS, install and verify the official release with:

```sh
scripts/bootstrap-zig.sh
```

The idempotent bootstrap verifies the published SHA-256 checksum, installs the official binary at
`~/.local/zig-0.15.2/zig`, and exposes it as `~/.cargo/bin/zig`. The published
`libghostty-vt-sys 0.2.1` build script invokes `zig` by name and does not honor a `ZIG` executable
variable, so `~/.cargo/bin` must be on `PATH` (the bootstrap prints the required export).

Zig's default global cache is `~/.cache/zig`; nothing in this repository overrides it. On macOS 26 with the Xcode 26
SDK, the bootstrap also creates a private SDK overlay below the Zig installation that maps the
SDK's `arm64e-macos` text-based stubs to the `arm64-macos` target expected by Zig 0.15.2. The real
Xcode SDK is not modified.

## Driving the app from a script

`fleet` ships a developer-only scripted-input driver so an automated reviewer can exercise the
real GUI on a machine where `osascript` keystrokes are blocked by the macOS Accessibility
permission. It is off unless `FLEET_DRIVE` names a file:

```sh
: > /tmp/fleet-drive/script.txt
FLEET_HOME=/tmp/fleet-drive FLEET_DRIVE=/tmp/fleet-drive/script.txt ./target/debug/fleet &
```

With `FLEET_DRIVE` set, the app spawns one foreground task on the window that polls the file
every 100 ms and executes the lines appended since the previous poll, so a script can be written
incrementally while the app runs:

```sh
printf 'wait 500\nkey ?\nshot /tmp/fleet-drive/help.png\nkey escape\nquit\n' \
  >> /tmp/fleet-drive/script.txt
```

| Line | Effect |
| --- | --- |
| `key <keystroke>...` | Dispatches each gpui keystroke to the window (`ctrl-s`, `shift-tab`, `?`, `enter`, `escape`, `j`). Several per line: `key ctrl-s ?`. |
| `type <text>` | Dispatches every character as a keystroke (`shift-` for uppercase), so text inputs and the terminal receive it. Inner spaces are kept. |
| `wait <ms>` | Pauses the script before the next line. |
| `shot <path.png>` | Raises the window, runs `/usr/sbin/screencapture -x <path>` and waits for it, then logs `done shot <path>`. With more than one display it passes one path per display, so the others land beside it as `<name>-2.png`, `<name>-3.png`, all logged. |
| `quit` | Quits the app. |

Blank lines and lines starting with `#` are ignored. Every executed line, every parse error and
every finished screenshot is appended to `$FLEET_DRIVE.log` with a timestamp, so a script runner
can wait on `done shot <path>` instead of sleeping. Keystrokes go through
`Window::dispatch_keystroke`, so they take the same path as real input: bindings resolve against
the focus chain of `docs/KEYMAP.md`.

The driver lives in `crates/fleet-app/src/drive.rs` and is started from
`shell/root/bootstrap.rs` right after the window opens. When `FLEET_DRIVE` is unset no task is
spawned and the app is unaffected.

## Logs

`fleet` installs a `tracing` subscriber at startup that writes to stderr and honours `$RUST_LOG`
(default `info`), so redirecting the process's stderr captures app-side errors and the driver's
actions:

```sh
RUST_LOG=fleet_app=debug ./target/debug/fleet > /tmp/fleet-gui/app.log 2>&1
```

## Crate map

| Crate | Responsibility |
| --- | --- |
| `fleet-core` | Pure domain types, schemas, validation, and helpers. |
| `fleet-proto` | Client/daemon wire messages, framing, terminal updates, and socket paths. |
| `fleet-git` | Git plumbing over the real `git` binary: model, reads, mutations, rebase, watch. |
| `fleet-term` | Daemon-owned PTYs, VT engines, terminal host, and key encoding. |
| `fleet-daemon` | Stores, adapters, services, jobs, and the `fleetd` socket server. |
| `fleet-client` | Async daemon connection, spawning, request APIs, events, and terminal attach. |
| `fleet-ui-kit` | Domain-independent GPUI theme, assets, icons, and components. |
| `fleet-cli` | Clap commands plus JSON and human output. |
| `fleet-app` | The `fleet` GPUI app: shell, state mirror, screens, dialogs, terminal rendering. |
| `fleet-lazygit` | The `fleet://lazygit` git UI, embedded in `fleet-app` and standalone. |

`docs/ARCHITECTURE.md` describes how they fit together; `docs/decisions/` records why.
