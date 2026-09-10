# rust-ipc-protocol — reviewer checklist

Apply to any fleetd PR touching `crates/fleet-proto`, `crates/fleet-client`,
`crates/fleet-daemon/src/server/`, `crates/fleet-daemon/src/machines/`, or the daemon's process
lifecycle. Each item is yes/no. "Why" says what breaks; "Fix" says what to do.

Standalone — this file is loaded on its own by `zed-quality-review`.

---

## Schema and protocol

**1. Is every new field additive — `#[serde(default)]` plus `#[serde(skip_serializing_if = …)]` on
`Option`/`Vec` — with no existing field renamed, retyped or repurposed?**
Why: `fleet-proto` sets `deny_unknown_fields` **zero** times, so an older peer silently ignores what
it does not know and a renamed field reads as absent instead of erroring. The corruption surfaces
later, in the wrong crate.
Fix: add a new field; leave the old one in place until every peer is upgraded, then remove it in a
separate versioned change.

**2. If a change is not backward compatible, did `PROTOCOL_VERSION`
(`crates/fleet-proto/src/lib.rs:4`) move, and did `docs/ARCHITECTURE.md:343`
("Protocol compatibility") and `docs/REMOTE-MACHINES.md` § 3 move in the same commit?**
Why: the daemon matches the constant literally (`crates/fleet-daemon/src/server/connection.rs:376`)
and remote daemon links require the *same* version, so a bump is a fleet-wide upgrade event. The
docs are authoritative, not descriptive.
Fix: bump, update both docs, and note the incompatibility explicitly.

**3. Could a capability string have avoided the version bump?**
Why: a hand-upgraded daemon cannot rely on lockstep upgrades the way Zed's auto-updating client can
(`zed/crates/collab/src/rpc.rs:1236`). Capabilities are fleetd's forward-compatibility mechanism.
Fix: add a `pub const … CAPABILITY: &str` (cf. `REMOTE_MACHINES_CAPABILITY`,
`crates/fleet-proto/src/lib.rs:7`; `PRUNE_REVIEWED_IDS_CAPABILITY`, `response.rs:29`) and advertise
it in `HelloResponse.capabilities`; never infer behavior from a version number.

**4. Does `crates/fleet-proto/tests/compatibility.rs` gain a byte-exact golden for every new message
shape, and an `assert_round_trip` for every new type?**
Why: `assert_frame` (`:23`) checks the length prefix, the exact bytes, and an independently written
fixture — it is the only thing that catches a `rename_all` slip or a field reorder.
Fix: add the golden alongside the type in the same commit.

**5. If the change claims older peers keep working, is there a test with a literal old payload?**
Why: the claim is otherwise untested prose. `hello_metadata_accepts_old_and_new_ipc_v4_envelopes`
(`compatibility.rs:280`) and `terminal_attention_fields_default_for_legacy_peers` (`:365`) are the
models.
Fix: paste the old JSON into a test and assert it deserializes with the expected defaults.

**6. Does any new envelope-level metadata use `#[serde(flatten)]`, and only for metadata?**
Why: `flatten` is the JSON analogue of a new protobuf field number, and `fleet-proto` has exactly
two (`HelloResponse`, `PongResponse`). Using it for payloads makes the wire shape unpredictable.
Fix: put payload fields inside the `ResponseBody` variant; reserve `flatten` for side-envelopes.

---

## Correlation and dispatch

**7. Is the pending entry inserted before the frame is written?**
Why: otherwise a fast response can arrive before its slot exists and is dropped as unknown.
Fix: build the `oneshot` and the `Command` first (`crates/fleet-client/src/connection.rs:175`);
never write then register.

**8. Is every path that removes a pending entry matched by a path that fails it — disconnect,
timeout, actor shutdown, caller drop?**
Why: an entry that outlives its request is a caller hung forever.
Fix: route all of them through `fail_pending` (`connection.rs:965`); Zed's equivalent is `take()`ing
the whole map on teardown (`zed/crates/rpc/src/peer.rs:156`).

**9. Is a new `RequestBody` variant handled in `services/dispatch.rs`, `services/router/classify.rs`
and `services/router/translate.rs`?**
Why: exhaustive `match` catches these at compile time — but only if the reviewer confirms the arm is
*correct*, not merely present. A variant routed to `Target::Local` when it should fan out silently
does the wrong thing on a federated setup.
Fix: check the routing target against `docs/REMOTE-MACHINES.md` § 6.

**10. Does the new variant have an explicit `request_timeout()` decision?**
Why: unlisted variants get the 10 s default. For a call that legitimately runs longer, that replaces
the real error text with a generic transport error while the daemon keeps working.
Fix: add it to the exemption list with a comment saying *why*
(`crates/fleet-client/src/connection.rs:590`) — an exemption is a decision, not an omission.

**11. If the variant touches a terminal, is it in `pty_input_request_is_ordered` and/or
`terminal_request_is_serialized`?**
Why: unserialized terminal requests run on the concurrent `FuturesUnordered` pool, so a resize can
overtake queued input or two attaches can both observe missing membership and leak a refcount.
Fix: add it at `crates/fleet-daemon/src/server/connection.rs:482` / `:498`.

**12. Can a known request be answered with an error instead of by closing the connection?**
Why: closing turns one refused operation into a full session loss plus a reconnect storm.
Fix: return `ErrorKind::Unsupported`; reserve connection teardown for undecodable frames.

---

## Liveness and failure

**13. Is every new `await` that touches the socket inside a `timeout`, `timeout_at` or `select!`
with a deadline?**
Why: one unbounded wire await hangs the actor and every request behind it.
Fix: use the existing budgets (`REQUEST_TIMEOUT`, `WRITE_BUDGET`, `HANDSHAKE_TIMEOUT`,
`SOCKET_WRITE_TIMEOUT`) rather than inventing a new constant; if none fits, add one next to them.

**14. Is failure reported as a typed `ErrorKind`/`DaemonError` variant rather than a string the UI
must substring-match?**
Why: `ProtoError { kind, message }` (`crates/fleet-proto/src/error.rs:37`) exists so the UI can
branch. A bare string forces prose parsing that breaks on the next wording change.
Fix: pick the right `ErrorKind`; add a variant if none fits.

**15. Is there no `unwrap()`/`expect()` on the wire path, and no `let _ = <fallible>`?**
Why: the workspace has three production `unwrap()` total, all in `fleet-git`; `todo!`,
`unimplemented!` and `dbg!` are `deny` at the workspace level. A discarded send error is the only
signal the peer is gone.
Fix: propagate with `?`, or discard explicitly with a `tracing::warn!`.

**16. Do reconnect/backoff changes preserve floor, doubling and ceiling — and does a nudge still
shorten the next attempt?**
Why: `nudge_reconnect` (`crates/fleet-daemon/src/machines/link.rs:307`) holds a retained permit so a
nudge racing a disconnect is not lost; a naive rewrite loses that.
Fix: keep `backoff_min`/`backoff_max`/`doubled()` intact, and add jitter
(`delay + rng.random_range(0..delay)`, seeded under `cfg(test)`) rather than removing structure.

**17. Does a lagged `broadcast` subscriber get resynced rather than disconnected?**
Why: a client that stalls for a few hundred ms during a streaming agent turn must not lose its
session; the recovery path (`AgentThreadSnapshot { from_seq }`, `RequestFullFrame`) exists for this.
Fix: follow `crates/fleet-daemon/src/server/connection.rs:276` and
`crates/fleet-client/src/terminal.rs:296`.

**18. Does nothing await while holding a `Mutex`/`RwLock` guard?**
Why: the client's `metadata: Arc<RwLock<..>>` and the daemon's registries are touched from the same
task that drives IO; an await under a guard deadlocks the actor on reconnect.
Fix: `drop` or scope the guard before the await.

**19. Are the new frames covered for oversize, short and split reads if the codec changed?**
Why: `MAX_FRAME_SIZE` is checked on encode *and* on the announced decode length
(`crates/fleet-proto/src/codec.rs:73`, `:132`) — a change that only checks one side reintroduces the
allocate-on-attacker-input bug Zed still has (`zed/crates/remote/src/protocol.rs:16`).
Fix: extend the codec tests (`codec.rs:213`, `:286`) rather than adding a new test file.

---

## Process, files and remote

**20. Are pid and socket files unlinked only after a `FileIdentity { dev, ino }` match?**
Why: without it a dying predecessor deletes its replacement's socket, and every client reconnects to
nothing.
Fix: go through `remove_if_owned` (`crates/fleet-daemon/src/server/listener.rs:323`); single-instance
enforcement is `flock(LOCK_EX | LOCK_NB)` (`:84`), not a pid-file read.

**21. Is a newly spawned daemon confirmed ready by a **ping**, not by the socket file existing?**
Why: the socket is bound before services finish initializing.
Fix: follow `ensure_daemon` (`crates/fleet-client/src/spawn.rs:69`); it is stronger than Zed's
file-existence poll (`zed/crates/remote_server/src/server.rs:1037`).

**22. Is any signal sent only after verifying pid + executable + start time?**
Why: a recycled PID means signalling an unrelated process.
Fix: `verified_signal_target` (`crates/fleet-client/src/spawn.rs:274`).

**23. Are child processes killed on drop, and is any load-bearing drop order commented?**
Why: drop order determines whether the SSH master or the proxy dies first; Zed comments its ordering
for exactly this reason (`zed/crates/remote/src/remote_client.rs:575`).
Fix: `kill_on_drop(true)` on spawned children; add the sentence explaining the order.

**24. Did a change under `crates/fleet-daemon/src/machines/` update `docs/REMOTE-MACHINES.md` in the
same commit?**
Why: that document is self-declared **AUTHORITATIVE** (`:3`) and changes to it require a DEVIATIONS
entry; code and doc disagreeing is a bug in one of them.
Fix: update the matching section (§ 3 protocol, § 5 link, § 9 bootstrap, § 13 test doubles).

**25. Is a bootstrap or provider change tested against `FakeMachine`, never a real host?**
Why: `services/bootstrap.rs` runs a git clone plus `cargo build --release` on the remote through
`sh -lc`; it is the riskiest surface in the repo and untestable by hand.
Fix: script the exec results with `FakeMachine`
(`crates/fleet-daemon/src/testing/machines.rs:34`) and assert the recorded argv.

---

## Tests

**26. Is the behavior covered by a test that does not depend on wall-clock timing?**
Why: sleep-based IPC tests are the flakiest thing in a CI run.
Fix: `tokio::time::pause`/`advance`, or drive the codec over `tokio::io::duplex`
(`crates/fleet-client/tests/generic_stream.rs:11`).

**27. Are the disconnect and reconnect paths exercised, not only the happy path?**
Why: every fleetd IPC bug class — pending leaks, lost attachments, coalescing, backoff — lives on
the failure path.
Fix: use `FakeMachine`/`FakeRemote` for links and `RemoteDaemon::start` for two-daemon flows
(`crates/fleet-daemon/tests/infra/mod.rs`, `docs/REMOTE-MACHINES.md` § 13).

**28. If the change adds a new `await` on a link, can a half-open connection be simulated?**
Why: fleetd has no fault injection today (no equivalent of Zed's `Connection::in_memory` `killed`
flag, `zed/crates/rpc/src/conn.rs:32`), so half-open detection is untestable — the gap should be
closed by whoever first needs it, not deferred again.
Fix: add the kill flag to the duplex-backed fake at the same time as the feature.

---

## Standing gaps (do not re-report as new findings)

These are known, documented in `SKILL.md` § "fleetd-specific guidance", and are not regressions
introduced by the PR under review. Flag them only when the PR touches the surrounding code:

- No jitter in either reconnect backoff (`crates/fleet-client/src/connection.rs:38-39`,
  `crates/fleet-daemon/src/machines/link.rs:61-62`).
- No receive-idle timeout and no `RemoteLink` keepalive ping (`machines/link.rs:444`).
- No dedicated exit code for "remote fleetd not running" (`crates/fleet-daemon/src/server/bridge.rs`).
- `run_connection` is not generic over the stream (`crates/fleet-client/src/connection.rs:288`).
- Raw stdio relay instead of JSON log records on stderr (`server/bridge.rs:20`).
- `RequestBody::` constructed at 139 sites in `crates/fleet-app/src/{views,screens,dialogs}` while
  `crates/fleet-client/src/api/` goes unused. Migrate the call sites you touch; do not sweep.
- No `tags: Vec<String>` on `ProtoError` (`crates/fleet-proto/src/error.rs:37`).
