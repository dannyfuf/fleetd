# Reviewer checklist — workspace architecture (fleetd)

Standalone. Apply to any PR that adds or moves a crate, adds a dependency, grows a module
past mixed concerns, changes error handling or logging, touches a persisted schema, or
edits `docs/`. Every item is a yes/no question with the reason and the fix.

Scope note: this list covers **crates, manifests, files, errors, logs, schemas, docs**. It
does not cover GPUI entities, components, styling, async lifetimes or the wire protocol —
those have their own checklists.

---

## Manifests

**Is every new dependency declared as `foo.workspace = true`, with the version only in the
root `[workspace.dependencies]`?**
Why: a crate-level version literal forks feature unification and the upgrade point.
fleetd has zero such literals today; a first one is a regression.
Fix: move the version to `Cargo.toml`'s `[workspace.dependencies]`; leave only features in
the crate manifest.

**Does the crate manifest carry `[lints] workspace = true`?**
Why: without it, the crate silently opts out of `dbg_macro`/`todo`/`unimplemented`/
`disallowed_methods` denial. All 10 fleetd crates have it.
Fix: add the two-line `[lints]` section.

**Does a new crate inherit `edition.workspace`, `rust-version.workspace`,
`publish.workspace`?**
Why: a crate on a different edition or MSRV breaks the workspace build in a way that only
shows on someone else's toolchain.
Fix: copy the preamble from any existing `crates/fleet-*/Cargo.toml`.

**Are test-only dependencies in `[dev-dependencies]`, and do shared fakes come from a
`test-support` feature rather than a copy-paste?**
Why: a test helper in `[dependencies]` ships in the release binary.
Fix: `[dev-dependencies]`, plus a `test-support` feature in the crate that owns the real
thing (fleetd's model: `crates/fleet-daemon/Cargo.toml` → `pub mod testing`).

**Does any new lint `allow` carry a `reason = "…"`?**
Why: an unexplained `allow` is permanent. The whole workspace currently has 20.
Fix: add the reason, or prefer `#[expect(..., reason = "…")]` so it fails when it becomes
unnecessary.

---

## Layering

**Does every new `fleet-*` dependency edge respect `docs/ARCHITECTURE.md:57-58`?**
The stated direction: `core <- proto <- {term, client, cli} <- {daemon, app}`; `ui-kit`
depends only on gpui; `lazygit` depends on `git` and `ui-kit`, never on `core` or `proto`.
Why: the direction is what keeps `fleet-core` pure, `fleet-ui-kit` domain-free and the
build parallel.
Fix: extract the shared type **downward** into a lower crate; never add the sideways edge,
never merge the crates.

**Do `fleet-core`, `fleet-git` and `fleet-ui-kit` still have no new dependency?**
Why: `fleet-core` is the no-I/O, no-clock domain crate
(`docs/decisions/0008-board-model-and-sync.md:37`); `fleet-git` is leaf plumbing;
`fleet-ui-kit` depends on `gpui` and nothing else (ADR 0003, and its own crate doc,
`crates/fleet-ui-kit/src/lib.rs:12-13`).
Fix: pass the data in as a plain type / `SharedString` / closure instead.

**Does the layering test still pass, and does the PR add its new prohibition to it?**
Why: a legal-but-wrong edge is invisible in review. Zed enforces its equivalent as a CI
test (`zed/tooling/xtask/src/workspace.rs:23-70`).
Fix: run `cargo test -p fleet-app --test workspace_layering`; add the pair to `FORBIDDEN`
when the PR establishes a new rule.

**Did model or plumbing logic migrate into a UI crate to avoid plumbing a type?**
Why: it kills the model/view split and makes the logic untestable without a window.
Fix: keep it in `fleet-git` / `fleet-core` and plumb the type.

**If a new crate was added: is it justified, or would a module in an existing crate do?**
Why: at 10 crates the boundaries are legible; each new crate is a permanent public API.
Fix: a new crate needs a compile-boundary or reuse reason, not just "this is a new
feature".

---

## Module shape

**Is there a new `mod.rs`?**
Why: file tabs and `rg` output become unreadable when 31 files are all called `mod.rs`
(`zed/.rules:14`). fleetd already uses the sibling convention in
`crates/fleet-app/src/dialogs/settings.rs` + `settings/`.
Fix: `foo.rs` beside `foo/`.

**Did new code go into an existing file unless it is a genuinely new logical component?**
Why: `zed/.rules:5` — "Avoid creating many small files." Splitting for its own sake is
churn.
Fix: add to the existing module.

**If a file was split, was it split on *concerns* into fleetd's shape?**
The shape: `draft.rs` / `view.rs` / `persistence.rs` (or `lifecycle.rs`, `schema.rs`,
`actions.rs`) / `tests.rs` beside a sibling `foo.rs`, as
`crates/fleet-app/src/dialogs/{settings,board_settings,card_detail}/` are.
Why: a split that just moves lines around adds imports without reducing coupling.
Fix: split model / presentation / persistence / dispatch, or don't split.

**Is the crate's `lib.rs` still a module list + re-exports + a `//!` summary and nothing
else?**
Why: every fleetd `lib.rs` opens with a one-sentence `//!` today.
Fix: move the logic into a module.

**Are new items `pub(crate)` / `pub(super)` unless another crate needs them?**
Why: an accidental `pub` becomes an API someone depends on.
Fix: narrow the visibility.

---

## Complexity

**Is any new function over ~100 lines?**
Why: function size is the complexity budget. fleetd's p90 is 31 lines, p99 95; Zed's p90
is 43. File length alone is *not* a finding — `crates/fleet-app/src/keymap.rs` is 1081
lines and correct, because it is one table.
Fix: extract a named helper; do not extract a new file.

**Are state machines exhaustive `enum` matches rather than boolean pairs?**
Why: the daemon relies on this — "adding a protocol variant must fail to compile until
classified" (`docs/REMOTE-MACHINES.md:119`).
Fix: model the states as an enum and match without a `_` arm.

**Is a new extension point a trait in the *lower* crate, with defaulted optional methods?**
Why: that is how a high crate plugs into a low one without the low one naming it. fleetd's
good examples: `VtEngine` in `fleet-term`, the daemon's `#[async_trait]` port traits
(`Files`, `Shell`, `Git`, `Github`, `Process`, `Clock`, `BoardBackend`, `MachineProvider`,
`AgentProvider`).
Fix: define the trait beside the thing it abstracts, not beside the caller.

**If the PR adds a dialog: does it edit six or more files to do so?**
Why: `Dialogs::` is matched at 140 sites and there is no `Dialog` trait; each new dialog
touches `Dialogs`, `context_name`, `width`, the `seed` match, the render match,
`DialogHost`, `actions.rs`, `keymap.rs`.
Fix: this is the moment to introduce the `Dialog` trait rather than the ninth copy of the
fan-out (see `patterns.md#p13-traits`). Not a blocker on its own.

---

## Errors

**Is there a `let _ =` on a fallible call?**
Why: the error is gone forever (`zed/.rules:8-12`). fleetd has 139 such sites; do not add
the 140th.
Fix: `?` + `.context("…")`, `if let Err(error) = …`, or a `log_err()` helper. If it is a
genuine fire-and-forget channel send, keep it **with a one-line comment saying so**.

**Is there a new `.unwrap()` in production code?**
Why: fleetd's effective production `unwrap()` count is **zero** — the highest bar in this
repo. The 22 surviving `expect()` calls are static invariants with messages
(`"static regex"`, `"static host id is valid"`).
Fix: `?`, or `.expect("why this cannot fail")` when it genuinely cannot.

**Does a new library-crate error type use `thiserror`, and a new binary-crate path use
`anyhow` + `.context("…")`?**
Why: `thiserror` is for callers that branch on the variant; everywhere else the chained
message is what you want. fleetd's split: `fleet-core`/`fleet-proto`/`fleet-git`/
`fleet-term`/`fleet-client` are `thiserror`; `fleet-app`/`fleet-cli`/`fleet-lazygit` are
`anyhow`; `fleet-daemon` is both.
Fix: match the crate's side of the line.

**Does every new `?` across an I/O or subprocess boundary carry a `.context("…")`?**
Why: ~60 context calls across 228k lines means most failures reach the log with no story.
Fix: add the context where the operation is named, not where it is caught.

**Is a `Result` in a new `Drop`, event handler or fire-and-forget path observed somewhere?**
Why: those paths cannot use `?`, so they are where errors vanish.
Fix: `warn_on_err()` / an explicit `tracing::warn!`.

---

## Logging

**Are new log macros fully qualified (`tracing::warn!`, not `use tracing::warn;`)?**
Why: greppable call sites, no macro-name collisions. fleetd is 162 qualified vs 24 bare.
Fix: qualify it.

**Does the level match severity?**
`error` = a failure the user will notice; `warn` = an anomaly the code recovered from;
`info` = lifecycle; `debug` = flow.
Why: `fleet-daemon` is 93 `warn` / 7 `info` / 1 `error` — `warn` has become a
general-purpose channel, so it no longer signals anything.
Fix: re-level the sites the PR touches. Do not open a re-levelling sweep.

**If the PR touches the daemon's request path, did it add or preserve a span?**
Why: `#[instrument]` appears 0 times workspace-wide, so no request can be correlated
across services.
Fix: `#[tracing::instrument(skip(…), fields(request_id, kind))]` on the dispatch entry
point.

**If the PR touches subscriber setup, is `RUST_LOG` honoured?**
Why: `fleet` and `fleet-lazygit` install an `EnvFilter`
(`crates/fleet-app/src/shell/root/bootstrap.rs:28-34`); `fleetd` installs none
(`crates/fleet-daemon/src/main.rs:57-59`), so `RUST_LOG` silently does nothing for the
daemon.
Fix: `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))`, and
document the variable in `docs/DEVELOPMENT.md`.

**Is a hot-path log rate-limited?**
Why: font failures repeat per cell per frame; `crates/fleet-ui-kit/src/paint_error.rs:4-9`
uses a `log_once` guard for exactly this.
Fix: gate it behind an `AtomicBool` or a generation counter.

---

## Persistence and schema

**Is the schema change additive?**
Why: an existing field that changes meaning makes older builds silently misread user data.
Fix: `#[serde(default, skip_serializing_if = "Option::is_none")]` on a new field.

**Are deprecated fields kept, with a comment naming the release that deprecated them?**
Why: so a user who downgrades can still read the file — Zed keeps deprecated *columns* for
this reason (`zed/crates/workspace/src/persistence.rs:546-548`).
Fix: leave the field in, add the comment.

**If a `*_VERSION` was bumped (`STATE_VERSION`, `CONFIG_VERSION`,
`BOARD_DOCUMENT_VERSION`, `PROTOCOL_VERSION`): is there a test that loads a fixture
written by the previous version?**
Why: all fleetd persisted schemas are still version 1, so the *first* bump sets the
precedent. `crates/fleet-proto/tests/compatibility.rs` is the model.
Fix: add the fixture test in the same PR as the bump.

**Are writes triggered from the GPUI thread spawned rather than awaited inline?**
Why: `docs/ARCHITECTURE.md:386` — "Render performs no filesystem access and starts no
request"; `docs/APP-CONTRACTS.md:101` — "Render prepares nothing."
Fix: `cx.background_executor().spawn(...)` inside a retained task, or send it over the
`Bridge`.

---

## Docs, tests and commit

**Do new `pub` items have `///`, and new crates a `//!`?**
Why: fleetd is at ~94% coverage on `pub` fns — well above Zed's 28%. That is a
differentiator worth keeping.
Fix: document the item, especially if it is a seam another crate compiles against.

**Do comments explain *why*, never *what*?**
Why: `zed/.rules:4`. fleetd's house style already does this — see the "why the flag is set
on send, not on reply" comments in `crates/fleet-core/src/state.rs`.
Fix: delete the summarising comment, or replace it with the non-obvious reason.

**Do tests live beside the code?**
`#[cfg(test)] mod tests` or a `tests.rs` sibling; only cross-crate integration goes in
`tests/`. Regression tests follow the `tests/bugfix_*.rs` convention.
Fix: move them.

**Do `make lint` and `make test` pass locally?**
`make lint` = `fmt-check` + `cargo clippy --workspace --all-targets --all-features -- -D
warnings`. `make test` builds `fleetd` first because app and integration tests launch the
real binary. Use `make clippy`, never bare `cargo clippy`.

**If the change contradicts a `docs/` section, does the same commit fix the doc?**
Why: `docs/README.md` gives each document authority over a domain; a contradiction means
the repo has two truths. `docs/DESIGN-SYSTEM.md:1244` states the same-commit rule for
tokens.
Fix: update `ARCHITECTURE.md` / `APP-CONTRACTS.md` / `DEVELOPMENT.md` / `REMOTE-MACHINES.md`
in the same commit.

**Is the commit message `<area>: <imperative lowercase summary>`?**
Areas: `app`, `daemon`, `core`, `ui-kit`, `proto`, `cli`, `client`, `git`, `lazygit`,
`tests`, `build`, `docs`.
Fix: rewrite the subject. No conventional-commit prefixes.

**If the PR records a load-bearing decision: is there an ADR, with a free number, indexed
in `docs/README.md`?**
Why: `docs/decisions/` already has two files numbered `0011`
(`0011-remote-machines.md`, `0011-terminal-agent-attention.md`) and the index lists only
11 of 12 — do not make it worse.
Fix: take the next free number, state what was adopted / rejected / constrained, add the
index row.

**Is the PR about one thing?**
Why: `zed/CONTRIBUTING.md:60`. Giant refactorings and stylistic-only changes are
explicitly unwelcome upstream, and the same judgement applies here — `mod.rs` renames,
`let _ =` cleanups and re-levelling belong in the PR that already touches the file.
