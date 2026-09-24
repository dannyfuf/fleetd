#!/usr/bin/env bash
#
# Board workflows, end to end, against a private daemon and a scripted agent.
#
# What it proves, in one command and with no network: a worktree board given the workflow
# preset runs a four-card diamond — A blocks B and C, B and C both block D — from Ready to
# Done by itself, one live run at a time, with every run started, briefed, reported and
# routed by the daemon rather than by this script. The head of the chain is released the way
# every other card is: moved into Ready with nothing blocking it, it starts at once.
#
# Hermetic by construction, because a smoke target that touches the developer's world is a
# target nobody runs twice:
#
#   * `FLEET_HOME` is a fresh `mktemp -d`, so the daemon this starts shares no state, socket
#     or log with the one the user is running. It is stopped and the directory removed on
#     every exit path, successful or not.
#   * `PATH` gets a private `bin/` first, holding a `codex` in the shape of
#     `crates/fleet-harness/src/agent/launcher.rs` — it answers the `--version` probe
#     `fleet-daemon`'s harness check makes and otherwise execs `fleet-harness agent`, which
#     speaks the real Codex wire protocol from the transcript written beside it. `config.json`
#     names the same shim under `agentBinaries` so the daemon never consults `PATH` at all.
#   * The repository is a local bare origin built by `git init --bare`, cloned over a `file://`
#     path. Nothing resolves a hostname.
#   * Every `FLEET_*` variable an enclosing Fleet session or delegation may have exported is
#     cleared before the first command, so running this from inside a Fleet terminal, an agent
#     thread or a card run cannot change what it does.
#
# The transcript is a `docs/TESTING-HARNESS.md` section 5 document: a line of text, one
# `shell` step that writes a report and hands it to `fleet subagent complete --result-file`,
# and `end_turn`. That is the whole child contract a card run depends on, so a break anywhere
# between the column's action and the delivered report fails this script.
#
# Usage: scripts/board-workflow-smoke.sh   (or `make smoke-workflow`, which builds first)
# Overrides: FLEET, FLEETD, HARNESS (binary paths), SMOKE_TIMEOUT (seconds for the chain).

set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
FLEET=${FLEET:-"$repository_root/target/debug/fleet"}
FLEETD=${FLEETD:-"$repository_root/target/debug/fleetd"}
HARNESS=${HARNESS:-"$repository_root/target/debug/fleet-harness"}
# The chain is eight runs deep — four cards through two action columns — and they are
# serialised by the board's one-live-run default, so this is a ceiling for all of them.
SMOKE_TIMEOUT=${SMOKE_TIMEOUT:-300}

# The workflow preset's columns (docs/BOARD.md section 11.6).
readonly TODO_COLUMN=todo
readonly READY_COLUMN=ready
readonly ACTION_COLUMN=in-progress
readonly DONE_COLUMN=done

workspace=
daemon_log_copy=

say() { printf '== %s\n' "$*"; }
fail() { printf 'board-workflow-smoke: %s\n' "$*" >&2; exit 1; }

# Stops the private daemon, keeps its log when something went wrong, and removes the home.
#
# The log lives inside the home this deletes, so a failure copies it out first and prints
# where it went: a path that no longer exists is not a diagnostic.
cleanup() {
    local status=$?
    trap - EXIT
    if [ -n "$workspace" ]; then
        stop_daemon
        if [ "$status" -ne 0 ]; then
            preserve_daemon_log
        fi
        rm -rf "$workspace"
    fi
    if [ "$status" -ne 0 ] && [ -n "$daemon_log_copy" ]; then
        printf 'board-workflow-smoke: the daemon log is at %s\n' "$daemon_log_copy" >&2
    fi
    exit "$status"
}

stop_daemon() {
    local pid_file="$workspace/home/fleetd.pid" pid=
    if [ -r "$pid_file" ]; then
        pid=$(head -n 1 "$pid_file" 2>/dev/null || true)
    fi
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        local waited=0
        while [ "$waited" -lt 100 ] && kill -0 "$pid" 2>/dev/null; do
            sleep 0.1
            waited=$((waited + 1))
        done
        kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    fi
    # A scripted agent outlives its daemon only when the daemon was killed mid-run; it is
    # named by this run's own transcript path, so nothing outside this workspace can match.
    pkill -f "$workspace/transcripts/child.json" 2>/dev/null || true
}

preserve_daemon_log() {
    local log="$workspace/home/logs/fleetd.log"
    [ -r "$log" ] || return 0
    daemon_log_copy=$(mktemp "${TMPDIR:-/tmp}/fleet-smoke-workflow-fleetd.XXXXXXXX.log")
    cp "$log" "$daemon_log_copy" 2>/dev/null || daemon_log_copy=
}

for binary in "$FLEET" "$FLEETD" "$HARNESS"; do
    [ -x "$binary" ] || fail "$binary is missing or not executable; run \`make build\` first"
done

# Nothing of the caller's Fleet identity may leak in: `FLEET_CARD` plus `FLEET_DELEGATION`
# would make `card move` refuse as a self-move, and `FLEET_SESSION` would resolve a bare
# `--worktree` against the caller's session rather than this run's.
unset FLEET_SESSION FLEET_DELEGATION FLEET_DELEGATION_TOKEN FLEET_CARD FLEET_BOARD \
    FLEET_TERMINAL FLEET_TERMINAL_ID FLEET_WATCH FLEET_BIN

workspace=$(mktemp -d "${TMPDIR:-/tmp}/fleet-smoke-workflow.XXXXXXXX")
trap cleanup EXIT
mkdir -p "$workspace/home" "$workspace/bin" "$workspace/transcripts" "$workspace/git-home"
export FLEET_HOME="$workspace/home"
export FLEET_DAEMON="$FLEETD"
export PATH="$workspace/bin:$PATH"

board=(--worktree=acme/api#agent)
fleet_board() { "$FLEET" board "${board[@]}" "$@"; }

# Runs git with every source of ambient configuration shut off, as the harness fixtures do:
# no user `~/.gitconfig`, no signing key, no credential helper, no terminal prompt.
smoke_git() {
    env HOME="$workspace/git-home" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
        git -c user.name='Fleet Smoke' -c user.email='smoke@fleet.test' \
        -c commit.gpgsign=false -c init.defaultBranch=main \
        -c protocol.file.allow=always "$@"
}

say "building a local origin for acme/api"
origin="$workspace/origins/acme/api.git"
seed="$workspace/seed/acme/api"
mkdir -p "$(dirname "$origin")" "$seed"
smoke_git init --quiet --bare "$origin"
smoke_git init --quiet "$seed"
printf '# api\n' >"$seed/README.md"
smoke_git -C "$seed" add --all
smoke_git -C "$seed" commit --quiet -m 'initial commit'
smoke_git -C "$seed" branch -M main
smoke_git -C "$seed" remote add origin "$origin"
smoke_git -C "$seed" push --quiet -u origin main
smoke_git --git-dir "$origin" symbolic-ref HEAD refs/heads/main

say "installing the scripted Codex and the private configuration"
transcript="$workspace/transcripts/child.json"
# One turn: say something, run the shell step that reports, end the turn. `end_turn` is what
# tells Fleet the turn settled rather than the harness dying mid-turn.
cat >"$transcript" <<'TRANSCRIPT'
{"version":1,"steps":[
  {"type":"text","text":"Working the card, then reporting to the board.","pace_ms":0},
  {"type":"shell","command":"report=$(mktemp) && printf '%s\\n' 'Did what the card asked.' > \"$report\" && fleet subagent complete --result-file \"$report\"; status=$?; rm -f \"$report\"; exit \"$status\""},
  {"type":"end_turn","status":"completed"}
]}
TRANSCRIPT

# The shape of `crate::agent::launcher`: `fleet-daemon` probes the binary with `--version`
# before it spawns a session and appends the vendor's own launch flags afterwards, so the
# probe is answered here and everything else is ignored.
#
# Both vendors are shimmed because the preset needs both: `in-progress` runs the card's own
# provider (Codex, as `card new --provider` asks) while `in-review` is a skill action, and a
# skill always runs on Claude (`fleet_core::board::automation::resolve_prefs`). One transcript
# serves both — the player speaks whichever protocol its `--provider` names.
for provider in codex claude; do
    shim="$workspace/bin/$provider"
    cat >"$shim" <<SHIM
#!/bin/sh
# Generated by scripts/board-workflow-smoke.sh: a scripted stand-in, not the vendor CLI.
for argument in "\$@"; do
  if [ "\$argument" = "--version" ]; then
    exec '$HARNESS' agent --provider $provider --transcript '$transcript' --version
  fi
done
exec '$HARNESS' agent --provider $provider --transcript '$transcript'
SHIM
    chmod +x "$shim"
done

# A fallback only: the daemon puts the `fleet` beside its own `fleetd` on the child's PATH
# (`services::agents::delegation::run::resolve_fleet_program`). This one bakes the private
# home in, so the child's `fleet subagent complete` reaches this run's daemon either way.
cat >"$workspace/bin/fleet" <<SHIM
#!/bin/sh
FLEET_HOME='$workspace/home' FLEET_DAEMON='$FLEETD' exec '$FLEET' "\$@"
SHIM
chmod +x "$workspace/bin/fleet"

# `agentBinaries` is what the daemon execve's for a native thread, so naming the shim here
# makes the run independent of PATH. The prepared-copy pool is off: a pool refreshing in the
# background would put jobs and worktree directories into a run that never asked for them.
cat >"$FLEET_HOME/config.json" <<CONFIG
{
  "hotPoolSize": 0,
  "hotRefreshIntervalMs": 0,
  "agentBinaries": {"codex": "$workspace/bin/codex", "claude": "$workspace/bin/claude"}
}
CONFIG

say "creating the worktree (this starts the private daemon)"
"$FLEET" create acme/api agent --url "$origin" >/dev/null

say "applying the workflow preset"
fleet_board columns preset workflow >/dev/null

# The preset adds the columns a board is missing and never rewrites one it already has, so a
# board built from `default_statuses()` gains Ready and In review but keeps its own bare
# `in-progress` (docs/BOARD.md section 11.6). Giving that column its action is the edit every
# such board's owner has to make, so the smoke makes it rather than assuming it away.
fleet_board columns edit "$ACTION_COLUMN" \
    --on-enter prompt \
    --instructions 'Implement this card in the current worktree. Do not commit.' \
    --expect 'make lint and make test pass' \
    --on-success in-review >/dev/null

# Prints the display key of a card `card new` has just created: the report's first line is
# `<KEY>  <title>`.
new_card() {
    local title=$1
    shift
    fleet_board card new "$title" --status "$TODO_COLUMN" --provider codex "$@" |
        head -n 1 | awk '{print $1}'
}

say "creating the diamond in $TODO_COLUMN"
# Every link is written while the card is still in Todo. A card that reaches Ready before its
# blockers are recorded is a card the engine may release at once.
a=$(new_card 'Throttle the board to one run')
b=$(new_card 'Give the column an action' --blocked-by "$a")
c=$(new_card 'Review the throttle' --blocked-by "$a")
d=$(new_card 'Ship the workflow' --blocked-by "$b" --blocked-by "$c")
say "cards: $a blocks $b and $c; $b and $c block $d"

say "releasing the chain"
# The dependants go into Ready first. Their blockers still stand in Todo, so they stay there
# until `advance_when_unblocked` collects each one as its last blocker finishes. The head goes
# into Ready last: nothing blocks it, so entering Ready advances it to the action column at once
# (rule 0, docs/BOARD.md section 11.7). Moving it any earlier would start a run before the
# dependants were in place.
fleet_board card move "$b" "$READY_COLUMN" >/dev/null
fleet_board card move "$c" "$READY_COLUMN" >/dev/null
fleet_board card move "$d" "$READY_COLUMN" >/dev/null
fleet_board card move "$a" "$READY_COLUMN" >/dev/null

# A card's own column, read out of a `--json` envelope.
#
# Every `CardRun` carries a `statusId` of its own — the column that run belongs to — so the
# card's field is picked by the one key that immediately precedes it and precedes nothing
# else: its `description`, which these cards leave empty.
readonly CARD_COLUMN='"description":"","statusId":"'

card_column() {
    fleet_board --json card show "$1" |
        grep -o "$CARD_COLUMN[^\"]*\"" | head -n 1 | sed "s/.*\"statusId\":\"//; s/\"\$//"
}

say "waiting for the chain to reach $DONE_COLUMN (up to ${SMOKE_TIMEOUT}s)"
deadline=$((SECONDS + SMOKE_TIMEOUT))
while :; do
    settled=1
    for key in "$a" "$b" "$c" "$d"; do
        [ "$(card_column "$key")" = "$DONE_COLUMN" ] || settled=0
    done
    [ "$settled" -eq 1 ] && break
    if [ "$SECONDS" -ge "$deadline" ]; then
        say "the board when time ran out:"
        fleet_board show || true
        for key in "$a" "$b" "$c" "$d"; do
            say "runs of $key:"
            fleet_board card runs "$key" || true
        done
        fail "the chain did not reach $DONE_COLUMN within ${SMOKE_TIMEOUT}s"
    fi
    sleep 2
done

say "asserting \`card wait\` exits 0 on the last card"
# Exit 0 is the terminal answer an orchestrator scripts against; 2 would mean the newest run
# is still live, or that no run ever started.
if ! fleet_board card wait "$d" --timeout "$SMOKE_TIMEOUT" >/dev/null; then
    fail "card wait $d exited $? instead of 0"
fi

say "asserting every card sits in $DONE_COLUMN"
view=$(fleet_board --json show)
total=$(printf '%s' "$view" | grep -o "$CARD_COLUMN[^\"]*\"" | wc -l)
in_done=$(printf '%s' "$view" | grep -o "$CARD_COLUMN$DONE_COLUMN\"" | wc -l)
if [ "$total" -ne 4 ] || [ "$in_done" -ne 4 ]; then
    printf '%s\n' "$view"
    fail "expected four cards in $DONE_COLUMN, read $in_done of $total column references"
fi

say "board workflows smoke passed: $a, $b, $c and $d all reached $DONE_COLUMN"
