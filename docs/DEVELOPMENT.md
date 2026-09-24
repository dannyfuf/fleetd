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
make build       # build the workspace with the dev profile
make release     # build the workspace with the release profile, into target/release
make check       # cargo check --workspace --all-targets
make test        # build fleetd, then cargo test --workspace against it
make fmt         # cargo fmt --all
make lint        # fmt --check plus Clippy with warnings denied
make doctor      # build Fleet and run its diagnostics
make bootstrap   # install and verify the pinned Zig toolchain
make timings     # build with Cargo's per-crate compile-time report
make build-info  # toolchain, compiler cache and target/ size
make prune       # drop build artifacts unused for SWEEP_DAYS (7) days; runs before every build
make fresh       # cargo clean, then rebuild from scratch (RELEASE=1 supported)
```

`RELEASE=1` switches `build`, `run`, `restart`, `daemon`, `doctor`, `timings` and the harness
targets to the release profile.

## Building

### The two profiles

| | Dev (`make build`, `make run`) | Release (`make release`, `make run-release`) |
| --- | --- | --- |
| Workspace crates | `opt-level = 1`, `debug = "limited"`, incremental | `opt-level = 3`, no debuginfo |
| Dependencies | `opt-level = 0` except a hot list at 3, no debuginfo | `opt-level = 3` |
| Output | `target/debug/` | `target/release/` |
| Cold build, 12-core 16 GB Linux | ~5 min | ~6 min |
| Use it for | everything day to day: editing, tests, the harness | measuring performance, a build you keep running |

The dev profile is Zed's shape (`docs/decisions/0001-gpui-and-toolchain.md`): the ~800 dependencies
compile unoptimized, except the ones GPUI spends its frames in — layout, text shaping and fonts,
SVG, images, the `wgpu` renderer, syntax highlighting, JSON and the proc-macros — which the
`[profile.dev.package]` list in `Cargo.toml` optimizes. A dependency that shows up hot in a profile
of a dev build joins that list; `[profile.dev.package."*"]` does not go back to `opt-level = 3`,
which more than doubled the cold build and pushed a 16 GB machine into swap.

`debug = "limited"` keeps backtraces and function breakpoints but not local variables. For a
debugging session that needs them, `CARGO_PROFILE_DEV_DEBUG=full make build`; switching back
recompiles the workspace crates once.

Only `cargo test` and `make test` use a third build of much of the graph: test-only features
(`tokio/test-util`, `gpui/test-support`) change those crates, so the first test run after a build
compiles them again (~7 min cold). Integration tests are one binary per crate
(`crates/<crate>/tests/integration.rs`), so each links the dependency graph once.

### Measuring

`make timings` builds with `cargo build --timings` and prints the path of the HTML report: a bar
per crate, the critical path and CPU use. It only shows crates that build compiled, so run it after
`make clean` for the cold picture or after an edit for the incremental one. `make build-info`
prints the toolchain, whether a compiler cache is active and its hit rate, and how much of
`target/` is incremental state.

### Sharing dependencies between worktrees

Build artifacts use Cargo's default target directory, `target/` inside this repository. Sharing
that repository-local directory between commands in the same worktree is supported. Do not point
multiple worktrees at one external `CARGO_TARGET_DIR`: Cargo's relative dep-info paths can make
one worktree accept another's stale artifacts. Give parallel worktrees separate target
directories when an override is necessary. The same holds for a shared `build.build-dir`:
workspace crates hash identically in every worktree, so they would overwrite each other.

To stop every new worktree from compiling all its dependencies again, share them through
[sccache](https://github.com/mozilla/sccache) instead. It keys on the content of each `rustc`
invocation and skips incremental builds, so third-party crates are shared and workspace crates
never are. Put a Cargo config in the directory that holds your worktrees (Cargo reads config from
the working directory's ancestors), for example `~/.fleet/worktrees/<owner>/fleetd/.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "/path/to/sccache"
```

It is per machine, not in the repository, and `make build-info` shows whether it is active. Do not
export `CARGO_TARGET_DIR` alongside it: sccache hashes every `CARGO_*` variable, so a per-worktree
value turns every lookup into a miss. Measured, a new worktree's first build drops from ~4m50s to
~4m: about a quarter of dependencies still miss because their build scripts embed the checkout
path, and build scripts themselves are never cached — `libghostty-vt-sys` still runs its ~100 s
`zig build` of Ghostty once per worktree.

### Cleaning up

Cargo never garbage-collects `target/`: dependency bumps, toolchain updates and profile changes
leave stale artifacts behind, and incremental state grows with every edit. From least to most
drastic:

```sh
make prune              # artifacts unused for SWEEP_DAYS (7) days or built by uninstalled toolchains
make clean-incremental  # incremental caches — usually most of target/debug; edits recompile a crate whole once
make clean-release      # target/release only; the dev build is untouched
make clean              # all of target/; the next build is a cold one
make fresh              # clean, then rebuild (RELEASE=1 supported)
make harness-prune      # harness run directories older than SWEEP_DAYS, outside target/
```

`make build` (and therefore `make run`) runs `make prune` first. It uses
[`cargo-sweep`](https://github.com/holmgr/cargo-sweep), installed on first use; anything it removes
is rebuilt on demand. A worktree you are done with takes its `target/` with it when you delete it —
removing the worktree directory is the cleanup.

## Restarting the daemon

`make run` restarts the daemon: it depends on `make restart`, so the freshly built app never talks
to a stale `fleetd`. Restarting is also explicit through `make restart` or:

```sh
fleet daemon restart
```

That command requests a graceful shutdown, falls back to `SIGTERM` when the daemon cannot answer,
waits for its socket to disappear, and starts the newly built sibling `fleetd` binary. Terminals
survive it: each PTY lives in a detached `fleetd pty-hold` process that the next daemon reattaches
to (`docs/ARCHITECTURE.md`, "Detached PTY holders"), so restarting after a daemon change no longer
kills the agents you have running. Only `ctrl-shift-q` stops them.

## Copying from terminal programs

Fleet accepts OSC 52 clipboard writes from the active terminal. This works when the app and daemon
both run on macOS and when the Mac app is connected through its local daemon to a terminal owned by
a remote Linux daemon: in both cases the text reaches the Mac clipboard.

`fleet clipboard copy` is the shell-facing helper. With no text argument it reads stdin; with
`-- "text"` it copies that argument:

```sh
printf '%s' 'text to copy' | fleet clipboard copy
fleet clipboard copy -- "text to copy"
```

The command preserves whitespace and newlines exactly, requires valid UTF-8, and rejects input over
1 MiB without truncating it. It base64-encodes the text as `ESC ] 52 ; c ; <base64> BEL` and writes
directly to `/dev/tty`, not stdout, so programs such as lazygit may capture the subprocess output.
It runs before daemon connection setup and works in any OSC 52-capable terminal, not only Fleet. A
process without a controlling terminal fails clearly.

For nvim, use a copy-only OSC 52 provider. The built-in `osc52` provider's paste waits ten seconds
for a reply, and Fleet deliberately does not answer OSC 52 clipboard queries:

```lua
if vim.env.FLEET_TERMINAL_ID then
  local osc52 = require("vim.ui.clipboard.osc52")
  local cached = { { "" }, "v" }
  local function copy(lines, regtype)
    cached = { vim.deepcopy(lines), regtype }
    osc52.copy("+")(lines)
  end
  local function paste() return vim.deepcopy(cached) end
  vim.g.clipboard = {
    name = "Fleet OSC52",
    copy = { ["+"] = copy, ["*"] = copy },
    paste = { ["+"] = paste, ["*"] = paste },
    cache_enabled = 0,
  }
end
```

For lazygit, put this in `~/.config/lazygit/config.yml`:

```yaml
os: { copyToClipboardCmd: "printf '%s' {{text}} | fleet clipboard copy" }
```

lazygit shell-quotes `{{text}}` itself; do not add quotes around it. These integrations copy out of
a terminal program. To send Mac clipboard content into a program, use Fleet's paste shortcut;
nvim's provider intentionally returns only its in-process copy cache.

## Running tests

Each crate's integration tests are one binary, `integration`, whose modules are the files in
`crates/<crate>/tests/` (`tests/integration.rs` declares them; `autotests = false` in the
manifest). A new test file needs a `mod` line there, and the `workspace_layering` suite fails
until it has one. Select a file by module path:

```sh
cargo test -p fleet-daemon --test integration pty_holder::
```

A test that creates a PTY terminal must end its session: the terminal's child lives in a
`fleetd pty-hold` process that deliberately outlives every daemon, so letting the test process
exit leaves a login shell running. `crates/fleet-daemon/tests/infra` ends the holders of a Fleet
home when its `DaemonProcess` is dropped, as a backstop rather than a substitute.

`make test` and `make ci` build `fleetd` before running workspace tests because app
integration tests launch the ordinary daemon binary. These targets set `FLEET_DAEMON`
to that freshly built workspace binary, overriding inherited paths to other checkouts.
`fleet-daemon`'s own tests ignore `FLEET_DAEMON` and always launch the binary Cargo built
for them, so both halves of a loopback link are the same build.

A daemon starts its PTY holders from its own executable, falling back to `FLEET_DAEMON` only when
that executable is a test harness rather than `fleetd`. Suites that launch the real binary are
therefore unaffected — a spawned `fleetd` resolves itself. The ones that need `FLEET_DAEMON` are
the tests that build a `Sessions` **in process** and create a terminal from it:
`cargo test -p fleet-daemon --lib` on its own cannot start a holder, while `make test` can because
it exports the freshly built binary.
`cargo test` alone builds its test
harness and can leave an older `target/debug/fleetd` in place. Before running app tests
directly, run `cargo build -p fleet-daemon`, then
`FLEET_DAEMON="$PWD/target/debug/fleetd" cargo test -p fleet-app`.

The opt-in delegation smoke tests invoke the real Claude and Codex binaries. On a workstation
where both vendor CLIs are installed and signed in, run:

```sh
cargo test -p fleet-daemon --features real-agents delegation::live
```

They are excluded from the ordinary workspace suite. `make doctor` checks the other half of this
path: a child that cannot reach the `fleet` CLI cannot report its result, and the
`subagent fleet CLI` line says whether it can and what makes that true. When a login shell
resolves the CLI the line reads `subagent fleet CLI ok <resolved-path>`. When a `fleet` sits
beside this `fleetd`, the line also names that directory, which Fleet prepends to a subagent
child's `PATH` — the child inherits it with no configuration of yours. A `fleet` the login shell
cannot resolve is therefore still a pass while the sibling exists; the check fails, and asks you
to fix `PATH`, only when neither is there. The line speaks for that daemon-side fallback alone:
doctor runs with no delegation in flight, so it cannot see the `fleet` path a caller sends with
its own `fleet subagent run`.

Two scripted smokes drive the board's automation end to end against a private daemon — a private
`FLEET_HOME`, a local git origin, scripted agent binaries on a private `PATH`, no network, no real
agent — and both run in `make ci`:

```sh
make smoke-workflow   # scripts/board-workflow-smoke.sh: the workflow preset carries a four-card diamond to Done
make smoke-reviews    # scripts/reviews-smoke.sh: a review card goes Pending review → Reviewing → Reviewed
```

`smoke-reviews` gives its bare-repo origin `refs/pull/1/head` and `refs/pull/2/head` (written with
`git update-ref`), runs `fleet board --context acme --reviews card new "Fixture PR" --pr
<owner>/<name>#1`, and asserts that the card reaches *Reviewed* with a succeeded run in its own
pull-request worktree, and that a second `card new --pr` prints `Existing`. The review column runs
on the Codex shim, so every review is a scripted transcript. A scripted `gh` sits on the private
`PATH` because creating a pull-request worktree runs `gh pr view`: it answers `pr view 1|2`, returns
`[]` for `pr list`, and fails anything else. It then proves a schedule: a fake `claude`, which acts
as the headless agent only when `FLEET_SCHEDULE` is set, creates a card by calling the private
`fleet` by its absolute path (the runner's login-shell environment may not carry the private
`FLEET_HOME`) with `board --board "$FLEET_BOARD" card new … --pr <owner>/<name>#2 --label github`,
and prints a Claude-shaped result line whose text is `SUMMARY: 1 created, 0 existing, 0 reopened`.
The smoke creates the schedule `--every 5 --disabled` so the daemon's own tick cannot race it — `run`
fires a disabled schedule anyway — runs it with `fleet schedule run --wait`, and asserts exactly one
run, succeeded with that summary, a job and a log, and that the card exists with the `github` label
(`docs/BOARD.md` §12). The smokes need `python3` to read the JSON envelopes.

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
| `--run-dir <path>` | Put the run directory somewhere other than `$TMPDIR/fleet-harness/<UTC>-<stem>/` (`/tmp` when `TMPDIR` is unset). |
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
