# Fleet documentation

| Document | Authority over |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Processes, crates, the daemon, the terminal pipeline, the client |
| [APP-CONTRACTS.md](APP-CONTRACTS.md) | How the parts of `fleet-app` plug into each other |
| [UX-SPEC.md](UX-SPEC.md) | What every screen shows and why |
| [KEYMAP.md](KEYMAP.md) | Which key does what, in which context |
| [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md) | The `fleet-ui-kit` tokens and component contracts |
| [BOARD.md](BOARD.md) | The card model, the reconciliation engine, and the board surface |
| [BOARD-JIRA.md](BOARD-JIRA.md) | The Jira backend, and what `acli` can and cannot do |
| [NATIVE-AGENTS.md](NATIVE-AGENTS.md) | Native Claude Code / Codex sessions: adapters, event model, thread state, the transcript, the decision surfaces, the agent tab |
| [REMOTE-MACHINES.md](REMOTE-MACHINES.md) | Authoritative remote-machine config, protocol, provider, routing, mirror, and recovery contract |
| [research/harness-protocols.md](research/harness-protocols.md) | Wire reference for the installed Claude Code version (OpenCode kept as a historical appendix) |
| [research/harness-codex-app-server.md](research/harness-codex-app-server.md) | Wire reference for the installed Codex app-server protocol |
| [research/agents-contracts.md](research/agents-contracts.md) | The shipped native-agent public API: names, signatures, serialized shapes, module paths |
| [SWARM-INVENTORY.md](SWARM-INVENTORY.md) | The swarm compatibility baseline; its Fleet deviations explicitly replace selected legacy behavior |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Building, running, testing, and driving the app |
| [decisions/](decisions) | Why the load-bearing choices were made |

Crate-level documentation lives beside the code: `crates/fleet-git/README.md` and
`crates/fleet-lazygit/README.md`.

## Decision records

Short, current records of choices that are expensive to revisit. Each states what was adopted,
what was rejected and why, and cites the research it was distilled from.

| # | Decision |
| --- | --- |
| [0001](decisions/0001-gpui-and-toolchain.md) | GPUI from Zed `v1.18.1`, Rust 1.97.1, and the boot sequence |
| [0002](decisions/0002-terminal-emulation.md) | `libghostty-vt` in the daemon, self-painted grid in the client |
| [0003](decisions/0003-design-system.md) | Fleet builds its own design system |
| [0004](decisions/0004-native-git-ui.md) | A native git UI instead of embedding lazygit |
| [0005](decisions/0005-diff-view.md) | Diff rendering: `syntect`, `similar`, uniform rows |
| [0006](decisions/0006-ux-lens-synthesis.md) | `UX-SPEC.md` as the synthesis of three UX lenses |
| [0007](decisions/0007-gui-smoke-procedure.md) | The reproducible GUI smoke procedure |
| [0008](decisions/0008-board-model-and-sync.md) | A backend-agnostic board model with a pure reconciliation engine |
| [0009](decisions/0009-jira-board-backend.md) | Jira through `acli`, not through the REST API |
| [0010](decisions/0010-native-agents.md) | Native agent sessions: structured protocols, completion authority, the reducer in `fleet-core` |
| [0011](decisions/0011-remote-machines.md) | Remote machines through one-daemon-per-machine federation and pluggable transports |
| [0012](decisions/0012-terminal-agent-attention.md) | Terminal agent attention: explicit hooks are the only notification source |
| [0013](decisions/0013-sqlite-agent-transcripts.md) | SQLite for agent transcripts: one event log, synchronous projections, one owned writer |
| [0014](decisions/0014-drop-opencode-add-codex.md) | Drop OpenCode, add Codex, and gate it on a capability string rather than a protocol bump |
