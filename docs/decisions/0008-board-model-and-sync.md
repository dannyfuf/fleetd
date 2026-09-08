# 0008 — A backend-agnostic board model with a pure reconciliation engine

**Adopted.** The board lives in three layers that never learn each other's shape.
`fleet_core::board` holds the domain — `Board`, `Card`, `Status`, `PropertySchema` — plus a
reconciliation engine (`board::sync`) that is a pure function of a local document and a remote
snapshot: it performs no I/O, reads no clock, and returns the mutations and the `PushOp`s to apply.
`fleet-daemon` owns the only moving parts: a `BoardBackend` adapter per remote system, a per-board
JSON document store, and the `Boards` service that loads, applies, validates, saves and emits.
`fleet-app` renders whatever `PropertySchema` describes, so it knows no backend by name.

- **A remote-specific core was rejected.** Modelling Jira issues directly would have put a second
  system's field set, its status vocabulary and its writability rules inside `fleet-core`, and the
  second backend would have had to fight them. Backend-specific fields ride in `Card.properties`
  described by a `PropertySchema` the backend publishes, and a `BackendDescriptor` drives both the
  CLI's `fleet board backends` and the app's settings dialog — so a new backend needs no client
  change at all.
- **An impure sync service was rejected.** Reconciliation is where the interesting bugs are: a
  create that should have been an update, a conflict resolved the wrong way, a delete that should
  have been an adoption. Keeping it a pure function of two values makes every one of those cases a
  table-driven unit test in `fleet-core`, with no store, no socket and no fake clock.
- **A markdown crate was rejected.** `pulldown-cmark` and friends bring a parser far larger than the
  subset a card description needs, and none of them emit the ADF that Jira requires. The reader in
  `fleet-ui-kit::markdown_text` and the ADF converter in the Jira backend are hand-written, small,
  and tested against fixtures.
- **A capability negotiated at runtime was rejected in favour of a declared one.** A backend states
  what it cannot write (`BackendSchema.readonly_fields`); `adopt_schema` copies the list onto the
  board, and `ops` refuses a local edit to one of those fields with `BoardError::ReadOnlyField`. A
  field the remote would silently drop can therefore never become a dirty card that can never be
  pushed.

The wire messages the board needs — the board, card, sync and backend families, `Snapshot.boards`
and `Event::BoardChanged` — are a protocol addition, so `PROTOCOL_VERSION` is 6. The board request
families are exempt from the client's generic RPC deadline: a `describe` or a `sync` is several
backend calls with their own retry budgets, and timing them out client-side would replace the
backend's own sentence with a transport error (`fleet-client/src/connection.rs::request_timeout`).

Consequences: `fleet-core` keeps its no-I/O, no-clock rule, so timestamps arrive as RFC3339 strings
produced daemon-side through the `Clock` adapter. Adding a backend is one module under
`adapters/board/`, a typed settings struct and a registration — nothing in `fleet-app`,
`fleet-cli` or `fleet-client` changes. `docs/BOARD.md` is the prose half of this contract and
changes together with `crates/fleet-core/src/board/`.
