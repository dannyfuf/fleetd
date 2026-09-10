// Fleet quality pass — fix workflow. Temporary; removed when the pass lands.
// Invoke with args: { wave: "w1" | "w2" | "w3" }

export const meta = {
  name: 'fleet-quality-fix',
  description: 'Implement and test one wave of verified quality-pass findings, one agent per file-disjoint batch',
  phases: [
    { title: 'Fix', detail: 'one agent per batch: implement, add regression tests, verify' },
  ],
}

const OWNERSHIP = {
  'w1-agents': [
    'crates/fleet-daemon/src/services/agents/**',
    'crates/fleet-core/src/agents/**',
    'crates/fleet-core/tests/compatibility.rs',
    'crates/fleet-proto/tests/compatibility.rs',
  ],
  'w1-server': [
    'crates/fleet-daemon/src/server/**',
    'crates/fleet-daemon/tests/server_events.rs',
    'crates/fleet-daemon/tests/remote_lifecycle.rs',
    'crates/fleet-client/**',
    'crates/fleet-app/src/shell/daemon.rs',
    'docs/APP-CONTRACTS.md',
  ],
  'w1-machines': [
    'crates/fleet-daemon/src/machines/**',
    'crates/fleet-daemon/src/testing/machines.rs',
    'crates/fleet-daemon/src/services/bootstrap.rs',
    'crates/fleet-daemon/src/services/mod.rs',
    'crates/fleet-daemon/src/services/composition.rs',
    'crates/fleet-daemon/src/main.rs',
    'crates/fleet-daemon/tests/remote_link.rs',
    'crates/fleet-daemon/tests/docs_contract.rs',
    'crates/fleet-daemon/tests/server_process.rs',
    'docs/REMOTE-MACHINES.md',
    'docs/DEVELOPMENT.md',
  ],
  'w1-worktrees': [
    'crates/fleet-daemon/src/services/worktrees.rs',
    'crates/fleet-daemon/src/services/worktrees/**',
    'crates/fleet-daemon/src/services/boards/worktree.rs',
    'crates/fleet-daemon/src/services/boards/tests.rs',
    'crates/fleet-daemon/tests/worktrees_lifecycle.rs',
    'crates/fleet-daemon/tests/reset_state.rs',
    'crates/fleet-daemon/tests/boards_service.rs',
  ],
  'w1-router-term': [
    'crates/fleet-daemon/src/services/router/**',
    'crates/fleet-daemon/src/services/sessions/**',
    'crates/fleet-daemon/src/services/watch_discovery.rs',
    'crates/fleet-daemon/src/services/watches.rs',
    'crates/fleet-daemon/tests/router_core.rs',
    'crates/fleet-term/**',
  ],
  'w1-jobs': [
    'crates/fleet-daemon/src/jobs/**',
    'crates/fleet-daemon/src/services/pool.rs',
    'crates/fleet-daemon/src/services/dispatch.rs',
    'crates/fleet-daemon/src/services/hosts.rs',
    'crates/fleet-daemon/src/services/update.rs',
    'crates/fleet-daemon/src/stores/config.rs',
    'crates/fleet-daemon/src/adapters/files.rs',
    'crates/fleet-daemon/src/testing/fakes.rs',
    'crates/fleet-daemon/src/testing/files.rs',
    'crates/fleet-daemon/tests/pool_lifecycle.rs',
    'crates/fleet-daemon/tests/sessions_lifecycle.rs',
  ],

  'w2-git': ['crates/fleet-git/**', 'crates/fleet-lazygit/src/bridge.rs', 'crates/fleet-lazygit/src/bridge/**', 'crates/fleet-lazygit/src/state/**'],
  'w2-lazygit': ['crates/fleet-lazygit/src/diff_view.rs', 'crates/fleet-lazygit/src/views/**', 'crates/fleet-lazygit/src/root.rs', 'crates/fleet-lazygit/src/root/**'],
  'w2-app-shell': [
    'crates/fleet-app/src/shell/root/**',
    'crates/fleet-app/src/terminal.rs',
    'crates/fleet-app/src/terminal/**',
    'crates/fleet-app/src/bridge.rs',
    'crates/fleet-app/src/bridge/**',
    'crates/fleet-app/src/dialogs/create_worktree.rs',
    'crates/fleet-app/src/async_util.rs (new)',
    'crates/fleet-app/src/lib.rs (module declaration line only)',
    'crates/fleet-app/src/keymap.rs',
    'docs/KEYMAP.md',
  ],
  'w2-agent-thread': [
    'crates/fleet-app/src/screens/agent_thread/**',
    'crates/fleet-app/src/screens/workspace.rs',
    'crates/fleet-app/src/screens/workspace/**',
    'crates/fleet-app/src/screens/agent_popup.rs',
    'crates/fleet-app/src/actions.rs',
    'crates/fleet-ui-kit/src/components/agent/decision_card.rs',
    'docs/NATIVE-AGENTS.md',
  ],
  'w2-board': [
    'crates/fleet-app/src/state/board.rs',
    'crates/fleet-app/src/screens/board.rs',
    'crates/fleet-app/src/screens/board/**',
    'crates/fleet-app/src/views/board_screen.rs',
    'crates/fleet-app/src/views/board_screen/**',
    'crates/fleet-ui-kit/src/components/kanban_column.rs',
    'crates/fleet-ui-kit/src/components/card_tile.rs',
    'crates/fleet-ui-kit/examples/gallery_board.rs',
  ],
  'w2-hub-misc': [
    'crates/fleet-app/src/dialogs/filter.rs',
    'crates/fleet-app/src/screens/hub.rs',
    'crates/fleet-app/src/screens/hub/**',
    'crates/fleet-app/tests/**',
    'crates/fleet-core/tests/**',
    'docs/README.md',
    'docs/decisions/**',
  ],

  'w3-markdown': [
    'crates/fleet-ui-kit/src/components/markdown.rs',
    'crates/fleet-ui-kit/src/components/markdown/**',
    'crates/fleet-ui-kit/src/components/markdown_text.rs',
    'crates/fleet-ui-kit/src/components/agent/transcript_list.rs',
    'crates/fleet-app/src/views/board_card_detail.rs',
  ],
  'w3-kit-components': [
    'crates/fleet-ui-kit/src/components/terminal_tab_strip.rs',
    'crates/fleet-ui-kit/src/components/key_value_list.rs',
    'crates/fleet-ui-kit/src/components/control.rs',
    'crates/fleet-ui-kit/src/components/sheet.rs',
    'crates/fleet-ui-kit/examples/gallery_terminal.rs',
    'crates/fleet-ui-kit/examples/gallery_input.rs',
    'crates/fleet-ui-kit/examples/gallery_data.rs',
    'crates/fleet-ui-kit/examples/kit_gallery.rs',
    'crates/fleet-app/src/views/detail/**',
  ],
  'w3-kit-styling': [
    'crates/fleet-ui-kit/src/theme/**',
    'crates/fleet-ui-kit/src/components/segmented_tabs.rs',
    'crates/fleet-app/src/views/watch_pane.rs',
    'crates/fleet-app/src/views/watch_pane/**',
    'crates/fleet-app/src/dialogs/mod.rs',
    'crates/fleet-app/src/dialogs/confirm.rs',
    'crates/fleet-app/src/dialogs/help.rs',
    'crates/fleet-app/src/dialogs/settings.rs',
    'docs/DESIGN-SYSTEM.md',
  ],
  'w3-palette': [
    'crates/fleet-app/src/dialogs/palette.rs',
    'crates/fleet-app/src/dialogs/palette/**',
    'crates/fleet-app/src/dialogs/host.rs',
    'crates/fleet-app/src/dialogs/host/**',
    'crates/fleet-app/src/dialogs/card_picker/**',
  ],
}

const WAVES = {
  w1: ['w1-agents', 'w1-server', 'w1-machines', 'w1-worktrees', 'w1-router-term', 'w1-jobs'],
  w2: ['w2-git', 'w2-lazygit', 'w2-app-shell', 'w2-agent-thread', 'w2-board', 'w2-hub-misc'],
  w3: ['w3-markdown', 'w3-kit-components', 'w3-kit-styling', 'w3-palette'],
}

const REPORT_SCHEMA = {
  type: 'object',
  properties: {
    outcomes: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          outcome: { type: 'string', enum: ['fixed', 'skipped', 'no_change_needed'] },
          what_changed: { type: 'string', description: 'the actual change, with file:line' },
          tests_added: { type: 'string', description: 'test names and their file, or why none' },
          verified: { type: 'string', description: 'the exact command you ran and its result' },
          notes: { type: 'string' },
        },
        required: ['id', 'outcome', 'what_changed', 'tests_added', 'verified'],
      },
    },
    files_touched: { type: 'array', items: { type: 'string' } },
    followups: { type: 'string', description: 'anything you found but deliberately did not do, and why' },
  },
  required: ['outcomes', 'files_touched', 'followups'],
}

const wave = (args && args.wave) || 'w1'
const batches = WAVES[wave]
if (!batches) throw new Error(`unknown wave ${wave}`)

const SCRATCH = '/tmp/claude-1000/-home-df--fleet-worktrees-dannyfuf-fleetd-chore-cleaning/3a50ef35-e725-4f61-bd9d-09b923302fce/scratchpad/batches'

phase('Fix')

const results = await parallel(
  batches.map((b) => () =>
    agent(
      `You are fixing verified quality findings in the fleetd Cargo workspace at the repository root.

Read your batch of findings first: \`${SCRATCH}/${b}.json\`. Each entry has the defect, the
evidence, a verifier's independent assessment (\`verify_reason\`), a proposed fix, a possibly
better \`corrected_fix\` (prefer it when present — it comes from the verifier who read the code),
the \`blast_radius\`, and a \`test_plan\`.

## Files you own

You may create and edit ONLY these paths:
${OWNERSHIP[b].map((p) => `  - ${p}`).join('\n')}

**Other agents are editing other parts of this same working tree right now.** Therefore:
- Never edit a file outside your list. If a fix genuinely requires one, do the part you can,
  and describe the remaining edit precisely in \`followups\`. Do not "just quickly" touch it.
- \`cargo check\` / \`cargo test\` may report errors in files you do not own. Those belong to a
  concurrent agent. Ignore them completely — never fix them, never revert them.
- Cargo may block on the target-directory lock. Wait; do not kill it or set a different
  CARGO_TARGET_DIR.
- Never run \`git add\`, \`git commit\`, \`git checkout\`, \`git stash\`, or \`git restore\`.

## How to work

1. Load the project skill that governs the area you are touching (\`.claude/skills/\`) and read
   its \`references/checklist.md\`. \`CLAUDE.md\`'s non-negotiables are binding.
2. For each finding, in severity order: read the code around it and its existing tests. Confirm
   the defect for yourself before changing anything. If you conclude the finding is wrong, mark
   it \`no_change_needed\` and say exactly why — that is a valid, useful outcome.
3. **Write the regression test first.** Run it and watch it FAIL for the stated reason. Then make
   the fix and watch it pass. A test you never saw fail is not a regression test. If you cannot
   make it fail first, say so in \`verified\` — do not claim you did.
4. Follow the surrounding code's idiom exactly: existing helpers, existing fake/adapter seams,
   existing test-module layout. Tests belong where this repo puts them (inline \`mod tests\`,
   \`src/<thing>/tests.rs\`, or \`crates/<crate>/tests/\`) — match the neighbours.
5. Keep each fix minimal and surgical. No drive-by refactors, no renames, no new features, no
   reformatting of untouched code. Do not widen a public API unless the fix needs it.
6. If \`docs/\` states behaviour your fix changes, update the doc in the same pass — docs/ is
   authoritative and code and doc move together.
7. Verify: \`cargo test -p <crate> <filter>\` for the tests you added, plus
   \`cargo check -p <crate> --all-targets\`. Report the exact command and its real result. Never
   report a test as passing that you did not run.

Be honest in the report. A skipped finding with a clear reason is worth more than a plausible
lie. Your batch is **${b}**.`,
      { label: `fix:${b}`, phase: 'Fix', schema: REPORT_SCHEMA },
    ).then((r) => ({ batch: b, ...(r || { outcomes: [], files_touched: [], followups: 'agent produced no result' } ) })),
  ),
)

const ok = results.filter(Boolean)
log(`${wave}: ${ok.reduce((n, r) => n + r.outcomes.filter((o) => o.outcome === 'fixed').length, 0)} findings fixed`)
return ok
