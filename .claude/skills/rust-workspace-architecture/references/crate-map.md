# fleetd crate map

Ten crates, one Cargo workspace (`Cargo.toml`, `resolver = "3"`, `edition = "2024"`,
`rust-version = "1.97.1"`, `publish = false` everywhere). Every entry below was read out
of the crate's own `Cargo.toml` and `src/lib.rs` in this worktree; the LOC figures come
from the fleetd audit.

The rule this map exists to defend, stated in the repo's own doc
(`docs/ARCHITECTURE.md:57-58`):

> Dependency direction: `core <- proto <- {term, client, cli} <- {daemon, app}`; `ui-kit`
> depends only on gpui; `lazygit` depends on `git` and `ui-kit`, never on `core` or `proto`.

## The DAG

```
fleet-core ──► fleet-proto ──┬─► fleet-term ────► fleet-daemon  [bin fleetd]
   │                │        │
   │                │        ├─► fleet-client ─► fleet-cli ──┐
   │                │        │        │                      │
   │                │        └────────┼──────────────────────┼──► fleet-app  [bin fleet]
   └────────────────┴─────────────────┴──────────────────────┘        ▲
                                                                      │
fleet-git ──► fleet-lazygit  [bin fleet-lazygit] ─────────────────────┤
   [leaf]         ▲                                                   │
                  │                                                   │
fleet-ui-kit ─────┴───────────────────────────────────────────────────┘
   [gpui only]
```

Read `A ──► B` as "B depends on A". Verified acyclic; verified against each manifest's
`[dependencies]` (not `[dev-dependencies]`, which are exempt from layering rules).

Exact `fleet-*` edges, per manifest:

| Crate | `fleet-*` dependencies (normal) |
| --- | --- |
| `fleet-core` | *(none — leaf)* |
| `fleet-git` | *(none — leaf)* |
| `fleet-ui-kit` | *(none — depends on `gpui` only)* |
| `fleet-proto` | `fleet-core` |
| `fleet-term` | `fleet-core`, `fleet-proto` |
| `fleet-client` | `fleet-core`, `fleet-proto` |
| `fleet-daemon` | `fleet-core`, `fleet-proto`, `fleet-term` |
| `fleet-cli` | `fleet-core`, `fleet-proto`, `fleet-client` |
| `fleet-lazygit` | `fleet-git`, `fleet-ui-kit` |
| `fleet-app` | `fleet-core`, `fleet-proto`, `fleet-client`, `fleet-cli`, `fleet-ui-kit`, `fleet-lazygit` |

Three facts worth naming:

- **`fleet-ui-kit` has zero `fleet_*` imports.** Its manifest lists exactly
  `unicode-segmentation`, `unicode-width`, `gpui`. The design-system boundary is a
  compile-time fact, not a convention (ADR 0003).
- **`fleet-daemon` does *not* depend on `fleet-git`.** `fleet-git` is consumed only by
  `fleet-lazygit`; the daemon has its own git layer at
  `crates/fleet-daemon/src/adapters/git.rs` over its `Shell` adapter. Two independent git
  argv layers exist — the workspace's largest structural duplication. The edge
  `fleet-daemon -> fleet-git` would be *legal* (leaf crate, no cycle) and is the cheap fix
  when the daemon's adapter next needs `fleet-git`'s hardening.
- **`fleet-app` depends on `fleet-cli`**, not the reverse: the `fleet` binary dispatches
  to the CLI when invoked with any argument and to the GPUI app otherwise
  (`crates/fleet-app/src/main.rs`).

## Per-crate responsibility

Prod/test LOC from the audit; "public shape" is the actual `src/lib.rs`.

### `fleet-core` — 8,291 prod / 6,016 test, 49 files
Pure domain. Identifiers, `config.json` / `state.json` schemas and validation, inspection
models, sessions, the native-agent reducer (`agents::ThreadProjection::apply`), the
board model and its pure `board::sync` reconciliation, slug, paths, sleep policy.
**No I/O, no clock, no gpui, no tokio** — deps are `chrono`, `regex`, `serde`,
`serde_json`, `thiserror`, `uuid`. `docs/decisions/0008-board-model-and-sync.md:37`
states the rule verbatim. Public shape: 17 `pub mod`s, no re-exports.
Owns `STATE_VERSION`, `CONFIG_VERSION`, `BOARD_DOCUMENT_VERSION` (all `1`).
*Add here:* a domain type or a pure function two surfaces must agree on.
*Never add here:* a filesystem call, a `SystemTime::now()`, a gpui type.

### `fleet-proto` — 2,140 / 1,222, 12 files
The IPC contract: `PROTOCOL_VERSION = 7`, `Request`/`Response`/`Event`/snapshot/job/
terminal types, the length-prefixed JSON `FleetCodec`. Public shape: 9 `pub mod`s +
one re-export. Golden compatibility tests in `tests/compatibility.rs`.
*Add here:* a wire type both the daemon and a client must name.
*See:* the `rust-ipc-protocol` skill before changing anything in it.

### `fleet-term` — 3,023 / 2,013, 12 files
Daemon-side PTY ownership and virtual-terminal emulation: the `VtEngine` trait,
`GhosttyEngine` (default `ghostty` feature, `libghostty-vt =0.2.1`, requires Zig 0.15.2),
key encoding, the terminal host thread. Uses **zero tokio** — pure blocking threads.
Public shape: `pub use engine::{VtEngine, EngineEvent, EngineError}`, `GhosttyEngine`,
host types, `Pty`.

### `fleet-daemon` — 40,084 / 28,293, 150 files — `[[bin]] name = "fleetd"`
Everything durable: stores (`config`, `state`), ports-and-adapters (`Files`, `Shell`,
`Git`, `Github`, `Process`, `Clock`, `BoardBackend`, `MachineProvider`, `AgentProvider`),
~25 services, the job manager, machines/router/mirror for remote federation, and the
Unix-socket server. Declares `[features] test-support = []` gating `pub mod testing`, and
dev-depends on itself with that feature on. Public shape: 7 `pub mod` + gated `testing`,
`pub use error::{DaemonError, DaemonResult}`.
*The invariant:* "Jobs and terminals belong to the daemon" (`README.md:5`); nothing the
user started is tied to a UI surface (`docs/ARCHITECTURE.md:12`).

### `fleet-client` — 3,269 / 1,877, 21 files
The tokio client: connect over `<home>/fleetd.sock`, auto-spawn and restart the daemon,
typed request `api/` modules, a `broadcast` event stream, terminal attachment. Public
shape: 5 private mods, `pub use` of `Client`, `ConnectError`, `ProtocolTransport`,
`protocol_transport`, `ensure_daemon`, `restart_daemon`, `TerminalHandle`,
`TerminalUpdate`.
*Note:* `fleet-app` bypasses the typed `api/` modules and builds `RequestBody` directly
in views (139 sites). That is a known gap, not a pattern to copy.

### `fleet-cli` — 4,576 / 4,569, 17 files
The clap surface: argument parsing, daemon-backed commands, swarm-compatible protocol-1
JSON envelopes, human output. Public shape: **one symbol**, `pub use commands::run`.
Has a `build.rs`.

### `fleet-ui-kit` — 21,674 / 4,012, 119 files (+7,749 example LOC)
The design system on raw gpui: tokens, the `Theme` gpui `Global` + `ActiveTheme` trait,
embedded Lucide icons, 67 component modules (73 `RenderOnce`). `#![warn(missing_docs)]`.
Its own crate doc is the contract (`crates/fleet-ui-kit/src/lib.rs:10-18`): no domain
types, no literal colors/sizes/durations, one `AssetSource` per app. Public shape:
8 pub mods + a `prelude` that re-exports `gpui::prelude::*`.
*Acceptance gate:* `cargo run -p fleet-ui-kit --example kit_gallery` — "if a state is not
in a gallery, it is not implemented" (`docs/DESIGN-SYSTEM.md:1251-1252`).

### `fleet-git` — 6,122 / 4,075, 40 files — `[[bin]] name = "fleet-git-seqedit"`
The typed, byte-preserving git backend: a shell-free `tokio::process` runner with explicit
argv, `GIT_TERMINAL_PROMPT=0` / `LC_ALL=C` / `GIT_OPTIONAL_LOCKS=0`, bounded reads, a
cancellation guard, a command log, per-`Repository` mutation serialisation, parsed diffs,
and the workspace's only `notify` watcher (`src/watch.rs`). `#![warn(missing_docs)]`.
Public shape: `pub use` of `Runner`, `Repository`, `GitError`, command-log types,
`sequence_editor`, plus `pub mod parse` and `pub mod watch`.
**This is the `git`-to-`git_ui` model crate in Zed's terms.** No `fleet-*` dependency.

### `fleet-lazygit` — 14,550 / 3,391, 45 files — `[[bin]] name = "fleet-lazygit"`
The native git UI in GPUI: an embedded `fleet://lazygit` tab plus a standalone binary.
Diff rendering is `syntect` + `two-face` + `similar` (ADR 0005). `#![warn(missing_docs)]`.
Public shape: `root::{Lazygit, LazygitEvent}` for embedders, plus `diff_view`, `drive`,
`keymap`. Depends on `fleet-git` and `fleet-ui-kit` only — **never** `fleet-core` or
`fleet-proto`; ADR 0010 notes the inline diff takes unified-diff *text* precisely so the
kit gains no `fleet-git` dependency.

### `fleet-app` — 42,204 / 18,925, 181 files — `[[bin]] name = "fleet"`
The GPUI app and the workspace root: state mirror, the `Bridge` to the daemon (a tokio
multi-thread runtime on a named OS thread), keymap/actions, screens, dialogs, views, the
terminal renderer. One `Entity<AppState>` threaded through ~326 signatures; screens and
views are plain structs with free `render(props, …, cx) -> AnyElement` functions
(`docs/APP-CONTRACTS.md`). Public shape: 6 `pub mod` + 5 `pub(crate) mod`,
`pub use shell::{Shell, run}`.
*Because it sits at the top of the graph, it is where the workspace-wide `tests/`
directory naturally lives* — put the layering/conformity test in
`crates/fleet-app/tests/`, or add an `xtask` crate if the workspace grows.

## Where a new thing goes

| You are adding… | It belongs in |
| --- | --- |
| A domain type both daemon and app must agree on | `fleet-core` |
| A pure function over that domain (reducer, reconciliation, validation) | `fleet-core` |
| A request, response, event or wire enum variant | `fleet-proto` (then classify it in the daemon `Router`) |
| A durable service, job kind, store or adapter | `fleet-daemon` |
| A port trait the daemon needs faked in tests | `fleet-daemon` (`adapters/`, `#[async_trait]`) |
| A typed client call over the socket | `fleet-client` (`api/`) |
| A CLI subcommand | `fleet-cli` |
| A reusable, stateless visual component | `fleet-ui-kit` (+ a gallery state) |
| A screen, dialog or view | `fleet-app` |
| Git plumbing (argv, parsing, watching) | `fleet-git` |
| Git UI | `fleet-lazygit` |
| Terminal emulation or PTY handling | `fleet-term` |

If the answer feels like "two crates need it", the answer is the *lower* of the two —
extract downward, never sideways.

## Documentation authority

`docs/README.md` assigns each document a domain; code and doc change in the same commit.
The ones this skill's area touches: `docs/ARCHITECTURE.md` (processes, crates, daemon,
terminal pipeline, client), `docs/APP-CONTRACTS.md` (how the parts of `fleet-app` plug
together), `docs/DEVELOPMENT.md` (building, running, testing), `docs/decisions/` (why the
load-bearing choices were made). `docs/decisions/` currently has two files numbered
`0011`, and `docs/README.md` indexes only 11 of the 12.
