# Fleet

Fleet is the native successor to `swarm`. It pairs a long-running daemon, `fleetd`, with the
native GPUI app and CLI, `fleet`, to manage copy-on-write worktrees, GitHub pull requests, and
terminal sessions that replace tmux. Jobs and terminals belong to the daemon, so background work
survives closing a dialog, workspace, or the entire UI.

## Requirements

- macOS.
- [rustup](https://rustup.rs/) with Rust 1.97.1. `rust-toolchain.toml` selects the pinned compiler,
  `rustfmt`, and Clippy.
- Xcode and its Metal toolchain. If Metal is missing, install it with
  `xcodebuild -downloadComponent MetalToolchain`.
- Zig 0.15.2. On Apple Silicon, `scripts/bootstrap-zig.sh` installs and verifies the pinned release
  and exposes it at `~/.cargo/bin/zig`.
- `git` and the GitHub CLI, `gh`, authenticated for the repositories Fleet will manage.

## Build and run

```sh
make bootstrap
make run
```

`make run` builds the workspace, restarts `fleetd` from the same debug build, and opens the app.
Use `make run-release` for an optimized build. Running `fleet` without a subcommand opens the app.
The client auto-spawns `fleetd` when its Unix socket is unavailable. Fleet stores config, state,
repositories, worktrees, caches, logs, trash, and daemon files under `FLEET_HOME`, which defaults to
`~/.fleet`:

```sh
FLEET_HOME=/path/to/fleet-home make run
```

To start copying compatible swarm v1 config and state without modifying `~/.swarm`:

```sh
./target/release/fleet import --from-swarm
```

## Configuration

Fleet reads `$FLEET_HOME/config.json` (normally `~/.fleet/config.json`). To place worktrees on a
Tailscale peer, configure the peer under `hosts` and optionally make it the default placement:

```json
{
  "hosts": {
    "dev-box": {
      "provider": "tailscale",
      "node": "dev-box",
      "user": "df",
      "sshOptions": [],
      "fleetd": "fleetd",
      "fleetHome": "~/.fleet"
    }
  },
  "defaultHost": "dev-box"
}
```

`node` is a Tailscale hostname or MagicDNS name. `user` is optional, `sshOptions` defaults to an
empty list, `fleetd` defaults to `fleetd`, and `fleetHome` defaults to `~/.fleet`. Fleet resolves
the node with the local Tailscale CLI and uses non-interactive OpenSSH to run a compatible remote
`fleetd`; it does not expose a daemon port on the tailnet. Set `defaultHost` to `local` to keep
implicit creation local, or pass `--host` for one creation. A non-local `defaultHost` must name a
configured host.

The advanced/testing transport is a tagged command entry such as
`{"provider":"command","run":["sh","-c","exec \"$@\"","--"],"fleetd":"/tmp/fleetd","fleetHome":"/tmp/remote","display":"loopback"}`.
It executes `run` followed by each remote command. Existing entries shaped as
`{"ssh":"arch-dev","swarmCommand":"swarm"}` still load, but are **legacy probe-only**: they
report reachability and cannot host Fleet worktrees until migrated to a federated provider.

## CLI reference

Run `fleet --help` or `fleet <command> --help` for generated help.

| Command | Description | JSON success fields |
| --- | --- | --- |
| `fleet create <REPO> <SLUG> [--branch <BRANCH>] [--base <BASE>] [--host <HOST>] [--url <URL>] [--default-branch <BRANCH>] [--hooks <JSON>] [--json]` | Create or find a worktree; `--url` is required for an unregistered repository. | `protocol`, `created`, `worktree` |
| `fleet open <TARGET>` | Ensure a worktree session exists; accepts a worktree id, stored session name, or `repo/slug` alias. | — |
| `fleet list [--json]` | List registered repositories and worktrees. | `protocol`, `version`, `repos`, `worktrees` |
| `fleet inspect [IDS]... [--fetch] [--repo <REPO>] [--json]` | Inspect all or selected worktrees, optionally fetching or restricting by repository. | `protocol`, `worktrees` |
| `fleet delete <IDS>... [--json]` | Unconditionally delete one or more exact worktree ids. | `protocol`, `ok`, `results` |
| `fleet prune [--dry-run] [--no-fetch] [--kill-sessions] [--repo <REPO>] [--json]` | Safely prune merged worktrees; fetching is on unless `--no-fetch` is used. | `protocol`, `dryRun`, `deleted`, `skipped` |
| `fleet kill <ID> [--json]` | Hard-kill the session for an exact worktree id. | `protocol`, `ok` |
| `fleet status [--json]` | Refresh local worktree runtime status. | `protocol`, `statuses` |
| `fleet path <ID> [--json]` | Print a local absolute path or `<host>:<path>` for a remote worktree. | `protocol`, `path`, `host` |
| `fleet sleep [SESSION] [--json]` | Apply sleep policy to a session or worktree; a sole running session is inferred. | `protocol`, `kept`, `closed`, `sessionKilled` |
| `fleet watch list [--session <id>] [--json]` | List cooperative and daemon-discovered watches (defaults to `FLEET_SESSION`); human rows contain id, source, label, status, start time, and terminal id. | `protocol`, `watches` |
| `fleet watch tail <id> [--follow]` | Print retained text on its original stdout/stderr channel; `--follow` polls every 250 ms until exit. | Raw retained stdout/stderr text |
| `fleet exec [--watch] [--label TEXT] -- CMD [ARGS...]` | Run a child with byte-exact passthrough; optionally publish a read-only subagent watch using `FLEET_SESSION` and numeric `FLEET_TERMINAL_ID` (`FLEET_TERMINAL` remains the human name). | Raw child stdout/stderr; child exit status |
| `fleet agent list` | List native-agent threads: id, provider, host, session, attention, worktree, title. | — |
| `fleet agent new <WORKTREE> --provider <claude\|codex> [--model <MODEL>] [--mode <ask\|accept-edits\|plan\|full-access>]` | Start a native-agent thread in a published worktree and print its id. Reports the typed `Unsupported` error, naming the terminal fallback, when the provider executable is missing or too old. | — |
| `fleet agent send <THREAD> <TEXT>` | Send or steer a message on a thread. | — |
| `fleet agent respond <THREAD> <GATE> <ANSWER>` | Answer an open permission, question, or plan gate with provider-neutral words. | — |
| `fleet agent interrupt <THREAD>` | Interrupt the active turn; the provider's terminal event stays authoritative. | — |
| `fleet agent stop <THREAD>` | Stop the provider and retain the transcript. | — |
| `fleet agent tail <THREAD> [--replay]` | Print one JSON `SeqEvent` per line until the provider exits; `--replay` starts from sequence 1. | One `SeqEvent` object per line |
| `fleet agent terminal [claude\|codex]` | Ensure a repository-level PTY agent session exists (the terminal fallback); defaults to `config.agent`. | — |
| `fleet agent-status <working\|finished\|permission\|question\|plan> [--session <SESSION>] [--terminal-id <ID>] [--json]` | Report agent lifecycle or attention; target flags default to `FLEET_SESSION` and `FLEET_TERMINAL_ID`. Silent on non-JSON success. | `protocol`, `ok`, `session`, `terminalId`, `activity`, optional `attention` |
| `fleet host list [--json]` | List configured hosts with provider, reachability, daemon link state, version, address, and known agent binaries. | `protocol`, `hosts` |
| `fleet host doctor <ID>` | Diagnose resolution, SSH/authentication, remote `fleetd`, protocol compatibility, and agent binaries for one host. | — |
| `fleet host bootstrap <ID> [--ref <GIT_REF>]` | Build and install a matching `fleetd` on the host, restart it, and wait for the federated link. | — |
| `fleet doctor` | Run environment diagnostics; exits unsuccessfully when any check fails. | — |
| `fleet import --from-swarm` | Start an import of compatible `~/.swarm/config.json` and `state.json`. | — |
| `fleet update` | Run self-update, wait for completion, then exit with restart code 75. | — |
| `fleet version`, `fleet -v`, or `fleet --version` | Print the package version and build Git revision. | — |

Commands that accept `--json` emit one compact line using swarm-compatible protocol 1 envelopes.
Their errors use `{"protocol":1,"error":{"kind":"<kind>","message":"<message>"}}`; other
commands use human-readable output. This public envelope is separate from daemon IPC version 7;
neither the bug-fix program nor the native-agent work changed CLI envelope version 1.

### Board

Each context has one board, created on first use. `fleet board show` displays its
columns and cards under a header naming the backend, its project, the age of the last
sync, and the dirty and conflict counts; `fleet board list` lists board summaries.
Use `fleet board create` with `--name`, `--prefix`, or `--backend local`, and
`fleet board set` to change its name, prefix, default repository
(`--default-repo owner/name`, `--clear-default-repo`), worktree-start setting,
conflict policy, branch template (`--branch-template "{key}-{slug}"`), or whether new
cards are pushed to the backend (`--push-new-cards`, off by default: a card created
locally stays local). Board labels are created and removed there too, with
`--add-label NAME` and `--remove-label <id|name>`: `--label` on `card new` and
`card edit` only selects labels the board already carries, so add one to the board
before a card can wear it.
`fleet board sync --wait` waits for a remote backend's sync job and prints its
counts and errors; `--full` ignores the incremental cursor and pulls the backend's
complete set. Local boards do not support remote sync.

`fleet board backends` lists the registered backend kinds with their capabilities and
the setting keys each accepts; `fleet board describe` prints what the board's backend
reports about itself: its statuses and their categories, labels, properties, people,
and the fields it cannot write back. Editing a read-only field fails with the daemon's
message.

Point a board at a backend with `fleet board set --backend jira --setting project=SP
--setting jql='sprint in openSprints()'`. `--setting` is repeatable and takes
`key=value`; values parse as JSON when they are valid JSON (`8`, `true`,
`["To Do","Done"]`) and stay strings otherwise, and `key=null` removes a key.
`--backend` starts the settings from empty, so a kind change must supply everything the
new kind needs in the same command; `--setting` without `--backend` keeps the kind and
merges into the stored settings. Changing the *kind* of a board whose cards are already
linked is refused; changing the settings of the same kind is not, and costs only the
incremental cursor. `fleet board create` takes the same `--setting` flag
alongside `--backend`.

The `jira` backend mirrors one Jira project through the Atlassian CLI, so it needs
`acli` on `PATH` and an authenticated session (`acli jira auth login`); Fleet never
holds an API token. Priority, estimate, due date, and parent are read-only on a Jira
board because `acli` cannot write them back.

Card commands are `fleet board card new <title>`, `show <key|id>`,
`edit <key|id>`, `move <key|id> <status> [--index N]`,
`comment <key|id> <body>`, `delete <key|id>`,
`worktree <key|id> [--repo owner/name] [--base REF] [--host H]`, and
`resolve <key|id> keep-local|take-remote`. New/edit accept description, status,
priority, labels, assignee, estimate, due date, and repository flags. `edit` alone
also takes `--archive` and the `--clear-labels`, `--clear-assignee`,
`--clear-estimate`, `--clear-due` and `--clear-repo` flags, which are the only way
to unset a field; `new` refuses them, since a card is born with nothing to clear.

Select a board with `--board <id>` or `--context <id>`; otherwise Fleet uses the
active context. Card selectors accept a display key, a card ID, or — for a card with no
remote issue — its local key.
Every board command accepts `--json` for protocol 1 envelopes. In the app, `g b`
opens the board, `c` creates a card, `enter` opens its detail, `x` opens the focused
card's remote issue in the browser, and `F` runs a full sync.

### Agent hooks

Fleet detects Claude Code, OpenCode, and Codex working/idle status from terminal output by default.
Only explicit agent hooks create attention notifications. Fleet injects `FLEET_SESSION` and
`FLEET_TERMINAL_ID` into every managed PTY, so the recommended Claude Code configuration is:

```json
{ "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "fleet agent-status working" }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "fleet agent-status finished" }] }],
    "Notification": [
      { "matcher": "permission_prompt", "hooks": [{ "type": "command", "command": "fleet agent-status permission" }] },
      { "matcher": "idle_prompt",       "hooks": [{ "type": "command", "command": "fleet agent-status question" }] }
    ],
    "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "fleet agent-status permission" }] }]
} }
```

`PermissionRequest` is a current Claude Code hook event and runs when a permission dialog is about
to be shown. Without hooks, Fleet still shows heuristic working/idle glyphs, but it does not show
agent attention toasts or play an agent attention sound.

Explicit permission, question, plan, and finished notifications use an in-app toast and the macOS
Glass sound by default. Either
channel can be disabled independently in `~/.fleet/config.json` (or `$FLEET_HOME/config.json`):

```json
{
  "ui": {
    "notifications": {
      "toast": true,
      "sound": true
    }
  }
}
```

## Keyboard basics

Fleet is modal. The status bar always shows the current mode; overlays shadow the Hub or Workspace
until closed.

| Mode | Purpose | Leave with |
| --- | --- | --- |
| Normal | Navigate Hub repositories, worktrees, and pull requests. | Open a session |
| Terminal | Send keys to the active PTY. | `ctrl-s` enters Prefix |
| Agent | Type into a native agent thread's composer and answer its decision cards. | Select another tab, `ctrl-s x` |
| Prefix | One-shot Workspace or Agent popup command after `ctrl-s`. | Next key or `Esc` |
| Scroll | Navigate and select terminal scrollback. | `Esc`, `q`, or `i` |
| Filter | Filter the current list. | `Enter` or `Esc` |
| Palette | Search navigation and actions. | `Enter` or `Esc` |
| Dialog | Edit or confirm an action. | `Enter` or `Esc` |
| Jobs | Inspect, cancel, or retry daemon jobs. | `J`, `q`, or `Esc` |
| Daemon | Report startup, disconnect, or doctor state. | Reconnect, `Esc`, or `ctrl-q` |
| FirstRun | Guide initial creation or import. | Complete an offered action |

The 16 keys and key groups to learn first are:

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | Move the cursor. |
| `h` / `l`, `←` / `→`, `S-Tab` / `Tab` | Focus the previous or next Hub pane. |
| `gg` / `G` | Jump to the first or last row. |
| `Enter`, `o` | Select a repository or open a worktree/session. |
| `1`–`9`, `gt` / `gT` | Switch to a numbered, next, or previous context. |
| `p` | Toggle Worktrees and Pull requests. |
| `n` | Clone a repository or create a worktree in the focused pane. |
| `d` | Delete the selected repository or worktree, with confirmation. |
| `/` | Filter the current list. |
| `:` | Open the command palette. |
| `i` | Toggle the detail panel. |
| `a` / `A` | In the Hub, open the floating Claude / Codex agent popup; in a Workspace, `ctrl-s a` / `ctrl-s A` start a native agent thread instead. |
| `r` | Refresh status, pull requests, and discovery as a job. |
| `J` | Open the Jobs panel. |
| `?` | Open help. |
| `Esc` / `q`, `ctrl-q`, `ctrl-shift-q` | Close the top layer; quit the app; or quit and stop the daemon. |

Inside a terminal, every bare key goes to the PTY. Dragging selects text and copies it immediately;
`cmd-c` copies the current selection and `cmd-v` pastes through the terminal's bracketed-paste
path. Soft-wrapped visual rows copy as one logical line. `ctrl-c` and `ctrl-v` remain terminal
keys. `ctrl-s` is the only Workspace prefix:
`ctrl-s s` returns to Hub, `ctrl-s S` sleeps then returns, `ctrl-s 1`–`9` switches tabs,
`ctrl-s h`/`l` changes tabs, `ctrl-s w` opens the last session, `ctrl-s c`/`x` creates/closes a
tab, `ctrl-s a`/`A` starts a native Claude/Codex agent thread, `ctrl-s F` opens the floating
agent popup (the terminal fallback), `ctrl-s [` enters Scroll, `ctrl-s ]` pastes,
and `ctrl-s J`/`?` opens Jobs/help. Inside the popup, `ctrl-q` hides it without stopping the
agent session. Use
`ctrl-s ctrl-s` to send a literal `ctrl-s`. See [docs/KEYMAP.md](docs/KEYMAP.md) for the complete,
authoritative map.

A native agent thread is a tab drawn by Fleet, not a PTY: the status bar reads `AGENT`, `Enter`
sends the composer and `Shift-Enter` inserts a newline, `Esc` interrupts a running turn, and a
permission, question or plan appears as a card in the thread that answers to bare keys
(`y`/`a`/`n`/`e`/`Esc`, `1`-`4`, `y`/`n`). The tab carries an amber dot when the agent is waiting
on you. The daemon owns the thread, so it survives closing the app, and its transcript survives a
daemon restart. `fleet agent` drives the same threads from the terminal, and the popup on
`ctrl-s F` stays available whenever a provider is missing or too old. See
[docs/NATIVE-AGENTS.md](docs/NATIVE-AGENTS.md).

## Architecture

```text
fleet-app     binary `fleet`: GPUI state mirror, screens, dialogs, and terminal rendering
fleet-daemon  binary `fleetd`: stores, adapters, services, jobs, PTYs, and Unix socket server
fleet-core    domain types, schemas, validation, defaults, and pure helpers; no I/O
fleet-proto   length-prefixed JSON Request/Response/Event types and terminal frame updates
fleet-term    portable PTYs, VT engine abstraction, Ghostty VT, terminal host, and key encoding
fleet-client  async daemon connect/spawn, requests, events, and terminal attachment
fleet-ui-kit  domain-independent GPUI theme, assets, Lucide icons, and reusable components
fleet-git     typed, byte-preserving Git backend that shells out to `git` with explicit argv
fleet-lazygit native lazygit clone: panels, keymap, diff view, and overlays
fleet-cli     Clap parser, protocol-1 JSON envelopes, and human-readable output
runtime       `fleet` -> `fleet-client` -> `$FLEET_HOME/fleetd.sock` -> `fleetd`
ownership     daemon owns jobs, sessions, PTYs, state, and filesystem work; clients mirror it
```

The dependency direction is `core <- proto <- {term, client, cli} <- {daemon, app}` and
`git <- lazygit <- app`; the UI kit depends only on GPUI. Read [architecture](docs/ARCHITECTURE.md),
the [UX specification](docs/UX-SPEC.md), and the [design system](docs/DESIGN-SYSTEM.md) for the
full contracts, and the [documentation index](docs/README.md) for everything else.

## Development

The checked-in `Makefile` provides all workspace targets:

```sh
make help
make check
make build
make run
make daemon
make test
make fmt
make clippy
```

`make test` runs the workspace tests. Targeted Cargo tests work normally, for example
`cargo test -p fleet-cli`. Keep parallel worktrees on separate target directories if overriding
`CARGO_TARGET_DIR`; shared external artifacts can be stale.

Run the complete UI-kit gallery or one of its focused galleries:

```sh
cargo run -p fleet-ui-kit --example kit_gallery
cargo run -p fleet-ui-kit --example gallery_data
cargo run -p fleet-ui-kit --example gallery_input
cargo run -p fleet-ui-kit --example gallery_structure
cargo run -p fleet-ui-kit --example gallery_terminal
cargo run -p fleet-ui-kit --example gallery_agent
```

For UI automation, set `FLEET_DRIVE` to an append-only script. The debug app polls it every 100 ms;
commands include `key`, `type`, `wait`, `shot`, and `quit`, and results go to `$FLEET_DRIVE.log`:

```sh
mkdir -p /tmp/fleet-drive
touch /tmp/fleet-drive/script.txt
FLEET_HOME=/tmp/fleet-drive FLEET_DRIVE=/tmp/fleet-drive/script.txt ./target/debug/fleet &
printf 'wait 500\nkey ?\nshot /tmp/fleet-drive/help.png\nkey escape\nquit\n' >> /tmp/fleet-drive/script.txt
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for Zig details, logs, and scripted-input semantics.

## Status

Fleet supports worktrees, terminal sessions, lifecycle operations, and native Claude
threads on configured Tailscale hosts through daemon federation. The app and CLI connect only to
the local `fleetd`; it routes work to the owning host, keeps cached remote inventory visible while
a host is offline, and resumes routing after the link recovers. Terminal sessions survive closing
the app because their owning `fleetd` retains them, but they do not survive that daemon restarting.
Native agent threads do: transcripts live under the owning daemon's `$FLEET_HOME/agents/`, and a
thread with a provider resume cursor is resumed the next time it is opened.
