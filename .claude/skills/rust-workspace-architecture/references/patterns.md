# Workspace architecture patterns — long form

Zed citations are `zed/crates/<crate>/src/<file>.rs:<line>` at tag **v1.18.1**, the GPUI
tag fleetd pins (`Cargo.toml`). Checkout used:
`/Users/danny/.swarm/repos/zed-industries/zed`. fleetd paths are relative to
`/Users/danny/.swarm/worktrees/dannyfuf/fleetd/chore-skills` and were verified in this
worktree.

Scale context: Zed is 250 workspace members, ~1.57M lines. fleetd is 10 crates, 646 `.rs`
files, 228k lines. Patterns that exist *because* Zed has 243 crates (`[lib] path`,
`script/new-crate`, `xtask`) are marked as such; do not import them wholesale.

---

## P1 — Workspace manifest: one version table, inherited everywhere {#p1-workspace-manifest}

**Zed.** 502 entries in `[workspace.dependencies]` (`zed/Cargo.toml:271-963`), including
path deps for workspace members, so no crate ever writes `{ path = "../foo" }`. Crate
manifests only say `anyhow.workspace = true`.

```toml
# zed/Cargo.toml:266-283 (trimmed; the gpui entry itself is at :357)
[workspace.package]
publish = false
edition = "2024"

[workspace.dependencies]
acp_thread = { path = "crates/acp_thread" }
gpui = { path = "crates/gpui", default-features = false }
```

Enforcement is mechanical, not social —
`zed/tooling/xtask/src/tasks/package_conformity.rs:25-56` walks `cargo metadata`, skips
`extensions/*` and `zed_extension_api` (they must publish standalone), and reports every
dependency that is not `Dependency::Inherited(_)` plus every crate whose manifest lacks
`[lints] workspace = true`.

`default-features = false` is **not** blanket policy in Zed: 21 of 502 entries set it,
where default features are genuinely costly (`gpui`, `gpui_platform`, `ashpd`, …).

**fleetd.** Already conformant, and better proportionally: `Cargo.toml` has 44
`[workspace.dependencies]` entries, zero crate-level version literals, and 5 entries set
`default-features = false` (`gpui`, `reqwest`, `syntect`, `tokio`, `two-face`). All 10
manifests carry `[lints] workspace = true` and inherit
`edition.workspace`/`rust-version.workspace`/`publish.workspace`.

**The gap.** Nothing enforces it. Add to `crates/fleet-app/tests/workspace_layering.rs`:

```rust
#[test]
fn every_crate_inherits_workspace_lints_and_deps() {
    for manifest in glob("crates/*/Cargo.toml") {
        let toml = parse(&manifest);
        assert!(toml.lints_workspace(), "{manifest} is missing `[lints] workspace = true`");
        for (name, dep) in toml.all_dependencies() {
            assert!(dep.is_inherited(), "{manifest}: `{name}` is not `.workspace = true`");
        }
    }
}
```

**Do not copy:** Zed's `[lib] path = "src/<crate>.rs"` rule (`zed/.rules:15`, 228/243
crates). fleetd uses `src/lib.rs` in all 10 crates and the names are unambiguous; the rule
pays off at 243 crates, not at 10.

---

## P2 — Workspace lints: deny hazards, allow style {#p2-lints}

```toml
# zed/Cargo.toml:1073-1096 (trimmed)
[workspace.lints.rust]
unexpected_cfgs = { level = "allow" }

[workspace.lints.clippy]
dbg_macro = "deny"
todo = "deny"
declare_interior_mutable_const = "deny"
redundant_clone = "deny"
disallowed_methods = "deny"

# We currently do not restrict any style rules
# as it slows down shipping code to Zed.
style = { level = "allow", priority = -1 }
type_complexity = "allow"
too_many_arguments = "allow"     # "in Rust it can be very tedious to reduce argument
                                 #  count without running afoul of the borrow checker."
```

The verbatim comment is the rule: style lints cost minutes of CI per push and produce
churn, so only lints that catch *bugs or leftovers* are denied, and every `allow` carries
a one-line justification.

**fleetd** (`Cargo.toml`) is the same shape with a better comment, and adds
`unimplemented = "deny"`:

```toml
# Hazards only. Clippy's default `style`, `complexity`, `perf` and `correctness`
# groups stay at warn and are enforced by `-D warnings` in `make clippy`, so this
# list never grants an allowance — it only raises what a warning would let slip.
[workspace.lints.clippy]
dbg_macro = "deny"
declare_interior_mutable_const = "deny"
disallowed_methods = "deny"
todo = "deny"
unimplemented = "deny"
# `redundant_clone` belongs here too, but the app and lazygit render paths still
# trip it. Add it once `cargo clippy … -W clippy::redundant_clone` is quiet.
```

Two gaps: there is **no `[workspace.lints.rust]` section** (add one, even empty of denies,
so future `rust` lints have a home), and `redundant_clone` is still deferred —
`crates/fleet-app/src/dialogs/palette.rs` alone carries 42 `.clone()`.

**`clippy.toml` as an architectural firewall.** Zed encodes real invariants, each with a
`reason` and a `replacement` (`zed/clippy.toml:8-18`): `std::process::Command::spawn` →
`smol::process::Command::spawn`, `smol::Timer::after` → `gpui::BackgroundExecutor::timer`
(non-determinism in tests), `serde_json::from_reader` → `from_slice`. Build scripts are
exempted at file scope with a `reason`.

fleetd's `clippy.toml` bans exactly one method (`serde_json::from_reader`) and explains
why the process/timer bans are *not* workspace-wide: "Fleet uses blocking process APIs on
dedicated Tokio/PTY threads. Process and timer restrictions belong at those ownership
boundaries." That reasoning is correct for fleetd — do not import Zed's list.

**When NOT to add an entry.** Never for a taste preference. Each existing entry is "this
API silently breaks correctness, determinism, or performance".

---

## P3 — Crate anatomy: named lib root, no `mod.rs` {#p3-anatomy}

**Zed [RULE]** `zed/.rules:14-15`:

> Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of
> `src/some_module/mod.rs`.
> When creating new crates, prefer specifying the library root path in `Cargo.toml` using
> `[lib] path = "...rs"` instead of the default `lib.rs` …

Measured: 228/243 crates set `[lib] path`; **6 `mod.rs` files in 1861**. The lib root is a
module list plus glob re-exports and nothing else (`zed/crates/gpui/src/gpui.rs:1-6,
92-100`).

**fleetd.** 0/10 set `[lib] path` — skip that, it is a 243-crate optimisation. But
**31 `mod.rs` files exist**, and the sibling convention is already in use right beside
them: `crates/fleet-app/src/dialogs/` contains both `settings.rs` + `settings/` and
`mod.rs`. Rename opportunistically when already editing a module; a rename-only PR is
churn (`zed/CONTRIBUTING.md:83` rejects exactly that).

Every fleetd `lib.rs` already opens with a one-sentence `//!` — keep that; it is the part
that matters.

---

## P4 — File and function size {#p4-size}

Zed, over 1732 non-test `.rs` files: median file 296 lines, p90 1761, 20% over 1000, and
`zed/crates/workspace/src/workspace.rs` is 18,879 lines. Over 32,880 function bodies:
median **7** lines, p75 18, p90 43, p95 68, p99 161; 8.0% over 50 lines, 2.5% over 100.

**[RULE]** `zed/.rules:5`: *"Prefer implementing functionality in existing files unless it
is a new logical component. Avoid creating many small files."*

The pairing is the discipline: Zed accepts 13k-line files and almost never a 200-line
function. **A reviewer pushes back on function length, not on file length, and never on
"this module should be split" for its own sake.**

**fleetd** is better on both axes: files median 263, p90 753, 6 over 2000 (two of them
test files); functions median 8, p90 31, p99 95, 4.4% over 50 lines.

**Where fleetd's file sizes *are* a real finding** — mixed concerns, not length:

| File | Lines | What it mixes |
| --- | ---: | --- |
| `crates/fleet-app/src/dialogs/palette.rs` | 2046 | Draft + ~40-variant `Command` enum + candidate building + render + dispatch + tests |
| `crates/fleet-app/src/dialogs/create_worktree.rs` | 1750 | Draft, validation, debounced base-ref lookup, render, submit, tests |
| `crates/fleet-app/src/screens/agent_thread/mod.rs` | 1312 | ~25-field view struct + event handling + render + gate editing + picker |

Cohesive-and-large is fine and needs no action:
`crates/fleet-core/src/agents/projection.rs` (2491, one pure reducer),
`crates/fleet-daemon/src/services/agents/providers/claude/map.rs` (2349, one provider
mapping), `crates/fleet-app/src/keymap.rs` (1081 — it *is* one table).

**The split shape fleetd already uses** (verified directories):

```
crates/fleet-app/src/dialogs/settings.rs        + settings/{draft,persistence,schema,tests,view}.rs
crates/fleet-app/src/dialogs/board_settings.rs  + board_settings/{draft,persistence,schema,tests,view}.rs
crates/fleet-app/src/dialogs/card_detail.rs     + card_detail/{actions,draft,lifecycle,tests,view}.rs
```

`palette.rs` and `create_worktree.rs` predate that convention. Split them into the same
shape while landing a change that already touches them.

---

## P5 — Model crate vs view crate {#p5-model-view}

Zed's measured pairs (`[dependencies]` of each):

| crate | has `gpui` | `ui` | `workspace` | `editor` | `project` |
| --- | --- | --- | --- | --- | --- |
| `terminal` | yes | – | – | – | – |
| `terminal_view` | yes | yes | yes | yes | yes |
| `project` | yes | – | – | – | – |
| `project_panel` | yes | yes | yes | yes | yes |
| `git` | yes | – | – | – | – |
| `git_ui` | yes | yes | yes | yes | yes |

**The rule:** a model crate *may* depend on `gpui` (it needs `Entity`, `Task`,
`EventEmitter`, `Context`) and on `settings`/`theme`; it must **not** depend on `ui`,
`workspace`, or `editor`. The boundary is one layer higher than "no gpui in models".

Why: (a) `ui`/`workspace`/`editor` are the slowest crates to compile, so a model change
does not rebuild them; (b) model crates are testable with `TestAppContext` and no window;
(c) two consumers can share a model (`terminal` is used by `terminal_view` *and* `agent`).

**Zed is inconsistent here** — `agent` depends on `ui` while `agent_ui` exists; `picker`
depends on `ui` + `workspace`. When Zed is inconsistent, copy the older, denser crates
(`terminal`, `project`, `git`).

**fleetd matches, and enforces it harder.** `fleet-git` (plumbing) vs `fleet-lazygit`
(UI) mirrors `git`/`git_ui` exactly; `fleet-core` and `fleet-proto` are I/O-free;
`fleet-ui-kit` goes further than Zed's `ui` by having *zero* domain dependencies, enforced
by the manifest rather than by convention. `docs/ARCHITECTURE.md:57-58` states the rule in
prose. Keep it; make it a test (P6).

---

## P6 — Enforced acyclicity *and* enforced shallowness {#p6-acyclicity}

Zed has zero cycles across 243 crates, and additionally forbids edges that are legal but
hurt build parallelism, as a unit test run in CI
(`zed/.github/workflows/run_tests.yml` → `cargo test --package xtask -- workspace::`):

```rust
// zed/tooling/xtask/src/workspace.rs:19-35
/// Crates that must not depend on each other, directly or transitively:
/// such edges chain large UI crates one after another and serialize the
/// build, badly hurting incremental compile times. Dev-dependencies are
/// exempt since they don't affect `cargo build`.
const FORBIDDEN_DEPENDENCIES: &[(&str, &str)] = &[
    ("agent_ui", "git_ui"),   ("file_finder", "project_panel"),
    ("git_ui", "agent_ui"),   ("git_ui", "search"),
    ("picker", "editor"),     ("project_panel", "git_ui"),
    ("search", "git_ui"),     ("sidebar", "git_ui"),
    ("title_bar", "git_ui"),  // …
];
```

The assertion message is the guidance (`zed/tooling/xtask/src/workspace.rs:65-70`):

> forbidden dependency paths between sibling feature crates; break the dependency (e.g. by
> extracting shared code into a lower-level crate) instead of joining these crates into one
> serial build chain

**fleetd has no equivalent.** Its graph is acyclic today and the rule is written down
(`docs/ARCHITECTURE.md:57-58`), but a review habit is not enforcement. The port, using
`cargo metadata` (fleetd has no `cargo_metadata` dep; shell out and parse with
`serde_json`, which `fleet-app` already depends on):

```rust
// crates/fleet-app/tests/workspace_layering.rs
use std::collections::{BTreeMap, BTreeSet};

/// Pairs that must not be connected, directly or transitively, through `[dependencies]`.
/// Dev-dependencies are exempt: tests may cross boundaries.
/// Source of truth: docs/ARCHITECTURE.md:57-58.
const FORBIDDEN: &[(&str, &str)] = &[
    ("fleet-lazygit", "fleet-core"),   // lazygit never speaks the domain
    ("fleet-lazygit", "fleet-proto"),  // …nor the wire protocol
    ("fleet-ui-kit", "fleet-core"),    // the design system has no domain types
    ("fleet-ui-kit", "fleet-proto"),
    ("fleet-core", "fleet-proto"),     // core is the leaf; proto sits above it
    ("fleet-git", "fleet-core"),       // git plumbing is a leaf too
    ("fleet-daemon", "fleet-app"),     // the daemon never names a UI crate
    ("fleet-daemon", "fleet-ui-kit"),
    ("fleet-core", "fleet-ui-kit"),
];

fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("cargo metadata");
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| {
            let name = package["name"].as_str().expect("name").to_owned();
            let deps = package["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .filter(|dep| dep["kind"].is_null()) // normal deps only
                .filter_map(|dep| dep["name"].as_str())
                .filter(|dep| dep.starts_with("fleet-"))
                .map(str::to_owned)
                .collect();
            (name, deps)
        })
        .collect()
}

fn dependency_path(
    graph: &BTreeMap<String, BTreeSet<String>>,
    from: &str,
    to: &str,
) -> Option<Vec<String>> {
    let mut stack = vec![vec![from.to_owned()]];
    let mut seen = BTreeSet::new();
    while let Some(path) = stack.pop() {
        let head = path.last().expect("non-empty path").clone();
        if head == to {
            return Some(path);
        }
        if !seen.insert(head.clone()) {
            continue;
        }
        for next in graph.get(&head).into_iter().flatten() {
            let mut extended = path.clone();
            extended.push(next.clone());
            stack.push(extended);
        }
    }
    None
}

#[test]
fn layering_rules_hold() {
    let graph = workspace_graph();
    let violations: Vec<String> = FORBIDDEN
        .iter()
        .filter_map(|&(from, to)| dependency_path(&graph, from, to))
        .map(|path| path.join(" -> "))
        .collect();
    assert_eq!(
        violations,
        Vec::<String>::new(),
        "forbidden dependency path between fleetd crates; extract the shared type into a \
         lower crate instead of adding the edge (docs/ARCHITECTURE.md:57-58)",
    );
}
```

When a PR establishes a new prohibition, the pair goes in the list in the same PR.

`fleet-app` is the right home for it because it sits at the top of the graph and already
has a `tests/` directory. If the workspace grows past ~15 crates, move it to an `xtask`
crate as Zed does.

---

## P7 — `pub fn init(cx: &mut App)` per crate {#p7-init}

Zed: **156 `pub fn init(`** across 153 files, plus 37 `init_*` variants. Signature is
almost always `pub fn init(cx: &mut App)`; state is passed explicitly when needed.
`zed/crates/zed/src/main.rs` is the *only* ordering authority (~100 calls).

```rust
// zed/crates/terminal_view/src/terminal_view.rs:107-116
pub fn init(cx: &mut App) {
    terminal_panel::init(cx);                       // 1. delegate to submodules
    register_serializable_item::<TerminalView>(cx); // 2. register into a global registry
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(TerminalView::deploy);   // 3. graft onto future entities
    })
    .detach();
}
```

An `init` does those three things and nothing else. Startup order is greppable in one
file; every crate is inert until initialised, so a test can init only what it needs.

**When NOT to.** No fallible or blocking work in `init` — it runs before the first frame.
Long work goes into `cx.background_spawn(...).detach()`.

**fleetd.** 3 `pub fn init(` total. `fleet-app` wires modules explicitly from
`crates/fleet-app/src/shell/root/bootstrap.rs`:

```rust
// crates/fleet-app/src/shell/root/bootstrap.rs (trimmed)
gpui_platform::application().with_assets(KitAssets).run(move |cx: &mut App| {
    Theme::init(ThemeMode::Dark, cx);
    keymap::init(cx);
    fleet_lazygit::keymap::init(cx);   // the embedded git pane brings its own key table
    // …
});
```

That is already Zed's shape at fleetd's scale — one ordering authority, one `init` per
thing that owns global state. **[JUDGE]** Adopting `init(cx)` more broadly is low
priority at 10 crates; adopt it only when it removes a dependency edge.

---

## P8 — Registries instead of upward dependencies {#p8-registries}

Three mechanisms, in Zed's order of preference.

**(a) `Global` registry + a free `register_*` function in the *low* crate** (113
`impl Global for`):

```rust
// zed/crates/workspace/src/workspace.rs:960-967
impl Global for ProjectItemRegistry {}

/// Registers a [ProjectItem] for the app. When opening a file, all the registered
/// items will get a chance to open the file, starting from the project item that
/// was added last.
pub fn register_project_item<I: ProjectItem>(cx: &mut App) {
    cx.default_global::<ProjectItemRegistry>().register::<I>();
}
```

The registry stores *function pointers* built from the generic parameter, type-erasing the
concrete type so the low crate never names it.

**(b) `cx.observe_new(|entity: &mut T, …|)`** — 100 sites — lets a high crate attach
actions and subscriptions to every *future* instance of a low-crate entity. This is how
~648 `register_action` calls reach `Workspace` without `workspace` depending on anything.

**(c) `inventory::submit!`** — 20 sites, for link-time registration where there is no init
hook (`zed/crates/db/src/db.rs:276-284`, `zed/crates/component/src/component.rs:25-29` is the `inventory::iter` consumer).

**fleetd.** 4 `impl Global`, **0 `observe_new`**. The one registry-shaped thing is
`DialogRegistry` in `crates/fleet-app/src/dialogs/host.rs`, keyed by the `EntityId` of the
`AppState` entity with `observe_release` cleanup. At 10 crates, explicit wiring is
defensible and greppable — this is not a gap to close for its own sake. Revisit if
`fleet-app`'s `shell/` starts having to name every screen and dialog, which is exactly
what the missing `Dialog` trait (§P13) would fix.

---

## P9 — Extracting a vocabulary crate to break a cycle {#p9-vocabulary}

When two crates need to name the same thing, the name moves *down* into a tiny crate.
Zed's `zed_actions` (994 lines; deps: `gpui`, `schemars`, `serde`) holds every action that
crosses crate boundaries, so `git_ui` can dispatch an action `agent_ui` handles without
either depending on the other. 42 crates depend on it, and it carries a real footgun
inline:

```rust
// zed/crates/zed_actions/src/lib.rs:6-13
// If the zed binary doesn't use anything in this crate, it will be optimized away
// and the actions won't initialize. So we just provide an empty initialization function
// to be called from main.
pub fn init() {}
```

Same shape: `command_palette_hooks` (lets crates filter the palette without
`command_palette` depending on them), `collections` (so no crate picks its own hash type),
`menu`, `paths`, `release_channel`. 37 Zed crates are pure leaves.

**fleetd.** 50 `actions!` invocations, all crate-local (`crates/fleet-app/src/actions.rs`,
`crates/fleet-lazygit/src/actions.rs`). No cross-crate action dispatch exists, so no
vocabulary crate is needed. ADR 0004 already handles the one collision risk by *renaming*
the embedded lazygit's key-context words (`LgDialog`, `LgConfirm`, `LgHelp`) rather than
sharing a namespace. If `fleet-app` and `fleet-lazygit` ever need to dispatch into each
other, extract `fleet-actions` — do not add the edge, and do not merge the crates.

---

## P10 — Actions: namespace + doc comment {#p10-actions}

Zed: 184 `actions!(…)`; the first argument is the user-visible namespace and becomes the
keymap string (`editor::MoveDown`).

```rust
// zed/crates/editor/src/actions.rs:409-413
actions!(
    editor,
    [
        /// Accepts the full edit prediction.
        AcceptEditPrediction,
```

**[RULE]** `zed/.rules:121`: *"Doc comments on actions are displayed to the user."* — the
`///` on an action is product copy, not developer notes.

**fleetd matches and is stricter.** 25 namespaces in `crates/fleet-app/src/actions.rs`,
one per key context, each variant carrying a doc comment that names its keystroke, and the
file header states the rule: "There is exactly **one action per row of
`docs/KEYMAP.md`**". `crates/fleet-app/src/keymap.rs` logs `tracing::error!` instead of
panicking on an invalid built-in binding. Nothing to change; see the `gpui-app-shell`
skill for the keymap side.

---

## P12 — Error handling: `anyhow` by default, `thiserror` at typed boundaries {#p12-errors}

Zed counts: 165/243 crates depend on `anyhow`, 23 on `thiserror`; 1905 `.context(`, 600
`.with_context(`, 707 `bail!(`, 248 `ensure!(`, 1536 `.log_err()`, 428
`detach_and_log_err`, 80 `debug_panic!`.

`ResultExt` is the sanctioned "swallow this but see it" tool, and its own doc warns
against the loud variant:

```rust
// zed/crates/gpui_util/src/lib.rs:210-227
pub trait ResultExt<E> {
    type Ok;
    fn log_err(self) -> Option<Self::Ok>;
    /// Like [`ResultExt::log_err`], but uses `{:?}` formatting so `anyhow::Error` values emit their
    /// full backtrace. Reach for this only when a backtrace is genuinely wanted — most call sites
    /// should stick with `log_err` / `warn_on_err`, whose output is a single chained error message.
    fn log_err_with_backtrace(self) -> Option<Self::Ok> where E: std::fmt::Debug;
    /// Assert that this result should never be an error in development or tests.
    fn debug_assert_ok(self, reason: &str) -> Self;
    fn warn_on_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: log::Level) -> Option<Self::Ok>;
    fn anyhow(self) -> anyhow::Result<Self::Ok> where E: Into<anyhow::Error>;
}
```

**[RULE]** `zed/.rules:8-12`, verbatim:

> Never silently discard errors with `let _ =` on fallible operations. Always handle errors
> appropriately:
> - Propagate errors with `?` when the calling function should handle them
> - Use `.log_err()` or similar when you need to ignore errors but want visibility
> - Use explicit error handling with `match` or `if let Err(...)` when you need custom logic
> - Example: avoid `let _ = client.request(...).await?;` - use `client.request(...).await?;`

**When NOT to use `thiserror`.** Only when a *caller must branch on the variant* — Zed's
23 are protocol/parsing crates (`rpc`, `terminal`, `lsp`-adjacent).

**fleetd.**

| Crate | thiserror | anyhow |
| --- | --- | --- |
| `fleet-core`, `fleet-git`, `fleet-proto`, `fleet-term`, `fleet-client` | yes | – |
| `fleet-daemon` | yes (`DaemonError`) | yes (at `main()` and orchestration glue) |
| `fleet-app`, `fleet-lazygit`, `fleet-cli` | – | yes |
| `fleet-ui-kit` | – | – |

That split is right and matches Zed's intent better than Zed's own numbers do. Three real
gaps:

1. **Context density.** ~60 `.context`/`.with_context` and only 4 `bail!` across 228k
   lines. Most `?` sites lose their story. Add `.context("…")` at every I/O and subprocess
   boundary as you touch it.
2. **139 `let _ =` sites.** Each is either a legitimate fire-and-forget channel send —
   which needs a one-line comment saying so — or a swallowed error, which needs handling.
3. **No `ResultExt`, no `debug_panic!`.** Add a small one in `fleet-core` over `tracing`:

```rust
// crates/fleet-core/src/result.rs — the sanctioned alternative to `let _ =`
/// Extension for observing an error without propagating it.
pub trait ResultExt<T, E> {
    /// Logs the error at `error` level and returns `None`.
    fn log_err(self) -> Option<T>;
    /// Logs the error at `warn` level and returns `None`. Use for recovered anomalies.
    fn warn_on_err(self) -> Option<T>;
}

impl<T, E: std::fmt::Display> ResultExt<T, E> for Result<T, E> {
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::error!(%error);
                None
            }
        }
    }

    fn warn_on_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(%error);
                None
            }
        }
    }
}
```

**Panic policy — the bar fleetd already holds.** Effective production `unwrap()` count
across the workspace: **0** (the 3 matches are in test bodies in `crates/fleet-git`). The
surviving `expect()` calls are static invariants with explanatory messages
(`LazyLock` regexes, `HostId::try_from("local-daemon").expect("static host id is valid")`).
`todo!`, `unimplemented!` and `dbg!` are denied at the workspace level and appear nowhere;
`docs/REMOTE-MACHINES.md:12-13` states "production paths never use `todo!` or
`unimplemented!`". Tests prefer `unwrap_or_else(|error| panic!("{error}"))`; lock poisoning
is *handled* (`unwrap_or_else(std::sync::PoisonError::into_inner)` in
`crates/fleet-term/src/host.rs`, `crates/fleet-git/src/watch.rs`,
`crates/fleet-client/src/terminal.rs`). Zed does not achieve this. **Defend it; do not
preach it.**

---

## P13 — Extension traits as the extension mechanism {#p13-traits}

Zed impl counts: `Render` 397, `EventEmitter` 279, `Focusable` 182, `RenderOnce` 121,
`Global` 113, `Item` 56, `ModalView` 56, `PickerDelegate` 51, `SerializableItem` 11,
`ProjectItem` 9, `Panel` 9.

`PickerDelegate` is the model for "one generic view, N behaviours": an associated type for
the row element, required methods for the minimum contract, defaulted methods for
everything optional, and a doc comment explaining a non-obvious constraint:

```rust
// zed/crates/picker/src/picker.rs:164-174
pub trait PickerDelegate: Sized + 'static {
    type ListItem: IntoElement;

    /// Name of the picker, this is the key for serialization. We could use the
    /// typename of the delegate but then a rename would break persistence.
    fn name() -> &'static str;
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn separators_after_indices(&self) -> Vec<usize> { Vec::new() }
```

Type erasure is `dyn`-based, not generic-explosion-based.

**fleetd's good example** is the daemon's ports: `#[async_trait]` is reserved for adapter
traits (`Files`, `Shell`, `Git`, `Github`, `Process`, `Clock`, `BoardBackend`,
`MachineProvider`, `RemoteEndpoint`, `AgentProvider`), while services are plain `Clone`
structs with inherent `pub async fn`s. Adapter tests assert exact argv; service tests
assert domain results. `VtEngine` in `fleet-term` is the other one
(`docs/ARCHITECTURE.md:19`). Keep naming new seams as traits in the *lower* crate.

**fleetd's missing seam — the `Dialog` trait.** `Dialogs::` is matched at **140 sites** in
`crates/fleet-app/src`; the enum has 16 variants
(`crates/fleet-app/src/dialogs/mod.rs`), each mapped by hand to a key-context word
(`context_name`), a card width (`width`), a `seed` arm and a `render` arm. Every dialog
module independently defines free functions named `seed` (13), `render` (21), `submit`
(5), `close` (3), `move_cursor` (4) with near-identical signatures
(`fn seed(state: &Entity<AppState>, cx: &mut App)`). Adding one dialog means editing
`Dialogs`, `context_name`, `width`, both matches, `DialogHost`'s field list, `actions.rs`
and `keymap.rs`.

The shape to move toward, one dialog at a time:

The trait itself is `gpui-app-shell`'s to define — this skill only owns the *seam* argument.
Use that shape verbatim; do not invent a second one:

```rust
// crates/fleet-app/src/dialogs/dialog.rs (proposed)
pub(crate) enum DismissDecision { Dismiss, Keep }

/// One modal surface. The host owns the draft; the impl owns behaviour.
pub(crate) trait Dialog: 'static {
    /// The key-context word `keymap.rs` binds against. Globally unique.
    const CONTEXT: &'static str;
    /// The draft this dialog owns inside `DialogHost`.
    type Draft: Default + 'static;

    fn draft(host: &mut DialogHost) -> &mut Self::Draft;
    fn width(cx: &App) -> Pixels;
    fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App);
    fn render(state: &Entity<AppState>, bridge: &Bridge, focus: &FocusHandle,
              host: &Entity<DialogHost>, window: &mut Window, cx: &mut App) -> AnyElement;
    /// Refuse `Esc` while an operation is in flight, instead of unbinding the key.
    fn on_before_dismiss(_draft: &Self::Draft) -> DismissDecision { DismissDecision::Dismiss }
    /// Reset the draft. Called by `dialogs::dismiss` after the veto passes.
    fn close(draft: &mut Self::Draft) { *draft = Self::Draft::default(); }
}
```

Migration order and the `dismiss` veto path live in
`.claude/skills/gpui-app-shell/references/patterns.md`.

Do this when the next dialog is added, not as a flag-day refactor
(`zed/CONTRIBUTING.md:81` — "Giant refactorings" are explicitly unwelcome, and the same
judgement applies here).

---

## P15 — Persistence: append-only, mismatch is a hard error {#p15-persistence}

```rust
// zed/crates/sqlez/src/domain.rs:3-9
pub trait Domain: 'static {
    const NAME: &str;
    const MIGRATIONS: &[&str];

    fn should_allow_migration_change(_index: usize, _old: &str, _new: &str) -> bool { false }
}
```

```rust
// zed/crates/sqlez/src/migrations.rs:66-89 (trimmed)
if let Some((_, _, completed_migration)) = completed_migrations.get(index) {
    if completed_migration == migration { continue; }               // already applied
    else if should_allow_migration_change(index, &completed_migration, &migration) { continue; }
    else { anyhow::bail!(formatdoc! {"
            Migration changed for {domain} at step {index}
            Stored migration:\n{completed_migration}
            Proposed migration:\n{migration}"}); }
}
self.eager_exec(&migration)?;
```

The discipline that follows:

- Migrations are a `&[&str]` indexed by position; **you may only append**. Editing index
  *n* after release aborts startup with a diff.
- Migration text is normalised through `sqlformat` before comparison, so reformatting is
  safe.
- Deprecated columns are kept, not dropped, with the reason inline:
  `zed/crates/workspace/src/persistence.rs:546-548` —
  `dock_visible INTEGER, // Deprecated. Preserving so users can downgrade Zed.`
- Writes from the UI thread go through a fire-and-forget helper that never blocks a frame
  (`zed/crates/db/src/db.rs:287-293`).

**fleetd has no SQL and no migration list** — it has versioned JSON documents, so the
translation is:

| Zed mechanism | fleetd equivalent |
| --- | --- |
| `MIGRATIONS: &[&str]`, append-only | `STATE_VERSION` / `CONFIG_VERSION` / `BOARD_DOCUMENT_VERSION`, all `1` today |
| `bail!` on changed migration text | `StateValidationError::UnsupportedVersion` in `crates/fleet-core/src/state.rs` |
| Kept deprecated column | `#[serde(default, skip_serializing_if = "Option::is_none")]` field + a comment |
| Golden fixture | `crates/fleet-proto/tests/compatibility.rs` (already exists for the wire) |

The rules to write down **before the first version bump ships**:

1. New fields are optional and defaulted; unknown fields are ignored (fleetd already sets
   `deny_unknown_fields` in exactly one place, `adapters/board/jira/settings.rs`).
2. An existing field never changes meaning. If the meaning must change, add a new field
   and keep the old one readable.
3. A deprecated field stays in the struct with a comment naming the release that
   deprecated it, so an older build can still load the file.
4. A version bump ships with a test that loads a fixture written by the previous version.
5. Writes from the GPUI thread are spawned, never awaited inline (fleetd already honours
   this — `docs/ARCHITECTURE.md:386`, "Render performs no filesystem access").

Related, already correct: agent transcripts (`agents/<thread>/events.ndjson`) and boards
(`boards/<board-id>.json`) carry their own versions outside `PersistedState v1`
(`docs/ARCHITECTURE.md:236-240`, `:509-513`).

---

## P16 — Logging: fully-qualified macros, runtime-filterable scopes {#p16-logging}

Zed, `log::` macros — fully-qualified vs bare (imported):

| macro | qualified | bare |
| --- | ---: | ---: |
| `log::error!` | 691 | 1 |
| `log::warn!` | 406 | 9 |
| `log::info!` | 636 | 15 |
| `log::debug!` | 453 | 1 |
| `log::trace!` | 148 | 11 |

2334 vs ~37. Always write the qualified form: greppable call sites, no macro-name
collisions.

Runtime filtering (`zed/crates/zlog/README.md:3-13`):

> Use the `ZED_LOG` environment variable to control logging output … `ZED_LOG=info,project=debug,agent=off`
> Levels can be one of: `off`/`none`, `error`, `warn`, `info`, `debug`, or `trace`.

and the same filter is settable at runtime from settings:

```rust
// zed/crates/zlog_settings/src/zlog_settings.rs:7-13
pub fn init(cx: &mut App) {
    cx.observe_global::<SettingsStore>(|cx| {
        let zlog_settings = ZlogSettings::get_global(cx);
        zlog::filter::refresh_from_settings(&zlog_settings.scopes);
    }).detach();
}
```

**fleetd uses `tracing`, not `log`** — the `log` crate is not a dependency anywhere and
there is no `tracing-log` bridge. Counts: 162 fully-qualified `tracing::*!` vs 24 bare.
Per crate: `fleet-daemon` 117 call sites (warn **93**, debug 16, info 7, error 1, trace 0),
`fleet-term` 26, `fleet-app` 17, `fleet-lazygit` 13, `fleet-client` 12. `fleet-core`,
`fleet-proto`, `fleet-git`, `fleet-cli` and `fleet-ui-kit` log nothing at all, which is
correct for their roles.

Three findings:

1. **Pick the qualified form and make it the rule.** 24 bare call sites are the drift.
2. **`RUST_LOG` is silently ignored by `fleetd`.** `crates/fleet-app/src/shell/root/bootstrap.rs:28-34`
   and `crates/fleet-lazygit/src/lib.rs:119-120` build an
   `EnvFilter::try_from_default_env()` defaulting to `info`;
   `crates/fleet-daemon/src/main.rs:57-59` calls `tracing_subscriber::fmt()` with a writer
   and no filter at all. Give the daemon the same `EnvFilter`, and document the variable in
   `docs/DEVELOPMENT.md` the way `zed/crates/zlog/README.md` documents `ZED_LOG`.
3. **Zero spans in a 40k-line async daemon.** `#[instrument]` appears 0 times workspace-wide.
   Correlating one request across services is impossible from the log today. Add spans on
   `crates/fleet-daemon/src/services/dispatch.rs` and the per-connection request loop in
   `crates/fleet-daemon/src/server/connection.rs`, carrying the request id and the
   `RequestBody` discriminant.

**Level discipline.** 93 `warn` against 1 `error` and 7 `info` means `warn` is being used
as a general-purpose channel. `error` = a failure the user will notice; `warn` = an anomaly
the code recovered from; `info` = lifecycle (bind, accept, shutdown); `debug` = flow.
Re-level as you touch each site, not in a sweep.

`fleet-ui-kit` deliberately has no `tracing` dep and uses one `eprintln!` behind an
`AtomicBool` (`crates/fleet-ui-kit/src/paint_error.rs:4-9`), because font failures repeat
per cell per frame. That is correct; leave it.

---

## P17 — Tests colocated, fixtures behind `test-support` {#p17-tests}

Zed: 827 `#[cfg(test)]` blocks, 49 `*_tests.rs` siblings, only 8 `tests/` integration
directories. **143 of 243 crates declare a `test-support` feature**, always transitive:

```toml
# zed/crates/workspace/Cargo.toml
[features]
test-support = ["client/test-support", "http_client/test-support", "db/test-support",
                "project/test-support", "gpui/test-support", "fs/test-support"]

# zed/crates/terminal_view/Cargo.toml — how a consumer turns it on
[dev-dependencies]
gpui      = { workspace = true, features = ["test-support"] }
workspace = { workspace = true, features = ["test-support"] }
```

Test-only code is gated with the same predicate everywhere:
`#[cfg(any(test, feature = "test-support"))]`. Fakes live in the crate that owns the real
thing, so no `test_utils` crate accumulates and no helper leaks into a release binary.

**fleetd matches on colocation** — 343 `#[cfg(test)]`, 53 `tests.rs` siblings, 7 `tests/`
directories, plus a `tests/bugfix_*.rs` regression convention
(`bugfix_connection.rs`, `bugfix_read_watch.rs`, `bugfix_mutation.rs`,
`bugfix_parse_patch.rs`). `make test` builds `fleetd` first and exports
`FLEET_DAEMON=target/debug/fleetd` because app socket tests launch the real binary.

**The gap:** only `fleet-daemon` declares `test-support`
(`crates/fleet-daemon/Cargo.toml`, gating `pub mod testing`, and dev-depending on itself
with the feature on). `fleet-core`, `fleet-proto`, `fleet-git` and `fleet-client` should
grow one for their builders and fakes rather than duplicating fixtures across `tests/`.
See the `rust-gpui-testing` skill for the testing side of this.

---

## P18 — Documentation: heavy at the seams, light in the leaves {#p18-docs}

Zed's `pub fn` doc coverage (13,490 sampled, overall 28.4%): `gpui` 86%, `theme` 78%,
`language` 50%, `ui` 36%, `settings`/`util`/`db` 27-28% — against `editor` 10%,
`workspace` 10%, `project` 12%, `git_ui` 7%, `terminal` 7%. Enforced only where it
matters: `#![warn(missing_docs)]` in `gpui`, `#![deny(missing_docs)]` in `theme`,
`theme_settings`, `release_channel`, `command_palette_hooks`.

**[RULE]** `zed/.rules:4`: *"Do not write organizational or comments that summarize the
code. Comments should only be written in order to explain "why" the code is written in
some way in the case there is a reason that is tricky / non-obvious."*

Suppressions carry reasons: 99 `reason = ` strings across 67 `#[expect(` and 345
`#[allow(`.

**fleetd is far better and should stay that way.** 1849/1960 pub fns documented = **94%**
(`fleet-ui-kit` 100%, `fleet-git` 100%, `fleet-client` 100%, `fleet-daemon` 83%,
`fleet-app` 85%). Every `lib.rs` opens with a `//!`; module files carry `//!` headers that
often name the doc section they implement (`§3.9`, `BOARD §8`). The whole workspace
carries **20 `#[allow(...)]`** and zero `#[expect(...)]`, plus 891 `#[must_use]`.

Two small moves: `#![warn(missing_docs)]` is on `fleet-git`, `fleet-lazygit` and
`fleet-ui-kit` only — extend it to `fleet-core` and `fleet-proto`, the crates everything
else compiles against. And give the remaining `#[allow(...)]` sites a
`reason = "…"`, as Zed does.

---

## P19 — Tooling and verification {#p19-tooling}

**[RULE]** `zed/.rules:137`: *"Use `./script/clippy` instead of `cargo clippy`"* — the
script adds `--workspace --release --all-targets --all-features -- --deny warnings` and
locally also runs `cargo shear` (unused deps), `typos`, and `buf lint`.

Zed's other config worth copying *in spirit*: pinned `rust-toolchain.toml`; a
`rustfmt.toml` with no style knobs at all; `.config/nextest.toml` with a slow-timeout and
a serial group for `package(db)`; `script/check-todos` failing CI on any `todo!`/`FIXME`;
generated CI workflows (`cargo xtask workflows`).

**fleetd's equivalents** (`Makefile`):

| Target | What it runs |
| --- | --- |
| `make fmt-check` | `cargo fmt --check` |
| `make clippy` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `make lint` | `fmt-check` + `clippy` |
| `make test` | builds `fleetd`, exports `FLEET_DAEMON`, then `cargo test --workspace` |
| `make check` | `cargo check` over the workspace |
| `make ci` | `lint` + `test` + `test-scripts` |

Use `make clippy`, never bare `cargo clippy` — the Makefile carries the flags.
`rust-toolchain.toml` pins `1.97.1`, `profile = "minimal"`, components `rustfmt` +
`clippy` (Zed also installs `rust-analyzer` + `rust-src` so editors agree with CI — worth
copying). `rustfmt.toml` is `edition`/`style_edition` only, exactly like Zed's.
`[profile.dev] opt-level = 1` with `[profile.dev.package."*"] opt-level = 3, debug = false`
— the documented reason is that dependency debuginfo dominated a ~19 GB `target/`
(`docs/DEVELOPMENT.md:29-40`).

**Missing in this worktree, in rough order of value:** no `.github/` at all (`make ci`
exists but nothing runs it); no unused-dependency check (`cargo shear --locked
--deny-warnings`); no `typos.toml`; no `.config/nextest.toml` slow-timeout, though fleetd
has socket tests that can hang; no `FIXME`-grep in `make lint` (`todo!`/`unimplemented!`
are already denied by clippy).

---

## Commit, PR and ADR conventions {#conventions}

**Commits.** `<area>: <imperative lowercase summary>`, where area is the crate short name.
Observed in `git log`: `app`, `daemon`, `core`, `ui-kit`, `proto`, `cli`, `client`,
`tests`, `build`, `docs`. One outlier uses Conventional Commits; do not copy it. No body
convention, no trailers. Branches: `fix/…`, `feat/…`, `chore/…`, `codex/…`.

Zed's PR-title rule is compatible and worth mirroring for PRs (`zed/.rules:143-146`):
imperative, correctly capitalised, no conventional-commit prefix, no trailing punctuation,
optionally prefixed with a crate name.

**Scope.** `zed/CONTRIBUTING.md:60`: *"Make the PR about **one thing only**"*. And
`:80-83` names what gets rejected: features whose complexity outweighs the benefit, giant
refactorings, non-trivial changes with no tests, and *"stylistic code changes that do not
alter any app logic. Reducing allocations, removing `.unwrap()`s, fixing typos is great;
making code 'more readable' — maybe not so much."*

**Docs move with code.** `docs/README.md` assigns every document a domain of authority;
`docs/KEYMAP.md:12` and `docs/REMOTE-MACHINES.md:3` declare themselves the source of
truth; `docs/DESIGN-SYSTEM.md:1244` requires a new token to land in `theme/tokens.rs`
**and** in §2 of that document in the same commit. Generalise: a code change that
contradicts a doc is a bug in one of the two, and both move in the same commit.

**ADRs.** `docs/decisions/` holds 12 files with clear titles, each stating what was
adopted, what was rejected, and the constraint it puts on future code — a discipline Zed
does not have at all. Two problems to fix: `0011-remote-machines.md` and
`0011-terminal-agent-attention.md` share a number, and `docs/README.md` indexes only 11,
omitting the second. Renumber to `0012` and add the index row before writing a thirteenth.

**Agent rules file.** There is no `.rules`, `AGENTS.md` or `CONTRIBUTING.md` in this
worktree; the root `CLAUDE.md` is the agent guide. Whatever holds the rules, adopt Zed's
meta-rule verbatim (`zed/.rules:167-184`): new rules must be non-obvious, repeatedly
encountered, and specific enough to act on; and

> Avoid architectural descriptions of a crate (module layout, data flow, key types). These
> go stale fast and the agent can gather them by reading the code. Rules should be **traps
> to avoid**, not **maps to follow**.

fleetd's map already exists in `docs/ARCHITECTURE.md`. The traps worth writing down: no
new `mod.rs`; no `let _ =` on a fallible call; `make clippy`, not `cargo clippy`;
fully-qualified `tracing::` macros; schema fields are additive; `docs/` and code move
together.
