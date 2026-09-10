---
name: rust-ipc-protocol
description: How fleetd designs its client/daemon wire protocol — envelope and correlation, request timeouts, ordering vs throughput classification, additive versioning with capability strings and byte-exact goldens, structured errors, reconnect, process lifecycle and remote SSH transport. Load it before adding or changing a `RequestBody`/`ResponseBody`/`Event` variant, touching `crates/fleet-proto`, `crates/fleet-client/src/connection.rs`, `crates/fleet-daemon/src/server/`, or `crates/fleet-daemon/src/machines/`, and before anything involving framing, handshake, reconnect, backoff, keepalive, socket/pid files, or `fleetd connect` over SSH. Also load it when reviewing a PR that adds a wire field, a capability, or a new await on a socket.
---

# rust-ipc-protocol

fleetd speaks one protocol — length-prefixed JSON, `PROTOCOL_VERSION = 7` — over three transports:
a local Unix socket (`fleet` app and CLI → `fleetd`), an SSH stdio pipe (`fleetd` → remote
`fleetd connect`), and `tokio::io::duplex` in tests. This skill is the contract for extending that
protocol without breaking a peer, and the failure-mode catalog for everything that awaits a socket.

Comparison patterns are verified against Zed **v1.18.1**, the GPUI tag fleetd depends on
(`Cargo.toml`); Zed's IPC lives in `crates/rpc`, `crates/client`, `crates/remote`,
`crates/remote_server`. fleetd is **ahead** of Zed on framing limits, goldens, capabilities and
pid-file safety — do not "improve" those toward Zed. `docs/ARCHITECTURE.md` § "Protocol
compatibility" (`:343`) and `docs/REMOTE-MACHINES.md` (self-declared **AUTHORITATIVE**, `:3`) govern
this area; a change that contradicts them is a bug in one of the two, and both move in one commit.

## When to use

- Adding, renaming or re-shaping a variant or field of `RequestBody`, `ResponseBody`, `Event`,
  `Snapshot`, `FrameUpdate` or `ProtoError`.
- Touching `crates/fleet-proto/src/codec.rs` (framing), or the handshake in
  `crates/fleet-daemon/src/server/connection.rs:367` / `crates/fleet-client/src/connection.rs:687`.
- Changing the client actor loop: correlation, timeouts, reconnect, backoff, queued commands
  (`crates/fleet-client/src/connection.rs`).
- Changing the daemon connection loop: admission, pending pool, event fan-out, lag handling,
  shutdown (`crates/fleet-daemon/src/server/{connection,listener,broadcast}.rs`).
- Anything under `crates/fleet-daemon/src/machines/` — remote link, providers, SSH, bootstrap.
- Daemon process lifecycle: pid file, socket file, detached spawn, readiness probe, exit codes
  (`crates/fleet-daemon/src/server/listener.rs`, `crates/fleet-client/src/spawn.rs`).

## When not to

- Pure GPUI/view work that only *calls* `Bridge::request` — that is `gpui-state-and-memory`.
- Choosing an executor, spawning a task or picking a channel inside one process —
  `rust-async-background-work`.
- Crate layering, lint policy, file-size and error-type conventions — `rust-workspace-architecture`.

## Rules

**1. One correlated envelope, one payload union.** Every frame is `Request { id, body }` or
`Response { id, result }` with `id` echoed back; nothing invents a side channel. fleetd already
does this (`crates/fleet-proto/src/request.rs:99`, `response.rs:21`); the only sanctioned extension
is a `#[serde(flatten)]` side-envelope like `HelloResponse` (`response.rs:37`) or `PongResponse`
(`response.rs:67`), which an older peer deserializes as a plain `Response` and ignores.

**2. Register the pending entry before the frame goes out.** A response can otherwise arrive before
its slot exists. `Client::request` builds the `oneshot` and the `Command` before handing it to the
actor (`crates/fleet-client/src/connection.rs:175`), and the actor owns the
`HashMap<u64, Pending>` alone. Zed states the same ordering explicitly in `peer.rs:456-479`.

**3. Put a deadline on every await that touches the wire — or a comment saying why not.**
Handshake, write, request and enqueue are all bounded (`connection.rs:35-39`;
`server/connection.rs:33-34`). The exemption list in `request_timeout()` (`connection.rs:590`) is
the model: it returns `None` for worktree-creation and board-backend calls, with six lines
explaining that a transport deadline would replace the backend's real error text (ADR 0008). Copy
that shape — an exemption is a documented decision, never an omission.

**4. Bounded incoming, never block the caller to send.** Backpressure belongs on the reader, not on
application code. fleetd is bounded on both sides — `COMMAND_CAPACITY = 256`,
`EVENT_CAPACITY = 1_024` (`connection.rs:40-41`), `OUTBOUND_QUEUE_CAPACITY = 64`,
`MAX_PENDING_REQUESTS = 64` (`server/connection.rs:35-36`) — which is stricter than Zed's unbounded
outgoing channel (`zed/crates/rpc/src/peer.rs:125`) and correct here. When a bounded send would
block, spawn a deadline-bounded send instead of awaiting inline (`connection.rs:265`).

**5. Classify every new request: does it need ordering, or throughput?** Concurrent dispatch on a
`FuturesUnordered` is the default (`server/connection.rs:140`), but PTY-input and terminal-membership
requests run inline so a resize cannot overtake input
(`pty_input_request_is_ordered`, `server/connection.rs:482`; `terminal_request_is_serialized`,
`:498`). This is fleetd's equivalent of Zed's `MessagePriority::{Foreground, Background}`
(`zed/crates/proto/src/typed_envelope.rs:94`; the scheduling split is
`zed/crates/collab/src/rpc.rs:947`). A new terminal-adjacent variant not added to those
predicates is an ordering bug.

**6. Never drop a request; answer it.** fleetd gets this from exhaustive `match` — 105
`RequestBody::` arms in `crates/fleet-daemon/src/services/dispatch.rs` — so a missing arm is a
compile error, not a peer hang. When a *known* variant cannot be served on this host, return
`ErrorKind::Unsupported`; do not close the connection. Zed needs a runtime fallback for the same
guarantee (`zed/crates/client/src/client.rs:1845`).

**7. Additive first; bump `PROTOCOL_VERSION` only when you cannot be additive.** New fields carry
`#[serde(default, skip_serializing_if = ...)]`; no field is ever renamed or repurposed, and unknown
fields are ignored (`deny_unknown_fields` appears zero times in `fleet-proto`). The daemon matches
the version literally and rejects a mismatch (`server/connection.rs:376-392`), so a bump forces
every peer — including remote daemons — to upgrade in lockstep. Update
`docs/ARCHITECTURE.md:343` and `docs/REMOTE-MACHINES.md` § 3 in the same commit.

**8. Advertise optional behavior as a named capability constant, not as a version.** A daemon a user
upgrades by hand cannot use Zed's exact-version-or-reject stance
(`zed/crates/collab/src/rpc.rs:1236`) as its only tool. fleetd has `REMOTE_MACHINES_CAPABILITY`
(`crates/fleet-proto/src/lib.rs:7`) and `PRUNE_REVIEWED_IDS_CAPABILITY` (`response.rs:29`) carried
in `HelloResponse.capabilities`. Add a `pub const`; never infer behavior from a version number.

**9. Every new wire shape gets a byte-exact golden.** `crates/fleet-proto/tests/compatibility.rs:23`
`assert_frame` checks the 4-byte prefix *and* decodes an independently written fixture, so a rename
or a case change fails the build. Legacy-peer tests (`:280`, `:365`) prove old payloads still parse.
Zed's `proto` crate has nothing at this granularity — keep the lead.

**10. Errors cross the wire as a stable kind plus a message.** `ProtoError { kind, message }`
(`crates/fleet-proto/src/error.rs:37`) with the 11-variant `ErrorKind` (`:8`) lets the UI branch
without substring-matching. Never widen a failure into a bare string. Zed's `Error { message, code,
tags }` (`zed/crates/proto/proto/zed.proto:549`) adds `key=value` tags — fleetd has no equivalent,
so a UI cannot render "install `<tool>`" without parsing prose; adding `tags` is the one Zed
borrowing worth making here.

**11. Model link liveness as an explicit state enum on a `watch` channel, not a bool.**
`LinkState { Connecting, Ready, Down, Legacy }` published through
`watch::Sender<LinkState>` (`crates/fleet-daemon/src/machines/link.rs:102`, `:210`) and the app's
`BridgeEvent::{Connected, ConnectFailed, ProtocolMismatch, Disconnected, Reconnected}`
(`crates/fleet-app/src/bridge.rs:88`) are the fleetd equivalents of Zed's `State`
(`zed/crates/remote/src/remote_client.rs:168`) and `Status` (`zed/crates/client/src/client.rs:280`).
Add a variant rather than a second boolean.

**12. On disconnect, fail pending requests; do not build a replay buffer.** `fail_pending`
(`connection.rs:965`) fails every in-flight `oneshot`; queued commands replay with duplicate
`ResizeTerminal` coalesced to the newest (`queue_disconnected_command`, `:652`). Zed's resumable
`ack_id` + `FlushBufferedMessages` replay (`zed/crates/remote/src/remote_client.rs:2029`, `:1903`)
only pays off for state deltas that cannot be recomputed — **do not port it**. What *is* missing is
jitter: both backoffs double without it (`connection.rs:38-39`; `machines/link.rs:61-62`, `:423`),
so hosts stampede a restarted daemon. Add `delay + rng.random_range(0..delay)` with a seeded RNG
under `cfg(test)`, as `zed/crates/client/src/client.rs:699-735` does.

**13. Own the daemon's process lifecycle explicitly: lock, identity-checked unlink, readiness ping.**
`SingletonGuard::acquire` takes `flock(LOCK_EX | LOCK_NB)` on `<home>/fleetd.pid`
(`server/listener.rs:68`, `:84`) and records a `FileIdentity { dev, ino }` (`:42`) so `Drop` only
unlinks files it still owns — strictly safer than Zed's `sysinfo` pid lookup
(`zed/crates/remote_server/src/server.rs:1148`). `ensure_daemon` confirms readiness with a **ping**,
not with the socket file existing (`crates/fleet-client/src/spawn.rs:69`), and `restart_daemon`
verifies pid + executable + start time before signalling (`:274`). Never add a server idle timeout:
Zed's `IDLE_TIMEOUT` (`server.rs:410`) is safe only because a headless project is disposable, and
`fleetd` owns long-lived PTYs.

**14. Keep the transport generic and test over `tokio::io::duplex`.** `protocol_transport<S>`
(`connection.rs:52`) and `AsyncDuplex` (`crates/fleet-daemon/src/machines/provider.rs:39`) already
erase the stream type; `FakeMachine` hands a duplex half to a test
(`crates/fleet-daemon/src/testing/machines.rs:34`) and `tests/generic_stream.rs:11` exercises the
codec over one. The gap is `run_connection` (`connection.rs:288`), still nailed to
`Established<UnixStream>` — every client reconnect test binds a real `UnixListener`. Generalize it
before adding reconnect behavior. Zed's seam is `RemoteConnection` + `MockRemoteConnection`
(`zed/crates/remote/src/transport/mock.rs:65`, `simulate_disconnect` at `:244`).

**15. Over the SSH bridge, stdout carries frames and stderr carries structured logs.** `fleetd
connect` (`crates/fleet-daemon/src/server/bridge.rs:20`) relays stdio raw and dumps the remote
daemon's output to `logs/fleetd.out` on the *remote* host, so a remote failure is only diagnosable
by SSHing in. Adopt Zed's shape: one JSON log record per stderr line
(`zed/crates/remote/src/json_log.rs:7` — `level` as a stable `usize`, not the enum), replayed by
`RemoteLink` into the local `tracing` subscriber with a `(remote)` fallback for non-JSON lines
(`zed/crates/remote/src/transport.rs:176`). Never interleave log text into the frame stream.

## Core patterns

Full catalog with Zed snippets: `references/patterns.md`.

### Correlated request with a policy-driven deadline — `references/patterns.md#correlation`

```rust
let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
let (response_tx, response_rx) = oneshot::channel();
let expires_at = request_timeout(&body).map(|duration| Instant::now() + duration);
let command = Command { request: Request { id, body }, response: Some(response_tx), expires_at };
timeout_at(Instant::now() + REQUEST_TIMEOUT, self.inner.commands.send(command))
    .await
    .map_err(|_| transport_error("Fleet daemon request timed out"))?
    .map_err(|_| transport_error("Fleet daemon connection is closed"))?;
match expires_at {
    Some(deadline) => match timeout_at(deadline, response_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(transport_error("Fleet daemon connection is closed")),
        Err(_) => Err(transport_error("Fleet daemon request timed out")),
    },
    None => response_rx.await.map_err(|_| transport_error("connection is closed"))?,
}
```

Both the enqueue and the response wait are bounded; ids 0 and 1 are reserved for `Hello` and
`Subscribe` (`connection.rs:687`).

### Additive field, guarded by a capability — `references/patterns.md#versioning`

```rust
/// Capability advertised by daemons that honor an exact reviewed-ID allowlist.
pub const PRUNE_REVIEWED_IDS_CAPABILITY: &str = "prune.reviewed_ids";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResponse {
    #[serde(flatten)]
    pub response: Response,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
}
```

Structs are `camelCase`, enums `snake_case` (`ErrorKind` is `kebab-case`, with `Tmux` renamed to
swarm's `tmux`, `error.rs:15-17`). No `PROTOCOL_VERSION` bump is needed for this shape.

### Byte-exact golden for the new shape — `references/patterns.md#goldens`

```rust
#[test]
fn request_wire_goldens() {
    assert_frame(Request { id: 1, body: RequestBody::DaemonPing },
                 r#"{"id":1,"body":{"type":"daemon_ping"}}"#);
}
```

`assert_frame` (`crates/fleet-proto/tests/compatibility.rs:23`) asserts the big-endian `u32` prefix,
the exact bytes, and that an independently built fixture decodes back to the same value.

### Ordering carve-out for stream-ordered requests — `references/patterns.md#ordering`

```rust
fn terminal_request_is_serialized(body: &RequestBody) -> bool {
    pty_input_request_is_ordered(body)
        || matches!(
            body,
            RequestBody::AttachTerminal { .. }
                | RequestBody::DetachTerminal { .. }
                | RequestBody::ResizeTerminal { .. }
        )
}
```

Everything else joins the `FuturesUnordered` pool bounded at `MAX_PENDING_REQUESTS = 64`, which
back-pressures the reader (`server/connection.rs:140`).

### Resync on lag instead of disconnecting — `references/patterns.md#lag`

```rust
Err(broadcast::error::RecvError::Lagged(skipped)) => {
    tracing::warn!(skipped, "event stream lagged; resyncing the client instead of dropping it");
    request_full_frames(&self.services, owner_id, &client, &attached).await;
    self.events.request_snapshot(Arc::clone(&self.services));
}
```

`server/connection.rs:276`; the client mirrors it per attachment — a lagged terminal stream resets
`received_full` and re-requests a full frame (`crates/fleet-client/src/terminal.rs:296`). Zed has no
equivalent; this is the best-reasoned IPC code in the repo, and its 9-line comment is part of it.

### Generic transport for tests

`protocol_transport<S>(stream) -> Framed<S, FleetCodec<Request, Value>>` (`connection.rs:52`) is
generic over any `AsyncRead + AsyncWrite + Unpin` and decodes into `serde_json::Value`, not
`Response`, so extended envelopes survive. The daemon's remote side does the same through
`AsyncDuplex` (`machines/provider.rs:39`), which is what makes `FakeMachine` and the two-daemon
harness (`docs/REMOTE-MACHINES.md` § 13) possible. Full snippet:
`references/patterns.md#transport`.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| Renaming or repurposing a wire field | Silently misreads an older peer's payload; `deny_unknown_fields` is absent, so nothing catches it | Add a new `#[serde(default)]` field; leave the old one until every peer is upgraded |
| Bumping `PROTOCOL_VERSION` for an optional behavior | Exact-match handshake (`server/connection.rs:376`) locks out every un-upgraded peer, including remote daemons | Ship a `pub const … CAPABILITY: &str` in `HelloResponse.capabilities` |
| A new variant with no `request_timeout()` decision | Falls into the 10 s default; a slow legitimate call is replaced by a generic transport error | Decide explicitly — timed, or exempt with a comment saying why (`connection.rs:590`) |
| A new terminal request left out of `terminal_request_is_serialized` | A resize or attach overtakes queued PTY input; the corruption is intermittent | Add it to the predicate at `server/connection.rs:498` |
| `let _ = ` on a fallible send, or `unwrap()` on the wire path | Loses the only signal that the peer is gone; the workspace has effectively zero production `unwrap` | Propagate with `?`, or discard explicitly with a `tracing::warn!` |
| Dropping a lagged subscriber | Costs the client its whole session for a few hundred ms of stall | Resync: re-request full frames plus a snapshot (`server/connection.rs:276`) |
| Building a `RequestBody::` in a view | 139 sites already couple `fleet-app` to the wire enum; a protocol change reaches into UI code | Call a `fleet-client` `api/` method, or add a `Bridge` facade method |
| Trusting a pid file, or unlinking a socket you may not own | A dying predecessor deletes its replacement's socket | `flock` + `FileIdentity` inode check before unlink (`server/listener.rs:84`, `:323`) |
| Treating "socket exists" as "daemon ready" | The daemon may still be initializing services | Ping after connect (`crates/fleet-client/src/spawn.rs:69`) |
| Awaiting while holding a `Mutex`/`RwLock` guard | Deadlocks the actor loop under reconnect | Scope or `drop` the guard before the await |

## fleetd-specific guidance

**Where things live.** Wire types in `crates/fleet-proto/src/` (`request`, `response`, `event`,
`snapshot`, `job`, `terminal`, `error`, `codec`); client actor in
`crates/fleet-client/src/connection.rs` (1 133 lines) with typed wrappers in `src/api/`; daemon in
`crates/fleet-daemon/src/server/` (`listener.rs` admission + singleton, `connection.rs`
per-connection actor, `broadcast.rs` event bus, `bridge.rs` the `fleetd connect` stdio proxy);
federation in `crates/fleet-daemon/src/machines/` (`link.rs`, `provider.rs`, `ssh.rs`,
`tailscale.rs`).

**Adding one `RequestBody` variant touches six places.** The variant; the daemon's local dispatch
(`services/dispatch.rs`, 105 arms); routing (`services/router/classify.rs:21`,
`router/translate.rs`); `request_timeout()` (`fleet-client/src/connection.rs:590`); the ordering
predicates if it is terminal-adjacent (`server/connection.rs:482`, `:498`); a golden in
`crates/fleet-proto/tests/compatibility.rs`. Exhaustive `match` catches the first three at compile
time; the rest are silent if forgotten — hence the checklist.

**The audit's live gap in this area is UI/protocol coupling.** `RequestBody::` is constructed at
**139 sites across 36 files** under `crates/fleet-app/src/{views,screens,dialogs}`, while
`fleet-client` already exposes typed modules (`api/agents.rs`, `api/boards.rs`, `api/terminals.rs`,
`api/worktrees.rs`, …) that the app bypasses entirely. `Bridge` is a transport, not a facade
(`crates/fleet-app/src/bridge.rs:363`, `:409`). Migrate **incrementally**: when you touch a view
that builds a request, move that one call behind a `Bridge` method or an `api/` function; do not
attempt a sweep. Contrast the daemon, which does have the facade (`Services` + ports/adapters).

**Other confirmed gaps, in priority order.** No jitter in either backoff (`connection.rs:38-39`,
`machines/link.rs:61-62`). No receive-idle timeout anywhere — a wedged peer holding an open socket
is only noticed by the app's 2 s health ping (`crates/fleet-app/src/bridge.rs:45`), so the CLI and
the `fleetd`↔`fleetd` link never detect a half-open connection; `RemoteLink::connected_loop`
(`machines/link.rs:444`) is where the idle timeout and a periodic `DaemonPing` belong. No dedicated
exit code for "remote fleetd not running", so `RemoteLink` cannot tell a terminal failure from a
blip (cf. `zed/crates/remote/src/proxy.rs:3-10`, `ServerNotRunning = 90`). No structured stderr logging
over the bridge (Rule 15); `run_connection` is not generic (Rule 14); no fault injection for
half-open links (Zed's `Connection::in_memory` `killed` flag, `zed/crates/rpc/src/conn.rs:32`).

**Remote bootstrap is the riskiest surface in the repo.**
`crates/fleet-daemon/src/services/bootstrap.rs` (558 lines) runs a git clone plus `cargo build
--release` on the remote through `MachineProvider::exec`, every step an `sh -lc` script, as
`JobKind::Custom("host.bootstrap")` (`:139`), verified by polling `hello().build_commit ==
checkout_ref` (`:126`). SSH defaults are in `machines/ssh.rs` (`BatchMode=yes`, `ConnectTimeout=10`,
`ControlMaster=auto`, `ControlPersist=300`). Changes here need a `docs/REMOTE-MACHINES.md` update in
the same commit and a test against `FakeMachine`, never against a real host.

**Keep doing (already at or above Zed's bar).** Byte-exact goldens with legacy-peer fixtures;
`MAX_FRAME_SIZE = 16 MiB` checked on encode *and* on the announced decode length (`codec.rs:11`,
`:73`, `:132`) where Zed checks neither; `roll_back` shrinking past `RETAINED_CAPACITY = 64 KiB`
(`codec.rs:87`); capability strings; `flock` + `FileIdentity`; resync on lag; ping-based readiness;
`MAX_CONNECTIONS = 128` admission (`server/listener.rs:29`); the `nudge_reconnect` retained permit
(`machines/link.rs:307`), which Zed has no equivalent of.

**Verification.** `make lint` (fmt-check + clippy `-D warnings`), `make test` (builds `fleetd`
first, because integration tests launch the real binary), `make check`. Commit as
`proto: <imperative lowercase summary>`, `client:`, or `daemon:`.

## Review checklist

Full list: `references/checklist.md`.

1. Is every new field `#[serde(default)]` + `skip_serializing_if`, with no field renamed or reused?
2. If `PROTOCOL_VERSION` moved, do `docs/ARCHITECTURE.md:343` and `docs/REMOTE-MACHINES.md` § 3 move
   in the same commit — and could a capability string have avoided the bump?
3. Does `crates/fleet-proto/tests/compatibility.rs` have a byte-exact golden for every new shape?
4. Does every new socket await sit inside a `timeout` / `timeout_at` / `select!` with a deadline?
5. Does the new `RequestBody` variant have a `request_timeout()` decision, and is it in the ordering
   predicates if it touches a terminal?
6. Is failure a typed `ErrorKind` variant rather than a string the UI must substring-match?
7. Is the pending entry inserted before the frame is written, and failed on every disconnect path?
8. Are pid/socket files unlinked only after a `FileIdentity` match, and is readiness a ping?
9. Does the test avoid wall-clock sleeps — `tokio::time::pause`/`advance`, or a duplex transport?
10. Are the disconnect and reconnect paths exercised, not just the happy path?

## Related skills

- `rust-async-background-work` — executors, task lifetimes, channel choice, tokio-vs-GPUI boundary.
- `rust-workspace-architecture` — crate layering, error types, lint policy, doc-with-code rule.
- `rust-gpui-testing` — deterministic async tests, fakes, `test-support`, IPC test harnesses.
- `gpui-state-and-memory` — how a view consumes `Bridge` responses without leaking entities.
- `zed-quality-review` — aggregate review pass; loads this skill's `references/checklist.md`.
