#!/bin/sh
# Runs every expected-to-fail scenario in this directory and inverts the verdict.
#
# The harness has no expected-to-fail marker of its own, and these files carry an extension a
# directory run does not collect (`scenario.rs::SCENARIO_EXTENSIONS`), so `fleet-harness run
# scenarios/` stays green and this script is the thing that watches them. Exit 0 means every one
# of them still fails for the TODO.md entry its header names; exit 1 means one has started
# passing, which is the moment to rename it to `.scenario`, move it up one directory and close
# the TODO entry in the same commit.
#
#   FLEET_HARNESS=target/debug/fleet-harness LANE=virtual scenarios/agents/expected-to-fail/run.sh
set -u
harness=${FLEET_HARNESS:-target/debug/fleet-harness}
lane=${LANE:-virtual}
here=$(dirname "$0")
status=0
for scenario in "$here"/*.expected-to-fail; do
    if "$harness" run "$scenario" --lane "$lane" >/dev/null 2>&1; then
        echo "PASSES NOW — flip it to a .scenario and close its TODO entry: $scenario"
        status=1
    else
        echo "still failing, as its header documents: $scenario"
    fi
done
exit $status
