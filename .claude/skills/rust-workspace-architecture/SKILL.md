---
name: rust-workspace-architecture
description: Crate layering, workspace manifests and lints, error-handling policy, module/file splitting, logging form, schema-migration discipline, docs and commit conventions for the fleetd Cargo workspace. Load it before adding or splitting a crate, adding a dependency edge between fleet-* crates, adding a `[workspace.dependencies]` entry or a lint, deciding `anyhow` vs `thiserror`, splitting a file that has grown past ~900 lines (a dialog, a service, a provider map), adding a `tracing` call site or a `#[instrument]` span, bumping a persisted `*_VERSION`, or writing a commit message or an ADR. Also load it when a review asks "does this edge belong here?", "should this be its own crate?", or "why is this file 2000 lines?".
---

# Rust workspace architecture (fleetd)

How fleetd's 10-crate Cargo workspace is layered, what may depend on what, and the
conventions that keep the graph, manifests, errors, logs and docs coherent. Patterns are
verified against Zed v1.18.1, the GPUI tag fleetd depends on (`Cargo.toml`); fleetd facts
against this worktree.

Keep doing, do not "improve": every dep inherited from `[workspace.dependencies]` (zero
crate-level version literals), `[lints] workspace = true` in all 10 manifests,
hazards-only clippy denies with a written justification, ~94% doc coverage on `pub`
items, `//!` on every `lib.rs`, colocated tests, `test-support` behind a cargo feature,
`fleet-ui-kit` depending on `gpui` and nothing else, and zero production `unwrap()`.

## When to use

- Adding a crate, moving code between crates, or adding a `fleet-*` dependency edge.
- Adding a third-party dependency, a workspace lint, or a `clippy.toml` entry.
- Splitting a file that mixes model + presentation + persistence + dispatch.
- Choosing `anyhow` vs `thiserror`; reviewing a `let _ =`, an `.unwrap()`, an `.expect()`.
- Adding `tracing` call sites or spans, or touching subscriber setup.
- Changing anything persisted: `STATE_VERSION`, `CONFIG_VERSION`,
  `BOARD_DOCUMENT_VERSION`, `PROTOCOL_VERSION`, the agents index.
- Writing a commit message, an ADR, or updating `docs/`.

## When not to

- GPUI entity/state/task questions — that is `gpui-state-and-memory`.
- Component API and styling inside `fleet-ui-kit` — `gpui-components`, `gpui-styling`.
- Wire-format and framing decisions — `rust-ipc-protocol`.
- Suggesting fleetd adopt Zed's many-entities app architecture: fleet-app's one
  `Entity<AppState>` + free `render(props, …, cx) -> AnyElement` model is deliberate
  (`docs/APP-CONTRACTS.md`). This skill governs crates and files, not entity granularity.

## Rules

**Declare every dependency once, in `[workspace.dependencies]`.** A crate manifest may
only say `foo.workspace = true`; a version literal there silently forks feature
unification and the upgrade point. fleetd already does this in all 10 manifests
(`Cargo.toml`, 44 entries) — nothing enforces it, so add the conformity test rather than
trusting review.

**Keep lints in `[workspace.lints]`, deny hazards only, and justify every entry.** Style
lints cost CI minutes and produce churn; deny only what catches bugs or leftovers.
fleetd's `[workspace.lints.clippy]` denies `dbg_macro`, `declare_interior_mutable_const`,
`disallowed_methods`, `todo`, `unimplemented`, with an inline note on why
`redundant_clone` is deferred — land that one once the app and lazygit render paths are
quiet. Add a `[workspace.lints.rust]` section so future `rust` lints have a home (Zed's
holds one entry, `zed/Cargo.toml:1073-1074`).

**Keep the crate graph acyclic and shallow, and prove it with a test.** Sibling crates
must not reach each other even transitively; a legal-but-wrong edge serializes the build
and is invisible in review. Zed enforces its version as a CI unit test
(`zed/tooling/xtask/src/workspace.rs:23-70`). fleetd's graph is acyclic and
`docs/ARCHITECTURE.md:57-58` states the direction, but nothing tests it — see
`## Core patterns`. Highest-leverage single change here.

**Split model from view at the crate boundary, not inside a crate.** A model crate may
use `gpui` types (`Entity`, `Task`) but must never depend on the UI crate; only the view
crate does (Zed: `terminal`/`terminal_view`, `git`/`git_ui`). fleetd mirrors this with
`fleet-git` (plumbing, zero `fleet-*` deps) vs `fleet-lazygit` (UI), and
`fleet-core`/`fleet-proto` are I/O-free. Never move parsing into `fleet-lazygit` to avoid
plumbing a type.

**Break a would-be cycle by extracting the shared vocabulary downward, never by merging
crates.** When two crates must name the same thing, the name moves into a smaller, lower
crate — Zed's `zed_actions` (`zed/crates/zed_actions/src/lib.rs:13`) exists purely so `git_ui`
and `agent_ui` can dispatch at each other without an edge. fleetd does not need it yet
(50 `actions!` sites, all crate-local); if `fleet-app` and `fleet-lazygit` ever need each
other's actions, extract `fleet-actions`.

**Use `pub fn init(cx: &mut App)` plus registries only where it removes a dependency
edge.** Zed's `init` does exactly three things — delegate to submodules, register into a
`Global` registry, `cx.observe_new` onto future entities
(`zed/crates/terminal_view/src/terminal_view.rs:107-116`) — with `main.rs` the single
ordering authority. fleetd has 3 `pub fn init(`, 4 `impl Global`, 0 `observe_new`, and
wires modules explicitly from `crates/fleet-app/src/shell/root/bootstrap.rs`. At 10
crates that is defensible; adopt registries only when a low crate would otherwise have to
name a high one.

**`thiserror` in library crates, `anyhow` + `.context("…")` in binaries.** Reserve a typed
error for the case where a *caller branches on the variant*. fleetd's split is already
right (`fleet-core`, `fleet-proto`, `fleet-git`, `fleet-term`, `fleet-client` are
`thiserror`; `fleet-app`, `fleet-cli`, `fleet-lazygit` are `anyhow`; `fleet-daemon` is
both). The weakness is density — ~60 `.context`/`.with_context` calls across 228k lines,
so most `?` sites lose their story. Add context at every I/O and subprocess boundary in
`fleet-daemon` and `fleet-app`.

**Never `let _ =` a fallible call.** Propagate with `?`, log with a helper, or `match`
explicitly — a silently swallowed error is the bug you cannot find later (`zed/.rules:8-12`).
fleetd has 139 `let _ =` sites; the legitimate fire-and-forget channel sends need a
one-line comment saying so, the rest need handling. fleetd has no `ResultExt`; add
`fleet_core::ResultExt { log_err, warn_on_err }` over `tracing` so a sanctioned
alternative exists (Zed: `zed/crates/gpui_util/src/lib.rs:210`).

**Function size is the complexity budget; file size is not.** Zed accepts 13k-line files
but almost never a 200-line function (function median 7 lines, p90 43, 2.5% over 100);
fleetd is better on both axes (median 8, p90 31, 4.4% over 50). Push back on a long
function; never split a module just to reduce line count (`zed/.rules:5`).

**Split a file when it mixes concerns, into the shape fleetd already uses:** `draft.rs` /
`view.rs` / `persistence.rs` (or `lifecycle.rs`, `schema.rs`, `actions.rs`) / `tests.rs`
beside a sibling `foo.rs`, as `crates/fleet-app/src/dialogs/settings/`, `board_settings/`
and `card_detail/` are. `dialogs/palette.rs` (2046 lines) and
`dialogs/create_worktree.rs` (1750) predate that convention and are the two to fix.

**Write log macros fully qualified, and make the filter runtime-configurable.**
`tracing::warn!`, never `use tracing::warn;` — greppable call sites, no macro collisions;
fleetd is 162 qualified vs 24 bare. `fleet` and `fleet-lazygit` install an `EnvFilter`
from `$RUST_LOG` (`crates/fleet-app/src/shell/root/bootstrap.rs:28-34`); `fleetd`
installs none (`crates/fleet-daemon/src/main.rs:57-59`), so `RUST_LOG` is silently
ignored by the daemon. Fix that, and add `#[instrument]` spans (0 workspace-wide today)
on the daemon's request path so one request can be correlated across services.

**Treat a shipped schema version as append-only.** New fields are
`#[serde(default, skip_serializing_if = "Option::is_none")]`; an existing field never
changes meaning; a deprecated field is kept with a comment so an older build can still
read the file. Zed hard-`bail!`s when a stored migration's text changes
(`zed/crates/sqlez/src/migrations.rs:81-88`). fleetd's persisted schemas are all version 1
(`fleet_core::state::STATE_VERSION`, `config::CONFIG_VERSION`,
`board::model::BOARD_DOCUMENT_VERSION`), rejected otherwise by
`StateValidationError::UnsupportedVersion` — so before the *first* bump ships, write the
rule down and add a fixture test from the previous version.

**Spend documentation effort on the seams.** Library-shaped crates get near-total coverage
and `#![warn(missing_docs)]`; leaf UI code gets less. fleetd is at ~94%, with
`#![warn(missing_docs)]` on `fleet-git`, `fleet-lazygit`, `fleet-ui-kit`; extend it to
`fleet-core` and `fleet-proto`, the crates everything else compiles against. Comments
explain *why*, never *what* (`zed/.rules:4`).

**Commit as `<area>: <imperative lowercase summary>`, and move code and docs together.**
Area is the crate short name: `app`, `daemon`, `core`, `ui-kit`, `proto`, `cli`,
`client`, `git`, `term`, `lazygit`, `tests`, `build`, `docs`. `docs/` is authoritative, not
descriptive (`docs/README.md` assigns each document a domain), so a change contradicting
a doc is a bug in one of the two and both move in the same commit;
`docs/DESIGN-SYSTEM.md:1244` states this for tokens — apply it to `ARCHITECTURE.md`,
`APP-CONTRACTS.md` and `REMOTE-MACHINES.md` too.

**One ADR per expensive-to-revisit decision, uniquely numbered and indexed.** An ADR
states what was adopted, what was rejected, and the constraint it puts on future code.
`docs/decisions/` is better than Zed, which has none — but two files claim `0011`
(`0011-remote-machines.md`, `0011-terminal-agent-attention.md`) and `docs/README.md`
indexes only 11. Renumber the second to `0012` and index it before adding a thirteenth.

## Core patterns

### Manifest: inherit everything

```toml
# crates/fleet-<name>/Cargo.toml — the whole preamble, verbatim shape
[package]
name = "fleet-<name>"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[lints]
workspace = true          # every one of the 10 crates has this; a new crate must too

[dependencies]
fleet-core.workspace = true
tokio = { workspace = true, features = ["macros", "rt", "sync"] }   # features only
```

Rationale and Zed's `package-conformity` check: `references/patterns.md#p1-workspace-manifest`.

### The layering test (add this)

```rust
// crates/fleet-app/tests/workspace_layering.rs — the rules of docs/ARCHITECTURE.md:57-58
const FORBIDDEN: &[(&str, &str)] = &[
    ("fleet-lazygit", "fleet-core"), ("fleet-lazygit", "fleet-proto"),
    ("fleet-ui-kit", "fleet-core"),  ("fleet-core", "fleet-proto"),
    ("fleet-daemon", "fleet-app"),
];

#[test]
fn layering_rules_hold() {
    let graph = workspace_graph(); // `cargo metadata --no-deps` → name -> normal deps
    let mut violations = Vec::new();
    for &(from, to) in FORBIDDEN {
        if let Some(path) = dependency_path(&graph, from, to) {
            violations.push(path.join(" -> "));
        }
    }
    assert_eq!(violations, Vec::<String>::new(),
        "forbidden dependency path; extract the shared type downward instead of adding the edge");
}
```

Dev-dependencies are exempt — tests may cross boundaries. Zed's original:
`zed/tooling/xtask/src/workspace.rs:23-70`. See `references/patterns.md#p6-acyclicity`.

### Handling a fallible call instead of discarding it

```rust
// The three sanctioned forms. `let _ = fallible()` is never one of them.
self.store.persist(&state).await.context("persist state.json")?;   // caller handles it

if let Err(error) = self.notify_clients(event) {                    // custom logic
    tracing::warn!(%error, "dropping event for a disconnected client");
}

// Fire-and-forget send: the receiver is gone only during shutdown.
let _ = self.events.try_send(BridgeEvent::Resync);
```

The third form is acceptable *only* with that comment. To observe-and-drop a `Result`,
add and use `fleet_core::ResultExt::log_err` (`references/patterns.md#p12-errors`).

### Logging: qualified macro, spanned request path

```rust
// crates/fleet-daemon/src/main.rs — give fleetd the filter `fleet` already has.
let filter = tracing_subscriber::EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
tracing_subscriber::fmt()
    .with_env_filter(filter)
    .with_writer((|| std::io::stderr()).and(file_writer))
    .init();

// crates/fleet-daemon/src/services/dispatch.rs — span the request, keep macros qualified.
#[tracing::instrument(skip(self, body), fields(request_id = id, kind = body.kind_name()))]
pub async fn dispatch(&self, id: u64, body: RequestBody) -> DaemonResult<ResponseBody> {
    tracing::debug!("dispatching");
}
```

Level discipline: `error` = user-visible failure, `warn` = recovered anomaly, `info` =
lifecycle, `debug` = flow. fleetd's daemon is 93 `warn` / 7 `info` / 1 `error`; `warn` is
a general-purpose channel there, so re-level as you touch each site.

### Additive schema change

```rust
/// Added in the 2026-xx release. Absent in files written by older builds.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attention: Option<AttentionReason>,

// Deprecated 2026-xx; kept so an older build can still read this file.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub legacy_status: Option<String>,
```

A `*_VERSION` bump means a field's *meaning* changed, and ships with a fixture test from
the previous version — as `crates/fleet-proto/tests/compatibility.rs` already does for
the wire protocol. See `references/patterns.md#p15-persistence`.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `foo = "1.2"` in a crate manifest | Forks feature unification and the upgrade point | `foo.workspace = true`; add the version to `[workspace.dependencies]` |
| A lint in one crate's `lib.rs`, or a denied style lint | Divergent enforcement; CI minutes and churn without catching bugs | `[workspace.lints]` + `[lints] workspace = true`, hazards only |
| Merging two crates to break a cycle | Serializes the build and destroys the boundary | Extract the shared vocabulary into a lower crate |
| Moving parsing into `fleet-lazygit` to avoid plumbing a type | Kills the model/view split; makes the logic untestable without a window | Keep it in `fleet-git`; plumb the type |
| A new `mod.rs` | Unsearchable file tabs; `rg` results all read `mod.rs` | `foo.rs` beside `foo/`, as `dialogs/settings.rs` + `dialogs/settings/` |
| Splitting a cohesive file to hit a line target | Churn with no reduction in complexity | Shorten the *functions*; split only on mixed concerns |
| `let _ = fallible()` | The error is gone forever | `?` + `.context`, `if let Err`, or `log_err()`; a fire-and-forget send gets a comment |
| `.unwrap()` in production code | fleetd's production count is effectively 0 — don't be the first | `?`, or `.expect("why this cannot fail")` for a genuine static invariant |
| `use tracing::warn;` | Call sites stop being greppable; macro-name collisions | `tracing::warn!` fully qualified |
| A daemon with no `EnvFilter` or spans | `RUST_LOG` silently does nothing; a request can't be traced | `EnvFilter` in `main`, `#[instrument]` on the dispatch path |
| Editing a shipped schema field's meaning | Older builds silently misread user data | Add a new optional field; keep the old one with a comment |
| Reusing an ADR number, or a change that contradicts `docs/` | The index goes ambiguous; the repo gets two truths | Next free number + a `docs/README.md` row; change code and doc in one commit |

## fleetd-specific guidance

**The map.** Responsibilities, the verified DAG and each crate's public API shape are in
`references/crate-map.md` — read it before adding any edge. The direction, from
`docs/ARCHITECTURE.md:57-58`: `core <- proto <- {term, client, cli} <- {daemon, app}`;
`ui-kit` depends only on gpui; `lazygit` depends on `git` and `ui-kit`, never on `core`
or `proto`.

**Where to start, in order of leverage.**

1. **Add the layering test** (`crates/fleet-app/tests/workspace_layering.rs`, snippet
   above) plus a conformity assertion that every crate manifest has
   `[lints] workspace = true` and no non-inherited dependency. Nothing enforces either,
   and both hold today, so the test lands green.
2. **`fleetd` logging.** `crates/fleet-daemon/src/main.rs:57-59` builds the subscriber
   with no filter, while `crates/fleet-app/src/shell/root/bootstrap.rs:28-34` uses
   `EnvFilter::try_from_default_env()`. Give the daemon the same filter, then add
   `#[instrument]` on `services/dispatch.rs` and the per-connection request loop.
3. **`dialogs/palette.rs` (2046) and `dialogs/create_worktree.rs` (1750)** — split into
   the shape `dialogs/{settings,board_settings,card_detail}/` already use, incrementally,
   the next time a change lands in them.
4. **The missing `Dialog` trait.** `Dialogs::` is matched at 140 sites in
   `crates/fleet-app/src`; every dialog module independently defines free `seed`,
   `render`, `submit`, `close` with near-identical signatures, dispatched from matches in
   `crates/fleet-app/src/dialogs/mod.rs`, so adding one dialog means editing `Dialogs`,
   `context_name`, `width`, both matches, `DialogHost`, `actions.rs` and `keymap.rs`.
   A trait plus a generated table collapses that to one file. The canonical shape is
   `gpui-app-shell`'s (`const CONTEXT`, `type Draft`, `on_before_dismiss`) — do not invent a
   second one, and do it when the next dialog is added, not as a flag-day refactor.
5. **Two git process layers.** `crates/fleet-git/src/command.rs` (shell-free
   `tokio::process`, cancellation guard, bounded reads, `GIT_TERMINAL_PROMPT=0`,
   `LC_ALL=C`, `GIT_OPTIONAL_LOCKS=0`, command log) is consumed only by `fleet-lazygit`;
   `crates/fleet-daemon/src/adapters/git.rs` reimplements git invocation over the `Shell`
   adapter with none of that hardening. ADR 0004 justifies two *UIs*, not two *process
   layers*. Do not add a third; when the daemon's git adapter next needs hardening, give
   `fleet-daemon` a `fleet-git` dependency (legal — `fleet-git` is a leaf), drop the
   duplicated argv construction, and keep `Git` as the daemon's port.
6. **`RequestBody::` is constructed at 139 sites** inside
   `crates/fleet-app/src/{views,screens,dialogs}`. `Bridge` (`fleet-app/src/bridge.rs`)
   is a transport, not a facade, and `fleet-client`'s typed `api/` modules are bypassed,
   so adding a protocol variant reaches into view code. Not a rewrite: add facade methods
   to `Bridge` for the families you touch, one at a time.
7. **Housekeeping.** Renumber `docs/decisions/0011-terminal-agent-attention.md` to `0012`
   and add it to the `docs/README.md` index (which lists 11). `docs/DEVELOPMENT.md:44`
   says `make run` does **not** restart the daemon while `Makefile:24` declares
   `run: restart` — fix the doc. There is no `CONTRIBUTING.md` and no `.github/`; the root
   `CLAUDE.md` agent guide holds *traps* (no `mod.rs`, no `let _ =`, `make clippy` not
   `cargo clippy`, qualified `tracing::`, the schema rule), never an architecture map —
   `docs/ARCHITECTURE.md` is the map. 31 `mod.rs` remain; rename opportunistically while
   already editing the module, never in a rename-only PR.

**Verification.** `make lint` (= `fmt-check` + `clippy -D warnings`), `make test`
(builds `fleetd` first, because app and integration tests launch the real binary),
`make check`, `make ci` (= `lint test test-scripts`). Use `make clippy`, not
`cargo clippy` — the Makefile carries the flags. Toolchain is pinned: Rust 1.97.1,
edition 2024, resolver 3, gpui at Zed tag `v1.18.1`.

## Review checklist

Full list in `references/checklist.md`.

1. Is every new dependency `foo.workspace = true`, with the version only in `[workspace.dependencies]`?
2. Does the new crate manifest carry `[lints] workspace = true` and inherit `edition`/`rust-version`/`publish`?
3. Does every new `fleet-*` edge respect `docs/ARCHITECTURE.md:57-58`, and does the layering test still pass — with any shared type extracted downward rather than an edge added sideways?
4. Is there a new `mod.rs`, or does the directory have a sibling `foo.rs`?
5. Is any new function over ~100 lines? (File length alone is not a finding.)
6. Is there a `let _ =` on a fallible call without a fire-and-forget comment?
7. Is it `thiserror` in a library crate and `anyhow` + `.context("…")` in a binary crate?
8. Are log macros fully qualified (`tracing::warn!`), and does the level match severity?
9. If a persisted schema changed: additive, deprecated fields kept, fixture test from the previous version?
10. Do the touched `docs/` sections change in the same commit, and is the commit `<area>: summary`?

## Related skills

- `rust-async-background-work` — executors, `Task` lifetimes, the `Bridge` tokio thread vs the GPUI executor.
- `rust-ipc-protocol` — `fleet-proto` wire evolution, framing, `PROTOCOL_VERSION`, golden tests.
- `gpui-state-and-memory` — entities, subscriptions, task retention, globals.
- `gpui-components` / `gpui-styling` — the `fleet-ui-kit` boundary this skill guards from outside.
- `rust-gpui-testing` — where a test lives, `test-support` features, deterministic async.
- `zed-quality-review` — loads `references/checklist.md` with the other skills' checklists.
