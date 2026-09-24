#!/usr/bin/env bash
#
# Review boards and schedules, end to end, against a private daemon and scripted agents.
#
# What it proves, in one command and with no network:
#
#   1. `fleet board --reviews card new … --pr owner/name#1` creates a pull-request card on the
#      context's Reviews board. With nothing blocking it, the card leaves Pending review at once
#      for Reviewing. The daemon creates the card's own worktree from `refs/pull/1/head`, runs the
#      review there and routes the card to Reviewed on success. A second `card new --pr` for the
#      same pull request prints `Existing <KEY>` and adds no card.
#   2. A schedule on that board, run with `fleet schedule run --wait`, launches its agent headless.
#      The agent records a pull request through the footer's `card new --pr` contract, and the
#      run ends `succeeded` with the agent's `SUMMARY:` line as its summary.
#
# Hermetic by construction, like scripts/board-workflow-smoke.sh, whose skeleton this follows:
#
#   * `FLEET_HOME` is a fresh `mktemp -d`. The daemon this starts shares no state, socket or
#     log with the user's daemon. It is stopped and the directory removed on every exit path.
#   * `PATH` gets a private `bin/` first. It holds:
#     - `codex`, the `fleet-harness agent` launcher that speaks the Codex wire protocol from a
#       transcript. The Reviewing column runs every card on Codex, so each review is scripted.
#     - `claude`, which does two jobs. Under a schedule run (`FLEET_SCHEDULE` set) it plays the
#       headless `claude -p`: it records a pull request with `fleet board … card new --pr` and
#       prints a Claude-shaped `result` line. Anywhere else it is the harness launcher, like
#       `codex`.
#     - `gh`, which answers `gh pr view <n>` for the fixture repository. CI has no `gh` and no
#       network, and the daemon asks `gh` for a pull request's head branch before it creates the
#       worktree.
#     - `fleet`, which pins this run's daemon binary. The schedule runner supplies its private
#       `FLEET_HOME`, so a child never reaches the user's daemon.
#     `config.json` names both agent shims under `agentBinaries`, so the daemon never consults
#     `PATH` for them.
#   * The "GitHub" repository is a local bare origin built by `git init --bare` and cloned over
#     a `file://` path. The pull requests are `refs/pull/<n>/head` refs written into it with
#     `git update-ref`, as GitHub does.
#   * Every `FLEET_*` variable an enclosing Fleet session may have exported is cleared first.
#
# Usage: scripts/reviews-smoke.sh   (or `make smoke-reviews`, which builds first)
# Overrides: FLEET, FLEETD, HARNESS (binary paths), SMOKE_TIMEOUT (seconds for each wait).

set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
FLEET=${FLEET:-"$repository_root/target/debug/fleet"}
FLEETD=${FLEETD:-"$repository_root/target/debug/fleetd"}
HARNESS=${HARNESS:-"$repository_root/target/debug/fleet-harness"}
SMOKE_TIMEOUT=${SMOKE_TIMEOUT:-300}

# The fixture repository. `fleet create` files an unregistered repository under the context
# that owns its owner, and creates that context, named after the owner, when none does.
readonly OWNER=acme
readonly NAME=api
readonly CONTEXT=$OWNER

# The Reviews preset's columns (contracts C2, docs/BOARD.md).
readonly PENDING_COLUMN=pending
readonly REVIEWING_COLUMN=reviewing
readonly REVIEWED_COLUMN=reviewed

# The line the scripted schedule agent ends with, and the summary the run must record for it:
# the text after `SUMMARY:`, trimmed.
readonly SCHEDULE_SUMMARY_LINE='SUMMARY: 1 created, 0 existing, 0 reopened'
readonly SCHEDULE_SUMMARY='1 created, 0 existing, 0 reopened'

workspace=
daemon_log_copy=

say() { printf '== %s\n' "$*"; }
fail() { printf 'reviews-smoke: %s\n' "$*" >&2; exit 1; }

# Stops the private daemon, keeps its log when something went wrong, and removes the home.
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
        printf 'reviews-smoke: the daemon log is at %s\n' "$daemon_log_copy" >&2
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
    pkill -f "$workspace/transcripts/review.json" 2>/dev/null || true
}

preserve_daemon_log() {
    local log="$workspace/home/logs/fleetd.log"
    [ -r "$log" ] || return 0
    daemon_log_copy=$(mktemp "${TMPDIR:-/tmp}/fleet-smoke-reviews-fleetd.XXXXXXXX.log")
    cp "$log" "$daemon_log_copy" 2>/dev/null || daemon_log_copy=
}

for binary in "$FLEET" "$FLEETD" "$HARNESS"; do
    [ -x "$binary" ] || fail "$binary is missing or not executable; run \`make build\` first"
done
command -v python3 >/dev/null 2>&1 || fail "python3 is required to read the JSON envelopes"

unset FLEET_SESSION FLEET_DELEGATION FLEET_DELEGATION_TOKEN FLEET_CARD FLEET_BOARD \
    FLEET_TERMINAL FLEET_TERMINAL_ID FLEET_WATCH FLEET_BIN FLEET_SCHEDULE

workspace=$(mktemp -d "${TMPDIR:-/tmp}/fleet-smoke-reviews.XXXXXXXX")
trap cleanup EXIT
mkdir -p "$workspace/home" "$workspace/bin" "$workspace/transcripts" "$workspace/git-home"
export FLEET_HOME="$workspace/home"
export FLEET_DAEMON="$FLEETD"
export PATH="$workspace/bin:$PATH"

reviews=(--context "$CONTEXT" --reviews)
fleet_reviews() { "$FLEET" board "${reviews[@]}" "$@"; }
fleet_schedule() { "$FLEET" schedule "${reviews[@]}" "$@"; }

# Reads a JSON document on stdin and runs the Python statements in $1 against it as `d`.
json() {
    python3 -c "import json, sys
d = json.load(sys.stdin)
$1"
}

# Runs git with every source of ambient configuration shut off, as the harness fixtures do.
smoke_git() {
    env HOME="$workspace/git-home" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
        git -c user.name='Fleet Smoke' -c user.email='smoke@fleet.test' \
        -c commit.gpgsign=false -c init.defaultBranch=main \
        -c protocol.file.allow=always "$@"
}

say "building a local \"GitHub\" for $OWNER/$NAME with pull requests #1 and #2"
origin="$workspace/origins/$OWNER/$NAME.git"
seed="$workspace/seed/$OWNER/$NAME"
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
# The pull requests' head: one commit on top of main, pushed as a branch and then published
# under GitHub's read-only pull refs, which is all the daemon fetches.
smoke_git -C "$seed" checkout --quiet -b review-fixture
printf 'Reviewed by the smoke.\n' >>"$seed/README.md"
smoke_git -C "$seed" commit --quiet -am 'change the readme'
smoke_git -C "$seed" push --quiet origin review-fixture
for number in 1 2; do
    smoke_git --git-dir "$origin" update-ref "refs/pull/$number/head" refs/heads/review-fixture
done

say "installing the scripted agents, gh and the private configuration"
review_transcript="$workspace/transcripts/review.json"
# One turn: say something, report a fixed review, end the turn. The column's instructions ask
# for gh and a real review; the scripted agent ignores them, which is the point.
cat >"$review_transcript" <<'TRANSCRIPT'
{"version":1,"steps":[
  {"type":"text","text":"Reviewing the pull request, then reporting to the board.","pace_ms":0},
  {"type":"shell","command":"report=$(mktemp) && printf '%s\\n' 'Verdict: approve' '' 'Summary: the fixture changes one line of the README.' '' '1. README.md:2 nit: end the sentence without a trailing space.' > \"$report\" && fleet subagent complete --result-file \"$report\"; status=$?; rm -f \"$report\"; exit \"$status\""},
  {"type":"end_turn","status":"completed"}
]}
TRANSCRIPT

# The private `fleet`: the schedule runner supplies this run's FLEET_HOME, while the wrapper pins
# the private daemon binary rather than allowing the CLI to start the sibling build implicitly.
cat >"$workspace/bin/fleet" <<SHIM
#!/bin/sh
FLEET_DAEMON='$FLEETD' exec '$FLEET' "\$@"
SHIM
chmod +x "$workspace/bin/fleet"

# The Codex stand-in for card runs, in the shape of `crate::agent::launcher`.
cat >"$workspace/bin/codex" <<SHIM
#!/bin/sh
# Generated by scripts/reviews-smoke.sh: a scripted stand-in, not the vendor CLI.
for argument in "\$@"; do
  if [ "\$argument" = "--version" ]; then
    exec '$HARNESS' agent --provider codex --transcript '$review_transcript' --version
  fi
done
exec '$HARNESS' agent --provider codex --transcript '$review_transcript'
SHIM
chmod +x "$workspace/bin/codex"

# The Claude stand-in. A schedule run is headless (`claude -p … <prompt>`) and the runner sets
# FLEET_SCHEDULE and FLEET_BOARD, so that is what selects the schedule behaviour. The prompt
# is ignored: the agent does what the footer asks, once, with a fixed pull request.
cat >"$workspace/bin/claude" <<SHIM
#!/bin/sh
# Generated by scripts/reviews-smoke.sh: a scripted stand-in, not the vendor CLI.
if [ -n "\${FLEET_SCHEDULE:-}" ]; then
  if [ -z "\${FLEET_BOARD:-}" ]; then
    echo 'reviews-smoke claude: FLEET_BOARD is not set in a schedule run' >&2
    exit 1
  fi
  '$workspace/bin/fleet' board --board "\$FLEET_BOARD" card new "Scheduled PR" \\
    --pr '$OWNER/$NAME#2' --label github || exit 1
  printf '%s\n' '{"type":"result","result":"$SCHEDULE_SUMMARY_LINE","is_error":false,"total_cost_usd":0}'
  exit 0
fi
for argument in "\$@"; do
  if [ "\$argument" = "--version" ]; then
    exec '$HARNESS' agent --provider claude --transcript '$review_transcript' --version
  fi
done
exec '$HARNESS' agent --provider claude --transcript '$review_transcript'
SHIM
chmod +x "$workspace/bin/claude"

# `gh pr view <n> --repo acme/api --json …`: the head branch of a same-repository pull request,
# which names the worktree. Anything else is a question this fixture cannot answer; `pr list`
# (the background pull-request inspection) gets an empty list.
cat >"$workspace/bin/gh" <<SHIM
#!/bin/sh
# Generated by scripts/reviews-smoke.sh: a fixture, not the GitHub CLI.
if [ "\$1" = pr ] && [ "\$2" = view ]; then
  number=\$3
  case "\$number" in
    1|2) ;;
    *) echo "no pull request \$number in the fixture" >&2; exit 1 ;;
  esac
  printf '{"number":%s,"title":"Fixture PR %s","url":"https://github.com/$OWNER/$NAME/pull/%s",' "\$number" "\$number" "\$number"
  printf '"author":{"login":"octo"},"headRefName":"fixture-pr-%s","baseRefName":"main",' "\$number"
  printf '"isDraft":false,"isCrossRepository":false,"headRepository":{"name":"$NAME","nameWithOwner":"$OWNER/$NAME"},'
  printf '"headRepositoryOwner":{"login":"$OWNER"},"reviewDecision":null,"statusCheckRollup":[],'
  printf '"additions":1,"deletions":0,"labels":[],"updatedAt":"2026-09-22T00:00:00Z"}\n'
  exit 0
fi
if [ "\$1" = pr ] && [ "\$2" = list ]; then
  echo '[]'
  exit 0
fi
echo "the reviews smoke's gh does not answer: gh \$*" >&2
exit 1
SHIM
chmod +x "$workspace/bin/gh"

cat >"$FLEET_HOME/config.json" <<CONFIG
{
  "hotPoolSize": 0,
  "hotRefreshIntervalMs": 0,
  "agentBinaries": {"codex": "$workspace/bin/codex", "claude": "$workspace/bin/claude"}
}
CONFIG

say "registering $OWNER/$NAME (this starts the private daemon)"
# A card's PR worktree needs a Fleet repository. `fleet create` is the one verb that clones an
# unregistered repository; the worktree it also makes is incidental.
"$FLEET" create "$OWNER/$NAME" base --url "$origin" >/dev/null

say "giving the Reviews board a column the scripted agent can run"
# The preset's Reviewing column asks for gh and a real review. The scripted agent ignores the
# instructions, and running the column on Codex makes every review a transcript.
fleet_reviews columns edit "$REVIEWING_COLUMN" --provider codex \
    --instructions 'Review {pr_url}. The smoke agent ignores this and reports a fixed review.' \
    >/dev/null
fleet_reviews columns

# One card's JSON envelope.
card_json() { fleet_reviews --json card show "$1"; }

# Waits until card $1 stands in column $2, dumping the board and its runs when time runs out.
wait_for_column() {
    local key=$1 column=$2 deadline=$((SECONDS + SMOKE_TIMEOUT)) current=
    while :; do
        current=$(card_json "$key" | json 'print(d["card"]["statusId"])')
        [ "$current" = "$column" ] && return 0
        if [ "$SECONDS" -ge "$deadline" ]; then
            say "the board when time ran out:"
            fleet_reviews show || true
            say "runs of $key:"
            fleet_reviews card runs "$key" || true
            fail "$key did not reach $column within ${SMOKE_TIMEOUT}s; it stands in $current"
        fi
        sleep 2
    done
}

say "recording pull request $OWNER/$NAME#1"
created=$(fleet_reviews card new "Fixture PR" --pr "$OWNER/$NAME#1")
printf '%s\n' "$created"
first_line=$(printf '%s\n' "$created" | head -n 1)
outcome=${first_line%% *}
key=${first_line#* }
[ "$outcome" = Created ] || fail "expected \`Created <KEY>\` first, read \`$first_line\`"

say "waiting for $key to go $PENDING_COLUMN -> $REVIEWING_COLUMN -> $REVIEWED_COLUMN (up to ${SMOKE_TIMEOUT}s)"
wait_for_column "$key" "$REVIEWED_COLUMN"

say "asserting $key's review succeeded in its own linked worktree"
card_json "$key" | json '
card = d["card"]
worktree = card.get("worktreeId")
if not worktree:
    sys.exit("the card has no linked worktree")
runs = [run for run in card.get("runs", []) if run.get("statusId") == "'"$REVIEWING_COLUMN"'"]
if not runs:
    sys.exit("the card has no run of the reviewing column")
latest = runs[-1]
if latest.get("outcome") != "succeeded":
    sys.exit("the latest review run ended %r, not succeeded" % latest.get("outcome"))
if latest.get("worktreeId") != worktree:
    sys.exit("the review ran in %r, not the card worktree %r" % (latest.get("worktreeId"), worktree))
pull = card.get("pullRequest") or {}
if pull.get("number") != 1:
    sys.exit("the card carries pull request %r, not #1" % pull)
print("linked worktree %s; review run %s succeeded" % (worktree, latest.get("id")))
'

say "asserting a second \`card new --pr\` finds the card"
again=$(fleet_reviews card new "Fixture PR again" --pr "https://github.com/$OWNER/$NAME/pull/1/files")
again_line=$(printf '%s\n' "$again" | head -n 1)
[ "$again_line" = "Existing $key" ] ||
    fail "expected \`Existing $key\` first, read \`$again_line\`"

say "creating a schedule on the Reviews board"
# Disabled, so the daemon's own tick never fires it: `--every 5` with no run yet is due at once,
# and a tick run racing the one below would record that one `skipped`. `run` fires a disabled
# schedule all the same.
schedule_id=$(fleet_schedule --json new --name 'Smoke intake' \
    --prompt 'Record the fixture pull request on {board}.' --every 5 --timeout 5 --disabled |
    json 'print(d["schedule"]["id"])')
say "schedule $schedule_id"

say "running $schedule_id and waiting for it"
# `run --wait` returns when its run ends, or at once with `Skipped` if another run were live;
# the script still waits for every recorded run to end before it reads them.
run_status=0
fleet_schedule run "$schedule_id" --wait || run_status=$?
deadline=$((SECONDS + SMOKE_TIMEOUT))
until fleet_schedule --json show "$schedule_id" |
    json 'sys.exit(0 if all(run.get("outcome") for run in d["schedule"].get("runs", [])) else 1)'; do
    if [ "$SECONDS" -ge "$deadline" ]; then
        fleet_schedule runs "$schedule_id" || true
        fail "a run of $schedule_id was still going after ${SMOKE_TIMEOUT}s"
    fi
    sleep 1
done
fleet_schedule runs "$schedule_id" || true

say "asserting the run succeeded with the agent's summary"
fleet_schedule --json show "$schedule_id" | json '
runs = d["schedule"].get("runs", [])
if not runs:
    sys.exit("the schedule recorded no run")
if len(runs) != 1:
    sys.exit("expected exactly one run, found %r" % [run.get("outcome") for run in runs])
run = runs[0]
if run.get("outcome") != "succeeded":
    sys.exit("the run ended %s: %s" % (run.get("outcome"), run.get("summary")))
summary = run.get("summary")
if summary != "'"$SCHEDULE_SUMMARY"'":
    sys.exit("the run summary is %r" % summary)
if not run.get("jobId") or not run.get("logPath"):
    sys.exit("the run has no job or no log: %r" % run)
print("the run succeeded as job %s: %s" % (run.get("jobId"), summary))
'
[ "$run_status" -eq 0 ] || fail "\`fleet schedule run --wait\` exited $run_status"

say "asserting the scheduled agent recorded pull request #2"
fleet_reviews --json show | json '
cards = [card for card in d["cards"] if (card.get("pullRequest") or {}).get("number") == 2]
if len(cards) != 1:
    sys.exit("expected one card for pull request #2, found %d" % len(cards))
card = cards[0]
if card.get("title") != "Scheduled PR":
    sys.exit("the card for #2 is titled %r" % card.get("title"))
if "github" not in card.get("labels", []):
    sys.exit("the card for #2 carries labels %r, not github" % card.get("labels"))
print("card %s records pull request #2" % card.get("id"))
'

say "reviews smoke passed: $key reviewed in its own worktree, and $schedule_id recorded #2"
