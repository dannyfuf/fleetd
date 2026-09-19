# 0018: Closed native-agent tabs are durable per installation

**Adopted** for `fleet-app`'s native-agent strip, `fleet-daemon`'s agent store,
and `fleet-proto`'s agent family. Closing a top-level native-agent tab records a
per-installation marker in the daemon. Reopening the tab clears that marker.

**Amends [ADR 0010](0010-native-agents.md)** by making tab visibility durable
without changing thread ownership. The daemon still owns and lists every
thread. Closing a tab does not stop or delete it.

**Amends [ADR 0013](0013-sqlite-agent-transcripts.md)** by adding migration 5
and the `closed_threads` table beside the per-installation `seen` cursors.

## A window-local set could not survive every replacement path

The app used an in-memory `closed` set to hide a top-level thread that the
daemon continued to list. That worked only while the window retained the set.
Quitting and relaunching the app created an empty set. Restarting the daemon or
reconnecting from its banner could also restore a tab when an incomplete
snapshot caused the reducer to prune the marker before the full snapshot.

The GUI harness cannot drive an app relaunch, and its daemon-restart scenario
passed before this change because that reconnect carries a complete snapshot.
The scenario stays as a guard for the restart path. The two failure shapes are
pinned by app-state tests instead: one starts from a fresh `AppState` seeded by
the daemon, the other replays an incomplete reconnect snapshot.

Keeping the set only in the app was rejected. Fleet deliberately has no
app-side on-disk mirror or cache, so an app-owned durable marker would create a
second persistence system and a second source of truth.

## The daemon stores one marker per installation and thread

Migration 5 adds `closed_threads`, keyed by `(client_id, thread_id)` with a
`closed_at` timestamp. The identity is `HelloClient.client_id`, the same stable
installation identity used by `agent.seen`. Two installations sharing a daemon
therefore keep independent tab strips.

A capable close persists the marker before owner-host routing. A store failure
is warned and the acknowledgement still follows. Reopen clears the marker and
then follows the same routing path. `AgentClosedThreads` returns the census for
that installation, and the app replaces its in-memory set before applying the
first snapshot. Snapshots never prune closed markers, so an incomplete snapshot
cannot forget one.

Global archival on `threads` was rejected. It would hide the thread for every
installation and would give a tab-close gesture thread-delete semantics.

## The wire change is additive

`agent.closed` gates `AgentClosedThreads` and `AgentThreadReopen`.
`PROTOCOL_VERSION` remains 8. An older client does not advertise the capability,
so close keeps its historical acknowledgement-only behaviour. An older daemon
does not advertise it, so the app keeps its in-memory set for that connection.

The closed census remains separate from `AgentThreadSummary`. Summary events
keep one shared fan-out instead of requiring a per-installation database read.

## Consequences

Migration 5 is irreversible. Its recorded source and hash cannot change after
shipping. A rollback must add a later migration rather than edit slot 5.

The owed thread-delete verb must clear every matching row in `closed_threads`.
A stale row is inert today because tab projection starts from listed summaries,
but deletion must not leave installation metadata behind.

Delegated children retain their window-local `attached` set. Closing an attached
child detaches it in that window and does not write `closed_threads`. Only a
top-level thread close uses the durable marker.
