# Phase 2 — Native agent attachments from the composer — Tracker

> Plan: ./paste-drop-media-2026-09-10-phase-2-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

Repo-specific, on top of the above:
- Load the skill `CLAUDE.md` names for the area before editing it, and `zed-quality-review` before calling the phase done.
- Commit as `<area>: <imperative lowercase summary>` — `ui-kit`, `daemon`, `app`, `docs`. One logical change per commit; the doc update rides with the code it describes.
- `fleet-ui-kit` may depend on `gpui` and nothing else. If a task seems to need a Fleet type in the kit, the design is wrong — the kit reports, the app decides.
- Run `make restart` after any daemon change before a manual check.

## Kickoff
- [x] Phase 1 has shipped: the chunked `StageMedia` with `MediaAnchor::Thread` stages into the thread leaf on its owner host; `fleet_app::media::{from_clipboard, from_external_paths, stage}` exist, `stage` takes a continuation and `force_copy`, and the upload registry supports cancel and reports progress.
- [x] I have read the thread-leaf spelling in the phase-1 tracker (P1-T03 note).
- [x] I have read the plan end to end, the roadmap, `TODO.md` §5, `docs/NATIVE-AGENTS.md` §7.2 and §8, and ADR 0013's attachment section.
- [x] Green baseline: `make lint`, `make test`, `make check`, `make harness`. — verified 2026-09-26 on the committed phase-1 tree: fmt + clippy clean, 4346 tests green; harness last run green on 2026-09-25 with no GUI change since
- [x] I am ready to start.

## Tasks
- [x] P2-T01 — Let a kit input report media instead of inserting it — verified: `cargo test -p fleet-ui-kit` 413 passed; strict kit clippy; both galleries compile; commit `e0de096`
- [x] P2-T02 — A pending-attachment chip — verified: same run as T01; transcript pill now built on the shared `AttachmentPill`; commit `e0de096`
- [x] P2-T03 — Providers receive staged attachments properly — verified: `cargo test -p fleet-daemon claude` 81 passed incl. leaf-containment, `..` and symlink escape goldens; Codex `localImage` wire test; commit `4b8d8b1`
- [x] P2-T04 — Validate once, and keep base64 out of SQL — verified: agents suite 483 passed; Base64 send recorded as `Path`; validation runs after the thread resolves; commit `4b8d8b1`
- [x] P2-T05 — Record attachments durably, and collect orphans — verified: store 132 unit + 7 integration tests; paused-clock sweep test keeps referenced/fresh/dirs/symlinks; full `make test` 4358 green; commit `4b8d8b1`
- [x] P2-T06 — The composer: pending chips, send, and the edge cases — verified: `cargo test -p fleet-app agent_thread` 123 passed, including the five required media cases plus the preflight/send race; workspace clippy and fmt clean; orchestrator gate on the integrated tree: `make test` 4365 green, `make harness` 104/104 scenarios passed (2026-09-26); manual matrix intentionally left for a human with real desktops and a remote host
- [x] P2-T07 — Move the docs with the code — verified: UX, app-contract and native-agent sections re-read against the implementation; `TODO.md` §5 was already closed by T05

## Manual verification matrix (P2-T06)

| Case | Claude | Codex | Notes |
| --- | --- | --- | --- |
| Screenshot → paste into a **local** thread's composer → send | | | chip staging→ready; agent sees the image; pill on the sent message |
| Screenshot → paste into a thread on a **Tailscale host** → send | | | file in that host's thread leaf? agent sees the image? |
| File drag → drop on the composer → send | | | non-image file: Claude can read it (`--add-dir`); Codex gets a mention |
| Attachment-only send (empty text) | | | allowed |
| Remove a chip while it is staging | | | no chip reappears; file later swept |
| Ninth attachment | | | refused, message names the limit |
| Image > 10 MiB / file > 50 MiB / a folder | | | refused before staging, message names the limit |
| 40 MB file → drop on the composer of a **remote** thread | | | chip shows progress; typing elsewhere on that host stays responsive |
| Link blip while a composer chip is uploading | | | chip shows waiting, then resumes to ready |
| Paste while the thread's link is `Down` | | | refused with the unreachable wording — not silent |
| Composer in Approval mode | | | media not accepted |
| Plain text paste into the composer | | | byte-identical to today |

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-10 — Original plan: insert the staged path into the composer as text, plus kit inputs and dialogs.
- 2026-09-24 — **Rewritten.** The user chose real attachments over path-as-text for the composer, because `TODO.md` §5 / NATIVE-AGENTS §7.2 / ADR 0013 already settle an attachment design that a path-as-text composer would only be ripped out for. Dialog inputs moved to phase 3. Consequences:
  - `MediaAnchor::Thread` (added to phase 1) routes by thread in the daemon, so **the app no longer needs the typed `HostId`** — the old "widen `ThreadHost` or read `AppState`" question is moot.
  - Every composer attachment is staged into the thread leaf, even for a local file, so it survives the original moving and sits inside the `--add-dir` grant.
  - Claude currently appends *any* `Path` as prompt text; T03 changes image paths inside the leaf to image blocks. Codex already maps them to `localImage`.
  - `TextInputEvent` is `Copy`; the media payload gets its own event type.
  - (Second revision, same day) Phase 1's upload became chunked and resumable, so the composer allows the settled 50 MiB per file; chips show progress and waiting state; removing a chip cancels its upload. Folders are refused in the composer.

- 2026-09-26 — Kit and daemon tasks landed (`e0de096`, `4b8d8b1`); decisions taken while integrating:
  - **`MultilineInput` relays media as `MultilineInputEvent::Media(TextInputMedia)`**, matching its single-enum relay, and `TextInputMedia` is its own non-`Copy` event with a second `EventEmitter` on `TextInput`.
  - **One attachments root.** `Media` owns `FleetHome::agents_attachments_path()` normalised once; the store writer, the orphan sweep, the Claude leaf-containment check and `--add-dir` all read it from `Media` (`thread_leaf`, `attachments_root`), so spellings cannot diverge.
  - **The Claude harness takes the daemon `Files` port** (`HarnessConfig.files`) to canonicalise, stat and read a staged image; an earlier draft built an ad hoc `RealFiles` inside the leaf and was rejected in review.
  - **Validation and Base64 materialisation run after the thread resolves** and the operation gate is held, so a send to an unknown or stopped thread writes nothing into a leaf.
  - **The leaf is created at thread start** (`ensure_thread_leaf`) so `--add-dir` always names an existing directory.
  - **The orphan sweep also runs every 6 h**, spawned in `start_periodic_tasks` with the shutdown token; it retains empty leaves.
  - TODO.md §5 is marked "Done" in the file's existing `## N. Done — …` convention rather than renumbered.
  - Follow-up (from the plan's risk section, confirmed): a remote-thread `AgentSend` carrying `Base64` still crosses the link as one frame before owner-side materialisation; other clients should stage through `StageMedia` first.

- 2026-09-26 — Composer and docs tasks completed:
  - Attachment metadata and size checks run on GPUI's background executor. A named reservation
    blocks send during that preflight, and a thread switch invalidates its late result before it
    can stage against the replacement thread.
  - Upload progress is copied from the shared registry in `WorkspaceScreen::sync_agent_views`;
    render only composes the prepared `PendingAttachmentState` values.
  - Transcript attachment pills stay **above** message text, matching the existing
    `rows/message.rs` child order; UX-SPEC's old “below” sentence was the stale side.
  - `RUSTC_WRAPPER="" cargo test -p fleet-app` ran: 1247 passed and 14 pre-existing socket tests
    failed because this managed sandbox rejects `UnixListener::bind` with `Operation not
    permitted` (`bridge/tests.rs:94`, plus the dependent driver-bind assertion). The same sandbox
    limitation stops `RUSTC_WRAPPER="" make test` at that fleet-app suite. The focused agent
    suite is green; workspace clippy with `-D warnings`, `cargo fmt --all -- --check`, and
    `RUSTC_WRAPPER="" make lint` pass.

### P2-T01 note — how `MultilineInput` relays media
(Record: new `MultilineInputEvent` variant, or a relayed second emitter — and why.)

- 2026-09-26 — A new `MultilineInputEvent::Media(TextInputMedia)` variant. `MultilineInput` already relays every inner event through one enum, so a second emitter would give the composer two subscriptions for one input. The inner `TextInputMedia` subscription is retained on the entity.

### P2-T05 note — sweep schedule
(Record: start-only, or start + periodic, and the tick chosen.)

- 2026-09-26 — Start + periodic, every 6 hours, with a 24 h mtime grace. A long-lived daemon therefore collects removed chips within a day; the tick is deliberately slow because the sweep stats every leaf entry.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- CLI `AgentSend` with `Base64` attachments to a remote thread is still one large frame on the link; the CLI should stage through `StageMedia` first.
- A thread-delete verb that also removes the thread's attachment leaf (none exists yet).
- Thumbnails in chips and transcript pills.
