# 0015 — Terminals live in detached holder processes, not in `fleetd`

**Adopted** for `fleet-term`'s `holder/` and `PtyBackend`, `fleet-daemon`'s
`services/sessions/{holder,adoption}.rs`, and the `$FLEET_HOME/pty/` layout. Every terminal's PTY
and login shell live in a `fleetd pty-hold` process that outlives the daemon; `fleetd` keeps the
virtual terminal, the scrollback and the frame protocol and reattaches over a Unix socket.
`docs/ARCHITECTURE.md`, "Detached PTY holders", is the specification; this file records why.

**Built.** `crates/fleet-term/src/holder/{protocol,replay,server,client}.rs` is the whole
mechanism, `crates/fleet-daemon/tests/pty_holder.rs` proves a shell and its terminal identifier
survive a real restart, and ADR [0002](0002-terminal-emulation.md) is unchanged: emulation stays in
the daemon, which is the constraint that ruled out every multiplexer.

## What the in-process PTY could not do

Every terminal was a PTY child of `fleetd`. The daemon restarts often — the "fleetd is outdated"
banner asks for it after any daemon change, and `fleet daemon restart` is the documented way to
pick up a new build — and each restart killed every shell and every coding agent inside it. On one
devbox the daemon restarted seven times in three hours, ending the user's Claude Code sessions each
time. `DaemonShutdown { stop_sessions: false }` existed and was honoured, and could not help: the
sessions died because they were children of the exiting process, not because anything asked them
to stop.

## The load-bearing choices

**One holder process per terminal, `setsid`-ed before `exec`.** It leaves the daemon's session and
process group, so SIGTERM to the daemon, a `ctrl-c` in the daemon's terminal, and an unclean exit
all pass it by. It owns a PTY and a socket and nothing else: no emulator, no scrollback, no
services, and it starts before any runtime does.

**`PtyBackend` is the seam, and releasing it detaches.** A terminal host drives either a local
`Pty` or a `HolderPty` and cannot tell which. The added verb is `detach()`: for a local PTY it can
only mean kill, for a holder it means close the socket and leave the child running. Dropping a
host, closing a connection and shutting down all detach; only an explicit `Kill` ends a child.

**The holder's greeting is authoritative, not the record.** `Hello` carries a protocol version, the
child pid and the kernel's current window size, and the daemon builds its emulator at that size.
The replay tail was produced for the grid the child is living at, which is not the grid the
terminal was created at. After the replay the holder signals the PTY's foreground process group as
a resize would, because a full-screen TUI repaints on `SIGWINCH` and on nothing else.

**A bounded byte tail, not serialised emulator state.** 1 MiB of raw child output replayed into a
fresh engine is version-independent and about a page of code. Serialising Ghostty's grid, history
and modes would couple an on-disk format to the emulator and have to be migrated with it.

**A record beside each socket, holding only what does not change.** Terminal and session identity,
the session's kind and cwd, the command, the cwd, the pids and the socket path. Window size is
absent on purpose — it changes on every resize and the holder reports its own — and the name is
rewritten when the user renames the terminal.

## Rejected

**tmux, or any multiplexer, as the holder.** It would work and is well tested, but `fleet-term`
owns the VT emulation, the scrollback budget and the frame diffing the client paints. Nesting a
second emulator means two sets of escape handling, two scrollbacks, two resize authorities and a
hard dependency on a binary Fleet does not ship — for a feature whose entire value is that the user
cannot tell a restart happened. ADR 0002 already decided where emulation lives.

**Re-parenting the PTY to `init` and keeping the master descriptor in the daemon.** A descriptor
cannot outlive the process holding it, so the master would still die with the daemon and the shell
would lose its terminal.

**Keeping holders in the daemon's process group and relying on signal handling.** Signals are
delivered to the group; a holder outside it is the only version that survives `ctrl-c` in the
daemon's terminal and a SIGKILL to the daemon alike.

**A socket path per terminal.** Identifiers are reused by `restart_terminal`, so a lingering
predecessor would unlink its successor's socket on the way out, and a predictable name under a
shared temporary directory is a name a local attacker can pre-create. The name carries a per-spawn
nonce instead, and a holder unlinks only a path that still stats to the socket it bound.

**An idle timeout on a holder with no daemon.** It would bound the orphan cases below, and it would
also kill the terminals of anyone who stops `fleetd` for longer than the timeout. A holder ends
when its child ends, and `ctrl-shift-q` ends it on demand.

## Consequences

- **Nothing above `PtyBackend` may assume a terminal's child is a child of this process.** Dropping
  a host, ending a connection, or shutting down must never imply killing.
- **An orphaned holder is invisible and expensive.** A live holder whose socket is unlinked, or
  whose record is lost, keeps a login shell — and whatever agent is in it — running until the
  machine reboots, with nothing pointing at it. Every failure path is written around that: a live
  holder's socket is never unlinked, an unreadable record is resolved back to its socket through
  the terminal identifier in the socket's name, a holder this build cannot speak to is stopped
  deliberately, and a terminal whose record cannot be written fails to open rather than leak.
  `fleet doctor`'s **pty holders** check is the backstop that names what is left.
- **A holder outlives every daemon, including one the user stopped on purpose.** Terminals run
  until their shell exits, `ctrl-shift-q`, or a reboot.
- **Scrollback across a restart is bounded by the replay budget**, not by
  `terminal.scrollbackBytes`: the first screen after a restart can be shorter than the one before.
- **The holder protocol is a contract between two `fleetd` builds that differ by design.** A holder
  started before an upgrade is still running the old binary when the new daemon reattaches. Add
  frames, never repurpose a tag, bump `HOLDER_PROTOCOL_VERSION` when a frame's meaning changes, and
  keep the byte-exact golden test in `holder/protocol.rs` honest. A version mismatch stops that
  holder and reports it; it never silently loses a terminal.
- **Sidecar records are append-only in the same sense as every other Fleet schema.** New fields are
  defaulted; `SIDECAR_VERSION` moves only when a field's meaning changes, and an unknown version
  costs the user that terminal.
- **Socket paths must stay inside the platform's `sun_path` budget.** `pty_socket_path` falls back
  to a private directory in the temporary directory and the record stores the result; callers read
  the recorded path rather than recomputing the choice.
- **Holders do not survive a machine reboot.** Anything that must belongs in `state.json`, not in
  `pty/`.

## Provenance

The failure this fixes was observed directly: seven `fleetd started` lines in three hours in
`~/.fleet/logs/fleetd.log` on the devbox, each one the end of a Claude Code session, and two of
them 0.93 s apart with no `fleetd stopped` between — the singleton race that this change also
closes, because a mistakenly deleted socket is far more expensive once the terminals behind it are
still alive. The app had documented the limitation rather than fixed it, in
`fleet-app/src/dialogs/help.rs` and in `UX-SPEC.md` §3.12 [D-17].
