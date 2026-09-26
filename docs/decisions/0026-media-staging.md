# 0026 — Stage media on the machine that owns its consumer

**Adopted** for clipboard images and operating-system file/folder drops, initially on the
Workspace terminal and floating agent terminal. Fleet transfers bytes through the existing
federated daemon link with one capability-gated, chunked, resumable protocol lifecycle, then
hands the owning-machine path to the caller. The same seam is reserved for native-agent
attachments and opted-in text inputs.

## Context

A terminal or native-agent thread shown by the local app may live on another machine. A path on
the app's machine has no meaning there. Sending an entire file as one length-prefixed JSON frame is
also unsafe: a remote host has one framed stream, one write lasting more than 10 seconds takes its
link down, and any large write queues terminal input behind it. A 10 MiB file expanded as base64
already takes about 11 seconds on a 10 Mbit/s uplink.

The receiver must not trust names or directory entries from a client, and interrupted transfers
must neither leak indefinitely nor delete unrelated files. The app must keep the user-visible
operation alive independently of either terminal surface and let the user cancel it.

## Decision

- **Stage on the owner.** `StageMedia { anchor, upload, op }` routes `Local`, `Terminal`, `Host`,
  and `Thread` anchors to the daemon that owns the consumer. Every operation repeats the anchor;
  the app generates the upload UUID, so routing is stateless and retries remain idempotent.
- **Use bounded, resumable operations.** `Begin`, `Chunk`, `Finish`, and `Cancel` use the ordinary
  correlated protocol. Chunks are 128 KiB decoded and at most four may be in flight to one host
  across all of its uploads. A full base64 chunk is 174,764 bytes; four are 699,056 bytes (about
  683 KiB), or about 0.56 seconds on a 10 Mbit/s link. Future code may reduce these constants but
  must not increase chunk × window without re-proving the interactive-latency and 10-second write
  bounds.
- **Keep upload traffic unordered.** Staging writes files, not PTY bytes or transcript state, so
  it stays outside both ordered lanes. The final terminal paste and later agent send are separate
  ordered requests.
- **Resume at the application layer.** A link failure returns the upload to a visible waiting
  state; only unacknowledged chunks are resent after reconnect, while `Begin` and `Finish` may be
  repeated. The receiver retains a completed `Finish` result until idle expiry for replay. An
  expired receiver upload causes one restart with the same stable id. `Begin` errors are surfaced
  as anchor failures rather than mistaken for expiry. App restart is not resumable.
- **Verify before publishing.** The app sends a regular-file/directory manifest and streams every
  regular file; the daemon sanitises each component, writes part files, SHA-256-verifies all files,
  then renames onto the final path. The app rejects a gesture requiring staging above 1 GiB or
  10,000 entries; local paths inserted without copying bypass staging and those limits. Symlinks
  and special entries inside folders are skipped, never followed. Receiver directory walks and
  part-file opens are descriptor-relative and no-follow, and each connection may retain at most
  16 active uploads. The app queues work behind 12 per-host worker permits so one gesture cannot
  exhaust the receiver cap.
- **Use two target roots.** Terminal, host, and local staging publishes below
  `~/Downloads/fleet/`, falling back to `$FLEET_HOME/media/` when Downloads is unavailable.
  Thread staging publishes below `$FLEET_HOME/agents/attachments/<thread-uuid>/`, the exact leaf
  Claude receives through `--add-dir`. The download sweep keeps completed items seven days and
  orphan parts one day, and never walks thread attachments.
- **Make lifetime app-wide.** `AppState` owns the upload registry, stored GPUI tasks, progress,
  per-host semaphores, and link waiters. Dropping a task cancels work; a pinned, button-only toast
  action also sends a best-effort protocol `Cancel`. `media::stage` returns stable per-item ids,
  exposes read-only progress and cancel-by-id, and completes with a result for every item, so the
  caller decides whether the finished path is typed into a PTY, becomes an agent attachment, or
  is inserted elsewhere. The gesture
  survives closure of its originating surface, but quitting the app drops the registry and is not
  resumable.
- **Gate behavior, not compatibility.** Daemons advertise `media.stage`; the router refuses an
  older remote before forwarding an unknown variant. `PROTOCOL_VERSION` remains 8 because the
  addition is optional and an exact-version bump would lock out otherwise-compatible peers.

## Alternatives rejected

- **One large protocol frame.** Base64 expansion can push a write past `WRITE_BUDGET`, taking the
  host down and blocking every terminal sharing its link.
- **A shared mount.** Fleet cannot assume the same filesystem, mount point, permissions, or
  availability on local and remote machines.
- **Out-of-band `scp` or `rsync`.** Either needs a second credential/transport path, bypasses link
  capability and liveness policy, and is unavailable for non-SSH providers.
- **Tar streaming for folders.** It adds a dependency and makes archive entries another trusted
  input surface; the explicit manifest is bounded and independently sanitised.
- **Insert the app-machine path.** It is wrong for a remote consumer and turns a successful
  gesture into a path that cannot be opened.

## Consequences

Remote media competes with interactive traffic only within the documented 128 KiB × 4 bound.
Partial transfers survive link blips for up to the receiver's three-minute idle expiry, but not an
app restart. Clipboard behavior still follows what GPUI identifies: Wayland copied-file URI lists
that arrive only as text remain text, while file drops arrive as paths. Thread attachments have a
different retention owner than staged downloads. The authoritative protocol and UX details live in
`docs/REMOTE-MACHINES.md` §3 and `docs/UX-SPEC.md` §3.6.
