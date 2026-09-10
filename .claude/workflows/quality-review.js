// Fleet quality pass — final review. Temporary scaffolding.
export const meta = {
  name: 'fleet-quality-review',
  description: 'Review the whole quality-pass branch against the zed-quality-review bar and audit every unverified claim',
  phases: [{ title: 'Review', detail: 'one reviewer per lens over the branch diff' }],
}

const FINDINGS = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          line: { type: 'integer' },
          severity: { type: 'string', enum: ['blocker', 'high', 'medium', 'low'] },
          rule: { type: 'string', description: 'the checklist rule or CLAUDE.md non-negotiable violated' },
          summary: { type: 'string' },
          failure_scenario: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['file', 'line', 'severity', 'rule', 'summary', 'failure_scenario', 'fix'],
      },
    },
    verdict: { type: 'string', description: 'ship / fix-first, and why, in two sentences' },
  },
  required: ['findings', 'verdict'],
}

const BASE = 'bf37903'

const LENSES = [
  {
    key: 'claims-audit',
    prompt: `Audit the HONESTY of this branch, not its taste.

Several fixes were reported as done by agents whose reports were not independently checked. For
each of the claims below, open the code and decide whether the claim is TRUE of the branch as it
stands. Report every claim that is false, overstated, or where the test named does not actually
cover the defect.

Pay closest attention to these, which were reported "fixed" but where nobody watched a test fail
first — for each, either confirm the change is genuinely non-behavioural (a doc, a log target, a
lint, a pure refactor with no observable change) or report it as an untested behavioural change:
  agents-projection-cloned-per-event, agents-create-rollback-leaks-provider,
  daemon-async-shutdown-select-not-biased, daemon-async-lag-recovery-swallows-failures,
  workspace-hygiene-daemon-id-write-swallowed, ipc-nudge-doc-drift,
  workspace-hygiene-make-run-doc-drift, core-git-term-uncommented-fire-and-forget-sends,
  core-git-term-bare-tracing-macros, app-async-before-timeout-duplicated,
  ui-kit-styling-app-hand-assembled-elevated-surface, ui-kit-components-cycler-tabs-gallery-gap,
  ui-kit-components-role-without-accessible-name, ui-kit-styling-hover-paints-over-selection,
  app-shell-palette-recomputes-every-candidate-per-notification,
  app-render-perf-card-picker-derives-candidates-in-render, workspace-hygiene-adr-0011-duplicate-number

The findings, with their original evidence and test plans, are in
.claude/workflows/state/batches/*.json. Also check: does every new test actually fail if you
revert its fix? Spot-check the three you consider most load-bearing by actually doing it.`,
  },
  {
    key: 'daemon',
    prompt: `Review the daemon-side diff (crates/fleet-daemon, fleet-core, fleet-client, fleet-proto).
Load the rust-async-background-work, rust-ipc-protocol and rust-workspace-architecture skills and
apply their references/checklist.md. Look hardest for: a fix that introduces a new deadlock, lock
ordering inversion, or lost wakeup; cancellation that now drops work it should finish; an error
path that changed from silent to panicking; a protocol change that breaks a peer running the old
build; unbounded growth the fix did not actually bound.`,
  },
  {
    key: 'app-kit',
    prompt: `Review the app-side diff (crates/fleet-app, fleet-ui-kit, fleet-lazygit).
Load the gpui-state-and-memory, gpui-performance, gpui-components and gpui-styling skills and apply
their references/checklist.md. Look hardest for: work still in a render path; a memoisation whose
cache key is wrong or never invalidates; an Entity/WeakEntity direction that leaks; a subscription
or Task dropped or retained wrongly; a virtualized list whose scroll/cursor/selection no longer
agrees with the model; a component that now takes a domain type.`,
  },
  {
    key: 'coherence',
    prompt: `Review the branch for CONSISTENCY and DOC TRUTH, across crate boundaries.
docs/ is authoritative. For every behavioural change in the diff, check the doc that governs it
(docs/README.md maps each document to its domain) actually describes the new behaviour. Then look
for the same concept now handled two different ways in two places — the pass touched many files
and that is exactly how inconsistency gets in. Also verify: every new public item has a doc
comment, no production unwrap/expect/todo/dbg/TODO was introduced, no bare .detach(), and no
"let _ =" on a fallible call without the shutdown comment CLAUDE.md requires.`,
  },
]

phase('Review')
const results = await parallel(
  LENSES.map((l) => () =>
    agent(
      `You are reviewing the completed quality-pass branch in the fleetd workspace at the repository
root, against the Zed-derived engineering bar in .claude/skills/.

The branch is \`chore/cleaning\`; review \`git diff ${BASE}..HEAD\` (about 100 files). \`git log
--oneline ${BASE}..HEAD\` shows the intent of each commit. Ignore \`.claude/\` — it is temporary
scaffolding that is deleted before merge.

${l.prompt}

Rules: do not leave the tree modified. The branch is fully committed, so to prove a test fails
without its fix you may edit a file, run the test, and then restore it with
\`git checkout -- <path>\` — always restore, and never run any other git write command
(no add, commit, stash, clean, checkout of a branch). Every other lens is read-only. Every finding needs a file:line you actually read
and the rule it violates. Report only what you would block or fix — no style nits, no speculative
refactors, no "consider". If the branch is good, say so: an empty findings list is a valid and
useful result. Be specific about what would actually go wrong.`,
      { label: `review:${l.key}`, phase: 'Review', schema: FINDINGS },
    ).then((r) => ({ lens: l.key, ...(r || { findings: [], verdict: 'no result' }) })),
  ),
)
return results.filter(Boolean)
