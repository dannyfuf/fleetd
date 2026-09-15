# 0016 — The end-to-end GUI harness

**Adopted** for `crates/fleet-drive/`, `crates/fleet-harness/`, `crates/fleet-app/src/drive.rs`
and the harness seams in `fleet-ui-kit` and `fleet-app`'s state. `docs/TESTING-HARNESS.md` is the
frozen specification; this file records what was chosen and what was rejected.
**Supersedes `0007-gui-smoke-procedure.md`.**

**The transport is a request/response Unix socket, not a polled file.** Fleet listens at
`FLEET_HARNESS_SOCK` and answers one newline-delimited JSON frame per request, correlated by id.
Every command is acknowledged after it has been applied *and* the frame that shows it has painted,
so a runner never sleeps to find out what happened and an error reaches the caller instead of a
log file. Command names are additive-only: a new capability is a new `cmd`, never a new frame
shape.

**The runner captures pixels, not the app.** `shot` settles the window and answers with its
geometry; `fleet-harness` shells out to `grim`. That keeps compositor tooling out of the product
binary and lets the capture backend change without recompiling `fleet`.

**The window lives on an isolated Hyprland headless output.** `hyprctl output create headless
fleet-harness-<run-id>` gives the run a monitor the developer never sees; the window is found by
its unique `Fleet [harness:<run-id>]` title and moved there by address. Teardown removes the
output from three places — the normal path, `Drop`, and a detached watchdog — because a leaked
monitor is a visible bug in someone's session. Hyprland 0.56.2 refuses `hyprctl keyword monitor`
and `windowrulev2` ("keyword can't work with non-legacy parsers"), so geometry is pinned with
`hyprctl eval` and asserted from `hyprctl monitors -j` rather than assumed.

**Input is synthetic, through `Window::dispatch_keystroke`.** It is focus-correct, works with no
compositor at all, and does not race the developer's real session. `wtype`/`ydotool` were
rejected: they would only exercise GPUI's platform layer, and they type into whatever happens to
be focused.

**Structured dumps are the oracle; pixels are evidence.** The app publishes a versioned
`UiSnapshot` built in update paths and memoised behind a key covering every input it reads, so
nothing here can reach a `render` body (`docs/APP-CONTRACTS.md`). Screenshots exist for a human or
an agent to look at, and golden-image comparison is tolerance-based and per-lane.

**Every run is hermetic.** A private `FLEET_HOME`, a child-only `HOME`, fake `gh`/`acli` first on
`PATH` and a private `fleetd` started by the run itself. One fixture implementation serves both
`fleet-harness` and the `fleet-app` integration tests, so "start an isolated fleetd" has one
definition in the workspace.

**Rejected: a nested compositor.** `Xvfb`, `weston` and `cage` are not installed on the
development box and a software Vulkan ICD is not either. The Hyprland headless output needs no
installation and renders on the real GPU. The lane abstraction is the single place to add another
backend, and `--lane headless` keeps working with no compositor at all.

**Rejected: file polling.** `0007`'s driver watched an append-only script every 100 ms and wrote
its results to a log the runner tailed. It cost a timer in every debug launch, it could not report
an error to the caller, and "has the command finished?" was a question only a sleep could answer.
The socket replaces all three, and the polling driver was deleted rather than kept alongside.

**Rejected: in-process GPU readback.** GPUI's renderer has no supported path for it at
`v1.18.1`, and adding one would make the harness a fork of the platform layer.

**Rejected: targeting elements through the accessibility tree.** GPUI does expose
`Window::debug_a11y_tree_json`, which looked like a free, already-maintained map from names to
bounds. It is not usable as an oracle: the tree is only built while accessibility is *active* —
`Window::draw` sends an update only when `a11y.is_active()` holds at both the start and the end of
the frame — and that flag is set by the platform adapter when an assistive-technology client
attaches, with no programmatic override. A harness whose targeting silently returns nothing unless
a screen reader happens to be running is worse than no targeting. The per-element recorder in
`fleet_ui_kit::harness` costs one thread-local branch when harness mode is off and is always
there when it is on. If Fleet ever adopts a11y roles broadly the two efforts reinforce each other,
and the recorder is where the names would come from.

**Consequence for Linux builds.** `gpui_platform` and `gpui_linux` are both declared
`default-features = false` upstream, so a consumer that names no backend gets a binary with no
Wayland or X11 code in it and `gpui::guess_compositor()` answers `"Headless"` on every desktop.
The workspace now asks for `gpui_platform`'s `wayland` and `x11` features explicitly. Without
that, `fleet` cannot open a window on Linux at all — the harness is what made it visible.
