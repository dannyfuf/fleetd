# rust-ipc-protocol — pattern catalog

Zed citations are `zed/crates/<crate>/src/<file>.rs:<line>` at tag **v1.18.1**, read in
`/Users/danny/.swarm/repos/zed-industries/zed`. fleetd citations are repo-relative paths in this
worktree. Every path below was verified with `ls`/`rg` before it was written.

Zed runs four IPC systems and shares the *message* layer while swapping the *transport* layer:
collab (WebSocket + zstd + protobuf), remote dev (SSH child-process stdio → Unix sockets),
CLI↔app (`ipc-channel`), LSP (`Content-Length` + JSON-RPC). fleetd runs one protocol
(length-prefixed JSON) over three transports: local Unix socket, SSH stdio, `tokio::io::duplex`.

---

## envelope

**One envelope, one payload union, id echoed back.**

Zed's `Envelope` carries `id`, optional `responding_to`, optional `original_sender_id`, optional
`ack_id` and a `oneof payload` reaching field number 479
(`zed/crates/proto/proto/zed.proto:20`, `:510 // current max`). Routing is a single
`if let Some(responding_to)` branch (`zed/crates/rpc/src/peer.rs:269`).

fleetd:

```rust
// crates/fleet-proto/src/request.rs:99
pub struct Request { pub id: u64, pub body: RequestBody }
// crates/fleet-proto/src/response.rs:21
pub struct Response { pub id: u64, pub result: Result<ResponseBody, ProtoError> }
```

`Response.result` serializes as `{"result":{"Ok":…}}` / `{"Err":…}`. `RequestBody` is internally
tagged (`#[serde(tag = "type", rename_all = "snake_case")]`, `request.rs:108`, 100 variants);
`ResponseBody` (`response.rs:216`) and `Event` are adjacently tagged (`tag = "type"`,
`content = "data"`).

**Divergence worth knowing.** Zed binds a request to its response type at compile time via
`request_messages!` (`zed/crates/proto/src/proto.rs:405`), so `client.request(GetHover{..})`
returns `GetHoverResponse` with no downcast. fleetd's `Response.result` is one flat
`ResponseBody` enum that every caller re-matches. A `trait FleetRequest { type Response }` would
close that, but the macro layer only pays for itself when a schema compiler generates the payloads
— which fleetd does not have. Low priority.

**When NOT to wrap.** A foreign protocol you do not own (LSP, DAP, MCP) keeps its own envelope;
Zed does exactly that (`zed/crates/lsp/src/lsp.rs:259`).

---

## correlation

**Insert the pending entry before the frame goes out.**

Zed (`zed/crates/rpc/src/peer.rs:456-479`, `request_dynamic`) allocates the id, inserts the `oneshot`
into `response_channels`, and *then* pushes onto `outgoing_tx` — all inside one closure, so a
response cannot arrive before its slot exists. Two details worth copying:

- The reader sends `(envelope, received_at, resume_tx)` and awaits `resume_rx` (`peer.rs:305-318`),
  which parks the reader until the requester resumes — preserving order between a response and the
  messages that follow it.
- `response_channels` is `Arc<Mutex<Option<HashMap<..>>>>`; the `Option` is the closed flag.
  `_end_connection` (`peer.rs:156`) `take()`s it on teardown so in-flight requests fail fast with
  "connection was closed" instead of hanging.

fleetd's equivalent is a single actor task with no locks on the hot path:

```rust
// crates/fleet-client/src/connection.rs:175
pub async fn request(&self, body: RequestBody) -> Result<ResponseBody, ProtoError> {
    let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
    let (response_tx, response_rx) = oneshot::channel();
    let enqueue_deadline = Instant::now() + REQUEST_TIMEOUT;
    let expires_at = request_timeout(&body).map(|duration| Instant::now() + duration);
    let command = Command { request: Request { id, body }, response: Some(response_tx), expires_at };
    timeout_at(enqueue_deadline, self.inner.commands.send(command))
        .await
        .map_err(|_| transport_error("Fleet daemon request timed out"))?
        .map_err(|_| transport_error("Fleet daemon connection is closed"))?;
    // … then await response_rx under `expires_at`, if any
}
```

`HashMap<u64, Pending>` is owned solely by `run_connection` (`connection.rs:288`, `:290`). Ids `0`
and `1` are reserved for `Hello` and `Subscribe` (`connection.rs:687`); ordinary requests start at
`2`. `fail_pending` (`connection.rs:965`) drains the map on every disconnect path so no entry can
outlive its connection.

Fire-and-forget has no map entry: `send_background` (`connection.rs:265`) passes
`response: None` and, when the bounded command channel is full, spawns a deadline-bounded send
rather than awaiting inline. Errors from those background requests are still surfaced — they are
injected into the event broadcast as `Event::Toast` by `publish_background_error`
(`connection.rs:510`), which first drops non-actionable `NotFound`/`Cancelled` kinds to `debug` —
so a fire-and-forget caller still sees the failures that matter.

**Streaming.** Zed's `request_stream` registers an unbounded sender instead of a `oneshot`; the
responder emits N envelopes with the same `responding_to`, terminated by `EndStream` **or** `Error`
(`zed/crates/rpc/src/peer.rs:511`). A `util::defer` guard removes the stream channel when the
consumer drops the stream (`peer.rs:541`), and a failed `unbounded_send` prunes it too
(`peer.rs:643`). fleetd has no streaming responses; its equivalent is the event broadcast plus
`FrameUpdate` deltas, which is simpler and should stay that way.

---

## dispatch

**Never drop a request. Answer it, even if the answer is an error.**

Zed keys handlers by `TypeId` and panics on duplicate registration, with `#[track_caller]` so the
panic names the offending file:line:

```rust
// zed/crates/rpc/src/proto_client.rs:117
fn add_message_handler(&mut self, message_type_id: TypeId, entity: AnyWeakEntity, handler: ProtoMessageHandler) {
    self.entities_by_message_type.insert(message_type_id, entity);
    let prev_handler = self.message_handlers.insert(message_type_id, handler);
    if prev_handler.is_some() { panic!("registered handler for the same message twice"); }
}
```

(also `zed/crates/client/src/client.rs:825`, `zed/crates/collab/src/rpc.rs:766`). An unhandled
message is answered with `ErrorCode::Internal "message {name} was not handled"`
(`zed/crates/client/src/client.rs:1845` → `zed/crates/rpc/src/peer.rs:684`).

fleetd gets the same guarantee from the type system instead: three exhaustive `match` layers —
connection-local (`crates/fleet-daemon/src/server/connection.rs:167-226`), routing
(`crates/fleet-daemon/src/services/router/classify.rs:21` producing
`Target::{Local, Host, Fanout, Unsupported}`), and local execution
(`crates/fleet-daemon/src/services/dispatch.rs:25`, 105 `RequestBody::` arms). A missing arm is a
compile error. Keep the `match`; a `TypeId` registry buys nothing for a closed enum.

The one gap the `match` does not cover: a *known* variant a given host cannot serve. Return
`ErrorKind::Unsupported` (`crates/fleet-proto/src/error.rs:8`), never close the connection —
closing on an undecodable frame is already the reader's behavior
(`server/connection.rs:155-157`), and it should not be reused for a semantic refusal.

Zed also buffers messages addressed to an entity that does not exist yet
(`EntityMessageSubscriber::Pending`, `zed/crates/rpc/src/proto_client.rs:180`, created by
`subscribe_to_entity`, `zed/crates/client/src/client.rs:753`). fleetd has no remote-entity
addressing, so this pattern does not apply.

---

## ordering

**Classify by ordering need, not by importance.**

Zed classifies every message type as `MessagePriority::{Foreground, Background}`
(`zed/crates/proto/src/typed_envelope.rs:94`) — 230 of ~380 types are `Background` — and the collab
server picks a scheduling strategy from it, with the deadlock spelled out in a comment:

```rust
// zed/crates/collab/src/rpc.rs:947
// Handlers for foreground messages are pushed into the following `FuturesUnordered`.
// This prevents deadlocks when e.g., client A performs a request to client B and
// client B performs a request to client A. …
const MAX_CONCURRENT_HANDLERS: usize = 256;
let mut foreground_message_handlers = FuturesUnordered::new();
let concurrent_handlers = Arc::new(Semaphore::new(MAX_CONCURRENT_HANDLERS));
if is_background { executor.spawn_detached(handle_message); }
else { foreground_message_handlers.push(handle_message); }
```

The permit is acquired *before* reading the next message, so the semaphore is real admission
control.

fleetd's axis is different and better matched to its workload — byte-stream ordering:

```rust
// crates/fleet-daemon/src/server/connection.rs:482
fn pty_input_request_is_ordered(body: &RequestBody) -> bool {
    matches!(body,
        RequestBody::StartWatch { .. } | RequestBody::AppendWatchOutput { .. }
        | RequestBody::FinishWatch { .. } | RequestBody::TerminalInput { .. }
        | RequestBody::TerminalKey { .. } | RequestBody::TerminalMouse { .. }
        | RequestBody::PasteTerminal { .. } | RequestBody::WheelTerminal { .. }
        | RequestBody::ScrollOrKeyTerminal { .. } | RequestBody::ScrollTerminal { .. })
}
// :498
fn terminal_request_is_serialized(body: &RequestBody) -> bool {
    pty_input_request_is_ordered(body)
        || matches!(body, RequestBody::AttachTerminal { .. }
            | RequestBody::DetachTerminal { .. } | RequestBody::ResizeTerminal { .. })
}
```

Serialized requests run inline; everything else joins the `FuturesUnordered` pool bounded at
`MAX_PENDING_REQUESTS = 64` (`server/connection.rs:36`, `:140`), which back-pressures the reader.
The reasoning is documented on `run_terminal_request` (`:478`): "Input, attachment membership, and
PTY dimensions are one ordered piece of per-connection state. Running them concurrently would let a
resize overtake input or let two Attach calls both observe a missing membership and leak a service
refcount."

**Gap.** There is no priority axis on top of the ordering axis, so a slow non-terminal request can
occupy all 64 pending slots ahead of an interactive one. If that ever bites, a
`RequestBody::priority()` is the shape to add — not a second pool.

---

## backpressure

**Bounded incoming; never make application code block to send.**

```rust
// zed/crates/rpc/src/peer.rs:125
// For outgoing messages, use an unbounded channel so that application code
// can always send messages without yielding. For incoming messages, use a
// bounded channel so that other peers will receive backpressure if they send
// messages faster than this peer can process them.
#[cfg(any(test, feature = "test-support"))]
const INCOMING_BUFFER_SIZE: usize = 1;      // 1 in tests: forces interleaving
#[cfg(not(any(test, feature = "test-support")))]
const INCOMING_BUFFER_SIZE: usize = 256;
```

The LSP reader applies the same idea at the OS-pipe level and tests it —
`INCOMING_MESSAGE_QUEUE_CAPACITY = 128` (`zed/crates/lsp/src/input_handler.rs:27`) with
`test_backpressure_when_messages_are_not_consumed` (`:146`) asserting
`received <= CAPACITY + 2` while the consumer is wedged.

fleetd is bounded on **both** sides, which is stricter and correct: `COMMAND_CAPACITY = 256`,
`EVENT_CAPACITY = 1_024` (`crates/fleet-client/src/connection.rs:40-41`);
`OUTBOUND_QUEUE_CAPACITY = 64`, `MAX_PENDING_REQUESTS = 64`
(`crates/fleet-daemon/src/server/connection.rs:35-36`). The "never block application code" half is
preserved by `send_background`'s spawn-on-full fallback (`connection.rs:265`).

Zed's unbounded outgoing channel is only safe because `WRITE_TIMEOUT` kills a connection that
cannot drain. Unbounded + no timeout is a memory leak.

---

## lag

**A slow subscriber gets resynced, not disconnected.**

fleetd, on `broadcast::error::RecvError::Lagged`:

```rust
// crates/fleet-daemon/src/server/connection.rs:276
Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
    tracing::warn!(skipped, "event stream lagged; resyncing the client instead of dropping it");
    request_full_frames(&self.services, owner_id, &client, &attached).await;
    self.events.request_snapshot(Arc::clone(&self.services));
}
```

The nine-line comment above it (`:266-275`) is part of the pattern: a streaming agent turn publishes
an event per 16 ms delta tick, and dropping the connection for a few hundred ms of client stall is
exactly the failure `AgentThreadSnapshot { from_seq }` recovery exists to avoid.
`request_full_frames` (`:508`) re-requests a full frame per attached terminal, because a broadcast
gap does not identify which terminal lost rows.

The client mirrors it per attachment: on `Lagged` it resets `received_full` and sends
`RequestFullFrame` instead of dropping the attachment
(`crates/fleet-client/src/terminal.rs:296-298`); frames are suppressed until the first `full: true`
frame arrives (`:270`, `:278-281`).

Zed has no equivalent — its bounded incoming channel applies OS-level backpressure instead. Both
are valid; fleetd's is required because the daemon's bus is a `broadcast`, which drops rather than
blocks.

---

## timeouts

**Every wire await has a deadline; the exemptions are documented.**

Zed's transport layer (`zed/crates/rpc/src/peer.rs:94-96`, loop at `:167-249`):

```rust
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);   // send Ping if idle
const WRITE_TIMEOUT: Duration      = Duration::from_secs(2);   // any single write
pub const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10); // idle → connection is dead
```

One `select_biased!` loop owns writing, keepalive, reading and the idle timer; every branch has its
own `WRITE_TIMEOUT` guard, including delivery of an incoming message into the bounded channel.
`create_timer` is injected so tests run on the GPUI virtual clock
(`add_test_connection`, `peer.rs:372`).

Zed's LSP layer additionally returns a **tri-state** result rather than a flattened `Err`:

```rust
// zed/crates/util/src/util.rs:772
pub enum ConnectionResult<O> { Timeout, ConnectionReset, Result(anyhow::Result<O>) }
```

`ConnectionReset` means the response oneshot was `Canceled` — the server dropped the handler — and
is logged distinctly from a timeout. The timeout branch removes the response handler before
returning so the map does not leak, and a pending request sends `$/cancelRequest` on drop via a
`defer` guard that is `abort()`ed on success (`zed/crates/lsp/src/lsp.rs:1515`).

fleetd's budgets:

| Bound | Value | Where |
| --- | --- | --- |
| `REQUEST_TIMEOUT` | 10 s | `crates/fleet-client/src/connection.rs:35` |
| `WRITE_BUDGET` | 10 s | `connection.rs:36` |
| `HANDSHAKE_TIMEOUT` (client) | 3 s | `connection.rs:37` |
| `HANDSHAKE_TIMEOUT` (daemon) | 2 s | `crates/fleet-daemon/src/server/connection.rs:33` |
| `SOCKET_WRITE_TIMEOUT` | 2 s | `server/connection.rs:34` |
| `hello_timeout` (remote link) | 10 s | `crates/fleet-daemon/src/machines/link.rs:63` |
| `START_TIMEOUT` / `PROBE_INTERVAL` (connect bridge) | 10 s / 50 ms | `server/bridge.rs:14-15` |

The daemon handshake is enforced by wrapping the whole negotiation, not each read:

```rust
// crates/fleet-daemon/src/server/connection.rs:413
async fn negotiate_hello_with_timeout(…) -> DaemonResult<Option<HelloClient>> {
    tokio::time::timeout(timeout, negotiate_hello(framed, services))
        .await
        .map_err(|_| DaemonError::Timeout("client Hello handshake".to_owned()))?
}
```

**The exemption pattern is fleetd's and is better documented than Zed's** — copy it verbatim when
adding a new long-running request family (`crates/fleet-client/src/connection.rs:590`):

```rust
fn request_timeout(body: &RequestBody) -> Option<Duration> {
    if matches!(body,
        RequestBody::CreateWorktree { .. } | RequestBody::CreateWorktreeFromPr { .. }
        | RequestBody::CreateWorktreeFromCard { .. }
        // A board backend validates and describes itself by shelling out to its own CLI,
        // which has a deadline and a retry budget of its own an order of magnitude past
        // this one. Timing these out here replaces the backend's own sentence — the
        // install hint, the throttling notice, the JQL Jira refused — with a transport
        // error, while the daemon keeps running the call the client stopped waiting for.
        | RequestBody::CreateBoard { .. } | RequestBody::UpdateBoard { .. }
        | RequestBody::DescribeBoardBackend { .. }
    ) { None } else { Some(REQUEST_TIMEOUT) }
}
```

This implements ADR 0008 (`docs/decisions/0008-board-model-and-sync.md`).

**Gap: no receive-idle timeout anywhere in fleetd.** A wedged peer holding an open socket is only
noticed by the app's health ping — `HEALTH_INTERVAL = 2 s`, `HEALTH_TIMEOUT = 3 s`
(`crates/fleet-app/src/bridge.rs:45`, `:47`). The `fleet` CLI and the `fleetd`↔`fleetd` link have
no idle detection at all. `RemoteLink::connected_loop` (`crates/fleet-daemon/src/machines/link.rs:444`)
is where a `RECEIVE_TIMEOUT` equivalent belongs.

---

## keepalive

**Two layers: transport keepalive and session heartbeat.**

Zed's transport can only detect "socket dead"; its session layer must also detect "the remote
process is wedged but the pipe is open". Hence a second set of constants
(`zed/crates/remote/src/remote_client.rs`):

```rust
const MAX_MISSED_HEARTBEATS: usize = 5;                          // :160
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);     // :161
const HEARTBEAT_TIMEOUT: Duration  = Duration::from_secs(5);     // :162
const INITIAL_CONNECTION_TIMEOUT: Duration =
    Duration::from_secs(if cfg!(debug_assertions) { 5 } else { 60 }); // :163
pub const MAX_RECONNECT_ATTEMPTS: usize = 3;                     // :166
```

The heartbeat task races the ping against a `connection_activity_rx` signal fed by the stdout and
stderr readers (`zed/crates/remote/src/transport.rs:170`, `:210`) — **any** traffic counts as a
heartbeat, so a busy link never pays for pings.

fleetd has `RequestBody::DaemonPing` and the app pings every 2 s, but `RemoteLink` never pings: it
only notices failure on a write or decode error. Adding a `HEARTBEAT_INTERVAL` /
`MAX_MISSED_HEARTBEATS` loop to `crates/fleet-daemon/src/machines/link.rs` — with the
any-traffic-counts optimisation — is the highest-value liveness fix in the crate.

---

## state

**Model the link as an explicit state enum, not booleans.**

Zed uses two shapes. The task-owning one (`zed/crates/remote/src/remote_client.rs:168`):

```rust
enum State {
    Connecting,
    Connected { remote_connection, delegate, multiplex_task, heartbeat_task },
    HeartbeatMissed { missed_heartbeats: usize, remote_connection, delegate, multiplex_task, heartbeat_task },
    Reconnecting,
    ReconnectFailed { remote_connection, delegate, error, attempts },
    ReconnectExhausted,
    ServerNotRunning,
}
```

with `can_reconnect()` / `heartbeat_missed()` / `heartbeat_recovered()` as *total* transitions
(`:226`, `:256`, `:274`) and a separate public projection for the UI (`:308`), so internal states
can be added without changing the UI contract. And the flat, `Copy`, watch-friendly one
(`zed/crates/client/src/client.rs:280`), whose `ReconnectionError { next_reconnection: Instant }`
lets the UI render a countdown instead of a spinner; `set_status` (`client.rs:686`) owns every side
effect in one place.

fleetd's equivalents:

- `LinkState { Connecting, Ready, Down, Legacy }` published on a `watch::Sender<LinkState>`
  (`crates/fleet-daemon/src/machines/link.rs:102`, set at `:210`, `:221`, `:235`), exposed by
  `RemoteEndpoint::state_changes()` (`:85`) and mirrored into `Event::HostLinkChanged`.
- `BridgeEvent::{Connected, ConnectFailed, ProtocolMismatch, Disconnected { attempt },
  Reconnected { restarted }}` (`crates/fleet-app/src/bridge.rs:88-128`). `Reconnected { restarted }`
  is a genuinely better signal than Zed's flat `Connected` — it tells the UI whether the daemon
  itself was replaced.

Add a variant rather than a parallel boolean; a boolean beside a state enum is how a state machine
becomes unprovable.

---

## reconnect

**Fail pending, replay queued, jitter the delay.**

Zed backs off exponentially with jitter and a seeded RNG under test-support
(`zed/crates/client/src/client.rs:87`, backoff loop at `:697`):

```rust
pub const INITIAL_RECONNECTION_DELAY: Duration = Duration::from_millis(500);
pub const MAX_RECONNECTION_DELAY: Duration = Duration::from_secs(30);
#[cfg(any(test, feature = "test-support"))] let mut rng = StdRng::seed_from_u64(0);
#[cfg(not(any(test, feature = "test-support")))] let mut rng = StdRng::from_os_rng();
let jitter = Duration::from_millis(rng.random_range(0..delay.as_millis() as u64));
cx.background_executor().timer(delay + jitter).await;
delay = cmp::min(delay * 2, MAX_RECONNECTION_DELAY);
```

Zed's remote transport is additionally **resumable**: every sent envelope is buffered in a
`VecDeque` and stamped with `ack_id = max_received`
(`zed/crates/remote/src/remote_client.rs:2029`); the receiver prunes the peer's buffer on each
incoming `ack_id` (`:1773`), and `resync()` sends an unbuffered `FlushBufferedMessages` before
replaying (`:1903`).

**Do not port the replay buffer.** It is only worth its cost when messages are state deltas that
cannot be recomputed. fleetd is request/response: the client can simply re-ask.
`fail_pending` (`crates/fleet-client/src/connection.rs:965`) resolves every in-flight `oneshot` with
a transport error, and `queue_disconnected_command` (`:652`) keeps the queue meaningful by
coalescing consecutive `ResizeTerminal` for one terminal to the newest — the same idea applied
narrowly. On reconnect, `establish` (`:687`) replays `Subscribe`, remembered attachments and
capabilities; a re-attach answered with `ErrorKind::NotFound` drops that attachment silently rather
than failing the whole reconnect (`:746-749`).

fleetd's own better-than-Zed piece: `RemoteLink::nudge_reconnect`
(`crates/fleet-daemon/src/machines/link.rs:307`) wakes a link sleeping in backoff and restarts the
delay at the floor, holding a retained permit so a nudge racing a disconnect is not lost. Zed has no
equivalent.

**The gap is jitter.** `INITIAL_RECONNECT_BACKOFF = 50 ms` → `MAX_RECONNECT_BACKOFF = 2 s`
(`connection.rs:38-39`) and `backoff_min = 1 s` → `backoff_max = 60 s`
(`machines/link.rs:61-62`, doubled at `:423` by `doubled()` `:753`) both double with no jitter. Many
hosts reconnecting to one restarted daemon is a thundering herd.

---

## versioning

**Additive by default; a version bump is the last resort; capabilities carry the rest.**

Zed pins one version and rejects on inequality, which is only humane because Zed auto-updates:

```rust
// zed/crates/rpc/src/rpc.rs:19
pub const PROTOCOL_VERSION: u32 = 68;
// zed/crates/collab/src/rpc.rs:1236
if protocol_version != rpc::PROTOCOL_VERSION {
    return (StatusCode::UPGRADE_REQUIRED, "client must be upgraded".to_string()).into_response();
}
```

Field-level evolution is additive and documented in-line — 96 `reserved` clauses across the
`.proto` files, and a migration note where a field is being retired
(`zed/crates/proto/proto/worktree.proto:121-125`: "once all supported peers use `TrashProjectEntry`,
remove this field and replace it with `reserved 3;` so the tag is never reused").

fleetd matches on the literal too, but adds the thing Zed lacks — capabilities:

```rust
// crates/fleet-daemon/src/server/connection.rs:376
RequestBody::Hello { protocol: fleet_proto::PROTOCOL_VERSION, client } => (Ok(…), Some(client)),
RequestBody::Hello { protocol, .. } => (Err(DaemonError::Unsupported(format!(
    "unsupported protocol {protocol}; expected {}", fleet_proto::PROTOCOL_VERSION))), None),
_ => (Err(DaemonError::Protocol("Hello must be the first request".to_owned())), None),
```

```rust
// crates/fleet-proto/src/lib.rs:4,7
pub const PROTOCOL_VERSION: u32 = 7;
pub const REMOTE_MACHINES_CAPABILITY: &str = "remote-machines";
// crates/fleet-proto/src/response.rs:29
pub const PRUNE_REVIEWED_IDS_CAPABILITY: &str = "prune.reviewed_ids";
```

The JSON analogue of a new protobuf field number is a `#[serde(flatten)]` side-envelope, which an
older peer deserializes as the base type and ignores:

```rust
// crates/fleet-proto/src/response.rs:37
pub struct HelloResponse {
    #[serde(flatten)] pub response: Response,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub capabilities: Vec<String>,
    #[serde(default)] pub daemon_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub build_commit: Option<String>,
}
```

`PongResponse` (`:67`) does the same for `DaemonIdentity { pid, boot_id }`. These are the crate's
only two `flatten`s — keep it that way; `flatten` is for envelope metadata, not for payloads.

Serde conventions in `fleet-proto` (97 `#[serde(` lines): structs `rename_all = "camelCase"` (33),
enums `rename_all = "snake_case"` (19) — with `ErrorKind` on `kebab-case` and `Tmux` renamed to
swarm's `tmux` (`error.rs:7`, `:16-17`). `#[serde(default…)]` appears 35 times, `skip_serializing_if`
22, and `deny_unknown_fields` **zero** times: unknown fields are silently ignored, which is what
makes forward compatibility work and is why a rename is invisible until it corrupts data.

Version bumps update `docs/ARCHITECTURE.md` § "Protocol compatibility" (`:343`) and
`docs/REMOTE-MACHINES.md` § 3 (`:38`) in the same commit. Remote daemon links require the *same*
version — routing never crosses incompatible builds — so a bump is a fleet-wide upgrade event.

**Legacy-peer fallback.** `establish_before` retries with a legacy Hello shape when the first
attempt fails a specific way (`crates/fleet-daemon/src/machines/link.rs:542`), and `HelloClient`
deserializes both a bare string and the current struct
(`crates/fleet-proto/src/request.rs:90`). That is the pattern for retiring a shape without a version
bump.

---

## goldens

**Byte-exact fixtures, plus a legacy-payload test per compatibility claim.**

```rust
// crates/fleet-proto/tests/compatibility.rs:23
fn assert_frame<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(message: T, golden: &str) {
    let mut codec = FleetCodec::<&T, T>::new();
    let mut frame = BytesMut::new();
    codec.encode(&message, &mut frame).unwrap();
    assert_eq!(&frame[..4], &(golden.len() as u32).to_be_bytes());
    assert_eq!(&frame[4..], golden.as_bytes());

    // Decode the fixed fixture independently of the encoder's output.
    let mut fixture = BytesMut::new();
    fixture.extend_from_slice(&(golden.len() as u32).to_be_bytes());
    fixture.extend_from_slice(golden.as_bytes());
    assert_eq!(codec.decode(&mut fixture).unwrap(), Some(message));
    assert!(fixture.is_empty());
}
```

Covering `request_wire_goldens` (`:42`), `response_wire_goldens` (`:174`), `event_wire_goldens`
(`:321`), plus two compatibility proofs that are the model for any new claim:
`hello_metadata_accepts_old_and_new_ipc_v4_envelopes` (`:280`) shows a `"protocol":6` payload still
parses, and `terminal_attention_fields_default_for_legacy_peers` (`:365`) shows additive terminal
fields default. Add `assert_round_trip` (`crates/fleet-proto/src/lib.rs:25`) for every new type; `error.rs:63` is
an example.

Zed's `proto` crate has no goldens at this granularity. Its nearest analogue is
`test_buffer_size` (`zed/crates/rpc/src/message_stream.rs:115`), which asserts the reusable
encode/decode buffer never stays above `MAX_BUFFER_LEN = 1 MiB` after a huge message — a memory
retention bug class, tested.

---

## framing

**Match the framing to the transport, and cap the frame.**

Zed uses three framings: `u32` LE length prefix + protobuf over the SSH byte stream
(`zed/crates/remote/src/protocol.rs:38`), zstd + protobuf over the collab WebSocket with a 1 MiB
buffer cap (`zed/crates/rpc/src/message_stream.rs:13`), and `Content-Length:` + JSON-RPC for LSP
because that is the spec. `read_message_raw` / `write_size_prefixed_buffer`
(`protocol.rs:64`, `:54`) let the proxy relay frames without decoding them.

**Zed has no maximum frame size check** — `read_message_with_len`
(`zed/crates/remote/src/protocol.rs:16`) resizes a buffer to an attacker-controlled `u32`. That is
acceptable for a process you spawned over your own SSH session and unacceptable for a socket anyone
on the box can connect to. fleetd is stricter and right to be:

```rust
// crates/fleet-proto/src/codec.rs:11
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;  // payload only, excludes the 4-byte prefix
// :87
const RETAINED_CAPACITY: usize = 64 * 1024;
fn roll_back(destination: &mut BytesMut, frame_start: usize) {
    destination.truncate(frame_start);
    if destination.capacity() > RETAINED_CAPACITY {
        let retained = BytesMut::from(&destination[..]);
        *destination = retained;
    }
}
```

The limit is checked on encode (`:73`) **and** on the announced decode length (`:132`), before any
allocation. The encoder backpatches the big-endian prefix after streaming JSON through a counting
writer and counts oversize bytes without buffering them, so `FrameTooLarge.actual` is truthful
(`:60-83`, `:104-112`). `roll_back` truncates a rejected frame and shrinks past `RETAINED_CAPACITY`
while leaving already-queued frames intact. Tested by
`accepts_the_exact_payload_limit_and_rolls_back_oversized_encoding` (`:213`) and
`rejects_announced_oversized_frames` (`:286`).

`FleetCodec<Encode, Decode>` is a zero-sized `PhantomData` pair (`:32-46`) so client and server
instantiate opposite directions from one type.

---

## errors

**A stable machine-readable kind, plus one human line.**

Zed keeps `anyhow::Error` as the universal currency and adds two extension traits so a code and its
tags survive the wire (`zed/crates/proto/src/error.rs:44` `ErrorCodeExt`, `:77` `ErrorExt`):

```rust
// doc comment, verbatim, zed/crates/proto/src/error.rs
/// return Err(Error::WrongReleaseChannel.with_tag("required", "stable").into())
/// match err.error_code() {
///   ErrorCode::Forbidden => alert("I'm sorry I can't do that."),
///   ErrorCode::WrongReleaseChannel =>
///     alert(format!("You need to be on the {} release channel.", err.error_tag("required").unwrap())),
```

`ErrorExt for anyhow::Error` downcasts to `RpcError` if present and otherwise degrades to
`ErrorCode::Internal` with the `{self:#}` chain flattened to one line — so an untagged internal
error crosses the wire without leaking a multi-line backtrace into a log field.
`RpcError::from_proto(error, request_type)` (`:170`) re-attaches the request name so `Display`
reads `"RPC request GetHover failed: …"`.

fleetd:

```rust
// crates/fleet-proto/src/error.rs:8
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind { NotFound, Conflict, Git, #[serde(rename = "tmux")] Tmux, Fs, Github,
    Remote, Validation, Cancelled, Unsupported, Unknown }
// :37
pub struct ProtoError { pub kind: ErrorKind, pub message: String }
```

The typed client API returns `Result<T, ProtoError>` and synthesises transport failures as
`ErrorKind::Unknown` (`crates/fleet-client/src/api.rs:21`,
`crates/fleet-client/src/connection.rs:973` `transport_error`). The `Tmux` serde rename preserves
swarm's on-wire name — a live example of why a rename needs a golden.

**The one Zed borrowing worth making:** `tags: Vec<String>` of `key=value` pairs on `ProtoError`, so
a UI can render "install `<tool>`" or "your Jira query was rejected: `<jql>`" without parsing the
message string. Today `message` is the only structured-ish field, and UIs that need a detail have to
substring-match it.

---

## process

**Lock the pid file, unlink only what you still own, confirm readiness with a ping.**

Zed's remote server (`zed/crates/remote_server/src/server.rs`) uses one directory per connection
identity holding `server.pid`, `stdin.sock`, `stdout.sock`, `stderr.sock` (`:763`, `:786`):
`check_pid_file` (`:1148`) reads the pid, looks it up via `sysinfo`, and deletes a stale file;
`kill_running_server` (`:990`) kills then unlinks pid **and all three sockets**; `spawn_server`
(`:1037`) polls for the socket files to appear, 20 ms apart, up to 10 s; `write_pid_file` (`:1175`)
removes any existing file first. Failure to launch gets a dedicated exit code:

```rust
// zed/crates/remote/src/proxy.rs:3-10
#[derive(Copy, Clone, Error, Debug)] #[repr(i32)]
pub enum ProxyLaunchError {
    // We're using 90 as the exit code, because 0-78 are often taken
    // by shells and other conventions and >128 also has certain meanings
    #[error("Attempted reconnect, but server not running.")]
    ServerNotRunning = 90,
}
```

`RemoteClient::monitor` maps 90 → `State::ServerNotRunning` (terminal, no retry) versus anything
else → reconnect.

fleetd is **stronger** on the file side. `SingletonGuard::acquire`
(`crates/fleet-daemon/src/server/listener.rs:68`) takes `flock(LOCK_EX | LOCK_NB)` on
`<home>/fleetd.pid` (`:84`) — real single-instance enforcement, not a pid lookup that races — and
records a `FileIdentity { dev, ino }` (`:42`, `:47`) for the pid file and the socket, so
`remove_if_owned` (`:323`) only unlinks a file whose inode still matches. A dying predecessor can
never delete its replacement's socket. A stale socket is probed with a connect before being unlinked
(`:146-154`). Admission is bounded by a `Semaphore`, `MAX_CONNECTIONS = 128` (`:29`, `try_admit`
`:236`).

The spawn path is stronger too. `ensure_daemon` (`crates/fleet-client/src/spawn.rs:69`) tries
connect + ping first and only checks the pid file on `ConnectError::Io`; the detached child is
reaped by a plain OS thread named `fleetd-reaper` (`:110`), deliberately not a tokio task, so
reaping survives runtime shutdown; startup polls every 50 ms up to 10 s and finishes with a **ping**,
not with "the socket file exists". Binary resolution is `$FLEET_DAEMON` → sibling of
`current_exe()` → `fleetd` on `PATH` (`:182-196`). `restart_daemon` (`:139`) does graceful shutdown,
then a `verified_signal_target` check comparing pid **+ executable + start time** (`:274`) so a
recycled PID is never signalled. The remote side detaches with `setsid` in `pre_exec`
(`crates/fleet-daemon/src/server/bridge.rs:92-98`).

**Do not copy Zed's `IDLE_TIMEOUT = 10 * 60 s`** (`zed/crates/remote_server/src/server.rs:410`). A
headless project is disposable; `fleetd` owns long-lived PTYs and must outlive every client.

**The gap is the exit code.** `crates/fleet-daemon/src/server/bridge.rs` returns
`anyhow::Error`, and `SpawnError` (`crates/fleet-client/src/spawn.rs:35`) has no `#[repr(i32)]`
mapping — so `RemoteLink` cannot distinguish "remote fleetd is not installed" (terminal) from
"the transport blipped" (retry).

**Graceful shutdown ordering is load-bearing and should be commented.** Zed says so explicitly
(`zed/crates/remote/src/remote_client.rs:575-582`): drop `multiplex_task` first *because it owns the
proxy process, a child of master_process*, then the heartbeat task, the connection, the delegate.
`kill_on_drop(true)` on every spawned child (`zed/crates/remote/src/transport/ssh.rs:517`, with the
comment "IMPORTANT: we kill this process when we drop the task that uses it") makes dropping a task
the kill mechanism. fleetd's daemon-side ordering (`server/connection.rs:316-317` `drop(pending);`
`drop(outbound);`, then the writer drained under `SOCKET_WRITE_TIMEOUT`) is correct but uncommented — add the sentence when you next touch it.

---

## transport

**One trait, a mock implementation, and no wall-clock sleeps in tests.**

Zed abstracts the client itself: `ProtoClient` (`zed/crates/rpc/src/proto_client.rs:58`) is a
five-method trait implemented three times — `Client` (collab WebSocket), `ChannelClient` (remote
stdio) and `NoopProtoClient` for tests — and everything above the transport takes the erased
`AnyProtoClient`, so `HeadlessProject` on a remote host and the collab server run the *same* handler
code:

```rust
pub trait ProtoClient: Send + Sync {
    fn request(&self, envelope: Envelope, request_type: &'static str) -> BoxFuture<'static, Result<Envelope>>;
    fn request_stream(&self, envelope: Envelope, request_type: &'static str)
        -> BoxFuture<'static, Result<BoxStream<'static, Result<Envelope>>>>;
    fn send(&self, envelope: Envelope, message_type: &'static str) -> Result<()>;
    fn send_response(&self, envelope: Envelope, message_type: &'static str) -> Result<()>;
    fn message_handler_set(&self) -> &parking_lot::Mutex<ProtoMessageHandlerSet>;
    fn is_via_collab(&self) -> bool;
    fn has_wsl_interop(&self) -> bool;
}
```

`is_via_collab` / `has_wsl_interop` leak transport identity through the abstraction — a smell Zed
tolerates for two behavioral forks. Keep the count of such methods at zero.

fleetd abstracts the *stream* instead, which suits a single protocol:

```rust
// crates/fleet-client/src/connection.rs:44
pub type ProtocolTransport<S> = Framed<S, FleetCodec<Request, Value>>;
// :52
pub fn protocol_transport<S>(stream: S) -> ProtocolTransport<S>
where S: AsyncRead + AsyncWrite + Unpin { Framed::new(stream, FleetCodec::new()) }
// crates/fleet-daemon/src/machines/provider.rs:39
pub trait AsyncDuplex: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T> AsyncDuplex for T where T: AsyncRead + AsyncWrite + Send + Unpin {}
```

Decoding into `serde_json::Value` rather than `Response` is what lets extended envelopes survive an
older client. `MachineProvider::open_stream() -> Result<Box<dyn AsyncDuplex>, MachineError>`
(`provider.rs:44-58`) is the port; `SshMachine`, `TailscaleMachine`, `CommandMachine` and
`FakeMachine` are the adapters.

Test doubles that exist today:

- `tests/generic_stream.rs:11` drives the codec over `tokio::io::duplex(4 * 1024)`.
- `FakeMachine` (`crates/fleet-daemon/src/testing/machines.rs:34`) scripts resolve/probe/exec
  results, records every argv, and hands the caller one half of a `DuplexStream`.
- `FakeRemote` (`:125`) queues request results, records requests, emits events and drives
  `LinkState`.
- `RemoteDaemon::start(home)` (`crates/fleet-daemon/tests/infra/mod.rs`, used by
  `tests/remote_link.rs:42`, `tests/remote_lifecycle.rs:103`, `tests/agents_remote.rs:277`) starts a
  private daemon process with a loopback `CommandMachine`. Documented in `docs/REMOTE-MACHINES.md`
  § 13.

**Two gaps.** (1) `run_connection` (`crates/fleet-client/src/connection.rs:288`) still takes
`Established<UnixStream>`, so every client reconnect test binds a real `UnixListener` in a
`TempDir` (`crates/fleet-client/tests/client.rs`) — slow and timing-dependent. Generalising it over
`S: AsyncRead + AsyncWrite + Unpin` is mechanical, since the codec already is. (2) No fault
injection. Zed's `Connection::in_memory(executor)` (`zed/crates/rpc/src/conn.rs:32`) returns two
halves plus an `Arc<AtomicBool> killed`; once killed, **writes error and reads hang forever**,
modelling a half-open TCP connection — exactly the bug class an idle timeout would catch and the one
fleetd cannot test today. Zed's other seams worth mirroring:
`MockRemoteConnection::simulate_disconnect` (`zed/crates/remote/src/transport/mock.rs:244`) and
`force_heartbeat_timeout` on the real client (`zed/crates/remote/src/remote_client.rs:1066`).

---

## logging

**stdout is the frame stream; stderr is structured logs.**

Zed's parent process parses each stderr line as a JSON log record and replays it into its own
logger, falling back to a prefix rather than dropping unparseable output:

```rust
// zed/crates/remote/src/transport.rs:176 (stderr_task)
if let Ok(record) = serde_json::from_slice::<LogRecord>(content) { record.log(log::logger()) }
else { std::io::stderr().write_fmt(format_args!("(remote) {}\n", String::from_utf8_lossy(content))).ok(); }
```

`LogRecord` (`zed/crates/remote/src/json_log.rs:7`) carries `level` as a stable `usize` — not the
enum, which would break across versions — plus `module_path` / `file` / `line` / `message` as
`Cow<'a, str>`. `handle_rpc_messages_over_child_process_stdio` (`transport.rs:128`) runs three
background tasks (stdin write, stdout read, stderr read), `select!`s the first to finish, then
awaits the process status and converts a signal death to an exit code — so the *reason* the pipe
closed is always attached to the failure.

fleetd does not do this. `run_connect` (`crates/fleet-daemon/src/server/bridge.rs:20`) relays stdio
raw, and the remote daemon's own output goes to `logs/fleetd.out` on the remote host, so a remote
failure is only diagnosable by SSHing in. Adopting `LogRecord` on stderr plus a `RemoteLink` replay
into the local `tracing` subscriber is the single largest observability win available in the
machines crate. It composes with the daemon-wide gap the audit found — no `EnvFilter`, no spans, no
`#[instrument]` — but does not depend on fixing it first.
