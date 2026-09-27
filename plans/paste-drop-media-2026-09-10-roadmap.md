# Paste and drop media into Fleet — Roadmap

> Phase plans:
>  - Phase 1: ./paste-drop-media-2026-09-10-phase-1-plan.md · ./paste-drop-media-2026-09-10-phase-1-tracker.md
>  - Phase 2: ./paste-drop-media-2026-09-10-phase-2-plan.md · ./paste-drop-media-2026-09-10-phase-2-tracker.md
>  - Phase 3: ./paste-drop-media-2026-09-10-phase-3-plan.md · ./paste-drop-media-2026-09-10-phase-3-tracker.md
>
> **Revised 2026-09-24** against `main` at `fa31272` (365 commits after the original baseline
> `0e74950`). Nothing from the original plan had been implemented. The revision re-grounded every
> reference and made three structural changes, listed under *What changed in the revision*.

## Summary

Fleet cannot accept an image. Pasting a screenshot into a terminal running Claude Code does
nothing, and dragging a file onto the window does nothing. Copying a *file* does paste something,
but it is wrong: GPUI's `ClipboardItem::text()` turns `ExternalPaths` into text by writing every
path back to back with no separator and no quoting (pinned
`gpui/src/platform.rs:2403-2417`). Two copied files paste as one garbage path, and a path with a
space breaks in the shell.

Every gesture should put the file on the machine that owns the target — the terminal's host or
the thread's host — before a path means anything there. A thread on a Tailscale devbox needs the
bytes shipped across the federated `fleetd` link and written to disk *there*.

The three surfaces want different results:

| Surface | What a paste or drop produces |
| --- | --- |
| Terminal (workspace, agent popup) | The staged file's absolute path, shell-quoted, typed into the PTY |
| Native agent thread composer | A real **attachment**: a pending chip in the composer, then an image block (Claude) or `localImage` part (Codex) on send |
| Dialog text inputs (`TextInput`) | The staged file's path, inserted at the caret |

## Why this is phased

**Phase 1** is the vertical slice every later phase stands on: the wire request, the capability,
the routing, the daemon's staging service and sweep, the app-side `media` module, and the terminal
gesture. It fixes the reported bug on its own. Every decision that is expensive to revisit is made
here.

**Phase 2** is native agent attachments. That design was settled after this roadmap was first
written (`TODO.md` §5, `docs/NATIVE-AGENTS.md` §7.2 and §8, ADR 0013) but never built. The wire,
the manager, both provider mappers and the transcript pills already carry attachments. What is
missing is the composer's pending-chip model, the durable store writer, the per-thread directory
and Claude's `--add-dir` grant, plus a Claude mapper that reads a staged image from disk. Pasting a
path as text into the composer would ship a stopgap the settled design then deletes, so the
composer gets real attachments or nothing.

**Phase 3** is the dialog `TextInput` gesture. It is small, and it reuses the kit event phase 2
adds. It is its own phase so the attachment work does not wait on picking which dialog fields
should accept a path.

Each phase ships alone and is useful alone.

## Phase list

### Phase 1 — Media staging and the terminal gesture

- **Goal:** paste or drop an image or file onto a Fleet terminal and have its path, on the
  terminal's own machine, land in the shell.
- **Shippable state:** a screenshot paste and a file-manager drag (files or folders) both insert
  a usable, quoted, absolute path. On a remote host, the bytes are uploaded in chunks, with
  progress and cancel, to `~/Downloads/fleet/` there first, and the inserted path is the remote
  one. Large files do not disconnect the host or make typing lag, and a link blip mid-upload
  resumes instead of failing. A local file paste inserts the original path, correctly
  quoted, with no copy. Text paste is byte-identical to today. Works on macOS and on Linux
  (Wayland and X11 are both product backends, `docs/DEVELOPMENT.md` §GPUI platform backends).

### Phase 2 — Native agent attachments from the composer

- **Goal:** paste or drop an image or file into the agent thread composer, see it as a pending
  chip, and have the provider receive it on send, for local and remote threads alike.
- **Shippable state:** a screenshot pasted into a composer for a thread on a Tailscale host is
  staged into that host's per-thread attachment directory. It shows as a removable chip, is sent
  as a Claude image block or Codex `localImage` part, is recorded in `item_attachments`, and
  renders as a pill on the sent user message. `TODO.md` §5 is closed.

### Phase 3 — The gesture in dialog text inputs

- **Goal:** chosen `TextInput` fields accept a pasted or dropped file as its local path, and no
  input pastes concatenated paths any more.
- **Shippable state:** the wired dialog fields insert a quoted local path. Every other field is
  listed with the reason it is not wired. `fleet-ui-kit` still depends only on `gpui`.

## Seams between phases

- **`RequestBody::StageMedia { anchor, upload, op }`**, a chunked, resumable upload
  (`op = Begin | Chunk | Finish | Cancel`) with
  `MediaAnchor { Local, Terminal { terminal }, Host { host }, Thread { thread } }`. All four arms
  are implemented, routed, tested and documented in phase 1, so phases 2 and 3 add no wire
  surface. `Thread` stages into that thread's own attachment directory on its owner host
  (`$FLEET_HOME/agents/attachments/<thread>/`), not into `~/Downloads/fleet/`. Phase 2 only has to
  consume it.
- **`fleet_app::media`**: `Attachment`, clipboard and drop sniffing, path quoting, an upload
  registry with a per-host window, and one `stage(...)` flow (with a `force_copy` flag) that
  returns the staged path to a caller-supplied continuation. Phase 1
  delivers to a PTY. Phase 2 turns the path into a pending chip. Phase 3 inserts it at a caret.
  There is exactly one staging flow. Phase 1 T09 owns that seam.
- **The kit media event.** Phase 2 adds an opt-in media mode to `TextInput` (relayed by
  `MultilineInput`). In that mode, a non-text paste or a file drop is *reported* as an event with
  gpui payloads, not inserted. Phase 3 turns it on for dialog fields. The kit never learns what a
  host or an anchor is.
- **The `media.stage` capability** gates all three phases. There is no second negotiation.
- **Limits.** Phase 1 allows up to 1 GiB per drop in 128 KiB chunks. The composer's own limits
  (`TODO.md` §5: 10 MiB per image, 50 MiB per file) sit comfortably inside that, so phase 2 needs
  no transport change.

## What changed in the revision (2026-09-24)

- **`MediaAnchor::Thread` was added, and the composer design changed from "insert a path" to real
  attachments.** The user chose this on 2026-09-24 over keeping path-as-text.
- **The dialog-input work moved to a new phase 3.** It used to share phase 2 with the composer.
- **The single-frame upload became a chunked, resumable one** (second revision, same day), so
  remote drops are seamless:
  - each remote link is one framed stream, and any write over `WRITE_BUDGET` (10 s) takes the host
    `Down`, so a whole-file frame could disconnect every terminal on that host;
  - chunks of 128 KiB with a per-host window of 4 bound the keystroke delay at ~0.55 s on a
    10 Mbit/s uplink;
  - a drop can be up to 1 GiB, and folders are supported;
  - progress and cancel are visible;
  - an upload waits for a reconnecting host and resumes after a blip.
- **Code changes since the original plans:** protocol version is **8**, not 7. ADR 0013 is
  taken, and the next free number is **0026**. `TextField` is gone, replaced by one `TextInput`
  (ADR 0020). The `FLEET_DRIVE` procedure is gone, replaced by the `fleet-harness` (ADR 0016).
  `base64` is already a workspace dependency. Board drag-and-drop now exists as a GPUI pattern.
  The original "use `.detach()`" guidance contradicted `CLAUDE.md` and was corrected.

## Cross-phase risks

- **The clipboard may not carry what we expect, on either platform.** On macOS, GPUI fills
  `ClipboardEntry::Image` from `NSPasteboardTypePNG`/TIFF. A browser image may arrive as HTML plus
  a file promise instead. On Linux, GPUI's Wayland and X11 clipboards are separate code paths.
  Phase 1 T01 found source support in both and live-tested Wayland PNG/text/URI-list payloads;
  macOS and X11 remain real-machine rows in the manual acceptance matrix.
- **The harness cannot prove any of this end to end.** `clipboard set/get` fails in every harness
  lane by design, and `drag` cannot inject an OS `ExternalPaths` drag (`docs/TESTING-HARNESS.md`
  §clipboard). Acceptance is `#[gpui::test]` coverage plus a recorded manual matrix. Extending the
  frozen harness contract is a follow-up that needs its own ADR, not a task in any of these
  phases.
- **Remote write budget and interactive latency.** Each write to a remote link is bounded by
  `WRITE_BUDGET` (10 s, `crates/fleet-daemon/src/machines/link.rs:39-44`), and frames to a host
  are written one at a time. Chunking keeps every upload frame near 171 KiB, and the per-host
  window keeps at most ~683 KiB in front of a keystroke. Phase 1 T05 proves this against a
  throttled fake link. If real links disagree, shrink the chunk or the window; never raise the
  budget.
- **Ordering against concurrent typing.** Staging deliberately stays off the ordered terminal path
  (phase 1 T05). Keys typed during an upload reach the PTY before the path does. This is accepted
  and surfaced with an in-flight toast.
- **Writing into `~/Downloads`.** The sweep may only delete regular files, directly in `fleet/`,
  whose names carry Fleet's timestamp stamp (phase 1 T07). The per-thread attachment directories
  are not swept by that rule. They belong to the agent store's own garbage collection (phase 2).
- **Large requests that bypass staging.** `AgentSend` can still carry `Base64` attachments from
  other clients (the CLI). Forwarded to a remote thread, that is again one large frame on the link.
  Phase 2 notes it, and the follow-up is for such clients to stage through `StageMedia` first.

## Suggested order

Phase 1, then phase 2, then phase 3. Phase 3 depends on phase 2's kit event, not on its attachment
work, so it could start once phase 2 T01 lands if someone wants it sooner.

Within phase 1 the order is load-bearing. T01 (what the clipboard actually carries, on both
platforms) gates everything. The wire work (T02–T06) settles before the app work (T08–T10), so the
app is written against a stable request shape.
