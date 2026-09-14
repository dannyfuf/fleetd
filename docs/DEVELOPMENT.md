# Fleet development

Fleet is a Cargo workspace built with Rust 1.97.1 and edition 2024. The repository's
`rust-toolchain.toml` selects the minimal 1.97.1 toolchain and installs `rustfmt` and `clippy`, so
ordinary `cargo` commands use the correct compiler automatically.

## Common commands

Run the project commands through the checked-in `Makefile` (`make help` lists them):

```sh
make run         # build the workspace, restart fleetd, and open Fleet (ARGS="..." is forwarded)
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

`make run` restarts the daemon: it depends on `make restart`, so the freshly built app never talks
to a stale `fleetd`. Restarting is also explicit through `make restart` or:

```sh
fleet daemon restart
```

That command requests a graceful shutdown, falls back to `SIGTERM` when the daemon cannot answer,
waits for its socket to disappear, and starts the newly built sibling `fleetd` binary. PTYs do not
survive it, so an already-running `fleetd` keeps its terminal sessions only when the app is started
without the restart: after `make build`, run the built binary directly
(`FLEET_HOME=… FLEET_DAEMON=… ./target/debug/fleet`) or `cargo run -p fleet-app`.

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

## GPUI platform backends

The workspace pins `gpui_platform` with `font-kit`, `wayland` and `x11`. Both Linux backends are
declared `default-features = false` upstream, so a consumer that names none gets a binary with no
Wayland and no X11 code in it and `gpui::guess_compositor()` answers `Headless` on every Linux
desktop — a `fleet` that opens no window on a machine that plainly has one. Fleet is a desktop
app on both display servers, so both backends are part of the product and the feature list in the
root `Cargo.toml` is load-bearing, not a convenience.

The end-to-end harness depends on the same choice from the other side: its `headless` lane is the
*absence* of `WAYLAND_DISPLAY` and `DISPLAY` (plus `ZED_HEADLESS=1`), not a different build, so
one binary serves every lane (`docs/TESTING-HARNESS.md` §4).

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

`fleet` ships a developer-only end-to-end harness so an automated reviewer, or you, can drive the
real GUI and get back screenshots, state dumps and a run directory. `docs/TESTING-HARNESS.md` is
the authority for the protocol, the snapshot and the artifacts; this section is how you run it.

```sh
cargo build --workspace
printf 'wait 500\nkey ?\nshot help\nkey escape\nquit\n' > /tmp/help.scenario
./target/debug/fleet-harness run /tmp/help.scenario
```

The checked-in corpus under `scenarios/` runs through `make`:

```sh
make harness            # the whole corpus, virtual lane, with pixels
make harness-headless   # the pixel-free part of the corpus, on a machine with no compositor
make harness-one SCENARIO=scenarios/pointer-basics.scenario
make harness-prune      # delete run directories older than SWEEP_DAYS
```

The runner prints its run directory and the lane on the first two lines, boots a private `fleetd`
and a harness-mode `fleet` against a temporary `FLEET_HOME`, executes the scenario, captures the
screenshots it asked for, and tears everything down — on success, on failure and on `Ctrl-C`.
It exits nonzero when any line fails, naming the line and its response.

| Flag | Effect |
| --- | --- |
| `--lane virtual` | Default when Hyprland answers. Creates its own headless output, moves the window there by title and captures it with `grim`. Your screen is never touched. |
| `--lane headless` | No compositor at all: full layout, state, keyboard and pointer handling, no pixels. `shot` fails with "no pixels in the headless lane". This is the lane a machine with no display can run. |
| `--lane attach` | Your current session and monitor, for watching a run live. Opt-in. |
| `--run-dir <path>` | Put the run directory somewhere other than `/tmp/fleet-harness/<UTC>-<stem>/`. |
| `--keep` | Keep the hermetic `home/` after a passing run. A failed run keeps it regardless. |
| `--continue-on-failure` | Run every remaining line, and every remaining scenario, instead of stopping at the first failure. The run still exits nonzero. |
| `--update-baselines` | Replace the golden images a virtual-lane run compares against. |

A scenario file is UTF-8 and line-oriented; blank lines and `#` comments are ignored. An optional
first `fixture: empty|one-repo|busy|board|agents` line picks the seeded state — omit it and you
get `empty`, a Fleet that has never been run.

| Line | Effect |
| --- | --- |
| `key <keystroke>...` | Dispatches each gpui keystroke to the window (`ctrl-s`, `shift-tab`, `?`, `enter`, `escape`, `j`). Several per line: `key ctrl-s ?`. Reports whether each one was handled. |
| `type <text>` | Dispatches every character as a keystroke, so text inputs and the terminal receive it. Inner spaces are kept. |
| `wait <ms>` | A wall-clock delay. Prefer `await`. |
| `shot <name>` | Settles the window and writes `shots/NNN-<name>.png`. |
| `dump <name>` | Writes the whole `UiSnapshot` to `dumps/NNN-<name>.json` and echoes a one-line summary. |
| `await <predicate> [ms]` | Waits until the predicate holds, default 5000 ms. A timeout reports the clause that failed, the value it saw and all six idle counters. |
| `assert <predicate>` | Evaluates once; a failure fails the run and carries the snapshot. |
| `move`/`click`/`press`/`release`/`drag`/`hover`/`scroll` | Pointer input, by target name or by `x y`. |
| `clipboard set <text>` / `clipboard get` | The application clipboard. Fails in every lane today; see below. |
| `resize <W>x<H>`, `blur`, `focus`, `advance <ms>` | Window and app-clock control. |
| `daemon kill\|stop\|cont\|restart`, `socket remove` | Runner-side fault injection; never app commands. |
| `job success\|failure\|long\|repeat <count>` | Submits a real daemon job of that shape through the run's own client, because the job registry is in memory and no fixture can seed it. Runner-side. |
| `meta` | Run id, lane, logical bounds, scale, title and frame. |
| `quit` | Asks the app to exit. |

Keystrokes go through `Window::dispatch_keystroke`, so they take the same path as real input:
bindings resolve against the focus chain of `docs/KEYMAP.md`.

Three things will bite you before anything else does, so check them first:

- **If the screenshot is not Fleet, it is your compositor, not the harness.** `grim -o <output>`
  photographs everything painted on the isolated output. A locked session paints its lock surface
  on every output including a brand-new headless one, and a shell that attaches a background or a
  bar layer to each output paints those too. The lane checks what it photographed and fails the
  `shot` rather than filing a picture with no Fleet in it, so this reads as a red run with a
  message naming the cause. Unlock the session, or run `--lane headless` and assert on `dump`.
- **The `headless` lane drives the keyboard as well as the pointer.** It did not until the app's
  stale-key queue stopped waiting for a frame callback GPUI's headless platform never delivers;
  `shot` and `clipboard` are the only two directives that lane cannot answer.
- **`clipboard set` / `clipboard get` fail in every lane.** The command reads its own write back
  and the read-back returns nothing, so it fails loudly rather than letting a paste scenario pass
  against an empty clipboard.

The transport is a Unix socket at `FLEET_HARNESS_SOCK`, one compact JSON object per line, one
request answered before the next is sent. The app side lives in `crates/fleet-app/src/drive.rs`
and `crates/fleet-drive/`; the runner is `crates/fleet-harness/`. Harness mode is off unless
`FLEET_HARNESS=1` or `FLEET_HARNESS_SOCK` is set, and then it pins the window to 1440x900
(override with `FLEET_HARNESS_SIZE=WxH`), titles it `Fleet [harness:<run-id>]` so the compositor
can find it, and turns motion off. Nothing above costs a frame in a normal launch.

To drive a Fleet you started yourself, set the socket and speak the protocol directly:

```sh
FLEET_HARNESS=1 FLEET_HARNESS_SOCK=/tmp/fleet.sock ./target/debug/fleet &
printf '{"id":1,"cmd":"key","args":{"keys":["?"]}}\n' | socat - UNIX-CONNECT:/tmp/fleet.sock
```

## Logs

`fleet` installs a `tracing` subscriber at startup that writes to stderr and honours `$RUST_LOG`
(default `info`), so redirecting the process's stderr captures app-side errors and the driver's
actions:

```sh
RUST_LOG=fleet_app=debug ./target/debug/fleet > /tmp/fleet-gui/app.log 2>&1
```

`fleetd` installs the same filter and honours `$RUST_LOG` (default `info`) for both its stderr and
its rotating `$FLEET_HOME/logs/fleetd.log`, so raising the level is how the daemon's `debug`
records — the full argv of a failed shell command, for instance — become visible:

```sh
RUST_LOG=fleet_daemon=debug ./target/debug/fleetd
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
| `fleet-drive` | The GUI-driving seams: the harness command socket (`socket`) and the lazygit file-script driver (`legacy`), each behind its own feature. |
| `fleet-harness` | The end-to-end harness runner: scenarios, lanes, fixtures, faults, reports (`docs/TESTING-HARNESS.md`). |

`docs/ARCHITECTURE.md` describes how they fit together; `docs/decisions/` records why.
