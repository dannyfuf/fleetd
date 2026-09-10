# Remote machines contract

**AUTHORITATIVE — implementers code against this; changes require a DEVIATIONS entry.**

## 1. Purpose and implementation state

Fleet runs one `fleetd` per machine. The local daemon federates remote daemons; clients continue
to connect only to the local socket. This contracts the config, protocol, transport, link, router,
mirror, CLI, app, bootstrap, and testing seams used by the parallel implementation stages.

The config/protocol shapes, transports, reconnecting links, router, mirror, clients, and test
doubles are implemented. Unsupported operations return a typed error; production paths never use
`todo!` or `unimplemented!`.

## 2. Config schema

`HostConfigEntry` serializes a tagged provider first and falls back to the legacy untagged shape.

```json
{"provider":"tailscale","node":"dev-box","user":"df","sshOptions":[],"fleetd":"fleetd","fleetHome":"~/.fleet"}
```

```json
{"provider":"command","run":["sh","-c","exec \"$@\"","--"],"fleetd":"/tmp/fleetd","fleetHome":"/tmp/remote","display":"loopback"}
```

```json
{"ssh":"arch-dev","swarmCommand":"swarm"}
```

Tailscale has `node: String`, `user: Option<String>`, `ssh_options: Vec<String>`, `fleetd:
String` (default `fleetd`), and `fleet_home: Option<String>` (default `~/.fleet`). Command has
`run`, `fleetd`, optional `fleet_home`, and optional `display`; it is advanced/testing transport:
every remote argv executes locally as `run ++ argv`. Legacy is probe-only. Present string and run
entries must be nonempty. `defaultHost` is `local` or a configured id; `Config::default_host()`
returns `None` for local and the configured `HostId` otherwise.

## 3. Protocol v7

`PROTOCOL_VERSION` is 7. `Hello { protocol, client: HelloClient }` defaults the client to
`ClientKind::App`; kinds are `App | Cli | Proxy`, with optional `host_id`. `HelloResponse` retains
the correlated response and capabilities and adds `daemon_id: String` (persisted as
`$FLEET_HOME/daemon-id`; a file whose contents are not a valid `HostId` is moved aside to
`daemon-id.invalid` and a fresh identity is minted, with both ids logged, rather than aborting
startup) plus `build_commit: Option<String>`. Capability `remote-machines` marks
federation support.

`HostStatus` keeps `id`, `reachable`, `error`, and `checked_at`, and defaultably adds `provider`,
`version`, `link: Connecting | Ready | Down | Legacy`, `address`, and `agent_binaries: Option<
AgentBinaries { claude, opencode }>`. Requests add `BootstrapHost { host, git_ref }`; PR creation
adds defaultable `host`; `DoctorHost { host }` provides scoped diagnostics. `ResponseBody::Path { path, host }` carries optional ownership.
`DeleteWorktrees -> WorktreesDeleted`, `InspectWorktrees -> Inspections`, and `PruneWorktrees ->
Pruned` preserve per-item outcomes. Mixed-host dismiss/sleep/kill extensions must likewise return
one outcome per requested item rather than failing the whole request.

Events add `HostLinkChanged { host, link, version, error }` and `TerminalReattach { terminal }`.
All prior request, response, and event variants remain valid.

## 4. Machines module

```rust
pub struct MachineAddress { pub host: String, pub user: Option<String>, pub display: String, pub online: Option<bool> }
pub struct ProbeReport { pub reachable: bool, pub latency_ms: Option<u64>, pub version: Option<String>, pub error: Option<String>, pub stderr: Option<String> }
pub struct ExecOutput { pub status: i32, pub stdout: String, pub stderr: String }
pub trait AsyncDuplex: AsyncRead + AsyncWrite + Send + Unpin {}
#[async_trait] pub trait MachineProvider: Send + Sync {
    fn id(&self) -> &HostId;
    fn provider_name(&self) -> &'static str;
    async fn resolve(&self) -> Result<MachineAddress, MachineError>;
    async fn probe(&self, timeout: Duration) -> ProbeReport;
    async fn exec(&self, argv: &[String], timeout: Duration) -> Result<ExecOutput, MachineError>;
    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError>;
    fn fleetd_binary(&self) -> &str;
    fn fleet_home(&self) -> Option<&str>;
    fn warning(&self) -> Option<String>;
}
#[async_trait] pub trait MachineLifecycle: Send + Sync {
    async fn ensure_up(&self) -> Result<(), MachineError>;
    async fn shutdown(&self) -> Result<(), MachineError>;
}
```

`MachineError` is `Unreachable | Auth | NotFound | Protocol | Unsupported | Io | Timeout`.
Conversion maps unreachable/auth/io to daemon Remote, unsupported to Unsupported, timeout to
Timeout, and protocol/not-found to their namesakes. `ChildStream::spawn(argv)` pipes stdin/stdout,
kills on drop, exposes `exit_code().await`, and maps SSH exit 255 to Unreachable.
`CommandMachine` runs `run ++ argv`; its stream argv is `run ++ [fleetd, connect, --home,
fleet_home]`, and its probe runs `true` then `fleetd --version` without using connect.

`TailscaleMachine::new(id,node,user,ssh_options,fleetd,fleet_home)` and `SshArgv` are the transport
stage seam. `LegacyMachine` probes the existing swarm protocol and never opens a stream.
`Machines::{from_config,rebuild,get,iter,endpoint,install_endpoint,endpoints}` owns providers and
lazily creates one non-legacy `RemoteLink` per host.

## 5. RemoteEndpoint and RemoteLink

```rust
#[async_trait] pub trait RemoteEndpoint: Send + Sync {
    fn host(&self) -> &HostId;
    fn state(&self) -> LinkState;
    fn hello(&self) -> Option<RemoteHello>;
    fn last_snapshot_seen(&self) -> Option<Snapshot>;
    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody>;
    fn nudge_reconnect(&self) {}
    fn events(&self) -> broadcast::Receiver<Event>;
    fn state_changes(&self) -> watch::Receiver<LinkState>;
    async fn close(&self);
}
```

`RemoteHello` contains version, daemon id, build commit, and capabilities. A successful link
handshake orders `Hello -> Subscribe -> GetSnapshot -> Ready -> HostLinkChanged`; the endpoint
retains that snapshot across `Down` transitions. `RemoteLink::new(
provider, LinkOptions { backoff_min, backoff_max, hello_timeout }) -> Arc<Self>` creates the link;
`connect()` starts it. It sends Hello as Proxy with the local daemon id in `host_id`, correlates
responses by request id, publishes remote events untranslated, and reconnects with exponential
backoff from 1s to 60s. State progresses Connecting -> Ready; transport/handshake loss becomes Down.

Every write to the remote stream is bounded by `WRITE_BUDGET` (10s) and races link shutdown: a peer
that stops reading — the remote server stops polling its reader at `MAX_PENDING_REQUESTS` — becomes
an ordinary disconnect (pending requests fail, state Down, backoff reconnect) instead of wedging the
link actor. `close()` bounds the join on that actor the same way and aborts it if the deadline
passes, so a closed endpoint never keeps its stream or ssh child alive.

`nudge_reconnect` wakes a link sleeping in reconnect backoff so its next attempt runs immediately
and its backoff restarts at `backoff_min`. It is not a no-op on a Ready link: the permit is retained
when the actor is not sleeping, so a nudge that races a disconnect shortens the following attempt
instead of being lost. It never tears down an established transport.

## 6. Router

`Target` is `Local | Host(HostId) | Fanout(Vec<(HostId, RequestBody)>) | Unsupported(&'static str)`.
`Router::route` calls one exhaustive `RequestBody` match; adding a protocol variant must fail to
compile until classified. Worktree-scoped operations resolve the worktree; session, terminal, job,
and thread operations resolve that id; agent creation resolves its worktree. Explicit-host create
targets that host. Explicit bulk ids partition by host and fan out when any target is remote.
Agent/list/inspect/prune/bulk behavior is merged across local and endpoints. Config, context, repo,
board, Hello, Ping, host, Doctor, update, and daemon lifecycle commands are local orchestration.

Forwarding is: classify -> translate ids -> clear placement (`host=None`) -> `endpoint.request` ->
`response_to_local`. Fanout runs parts concurrently and `merge_fanout` retains per-item failures.
Terminal and job ids are bijectively remapped from a shared local counter; worktree ids pass
through and register ownership; thread UUIDs pass through but register ownership. Local session
ids are exactly `<host>/<remote>` and reverse by splitting the first slash. A Down transition
clears that host's mappings and triggers terminal-ended behavior; Ready rebuilds from snapshot.

`router/create.rs` owns ensure-repo-then-create; `sessions.rs` owns attach/detach frame hooks;
`lifecycle.rs` owns bulk lifecycle merge; `agents.rs` owns agent classification and thread-event
registration. Router-core owns `mod/classify/ids/translate`; later feature stages own their hook.

## 7. Mirror

`Mirror::apply(host,snapshot)` replaces a host fragment and records `received_at`; `mark_stale`
keeps the fragment visible but offline, and `clear` removes it. `fragment`, `host_of_worktree`,
`worktrees`, `sessions`, `statuses`, and `agent_threads` expose merged views. Remote worktrees carry
their host and remote session ids are prefixed. Local state wins for local records; the remote
daemon's fragment is the only source of truth for records owned by that host. Snapshot merge keeps
local contexts/repos/jobs and adds host-tagged remote worktrees/statuses/sessions/threads without
persisting the remote copies locally.

## 8. Proxied degradation

`fleet_core::sessions::default_terminals(config, agent, proxied)` replaces
`fleet://lazygit` with the plain `lazygit` PTY command when proxied. Native structured agent tabs
are explicitly exempt because clients draw them from protocol events.

## 9. Bootstrap job

`Bootstrap::start(host, git_ref)` submits a daemon job. Through `MachineProvider::exec`, it checks
git/cargo, creates or fetches `<fleetHome>/src/fleet`, checks out the requested ref (default local
build commit), runs `cargo build --release -p fleet-daemon`, installs the result as the configured
fleetd binary, restarts the remote daemon, nudges the endpoint out of reconnect backoff
(`RemoteEndpoint::nudge_reconnect`, § 5) so the probe observes the new daemon rather than waiting out
a grown backoff, and probes until the link reports the matching build.
Every step logs to the ordinary job log and cancellation stops before the next command.

## 10. Doctor and host status

Host status reports id, provider, reachability, checked time, resolved address, link state, daemon
version, last error/stderr, and optional Claude/OpenCode availability. Doctor distinguishes resolve,
SSH/auth, fleetd presence/version, protocol mismatch, and agent-binary checks. Legacy entries report
`Legacy` plus a migration hint. An unreachable host never makes unrelated host results disappear.

## 11. CLI surface

`fleet host list [--json]`, `fleet host doctor <id>`, and `fleet host bootstrap <id> [--ref <git
ref>]` are stable. Worktree creation accepts `--host`; omitted placement may use `defaultHost`.
Remote `fleet path` renders `<host>:<path>` and supports the protocol-one JSON envelope. Host
commands return nonzero on typed daemon failures.

## 12. App Location rule

```rust
pub struct Location { pub host: Option<HostId>, pub path: String }
impl Location { pub fn local_path(&self) -> Option<PathBuf>; }
```

`local_path` returns `Some` only when host is absent. Remote worktree paths must never be passed to
local filesystem or embedded native-Git code.

## 13. Test doubles and two-daemon harness

`FakeMachine` scripts resolve/probe/exec results, records every exec argv, and returns one half of
`tokio::io::duplex`; tests take the peer half. `FakeRemote` queues request results, records requests,
emits events, and changes LinkState. `RemoteDaemon::start(home)` starts a private `DaemonProcess`
and exposes a loopback `CommandMachine` using `sh -c 'exec "$@"' --`; its fleetd path comes from
`FLEET_DAEMON`. Provider tests should probe without depending on the unfinished connect bridge.

## 14. Ownership table

| Stage | Owns |
|---|---|
| contracts (this) | Cargo.toml/Cargo.lock, all mod.rs/lib.rs/commands.rs registrations, fleet-proto/*, fleet-core/src/{model,config}.rs, server/connection.rs, dispatch.rs (until router-core), machines/{provider,child,command,legacy,registry}.rs, testing/machines.rs, tests/infra, docs/REMOTE-MACHINES.md, ADR 0011 first version |
| transport | machines/tailscale.rs, machines/ssh.rs |
| link (then link-reconnect) | machines/link.rs, server/bridge.rs, fleet-daemon/src/main.rs (connect body), fleet-client/src/{connection,spawn,lib}.rs, fleet-client/tests/*, fleet-daemon/tests/remote_link.rs |
| hosts (then doctor-agents) | services/hosts.rs, services/maintenance.rs, services/doctor.rs, tests/doctor_checks.rs, tests/hosts_status.rs; doctor-agents also services/agents/providers/opencode/{mod,server}.rs |
| hosts-cli | fleet-cli/src/commands/hosts.rs, services/bootstrap.rs, tests/bootstrap_host.rs, fleet-cli/tests/hosts.rs |
| router-core | services/router/{mod,classify,ids,translate}.rs, services/dispatch.rs, tests/router_core.rs |
| mirror | services/mirror.rs, services/snapshots.rs, services/import.rs, fleet-core/src/state.rs, services/worktrees/recovery.rs, tests/{remote_mirror,import_from_swarm,reset_state}.rs |
| cli-worktrees | fleet-cli/src/commands/{worktrees,agents,sessions,tests}.rs, fleet-cli/src/human.rs, fleet-cli/tests/* except hosts.rs |
| app-hub | fleet-app/src/dialogs/create_worktree.rs, views/worktrees_list.rs, screens/hub/**, dialogs/settings/** |
| app-workspace | fleet-app/src/screens/workspace/**, screens/agent_thread/**, bridge/**, state/** |
| docs | docs/decisions/0011-remote-machines.md (amend), docs/README.md, README.md, docs/APP-CONTRACTS.md, docs/ARCHITECTURE.md, docs/NATIVE-AGENTS.md, docs/decisions/0010-native-agents.md, docs/SWARM-INVENTORY.md |
| sessions (wave 2) | services/sessions/**, services/router/sessions.rs, fleet-core/src/sessions.rs, tests/sessions_lifecycle.rs, tests/remote_sessions.rs |
| remote-create (wave 2) | services/worktrees/{creation,publication,post_create}.rs, services/boards/worktree.rs, services/router/create.rs, tests/{worktrees_lifecycle,boards_service,remote_create}.rs |
| lifecycle (wave 2) | services/{sleep,inspect,prune,repos,contexts,worktrees}.rs, services/worktrees/trash.rs, services/router/lifecycle.rs, tests/{inspect_behavior,prune_safety,sleep_policy,repos_service,contexts_service,remote_lifecycle}.rs |
| agents-routing (wave 2) | services/router/agents.rs, services/agents/{manager.rs,manager/**,store.rs,thread.rs}, tests/agents_remote.rs |
| integrate | everything |

## 15. Definition of done per stage

Every stage implements all owned skeleton bodies, preserves these signatures or records a
DEVIATIONS reason, adds its specified unit/integration tests, formats only owned files, and passes
crate check, rustfmt check, clippy with `-D warnings`, the private-daemon whole-crate test helper,
and workspace checking when core/proto changes. Transport proves argv/error mapping; link proves
Hello/correlation/reconnect; hosts and CLI prove status/bootstrap; router proves exhaustive routing
and id translation; mirror proves stale/merge/source-of-truth; feature stages prove remote create,
session, lifecycle, and agent behavior; app stages prove no local-path use for remote locations;
docs remove obsolete unsupported claims; integrate runs the complete workspace and manual matrix.
