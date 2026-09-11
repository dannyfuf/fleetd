# 0010 — Native agent sessions

**Adopted** for `fleet-daemon`'s `services/agents/`, `fleet-core::agents`, and the
`screens/agent_thread/` tab in `fleet-app`. Claude Code and OpenCode run as structured sessions
Fleet understands — turns, tools, permissions, questions, plans — instead of as a PTY the user
reads. `docs/NATIVE-AGENTS.md` is the specification; this file records why the load-bearing
choices are what they are.

**Superseded in part by [ADR 0011](0011-remote-machines.md):** remote worktrees now route native
agent requests to the daemon that owns the worktree. That daemon runs the provider and stores the
transcript; the structured protocol, reducer, completion authority, and UI decisions below remain
in force.

**Superseded in part by [ADR 0013](0013-sqlite-agent-transcripts.md)** (SQLite replaces the
NDJSON log and `index.json`) **and by [ADR 0014](0014-drop-opencode-add-codex.md)** (Codex
replaces OpenCode). Two bullets below are therefore historical and are kept because they record
what was verified: "one managed `opencode serve` per thread" is withdrawn with the harness, and
"Rejected for the first release: Codex app-server" is the decision 0014 reversed, with its reasons
written down there.

**Amended by the harness seam of `NATIVE-AGENTS.md` §3.1.** "The daemon owns the process" is
narrower than it reads: the *adapter* owns the process and the *manager* owns the decision to
replace it. `Harness::apply_runtime` reports a `RestartPlan` rather than performing one, because
Claude's model, effort and mode are launch flags and only the manager knows whether a turn is
running. A control change that costs a restart is refused mid-turn rather than killing the turn.

- **Speak the structured protocol, never parse the terminal.** Both adapters consume a machine
  protocol and normalise it into one `AgentEvent` stream. Screen-scraping a harness's TUI was
  rejected: the output is a rendering, not a contract, and every state Fleet needs (a turn
  settling, a permission opening) is expressed there only as pixels.
- **Claude Code over bidirectional stream-json on stdio, with no Node sidecar.** The `claude`
  CLI is a native binary and the protocol is newline-delimited JSON, which `serde_json` reads
  directly, so the official Agent SDK's Node runtime would add a language runtime, an install
  step and a second failure mode for no capability Fleet lacks. The cost is that the control
  protocol is not a documented public contract, which is paid for with a version gate, recorded
  fixtures per CLI version, `raw` diagnostics on every event, and the terminal fallback.
- **One managed `opencode serve` per thread**, spawned in the worktree with its own process
  group. A shared server was rejected because MCP registration and directory registration are
  server-wide while the working directory belongs to the thread; one server per thread keeps a
  thread's blast radius to itself. Every request still carries `?directory=`, so the routing key
  is `(base_url, directory, session_id)` and an external-server mode stays possible later.
- **Only the provider's authoritative primitive completes a turn.** Claude's single `result`
  message and OpenCode's `session.status idle` are the sole terminal signals; silence, a stream
  closing, `step-finish`, or process liveness never are. Inferring completion is how a UI claims
  an agent finished while it is still thinking, and how an interrupted turn is reported as
  success — the interrupted Claude turn still emits its own `result`, which is what settles it.
  Stream or process loss is an explicit failure (`SessionExited { expected: false }`), never an
  inferred success.
- **The reducer lives in `fleet-core`, not in the daemon or the app.** `ThreadProjection::apply`
  is a pure function from `SeqEvent` to state, so the daemon reduces it to persist and broadcast,
  the client mirror replays the same code against the same events, and both cannot disagree
  about a thread. It also makes the whole state machine — attention included — testable by
  replaying the recorded fixtures with no process and no window.
- **An in-house Markdown renderer** (`fleet-ui-kit::markdown`), rather than Zed's `markdown`
  crate or a general CommonMark stack. The input is not a document, it is a stream: the property
  that matters is that every decided block of a prefix stays a block of the whole at the same
  index, so a transcript never reflows behind the reader. A small parser that guarantees that,
  and never panics or loops on any input, is worth more than the features it omits (tables,
  images, indented code). ADR 0003's "no third-party component library" applies here too.
- **The inline diff is `fleet-lazygit`'s, extracted rather than reimplemented.**
  `fleet_lazygit::diff_view` takes unified-diff *text*, so an embedder needs no git plumbing and
  `fleet-ui-kit` gains no `fleet-git` dependency, while the rows keep the ADR 0005 stack
  (`syntect`, `similar`, uniform rows) and therefore the same geometry as a full-window diff.
- **Rejected for the first release:** Codex app-server and ACP agents, a thread sidebar, a
  detached inspector or diff pane, voice, and a full editor as the composer. Also
  rejected: modal permission dialogs — a decision is a card in the thread, because the reason it
  is being asked is the transcript above it.
- **Still the fallback:** the PTY popup on `^s F` (ADR-era `^s a`/`^s A`). A provider that
  reports itself unavailable — missing executable, unsupported version — names that fallback in
  its error instead of leaving the user with nothing.

Provenance: `docs/research/harness-protocols.md` (the wire reference for Claude Code 2.1.263 and
OpenCode 1.17.18), the raw captures under `docs/research/fixtures/agents/`, and
`docs/research/agents-contracts.md` (the frozen public API the stages implemented against).
`t3code` (`pingdotgg/t3code` @ `8b2838e`) is the prior art the ownership boundary and the
completion rules are copied from.
