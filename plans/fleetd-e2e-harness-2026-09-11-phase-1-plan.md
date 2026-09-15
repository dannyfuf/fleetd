# Fleet e2e harness — Phase 1: a driver I can trust — Plan
> Tracker: ./fleetd-e2e-harness-2026-09-11-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Fleet's existing scripted-input driver appends lines to a file that the app polls every 100 ms,
and screenshots it by running the macOS-only `/usr/sbin/screencapture`. Linux is now the only
development platform, so screenshots do not work at all, and nothing the driver does is
acknowledged, so a runner has to sleep and hope.

This phase replaces the transport and the launch story. The driver moves into its own crate and
speaks a request/response protocol over a unix socket, so every command is answered. A new
dev-only runner binary, `fleet-harness`, boots a hermetic `fleetd` and `fleet` against a temporary
`FLEET_HOME`, places the window on an isolated Hyprland headless output that the developer never
sees, executes a scenario file, captures real PNGs with `grim`, and tears everything down. At the
end of this phase the harness cannot yet *assert* anything — that is Phase 2 — but every action it
takes is confirmed, reproducible and isolated.

## Sizing call

**Phased**, and this is phase 1 of 5 — see
[the roadmap](./fleetd-e2e-harness-2026-09-11-roadmap.md). This phase alone is a focused week: a
crate extraction, a protocol change with a doc supersession, a new runner crate, a process fixture,
a compositor lane abstraction and a capture backend. It is not split further because the runner is
useless without the socket, and the socket is untestable without the runner.

## Repository context

- **Project type:** Rust, one Cargo workspace, edition 2024, toolchain pinned to 1.97.1 by
  `rust-toolchain.toml`. Ten crates under `crates/`.
- **Test command:** `make test` (builds `fleet-daemon` first, then
  `FLEET_DAEMON=target/debug/fleetd cargo test --workspace`).
- **Lint command:** `make lint` (= `cargo fmt --all -- --check` plus
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`).
- **No separate type-check step** — `cargo check`/`clippy` cover it. `make check` exists.
- **Existing driver:** `crates/fleet-app/src/drive.rs` (24 lines) delegates to
  `crates/fleet-lazygit/src/drive/support/{mod,protocol,files}.rs` (~880 lines). `Dialect::Fleet`
  and `Dialect::Lazygit` select which commands are accepted and whether lines are acknowledged.
  Spawned from `crates/fleet-app/src/shell/root/bootstrap.rs:91` when `FLEET_DRIVE` is set.
- **Existing line grammar:** `key`, `type`, `wait`, `shot`, `quit` (plus `wheel`/`hwheel` for the
  lazygit dialect). Documented in `docs/DEVELOPMENT.md` §"Driving the app from a script".
- **Existing hermetic-daemon fixture:** `crates/fleet-app/tests/common/mod.rs` — starts a real
  `fleetd` against a `tempfile::TempDir` home, puts a fake `gh` on `PATH`, sets child-only `HOME`,
  cleans up on `Drop`. This is the prototype for the harness fixture, not a new invention.
- **GPUI:** from Zed tag `v1.18.1`, via `gpui` + `gpui_platform`. `gpui_platform` pulls `gpui_linux`
  with its default `wayland` + `x11` features, so the Linux backends are compiled in.
  `gpui::guess_compositor()` returns `"Headless"` when neither `WAYLAND_DISPLAY` nor `DISPLAY` is
  set (or when `ZED_HEADLESS` is set), and the headless window's `draw` discards the scene — full
  layout, text shaping and entity plumbing, no pixels.
- **Window:** opened in `crates/fleet-app/src/shell/root/bootstrap.rs:67` with a fixed
  `WindowOptions`, `TitlebarOptions { title: Some("Fleet") }`.
- **Compositor on the dev box (verified during planning):** Hyprland 0.56.2.
  `hyprctl output create headless <name>` creates an isolated 1920×1080 monitor,
  `grim -o <name> out.png` captures it as a valid PNG, and `hyprctl output remove <name>` restores
  the session. `grim`, `slurp`, `wtype` and `hyprctl` are installed; `Xvfb`, `weston`, `cage` and a
  software Vulkan ICD are not.
- **Docs that constrain this work:** `docs/README.md` assigns each document a domain;
  `docs/DEVELOPMENT.md` owns "building, running, testing and driving the app";
  `docs/decisions/0007-gui-smoke-procedure.md` adopts the driver this phase replaces.

## Assumptions

- Development and harness runs both happen on the Linux dev box. macOS support is retained only
  where it costs nothing (one `screencapture` branch in the capture backend); no macOS-specific
  work is planned, and no macOS lane is tested.
- The developer's Hyprland session is running and reachable when the `virtual` lane is used. When
  it is not, the harness falls back to the `headless` lane and says so, rather than failing.
- The harness is a development tool. It ships in the workspace but is never a dependency of a
  product crate, and `fleet`/`fleetd` gain no new runtime cost when its env vars are unset.
- The scenario *line grammar* (`key`, `type`, `shot`, …) is worth preserving because
  `docs/DEVELOPMENT.md` documents it and it reads well; only the transport changes.

## Out of scope

- Assertions, state dumps and `await` — Phase 2.
- Mouse input and named targets — Phase 3.
- Scripted agents, seeded fixtures and fault injection — Phase 4.
- The scenario corpus, golden images and `make` targets — Phase 5.
- Driving input through the real compositor (`wtype`, `ydotool`). Synthetic `dispatch_keystroke`
  is focus-correct, works headless, and does not race the developer's session. A real-input backend
  would only test the platform layer and is deliberately left out; revisit only if a bug is ever
  traced to GPUI's Wayland input handling.
- Changing `fleet-lazygit`'s own behaviour beyond following the crate move.

## Affected areas

- `crates/fleet-lazygit/src/drive/` — moved out; `fleet-lazygit` becomes a consumer.
- `crates/fleet-drive/` (new) — shared driver: socket server, envelope, line grammar, input
  dispatch, capture invocation.
- `crates/fleet-app/src/drive.rs`, `crates/fleet-app/src/shell/root/bootstrap.rs` — harness mode,
  deterministic window, driver startup.
- `crates/fleet-harness/` (new) — the runner binary, environment fixture, display lanes, capture
  backends, run directory.
- `crates/fleet-app/tests/common/mod.rs` — its `Daemon` fixture moves to `fleet-harness` and the
  test re-uses it.
- `Cargo.toml` (workspace members and `[workspace.dependencies]`), `Makefile`.
- `docs/DEVELOPMENT.md`, `docs/README.md`, `docs/decisions/` (new record superseding 0007).

## Tasks

### P1-T01 — Extract the shared driver into `crates/fleet-drive`
- **Intent:** stop `fleet-app` depending on `fleet-lazygit` for generic input plumbing, and give the
  driver a home that can grow.
- **Touches:** `crates/fleet-lazygit/src/drive/`, `crates/fleet-app/src/drive.rs`,
  `crates/fleet-drive/` (new), root `Cargo.toml`.
- **Steps:**
  - Load the `rust-workspace-architecture` skill first; it owns crate-layering and manifest rules.
  - Create `fleet-drive` with the existing `support` modules moved verbatim, plus its tests.
  - Add it to the workspace members and `[workspace.dependencies]`; have `fleet-app` and
    `fleet-lazygit` depend on it and delete the old module.
  - Keep `Dialect` for now; it is removed in P1-T02 when the protocol unifies.
  - Confirm no product crate gained a dependency edge it should not have.
- **Verification:** `make lint`; `make test`; `cargo tree -p fleet-app -e normal | grep fleet-` shows
  `fleet-drive` and no new edges.
- **Done when:** the driver compiles from its own crate, both consumers use it, and the old path is
  gone.

### P1-T02 — Replace file polling with a request/response unix socket
- **Intent:** make every command answerable, so a runner never has to sleep to find out what
  happened, and errors reach the caller instead of a log line.
- **Touches:** `crates/fleet-drive/src/`, `crates/fleet-app/src/drive.rs`,
  `crates/fleet-app/src/shell/root/bootstrap.rs`.
- **Steps:**
  - Load `rust-async-background-work` and `rust-ipc-protocol`; the envelope and the accept loop are
    governed by them even though this socket is app-local.
  - Define the envelope: newline-delimited JSON, request `{"id":u64,"cmd":String,"args":{…}}`,
    response `{"id":u64,"ok":bool,"data":{…},"error":String|null}`. Document that `cmd` values are
    additive-only.
  - Listen on the path in `FLEET_HARNESS_SOCK`; accept one client at a time; run the listener on the
    background executor and hand each request to the window through a channel, replying with the
    outcome.
  - Port the existing steps as commands: `key`, `type`, `wait`, `shot`, `quit`, `wheel`. Each replies
    after it has actually been applied; `key` reports whether the binding was handled, which the
    current driver only writes to a log.
  - Retire `FLEET_DRIVE` file polling and the `Dialect` split. The *line grammar* survives: the
    runner parses scenario files into commands (P1-T04).
  - Keep the driver task cancelled on window close, as today, and keep its tests.
- **Verification:** `make lint`; `make test`; new tests in `fleet-drive` cover envelope
  round-tripping, an unknown `cmd` producing `ok:false` rather than a panic, and window close
  cancelling an in-flight request.
- **Done when:** a hand-written `socat`/`nc` session against a running harness-mode `fleet` can send
  `{"id":1,"cmd":"key","args":{"keys":["?"]}}` and read back a success response.

### P1-T03 — Harness mode: deterministic window, no animation
- **Intent:** two runs of the same scenario must produce the same layout, so screenshots and later
  assertions are comparable.
- **Touches:** `crates/fleet-app/src/shell/root/bootstrap.rs`, `crates/fleet-app/src/drive.rs`,
  wherever animation durations are read (`fleet-ui-kit` tokens).
- **Steps:**
  - Load `gpui-app-shell` (window and startup) and `gpui-styling` (animation and token access).
  - Gate on `FLEET_HARNESS=1`: fixed logical window bounds (default 1440×900, overridable by
    `FLEET_HARNESS_SIZE=WxH`), a window title of `Fleet [harness:<run-id>]` so the compositor can
    find it, and animations reduced to zero duration.
  - Make the title and bounds readable back over the socket via a `meta` command (returns run id,
    window bounds, scale factor, platform lane) — the capture backend needs the geometry.
  - Ensure nothing in harness mode changes behaviour when the env var is unset: one branch at
    startup, no per-frame cost.
- **Verification:** `make lint`; `make test`; launching with and without `FLEET_HARNESS` shows the
  title and bounds differ only in harness mode; `meta` returns the same bounds twice across restarts.
- **Done when:** two consecutive harness launches report identical window geometry and the app is
  byte-identically laid out for the same scenario prefix.

### P1-T04 — Create `crates/fleet-harness` and the runner CLI skeleton
- **Intent:** one command that an agent can run, which owns the whole lifecycle and leaves a run
  directory behind.
- **Touches:** `crates/fleet-harness/` (new), root `Cargo.toml`, `Makefile`.
- **Steps:**
  - New workspace member, `publish = false`, binary `fleet-harness`, tokio + clap, depending on
    `fleet-drive` for the envelope types only. No product crate depends on it.
  - `fleet-harness run <scenario> [--lane …] [--keep] [--run-dir …]`: parse the scenario file into
    commands using the documented line grammar, connect to the app socket, send each command, record
    request and response.
  - Run directory layout: `run.jsonl` (every request/response with timestamps), `scenario.txt`
    (a copy of what ran), `app.log`, `fleetd.log`, `shots/NNN-<name>.png`, `home/` (the hermetic
    `FLEET_HOME`). Default root `/tmp/fleet-harness/<UTC timestamp>-<scenario stem>/`, printed on
    the first line of output so it can be found.
  - Exit nonzero when any command answers `ok:false` or the app dies; print the failing line, its
    response and the run directory path.
  - Teardown is unconditional: it runs on success, on failure, on a scenario error and on SIGINT.
- **Verification:** `make lint`; `make test`; `fleet-harness run` on a scenario of `wait 100` then
  `quit` exits 0 and leaves a populated run directory; the same with a deliberately bad line exits
  nonzero and names the line.
- **Done when:** the runner owns the lifecycle end to end and never leaves a stray process or
  directory behind unless `--keep` was passed.

### P1-T05 — Hermetic environment fixture
- **Intent:** a run must never read or write the developer's real `~/.fleet`, real repos, or real
  GitHub.
- **Touches:** `crates/fleet-harness/src/env.rs` (new), `crates/fleet-app/tests/common/mod.rs`.
- **Steps:**
  - Move the `Daemon` fixture from `crates/fleet-app/tests/common/mod.rs` into `fleet-harness` as a
    library type and have the app integration tests use it, so there is one implementation of
    "start an isolated fleetd" in the workspace.
  - Extend it for the app: temporary `FLEET_HOME`, child-only `HOME`, a `PATH` with the fake `gh`
    prepended, `FLEET_DAEMON` pointed at the freshly built `fleetd`, `RUST_LOG` configurable, stdout
    and stderr of both processes captured to the run directory.
  - Wait for daemon readiness by probing `daemon_ping`, as the existing fixture does — never sleep.
  - Reap both processes on drop, including on panic; verify the socket file is gone afterwards.
  - Load `rust-gpui-testing` before reworking the shared fixture; it owns fixture and fake
    conventions.
- **Verification:** `make lint`; `make test` (the moved fixture must keep
  `crates/fleet-app/tests/board_flow.rs` and `jobs_panel.rs` green); a run with
  `FLEET_HOME` unset still touches nothing under `~/.fleet` (check with `ls -la ~/.fleet` before and
  after, and by watching the run's `fleetd` argv).
- **Done when:** one implementation of the isolated-daemon fixture serves both the app tests and the
  harness, and a harness run provably leaves the real Fleet home untouched.

### P1-T06 — Display lanes: `headless`, `virtual`, `attach`
- **Intent:** choose where the window lives, without ever stealing the developer's screen by
  accident.
- **Touches:** `crates/fleet-harness/src/lane.rs` (new).
- **Steps:**
  - `headless`: set `ZED_HEADLESS=1` and clear `WAYLAND_DISPLAY`/`DISPLAY`. Full logic, no pixels.
    `shot` must answer `ok:false` with a clear "no pixels in the headless lane" message rather than
    writing an empty file.
  - `virtual` (the default when a Hyprland instance is reachable): create a dedicated output with
    `hyprctl output create headless fleet-harness-<run-id>`, pin its mode and scale to a known value,
    bind the app's window to it by title rule, capture with `grim -o`, and remove the output in
    teardown. Note for the implementer: on this box `hyprctl keyword monitor …` is refused with
    "keyword can't work with non-legacy parsers"; set the mode through `hyprctl output` or a
    `hyprctl --batch` dispatch chain and assert the resulting geometry with `hyprctl monitors -j`
    rather than assuming it took.
  - `attach`: use the developer's current session and monitor, for watching a run live. Opt-in only.
  - Detect the lane automatically (virtual if `hyprctl` answers, else headless) and let `--lane`
    override. Print the chosen lane in the first line of output.
  - Teardown must remove a created output even if the app crashed or the runner was interrupted;
    a leaked virtual monitor is a visible bug in the developer's session.
- **Verification:** `make lint`; `make test`; `hyprctl monitors -j` lists exactly `eDP-1` before and
  after a `virtual` run, including after `kill -INT` mid-run; a `headless` run completes with no
  compositor env set.
- **Done when:** all three lanes run the same scenario, and the virtual lane leaves no trace.

### P1-T07 — Capture backend, Wayland first
- **Intent:** make `shot` produce a real PNG of the Fleet window on Linux.
- **Touches:** `crates/fleet-harness/src/capture.rs` (new), `crates/fleet-drive/` (the `shot`
  command now returns geometry and delegates).
- **Steps:**
  - Decide and record where the capture runs: the *runner* captures, not the app. The app's `shot`
    command raises and settles the window, replies with its geometry and frame number, and the
    runner shells out. This keeps `grim`/`hyprctl` out of the product binary and lets the capture
    backend change without touching `fleet-app`.
  - `virtual` lane: `grim -o fleet-harness-<run-id> <path>` (whole isolated output).
  - `attach` lane: resolve the window rect from `hyprctl clients -j` by matching the harness title,
    then `grim -g "<x>,<y> <w>x<h>"`.
  - Retain a minimal macOS branch (`screencapture -x -R`) so the code does not become
    Linux-only by accident, but do not build a macOS lane.
  - Fail loudly and specifically when a tool is missing ("grim not found; install grim or use
    --lane headless"), never silently.
  - Name shots `shots/<NNN>-<name>.png` by command sequence so the order is obvious, and return the
    written path in the response.
- **Verification:** `make lint`; `make test`; a `virtual` run of `wait 1000` + `shot hub` produces a
  PNG whose dimensions match the reported output size and which visibly shows the Fleet hub
  (open it and look); a `headless` run of the same scenario fails that line with the expected
  message.
- **Done when:** a screenshot of the real app lands in the run directory on this Linux box.

### P1-T08 — Documentation: replace the smoke procedure
- **Intent:** `docs/` is authoritative, and it currently documents a driver that no longer exists.
- **Touches:** `docs/DEVELOPMENT.md`, `docs/README.md`, `docs/decisions/0007-gui-smoke-procedure.md`,
  `docs/decisions/0016-e2e-harness.md` (new), `docs/TESTING-HARNESS.md` (new, stub).
- **Steps:**
  - Rewrite `docs/DEVELOPMENT.md` §"Driving the app from a script" to describe the socket, the
    runner, the lanes and the run directory; keep the line-grammar table, updated.
  - Add decision record `0016 — The end-to-end harness`, stating what was adopted (socket transport,
    runner-side capture, Hyprland headless output, synthetic input) and what was rejected and why
    (file polling, `wtype`/`ydotool` real input, in-process GPU readback, a nested compositor
    install). Mark `0007` superseded by it, in the same commit.
  - Add `docs/TESTING-HARNESS.md` as the future authority for the harness, with a row in
    `docs/README.md`'s table. In this phase it documents the envelope, the grammar, the lanes and
    the run directory; later phases extend it.
  - Load `zed-quality-review` before calling the phase done.
- **Verification:** `make lint`; `make test`; every command in the rewritten docs is copy-pasteable
  and was actually run; `grep -rn FLEET_DRIVE docs/ crates/` returns nothing stale.
- **Done when:** a reader who follows `docs/DEVELOPMENT.md` gets a working screenshot on the first
  try, and no document describes the retired driver.

## Verification

```sh
make lint     # cargo fmt --all -- --check + cargo clippy --workspace --all-targets --all-features -- -D warnings
make test     # builds fleetd, then FLEET_DAEMON=… cargo test --workspace
make restart  # after any daemon-side change, so the running fleetd matches the build
```

Plus the phase's own acceptance run, from a clean tree:

```sh
cargo build --workspace
printf 'wait 500\nkey ?\nshot help\nkey escape\nquit\n' > /tmp/help.scenario
./target/debug/fleet-harness run /tmp/help.scenario
```

It must print the run directory and the chosen lane, exit 0, leave `shots/002-help.png` showing the
help overlay, leave `run.jsonl` with one response per command, and leave `hyprctl monitors -j`
reporting only `eDP-1`.

## Definition of done

- [ ] Every task in the tracker is checked off, with its verification output recorded.
- [ ] `make lint` is clean.
- [ ] `make test` passes on a clean tree.
- [ ] The acceptance run above succeeds and its screenshot has been opened and looked at.
- [ ] No leaked Hyprland output, process, socket or temp directory after a run, including after an
      interrupted run.
- [ ] `docs/DEVELOPMENT.md`, `docs/README.md`, decision `0007` (superseded) and the new `0016` and
      `docs/TESTING-HARNESS.md` all match the code, updated in the commits that changed it.
- [ ] The tracker reflects reality, including anything that was done differently from this plan.
- [ ] Follow-ups discovered mid-flight are recorded in the tracker's Follow-ups section.

## Risks and rollback

- **The crate extraction ripples.** `fleet-app` depending on `fleet-lazygit` for the driver is
  load-bearing today. Rollback: the move is a single commit; revert it and the driver still works
  from its old home while the socket work continues in `fleet-lazygit`.
- **Retiring `FLEET_DRIVE` breaks someone's muscle memory.** Mitigated by preserving the line
  grammar and by rewriting the docs in the same commit. Rollback: re-add the file-polling path
  behind the old env var; it is ~80 lines.
- **Hyprland's output/keyword API differs from what planning observed.** Verified working at 0.56.2;
  if a future version changes it, the lane abstraction is the one place to fix, and `--lane headless`
  and `--lane attach` both keep working meanwhile.
- **The window refuses to open on the virtual output.** Fall back to `attach` with a warning rather
  than failing the run, and record the fallback in `run.jsonl` so a screenshot taken from the
  developer's real screen is never mistaken for an isolated one.
- **A harness-mode branch changes production behaviour.** Keep the gate to startup and to one
  already-loaded flag; `zed-quality-review` at the end of the phase is where this gets checked.
