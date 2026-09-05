# Cooperative subagent watches

Prepend this directory in the shell profile read by Fleet's login shells:

```sh
export PATH="/absolute/path/to/fleetd/scripts/shims:$PATH"
# Optional when fleet is not on PATH:
export FLEET_BIN="/absolute/path/to/fleetd/target/debug/fleet"
```

The `codex` and `claude` shims find the real executable by scanning PATH outside
this directory. They preserve every argument and invoke `fleet exec --watch`
only when `FLEET_TERMINAL_ID` is set, `FLEET_WATCH` is unset, and stdout is not a tty.
The tty check keeps the user's interactive top-level agent unchanged; a piped
subagent gets a read-only output watch. Watched children cannot nest watches.
Fleet login shells receive `FLEET_SESSION` (session ID), `FLEET_TERMINAL` (human
terminal name, preserved for existing scripts), and `FLEET_TERMINAL_ID` (numeric,
daemon-local terminal ID). The wrapper associates watches using `FLEET_TERMINAL_ID`;
both it and `FLEET_SESSION` must be valid to register. Shims remain transparent
under older daemons that only supply the terminal name.

Daemon connection failure silently runs the command with inherited stdio.
Set `FLEET_DEBUG=1` for a one-line reason for any passthrough decision, including
missing or invalid environment variables. Output to the invoking agent remains raw
bytes; only the daemon's display copy is decoded as lossy UTF-8. Fleet does not
allocate a PTY for the child. Closing a watch never kills the child.
