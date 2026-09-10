// Fleet quality pass — run one wave of pre-written batch briefs. Temporary scaffolding.
export const meta = {
  name: 'fleet-quality-finish',
  description: 'Run one wave of quality-pass batches from their pre-written briefs, one agent per file-disjoint batch',
  phases: [{ title: 'Fix', detail: 'one agent per batch' }],
}
const WAVES = {
  w1: ['w1-agents', 'w1-server', 'w1-machines', 'w1-router-term', 'w1-jobs'],
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
          what_changed: { type: 'string' },
          tests_added: { type: 'string' },
          watched_it_fail: { type: 'boolean' },
          verified: { type: 'string', description: 'the exact command run and its real result' },
        },
        required: ['id', 'outcome', 'what_changed', 'tests_added', 'watched_it_fail', 'verified'],
      },
    },
    files_touched: { type: 'array', items: { type: 'string' } },
    followups: { type: 'string' },
  },
  required: ['outcomes', 'files_touched', 'followups'],
}
const wave = (args && args.wave) || 'w1'
const batches = WAVES[wave]
if (!batches) throw new Error(`unknown wave ${wave}`)
phase('Fix')
const results = await parallel(
  batches.map((b) => () =>
    agent(
      `You are fixing verified quality findings in the fleetd Cargo workspace at the repository root.\n\nYour brief is the file \`.claude/workflows/state/briefs/${b}.txt\`. Read it first and follow it exactly.`,
      { label: `fix:${b}`, phase: 'Fix', schema: REPORT_SCHEMA },
    ).then((r) => ({ batch: b, ...(r || { outcomes: [], files_touched: [], followups: 'no result' }) })),
  ),
)
const ok = results.filter(Boolean)
log(`${wave}: ${ok.reduce((n, r) => n + r.outcomes.filter((o) => o.outcome === 'fixed').length, 0)} fixed`)
return ok
