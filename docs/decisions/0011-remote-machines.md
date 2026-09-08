# 0011 — Remote machines through daemon federation

**Adopted** for remote worktrees, terminal sessions, lifecycle operations, and native-agent
threads. One `fleetd` runs on every machine; the local daemon federates configured peers through
a `MachineProvider`, and the app and CLI continue to use only the local daemon socket.

- **Forward Fleet protocol requests to the daemon that owns the worktree.** The remote daemon
  already owns local filesystem, Git, PTY, process-observation, Claude stdio, and OpenCode HTTP/SSE
  behavior. Reusing it avoids parallel SSH implementations of every execution surface.
- **Make transport an adapter.** `MachineProvider` resolves, probes, executes bounded bootstrap
  commands, and opens one byte stream to `fleetd connect`. Tailscale discovery plus OpenSSH is the
  first provider; the command provider is an advanced/testing seam and legacy swarm entries remain
  probe-only. A future provisioned provider can separately implement `MachineLifecycle`.
- **Keep the app and CLI transport-blind.** The local daemon routes requests, remaps daemon-local
  terminal/job/session identifiers, mirrors remote snapshot fragments, and rebroadcasts translated
  events. Remote daemons receive an ordinary protocol request with host placement cleared.
- **Run OpenCode entirely on the owning machine.** Its localhost server and SSE consumer remain
  inside the remote daemon. No OpenCode port is exposed on the tailnet.
- **Require protocol lockstep.** A remote link opens Hello as `ClientKind::Proxy`; incompatible
  protocol versions fail the handshake. `fleet host bootstrap` is the recovery and deployment path.

Rejected alternatives:

- Running every remote command from the local daemon over SSH would duplicate Git, PTY, process,
  hook, and agent implementations and would tie terminal lifetime to the local link.
- Letting the app connect to multiple daemons would leak transport, routing, and partial-failure
  policy into every client surface.
- Embedding `tsnet` would add a second network stack and authentication surface when the installed
  Tailscale and OpenSSH tools already provide discovery and transport.

Consequences: both daemons must run compatible builds; daemon-local ids require translation;
cached remote inventory becomes stale, rather than disappearing, while a link is down; bootstrap
must compile for the remote architecture; and SSH authentication failures remain user-visible.

Provenance: `plans/remote-worktrees-tailscale-2026-09-08-roadmap.md`, its phase-1 plan, ADR 0010,
and the legacy topology recorded in `docs/SWARM-INVENTORY.md`.
