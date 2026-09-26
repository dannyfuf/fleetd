# Phase 2 — Native agent attachments from the composer — Plan

> Tracker: ./paste-drop-media-2026-09-10-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
>
> **Rewritten 2026-09-24.** This phase used to insert a staged path into the composer as text.
> After the attachment design settled (`TODO.md` §5, `docs/NATIVE-AGENTS.md` §7.2/§8, ADR 0013),
> the user chose real attachments instead. The dialog-input work that shared this phase moved to
> phase 3. References are verified at `fa31272`.

## Summary

Paste or drop an image or file into the native agent thread composer. It appears as a pending
chip under the draft, and on send the provider receives it as a real attachment: a base64 image
block for Claude, a `localImage` part for Codex. The file first lands in the thread's own
attachment directory **on the thread's owner host**, so it works the same for a thread on a
Tailscale devbox as for a local one. The sent message is recorded with a durable reference, not
base64, and renders with its attachment pill.

This closes `TODO.md` §5 ("Attachments are not built").

## Sizing call

**Phased**: phase 2 of 3; see ./paste-drop-media-2026-09-10-roadmap.md.

This phase is mostly assembly. The wire (`Attachment`, `AttachmentSource`,
`UserInput.attachments`, `AgentSend`), the manager's projection, both provider mappers, the SQLite
table and the transcript pills already exist. What is missing is concentrated:

- a kit input event and a chip;
- the composer's pending model and send;
- one Claude mapper change;
- the `--add-dir` wiring;
- validation;
- the store writer and GC.

It needs phase 1's `StageMedia` with `MediaAnchor::Thread` and nothing else from it.

## Repository context

Verified at `fa31272`. Phase 1's context is not repeated.

**Already built:**
- `Attachment { name, media_type, source }` and `AttachmentSource::{Path, Url, Base64}`
  (`crates/fleet-core/src/agents/provider.rs:9-32`). `UserInput.attachments` (`:108-130`).
  `RequestBody::AgentSend { thread, input }` (`fleet-proto/src/request.rs:229-235`).
- `AgentSend` routes to `resolver.host_of_thread` (`services/router/agents.rs:340-361`). The
  `ThreadId` is global and is not rewritten (`translate.rs:22-52`). The provider runs on the owner
  daemon (`services/dispatch.rs:194-202`, `agents/harness/process.rs:319-342`), so **an attachment
  path must be a path on the owner host**.
- The manager accepts attachment-only messages (`services/agents/manager/commands.rs:435-443`) and
  copies attachments unchanged into `ItemKind::UserMessage` (`manager/apply.rs:207-228`), which is
  serialized into the event log and `items.detail_json` (`store/project/codec.rs:18-36`). **A
  `Base64` source therefore lands in SQL today**, which violates ADR 0013's "references, not
  base64" (`docs/decisions/0013-sqlite-agent-transcripts.md:85-96`).
- **Codex mapper** (`agents/codex/params.rs:128-156`): an image `Path` becomes `localImage`, and
  any other `Path` becomes a `mention`. This already works with a staged path.
- **Claude mapper** (`agents/claude/mod.rs:748-802`):
  - It validates 120 000 characters and 8 attachments.
  - `Base64` images of GIF/JPEG/PNG/WebP (`:66-68`) become image blocks, capped at 10 MiB.
  - **Any `Path`, including an image, is appended to the prompt as text** (`:780-784`).
  - Blocks are ordered `[images…, final text]` (`:797-802`, golden
    `agents/claude/tests/goldens.rs:42-64`). The final text block must stay last, or
    `/skill args` stops being read as a slash command (`docs/NATIVE-AGENTS.md:1136-1150`).
- **`--add-dir`** is emitted only if `launch.attachments_dir` is set (`agents/claude/argv.rs:124-127`).
  That field is always `None`, set at `services/agents/providers/mod.rs:402-418`. The argv
  contract expects a per-thread leaf (`argv.rs:23-33`).
- **SQLite `item_attachments`** (`services/agents/store/schema.rs:315-329`): `thread_id`,
  `item_id`, `attachment_id`, `relative_path` (under `$FLEET_HOME/agents/attachments`), `mime`,
  `bytes`. It has **no writer and no reader**; the only delete is the mirror discard
  (`store/mirror.rs:404-434`). `Store::root()` is unused and waits for this phase
  (`store/mod.rs:191-197`). The intended delete order is to commit the SQL removal, then delete
  files, then sweep leftovers at start (`schema.rs:21-24`).
- **No thread-delete verb exists.** Close is a per-installation marker
  (`manager/commands.rs:381-395`), and index omission is a soft delete (`store/index.rs:8-73`).
  File GC therefore cannot hang off thread deletion. It runs as an orphan sweep.
- **Transcript pills already render.** `rows/item.rs:59-100` maps each attachment to a name
  (`:789-808`), and `fleet-ui-kit/src/components/agent/rows/message.rs:43-53,103-120` draws a
  paperclip pill *above* the text. `docs/UX-SPEC.md:1018` says *below*. That disagreement is a bug
  in one of them. The optimistic pending bubble hard-codes an empty list (`rows/item.rs:345-360`).
- **Settled limits** (`TODO.md:58-59`): 120 000 characters, 8 attachments, 10 MiB per image,
  50 MiB per file. Only the Claude mapper enforces any of them.

**Composer:**
- `AgentThreadView.input: Entity<MultilineInput>` (`screens/agent_thread/mod.rs:146-152`), built
  and subscribed at `:261-279`, with `on_input_event` at `:691-710` handling
  Submit/Trigger/Changed/Escape. `MultilineInput` wraps an `Entity<TextInput>`
  (`fleet-ui-kit/src/components/multiline_input/input.rs:19-29`). Its events are at
  `multiline_input.rs:25-39`.
- **The view already knows its `ThreadId`** (`mod.rs:146-152,261-266`). Because
  `MediaAnchor::Thread` routes by thread inside the daemon, the app does **not** need the typed
  `HostId`. The old "plumb `HostId` into `ThreadHost`" problem is gone. `ThreadHost`
  (`mod.rs:101-108`) still supplies the display name for the toast.
- **Attachments are already modelled** in `composer.rs`:
  - `accepts_attachments` per `ComposerMode` (`:60-76`);
  - `submit_gate` counts attachments as content (`:132-148`), with tests
    (`tests/composer.rs:37-108`).
  But the send path calls `submit_gate(&text, 0, …)` (`actions.rs:103-108`) and builds
  `attachments: Vec::new()` (`mod.rs:722-733`).
- **Link down:** `is_unreachable()` (`mod.rs:554-558`) makes the input read-only with an
  "unreachable" placeholder (`sync.rs:613-653`, `presentation.rs:316-320`). A send that races the
  transition restores the text and shows a notice (`actions.rs:134-151`). Media must follow the
  same rules.

**Kit:**
- `TextInput` paste is text-only (`components/input/handlers.rs:254-267`). `TextInputEvent`
  (`components/input.rs:45-64`) is **`Copy`** with no payloads, so a media payload must not be
  squeezed into it. `TextInput::insert` (`input.rs:168-180`) is the caret insertion phase 3 uses.
- `components/agent/` holds the composer bar, rows and docks (`agent.rs:10-20`).
- A generic `Chip` exists (`components/chip.rs:15-91`). `ComposerChip` is a settings menu trigger
  (`agent/composer_bar.rs:22-65`), not an attachment.
- The transcript attachment pill is private (`rows/message.rs:103-120`). **No removable or pending
  attachment chip exists.**
- Galleries: `examples/gallery_agent.rs` is the acceptance test for agent components
  (`docs/NATIVE-AGENTS.md:1602-1619`), and `examples/gallery_input.rs` for inputs.

**Skills to load:**
- `gpui-components` and `gpui-styling` (kit);
- `gpui-state-and-memory` (composer model, tasks);
- `rust-async-background-work` (staging, manager);
- `rust-workspace-architecture` (store writer, errors);
- `rust-gpui-testing` (all tests);
- `zed-quality-review` before done.

## Assumptions

- Phase 1 has shipped: `StageMedia` with `MediaAnchor::Thread`, which stages into the thread's leaf
  directory on its owner host (spelling recorded in the phase-1 tracker, P1-T03 note), plus
  `fleet_app::media::{from_clipboard, from_external_paths, stage}`.
- **Every composer attachment is staged into the thread leaf, even for a local thread and a local
  file.** An attachment must survive the user moving or deleting the original, and the path has to
  be inside the directory `--add-dir` grants. This is the opposite of phase 1's local-`Paths`
  short-circuit and is passed as a parameter, not special-cased.
- Phase 1's upload is chunked and resumable (up to 1 GiB per drop), so the composer's settled
  limits of 10 MiB per image and 50 MiB per file are enforced as limits of their own, not
  transport caps. A remote thread gets the same progress, waiting-for-reconnect and resume
  behaviour as a remote terminal.
- A **folder** dropped on the composer is refused ("drop the files instead"). An attachment is a
  file.
- The attachment's `media_type` comes from `ImageFormat` for clipboard images, and from the file
  extension for files, using a small fixed table (the four Claude image types plus
  `application/octet-stream`). No new dependency.
- Pending chips are not persisted across an app restart or thread switch. The unreferenced file
  is collected by the orphan sweep (T05).

## Out of scope

- Dialog `TextInput`s (phase 3).
- Folder attachments.
- Files over the settled limits (10 MiB per image, 50 MiB per file).
- Image thumbnails in the chip or in the transcript pill. A name and an icon are enough.
- A thread-delete verb and its file cleanup.
- Restoring pending chips after a restart.
- Leading text blocks before images for Claude. The current `[images…, final text]` order stays.

## Affected areas

- **`crates/fleet-ui-kit`**: `components/input.rs` and `input/handlers.rs` (media mode and event);
  `components/multiline_input.rs` and `multiline_input/input.rs` (relay, builder);
  `components/agent/` (new pending-attachment chip, and the pill shared with the transcript);
  `examples/gallery_input.rs`, `examples/gallery_agent.rs`.
- **`crates/fleet-daemon`**: `agents/claude/mod.rs` (path images), `services/agents/providers/mod.rs`
  (`attachments_dir`), `services/agents/manager/commands.rs` (validation, base64 materialisation),
  `services/agents/store/*` (writer and orphan sweep), `services/maintenance.rs` or daemon start
  (sweep hook).
- **`crates/fleet-app`**: `screens/agent_thread/{mod.rs, actions.rs, composer.rs, rows/item.rs}`
  and the composer render; `media.rs` (the `media_type` table).
- **`docs/`**: `NATIVE-AGENTS.md`, `TODO.md` §5, `UX-SPEC.md`, `DESIGN-SYSTEM.md`,
  `APP-CONTRACTS.md` if the event pattern is new, and ADR 0013 if its attachment section needs an
  as-built note.

## Tasks

### P2-T01 — Let a kit input report media instead of inserting it

**Intent:** an opt-in `TextInput` mode where a non-text paste or a file drop becomes an event with
gpui payloads. The kit learns nothing about Fleet.

**Touches:** `fleet-ui-kit/src/components/{input.rs, input/handlers.rs, multiline_input.rs, multiline_input/input.rs}`,
`examples/gallery_input.rs`.

**Steps:**
- Load `gpui-components`, `gpui-styling` and `gpui-state-and-memory`.
- Add a separate, non-`Copy` event type, for example
  `pub enum TextInputMedia { Pasted(ClipboardItem), Dropped(ExternalPaths) }`, with a second
  `EventEmitter` impl on `TextInput`. Keep `TextInputEvent` `Copy`; many subscribers match on it.
  Name the variants for what happened, not what the app does.
- Add a builder or setter, `accepts_media(bool)`, off by default. When it is off, behaviour is
  byte-identical to today.
- When it is on:
  - `paste` (`handlers.rs:254-267`) emits `Pasted` if the item has an `Image` or `ExternalPaths`
    entry. It inserts text only when the item is text-only.
  - The input root takes `on_drop::<ExternalPaths>` and emits `Dropped`, with a token-only hover
    affordance.
  - A read-only input does neither.
- `MultilineInput` exposes the same switch and relays `TextInputMedia` in its own event, as a new
  `MultilineInputEvent` variant or a relayed second emitter. Pick the one that matches how it
  relays today, and say which in the tracker.
- Gallery: show an input in the drop-hover state in `gallery_input.rs`.

**Verification:** `cargo test -p fleet-ui-kit` (a `#[gpui::test]` writes an image and a paths item
to the test clipboard and asserts on the event, and on byte-identical text paste with the mode
off); `cargo run -p fleet-ui-kit --example gallery_input`; `make lint`.

**Done when:** with the mode on, a media paste or drop emits exactly one event and inserts nothing.
With it off, nothing changes. `fleet-ui-kit/Cargo.toml` is unchanged.

### P2-T02 — A pending-attachment chip

**Intent:** the kit component for an attachment that is uploading (with progress), waiting for a
reconnect, ready or failed, and removable.

**Touches:** `fleet-ui-kit/src/components/agent/` (new module), `rows/message.rs`,
`examples/gallery_agent.rs`.

**Steps:**
- Load `gpui-components` and `gpui-styling`.
- A `RenderOnce` chip with no domain types. It takes:
  - a display name;
  - a state: `Staging(progress 0..=1)`, `Waiting` (the host is reconnecting), `Ready`, or
    `Failed(SharedString)`;
  - an `on_remove` handler.
  The icon, tone and spinner or animation all come from tokens.
- Share the visual with the transcript pill (`rows/message.rs:103-120`) instead of duplicating it.
  Promote the pill, or build both on one private primitive.
- Add a composer-bar slot for a row of chips (`agent/composer_bar.rs`).
- Gallery: every state, a row of 8 chips, and a long name that truncates, in `gallery_agent.rs`.

**Verification:** `cargo test -p fleet-ui-kit`; `cargo run -p fleet-ui-kit --example gallery_agent`;
`make lint`.

**Done when:** the gallery shows every chip state, and the transcript pill still renders as before.

### P2-T03 — Providers receive staged attachments properly

**Intent:** a staged image path reaches Claude as an image block, and Claude can read non-image
files in the leaf.

**Touches:** `agents/claude/mod.rs:748-802`, `agents/claude/tests/goldens.rs`,
`services/agents/providers/mod.rs:402-418`.

**Steps:**
- Load `rust-async-background-work`.
- Claude mapper: a `Path` attachment whose `media_type` is one of the four supported images **and**
  whose canonical path lies inside this thread's attachment leaf is read, checked against the
  10 MiB image limit, and emitted as a base64 image block. Any other `Path` keeps today's
  behaviour: the path is appended as text. Read off the runtime thread (`spawn_blocking`), or read
  before the mapper if the mapper must stay synchronous.
- Keep `[images…, final text]`. Add a golden for a path image, and one for a path outside the leaf
  (stays text).
- Set `attachments_dir: Some(leaf)` for Claude launches (`providers/mod.rs:402-418`), creating the
  leaf `0700` if missing, so Claude can open non-image files there. Codex needs nothing, because
  `localImage` and `mention` already take paths.
- Add a Codex wire test that a staged image path becomes `localImage`, pinning the behaviour this
  phase now relies on.

**Verification:** `cargo test -p fleet-daemon claude`; `cargo test -p fleet-daemon codex`;
`make lint`.

**Done when:** the goldens show a path image as an image block before the final text, and the
Claude argv carries `--add-dir <leaf>`.

### P2-T04 — Validate once, and keep base64 out of SQL

**Intent:** the settled limits are enforced for every provider, and nothing persists bytes in a row.

**Touches:** `services/agents/manager/commands.rs:435-518`, the phase-1 `Media` service.

**Steps:**
- Before provider dispatch, validate, all as `ErrorKind::Validation` with messages that name the
  limit:
  - ≤ 120 000 characters;
  - ≤ 8 attachments;
  - an image ≤ 10 MiB;
  - a file ≤ 50 MiB (stat the path).
  Remove the now-duplicate checks from the Claude mapper, or keep them there as assertions,
  whichever the tests make clearer.
- A `Base64` source arriving over the wire (e.g. from the CLI) is **materialised** into the
  thread's leaf through the phase-1 `Media` service and rewritten to `Path` before the manager
  records the item, so ADR 0013 holds. `Url` stays as is.
- Tests: over each limit; a `Base64` send ends up as a `Path` in the recorded item and as a file in
  the leaf.

**Verification:** `cargo test -p fleet-daemon agents`; `make test`.

**Done when:** each limit is refused with its message, and no recorded item contains base64.

### P2-T05 — Record attachments durably, and collect orphans

**Intent:** `item_attachments` gets its writer, and unreferenced files do not accumulate.

**Touches:** `services/agents/store/{writer.rs, schema.rs, mod.rs}`, daemon start or
`services/maintenance.rs`.

**Steps:**
- Load `rust-workspace-architecture`.
- When a user item with attachments is recorded, write one `item_attachments` row per attachment
  that lies under the attachments root, in the same transaction as the item:
  - `relative_path` is relative to `agents_attachments_path()`;
  - `bytes` comes from `stat`;
  - `mime` is the `media_type`.
  Paths outside the root and `Url`s get no row.
- **Orphan sweep at daemon start**, per ADR 0013. Delete a file in a thread leaf only if **all**
  of these hold:
  1. it sits directly in a leaf under the attachments root;
  2. no `item_attachments` row references it;
  3. it is a regular file;
  4. its mtime is older than a 24 h grace, so a chip staged just before a restart survives long
     enough to be sent.
  Also run it on a slow periodic tick, so a long-lived daemon collects removed chips. Write the
  invariant as a doc comment.
- Remote mirrors never hold attachment files; only the owner host does. Leave `mirror.rs`'s row
  discard as is and add a comment saying why.
- Tests: rows written in-transaction; the sweep keeps referenced files, fresh files, directories and
  symlinks, and removes an old unreferenced file.

**Verification:** `cargo test -p fleet-daemon store`; `make test`.

**Done when:** a sent attachment has a row, and an abandoned staged file is collected after the
grace period.

### P2-T06 — The composer: pending chips, send, and the edge cases

**Intent:** the user-visible feature.

**Touches:** `screens/agent_thread/{mod.rs, actions.rs, composer.rs, rows/item.rs}`, the composer
render, `media.rs`.

**Steps:**
- Load `gpui-state-and-memory` and `rust-async-background-work`.
- Turn on `accepts_media` for the composer input only while `ComposerMode::accepts_attachments`
  (`composer.rs:60-76`) and the link is up. Update the flag in the same place the read-only and
  placeholder state are synced (`sync.rs:613-653`), not in `render`.
- The model is a `Vec<PendingAttachment { id, upload, name, media_type, state, path: Option<PathBuf> }>`
  on the view. The upload task lives in phase 1's upload registry, not on the chip. The staging
  continuation looks the chip up by `id`, so a removed chip's late result is dropped. **Removing a
  chip while it uploads cancels that upload** through the registry.
- Chip state mirrors the registry's upload state (the P2-T02 states). Read progress in the update
  path, never in `render`.
- On a media event:
  - convert it with the phase-1 helpers;
  - refuse past 8 chips, a folder, an image over 10 MiB or a file over 50 MiB, with a notice
    before staging anything;
  - push a `Staging` chip;
  - call `media::stage` with `MediaAnchor::Thread { thread }` and `force_copy = true`;
  - on the path, set `Ready`; on an error, set `Failed(message)`.
  The toast reads "Copying <name> to <host>…" for a remote thread.
- Link down or unreachable: media events are refused with the same wording as the unreachable
  placeholder. They must not silently do nothing.
- Send:
  - `submit_gate` gets the real `Ready` count (`actions.rs:103-108`);
  - send is refused while any chip is `Staging` ("still copying <name>");
  - `Failed` chips are excluded, and the user is told they were;
  - `AgentSend` carries `Attachment { name, media_type, source: Path }` for each `Ready` chip
    (`mod.rs:722-733`);
  - clear the chips on dispatch;
  - on a refused or raced send, restore them with the text (`actions.rs:134-151`).
- The optimistic pending bubble shows the attachment names (`rows/item.rs:345-360`).
- Tests (`#[gpui::test]`, `run_until_parked`):
  - paste → chip → ready → send carries the attachment;
  - remove during staging drops the late result;
  - the ninth attachment is refused;
  - unreachable refuses;
  - an attachment-only send is allowed.

**Verification:** `cargo test -p fleet-app agent_thread`; `make lint`; `make test`;
`make harness` (non-regression); `make restart`, then fill the manual matrix.

**Done when:** every manual matrix row passes, for local and remote threads, on Claude and on
Codex.

### P2-T07 — Move the docs with the code

**Touches:** `docs/NATIVE-AGENTS.md`, `TODO.md`, `docs/UX-SPEC.md`, `docs/DESIGN-SYSTEM.md`,
`docs/APP-CONTRACTS.md`, `docs/decisions/0013-sqlite-agent-transcripts.md` if needed.

**Steps:**
- `NATIVE-AGENTS.md`:
  - remove "Attachments are deferred" (`:1741-1743`) and the status line at `:1681-1685`;
  - document the leaf directory, staging over `StageMedia`, the Claude path-image rule, validation,
    the `item_attachments` writer and the orphan sweep.
- `TODO.md`: delete §5 and renumber, or mark it done in whatever way the file's convention uses.
- `UX-SPEC.md`: the composer gesture, chip states, refusal copy, link-down behaviour. Also resolve
  pill *above* versus *below* (`:1018` versus `rows/message.rs:43-53`) by fixing whichever is
  wrong, in the same commit.
- `DESIGN-SYSTEM.md`: `TextInput`'s media mode and event, and the chip component contract.
- `APP-CONTRACTS.md`: how a screen consumes a kit media event, if that is a new pattern.
- Each doc edit rides with its code commit.

**Verification:** `make lint`; re-read each edited section against the code.

**Done when:** no doc says attachments are deferred, and none contradicts the implementation.

## Verification

```sh
make lint
make test
make check
make harness     # non-regression; clipboards and OS drags are not scriptable
make restart     # before the manual matrix
cargo run -p fleet-ui-kit --example gallery_agent
cargo run -p fleet-ui-kit --example gallery_input
```

## Definition of done

- [ ] Every P2 task is ticked in the tracker, with verification output beside it.
- [ ] `make lint`, `make test`, `make check` and `make harness` pass.
- [ ] `fleet-ui-kit/Cargo.toml` is unchanged, and the kit names no Fleet type.
- [ ] The galleries show the input's drop-hover state and every chip state.
- [ ] The manual matrix passes for a local and a Tailscale-hosted thread, on Claude and on Codex.
- [ ] No recorded item contains base64, and every sent attachment has an `item_attachments` row.
- [ ] Spawn rule held: each task either reports its own failures before `.detach()` or uses
      `detach_and_log_err`. No `unwrap`/`TODO`, and no silent `let _ =`.
- [ ] Docs match the code, and `TODO.md` §5 is closed.
- [ ] `zed-quality-review` has been run over the diff.

## Risks and rollback

- **Staging to the wrong machine.** The `Thread` anchor makes the daemon route by thread, so the
  app cannot pick the wrong host. Never substitute `Local` for a thread.
- **The Claude path-image read is a file read driven by the client.** It is restricted to the
  thread's own leaf after canonicalisation. Test `..` and symlink escapes.
- **The orphan sweep deletes a file that is about to be sent.** The 24 h grace covers restarts. A
  chip older than that is unusual. If one is sent after its file was swept, the provider errors,
  and that error must surface, not vanish.
- **Base64 over the wire from other clients.** This is handled by materialising, not by rejecting,
  so the CLI keeps working. For a *remote* thread, though, such an `AgentSend` still crosses the
  link as one large frame, which is the failure phase 1's chunking exists to avoid. Leave it
  working, record it as a follow-up (the CLI should stage through `StageMedia` first), and do not
  add new large-frame paths.
- **Rollback.** Revert the app commits and the composer is unchanged; the daemon side stays inert
  (no client sends `Path` attachments into the leaf). Revert the daemon commits as well and the
  mappers go back to today's behaviour.
