# 0011 — Terminal agent attention

**Adopted** for daemon-hosted PTY agents, their protocol/snapshot state, and app notifications.

- **Heuristic idle is status-only.** Terminal output counters still drive `Working` and `Idle`
  glyphs, including the 2.5-second silence debounce, but silence cannot prove a turn completed or
  that a person is needed. A heuristic transition therefore never produces a toast or sound,
  whether received as an event or recovered from a snapshot.
- **Explicit hooks are the sole PTY notification source.** `fleet agent-status working` clears
  attention. `finished`, `permission`, `question`, and `plan` set `Idle` plus an authoritative
  attention reason. The reason persists until explicit working, user input, terminal close, or
  real byte-advancing output outside the input-echo window proves the agent resumed.
- **Terminal and native agents share one vocabulary.** PTY protocol events and snapshot windows
  reuse `fleet_core::agents::AttentionKind::{Permission, Question, Plan, Finished}`. The app uses
  the same native `NeedsYou` amber tab mark and notification copy, but native provider events and
  reducers remain the authority for structured threads.
- **Compatibility is additive.** Attention fields are optional/defaulted and omitted when absent,
  so legacy clients and daemons retain their previous activity-only wire shape. Snapshot seeding is
  silent; later edges into semantic attention notify once through the existing configured toast
  and sound channels.
