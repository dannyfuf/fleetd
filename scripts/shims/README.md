# Cooperative subagent watches

`codex` and `claude` are one shim (`claude` is a symlink to `codex`). It puts a
subagent's output under `fleet exec --watch` without changing how the agent runs.

## Install

Prepend this directory in the shell profile that Fleet's login shells read:

```sh
export PATH="/absolute/path/to/fleetd/scripts/shims:$PATH"
# Only when fleet is not already on PATH:
export FLEET_BIN="/absolute/path/to/fleetd/target/debug/fleet"
```

## Contract

- The real executable is the first `codex`/`claude` on `PATH` outside this
  directory; every argument is passed through unchanged.
- A watch is registered only when `FLEET_TERMINAL_ID` is set, `FLEET_WATCH` is
  unset, and stdout is not a tty. The tty check leaves the user's interactive
  top-level agent alone; a piped subagent gets a read-only output watch, and
  `FLEET_WATCH` keeps watched children from nesting watches.
- Fleet login shells supply `FLEET_SESSION` (session ID), `FLEET_TERMINAL_ID`
  (daemon-local numeric terminal ID) and `FLEET_TERMINAL` (human terminal name,
  kept for existing scripts). Watches associate by `FLEET_TERMINAL_ID`; both it
  and `FLEET_SESSION` must be valid. Under older daemons that supply only the
  name, the shim is transparent.
- An unavailable Fleet executable or a daemon connection failure runs the
  command with inherited stdio. `FLEET_DEBUG=1` prints a one-line reason for
  each passthrough decision, including missing or invalid environment variables.
- The invoking agent receives raw bytes; only the daemon's display copy is
  decoded as lossy UTF-8. The child gets no PTY, and closing a watch never kills
  it.
