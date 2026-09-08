# Fleet documentation

| Document | Authority over |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Processes, crates, the daemon, the terminal pipeline, the client |
| [APP-CONTRACTS.md](APP-CONTRACTS.md) | How the parts of `fleet-app` plug into each other |
| [UX-SPEC.md](UX-SPEC.md) | What every screen shows and why |
| [KEYMAP.md](KEYMAP.md) | Which key does what, in which context |
| [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md) | The `fleet-ui-kit` tokens and component contracts |
| [NATIVE-AGENTS.md](NATIVE-AGENTS.md) | Native Claude Code / OpenCode sessions: adapters, event model, thread state, the agent tab |
| [research/harness-protocols.md](research/harness-protocols.md) | Wire reference for the installed Claude Code and OpenCode versions |
| [research/agents-contracts.md](research/agents-contracts.md) | The shipped native-agent public API: names, signatures, serialized shapes, module paths |
| [SWARM-INVENTORY.md](SWARM-INVENTORY.md) | The swarm behavior Fleet must preserve, 1:1 |
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
| [0008](decisions/0008-native-agents.md) | Native agent sessions: structured protocols, completion authority, the reducer in `fleet-core` |
