// Fleet quality pass — audit workflow.
// Temporary orchestration scaffolding for the chore/cleaning quality pass.
// Removed from the tree once the pass lands.

export const meta = {
  name: 'fleet-quality-audit',
  description: 'Audit the fleetd workspace against the Zed-derived project skills and verify every finding',
  phases: [
    { title: 'Audit', detail: 'one finder per area, each loading its matching project skill' },
    { title: 'Verify', detail: 'adversarial re-check of each finding against the real code' },
  ],
}

const FINDINGS_SCHEMA = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string', description: 'short kebab-case slug unique within this area' },
          file: { type: 'string', description: 'repo-relative path' },
          line: { type: 'integer' },
          severity: { type: 'string', enum: ['high', 'medium', 'low'] },
          category: {
            type: 'string',
            enum: ['bug', 'reliability', 'performance', 'abstraction', 'consistency', 'test-gap', 'doc-drift'],
          },
          rule: { type: 'string', description: 'the skill/doc rule violated, e.g. "gpui-performance: render prepares nothing"' },
          summary: { type: 'string', description: 'one sentence stating the defect' },
          evidence: { type: 'string', description: 'the code that proves it, quoted, with file:line' },
          failure_scenario: { type: 'string', description: 'concrete inputs/state -> wrong behaviour. For non-bugs, the concrete cost.' },
          proposed_fix: { type: 'string' },
          test_plan: { type: 'string', description: 'the exact test that would fail before the fix and pass after' },
        },
        required: ['id', 'file', 'line', 'severity', 'category', 'rule', 'summary', 'evidence', 'failure_scenario', 'proposed_fix', 'test_plan'],
      },
    },
    notes: { type: 'string', description: 'anything the schema could not hold: broad patterns, area health, what you did not cover' },
  },
  required: ['findings', 'notes'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          verdict: { type: 'string', enum: ['CONFIRMED', 'PLAUSIBLE', 'REFUTED'] },
          reason: { type: 'string', description: 'what you actually read that decided it, with file:line' },
          corrected_file: { type: 'string' },
          corrected_line: { type: 'integer' },
          corrected_fix: { type: 'string', description: 'the fix as it should actually be written, if the proposal was wrong or incomplete' },
          effort: { type: 'string', enum: ['trivial', 'small', 'medium', 'large'] },
          blast_radius: { type: 'string', description: 'other files/tests this fix touches' },
        },
        required: ['id', 'verdict', 'reason', 'effort', 'blast_radius'],
      },
    },
  },
  required: ['verdicts'],
}

const COMMON = `
You are auditing the fleetd Cargo workspace at the repository root. This is a READ-ONLY audit:
do not edit any file.

The repo ships project skills under .claude/skills/ distilled from the Zed codebase. Load the
skill named for your area with the Skill tool FIRST, read its references/checklist.md, and audit
against it. docs/ is authoritative: when code and a doc disagree, that is a finding.

What counts as a finding, in priority order:
1. Real bugs — wrong behaviour for some concrete input or interleaving. Race conditions, lost
   updates, stale state, off-by-one, panics, error paths that silently swallow, resource leaks,
   unbounded growth, incorrect cancellation.
2. Reliability sloppiness — swallowed errors (\`let _ =\` on a fallible call), bare .detach(),
   unwrap/expect in production, missing timeouts, missing cleanup on failure, inconsistent
   handling of the same condition in two places.
3. Performance — work in a render path, per-frame allocation or recompute, O(n^2) over a
   collection that grows, blocking the foreground thread, unvirtualized lists.
4. Wrong abstraction / composition — duplicated logic that wants one helper, a component taking
   domain types it should not, a god-function, state that should be derived being stored, an
   Entity that should be a plain struct or vice versa, inconsistent APIs for the same concept.
5. Test gaps — an untested branch that could plausibly regress, a test that cannot fail, a test
   that depends on wall-clock time or ordering.

Rules for what you report:
- Every finding must cite a file:line you actually read, and quote the code.
- No style nits, no "consider renaming", no speculative refactors, no new features.
- Do not report something the code deliberately does and a doc or skill endorses.
- Prefer 8 findings you are sure of over 30 you are guessing at. Depth beats breadth.
- For every finding, the test_plan must name a concrete test that fails before the fix.
- Read the code. Do not rely on grep alone for a claim about behaviour.
`

const AREAS = [
  {
    key: 'app-state',
    skill: 'gpui-state-and-memory',
    scope: `crates/fleet-app/src/{state,state.rs,shell,watches,watches.rs,bridge.rs,bridge} and every
cx.observe / cx.subscribe / cx.notify / Task field / Subscription field / Entity in fleet-app.
Focus on: subscriptions that outlive their target, Entity vs WeakEntity direction, tasks dropped
or detached where they should be retained, notify storms, notify missing after a mutation,
re-entrant entity updates, state that should be derived.`,
  },
  {
    key: 'app-render-perf',
    skill: 'gpui-performance',
    scope: `every render / render_prepared / RenderOnce::render body in crates/fleet-app and
crates/fleet-lazygit. Focus on: IO, requests, cx.notify, sorting, filtering, string formatting,
diffing, highlighting or allocation inside render; projections recomputed per frame that should
be memoised per revision; lists that can exceed the viewport and are not virtualized; clones of
large structures on the hot path.`,
  },
  {
    key: 'ui-kit-components',
    skill: 'gpui-components',
    scope: `crates/fleet-ui-kit/src/components/**. Focus on: components that take domain types,
inconsistent builder APIs for the same concept across components, the wrong rendering tier
(RenderOnce vs Render vs Element), slots that should exist, near-duplicate components that want
one parameterized component, gallery coverage gaps for shipped variants.`,
  },
  {
    key: 'ui-kit-styling',
    skill: 'gpui-styling',
    scope: `crates/fleet-ui-kit/src/{theme,text.rs,tone.rs,icons.rs,focus.rs,truncate.rs} and every
div() chain in fleet-ui-kit and fleet-app. Focus on: literal colors/sizes/durations outside the
theme, tokens defined but unused or duplicated, inconsistent hover/focus/disabled treatment for
the same role, elevation used inconsistently, truncation done ad hoc.`,
  },
  {
    key: 'app-shell',
    skill: 'gpui-app-shell',
    scope: `crates/fleet-app/src/{actions.rs,keymap.rs,dialogs,shell}. Focus on: actions declared
but never bound or handled, keybindings that conflict or contradict docs/KEYMAP.md, focus handles
not restored on dismiss, dialogs that duplicate the same open/close/validate logic instead of
sharing a Dialog abstraction, list navigation reimplemented per screen, toast/error handling that
diverges from docs/UX-SPEC.md.`,
  },
  {
    key: 'app-async',
    skill: 'rust-async-background-work',
    scope: `crates/fleet-app/src/{bridge.rs,bridge,drive.rs,terminal,terminal.rs} and every
cx.spawn / background_spawn / .detach / debounce timer in fleet-app and fleet-lazygit. Focus on:
stale async results applied after the target moved on (no generation/epoch check), missing
cancellation, blocking the foreground thread, debounce constants that differ for the same
interaction, dropped errors, requests issued per keystroke.`,
  },
  {
    key: 'daemon-async',
    skill: 'rust-async-background-work',
    scope: `crates/fleet-daemon/src/{server,jobs,services/pool.rs,services/dispatch.rs,services/sleep.rs}
and crates/fleet-term. Focus on: tokio::spawn without a CancellationToken or JoinHandle owner,
blocking calls on the async runtime, select! loops that lose messages or can busy-spin, PTY /
subprocess threads that leak on failure, unbounded channels, shutdown paths that do not drain,
timers that use wall clock where they should use a monotonic clock.`,
  },
  {
    key: 'ipc',
    skill: 'rust-ipc-protocol',
    scope: `crates/fleet-proto, crates/fleet-client, crates/fleet-daemon/src/{server,machines}.
Focus on: request paths with no timeout, reconnect that loses subscriptions or replays wrongly,
version/capability handling, framing limits, error decoding, partial writes, correlation-id
reuse, remote SSH bootstrap failure paths.`,
  },
  {
    key: 'daemon-services',
    skill: 'rust-workspace-architecture',
    scope: `crates/fleet-daemon/src/services/{worktrees.rs,repos.rs,router,sessions,maintenance.rs,
watch_discovery.rs,watches.rs,mirror.rs,hosts.rs,boards} and crates/fleet-daemon/src/adapters.
Focus on: real bugs in the service logic — state that can desync from disk, operations that are
not idempotent on retry, cleanup skipped on the error path, races between concurrent requests for
the same resource, filesystem operations that assume success.`,
  },
  {
    key: 'agents',
    skill: 'rust-workspace-architecture',
    scope: `crates/fleet-daemon/src/services/agents/** and crates/fleet-core/src/agents/**.
Focus on: the event reducer and projection — event orderings that produce wrong thread state,
duplicate or out-of-order events, completion authority, provider map.rs translation gaps against
docs/research/harness-protocols.md, session resume, process lifecycle on crash.`,
  },
  {
    key: 'core-git-term',
    skill: 'rust-workspace-architecture',
    scope: `crates/fleet-core (excluding agents/), crates/fleet-git, crates/fleet-term.
Focus on: pure-logic bugs — parsing, path handling, name sanitisation, sorting/ordering
instability, unicode/width handling, git porcelain parsing that breaks on unusual output,
scrollback/grid indexing.`,
  },
  {
    key: 'workspace-hygiene',
    skill: 'rust-workspace-architecture',
    scope: `the whole workspace, mechanically. Run ripgrep for: production unwrap(), expect(
without a static-invariant message, "let _ =" on fallible calls, bare .detach(), TODO/FIXME,
dbg!, panic!/todo!/unimplemented!, unbounded channel constructors, .clone() in hot loops.
Then read each hit and report the ones that are real. Also check: files past ~900 lines that
should be split, dependency edges that violate the layering in docs/ARCHITECTURE.md, crate-level
version literals, missing [lints] workspace = true, and doc/code drift where docs/ describes
behaviour the code no longer has.`,
  },
  {
    key: 'tests',
    skill: 'rust-gpui-testing',
    scope: `every test in the workspace. Focus on: tests that cannot fail (assert on a value they
just computed the same way), tests that sleep or read the wall clock, tests coupled to iteration
order of a HashMap, fakes that diverge from the real adapter's contract, and — most importantly —
untested branches that carry real risk: error paths, reconnect, cancellation, concurrent access,
empty/one/many boundaries. Report the highest-value missing tests as test-gap findings with the
exact test to write.`,
  },
]

phase('Audit')

const results = await pipeline(
  AREAS,
  (area) =>
    agent(
      `${COMMON}\nYour area: **${area.key}**.\n\nLoad the \`${area.skill}\` skill first.\n\nScope:\n${area.scope}\n\nUse the id prefix "${area.key}-" for every finding.`,
      { label: `audit:${area.key}`, phase: 'Audit', schema: FINDINGS_SCHEMA },
    ),
  (found, area) => {
    if (!found || !found.findings.length) return { area: area.key, findings: [], notes: found ? found.notes : '' }
    return agent(
      `You are an adversarial verifier. Another agent audited the fleetd workspace area
"${area.key}" and produced the findings below. Your job is to REFUTE each one.

For each finding: open the cited file, read the surrounding code and its tests, and decide.
- REFUTED — the code does not do what the finding claims, the behaviour is intentional and
  documented, an existing test already covers it, or the "bug" cannot actually occur.
- CONFIRMED — you reproduced the reasoning against the real code and the defect is real.
- PLAUSIBLE — real-looking but you could not fully prove it from the code alone.

Default to REFUTED when uncertain. Be harsh: a wrong finding costs more than a missed one.
When a finding is real but its proposed fix is wrong, naive, or would break something else,
set corrected_fix to what should actually be done. Set blast_radius to the other files and
tests the fix touches. Do not edit any file.

FINDINGS:
${JSON.stringify(found.findings, null, 1)}`,
      { label: `verify:${area.key}`, phase: 'Verify', schema: VERDICT_SCHEMA },
    ).then((v) => ({ area: area.key, notes: found.notes, findings: found.findings, verdicts: v ? v.verdicts : [] }))
  },
)

const kept = []
for (const r of results.filter(Boolean)) {
  const byId = new Map((r.verdicts || []).map((v) => [v.id, v]))
  for (const f of r.findings || []) {
    const v = byId.get(f.id)
    if (!v || v.verdict === 'REFUTED') continue
    kept.push({ ...f, area: r.area, verdict: v.verdict, verify_reason: v.reason, effort: v.effort, blast_radius: v.blast_radius, corrected_fix: v.corrected_fix || null, file: v.corrected_file || f.file, line: v.corrected_line || f.line })
  }
}

log(`${kept.length} findings survived verification across ${results.filter(Boolean).length} areas`)

return {
  kept,
  notes: results.filter(Boolean).map((r) => ({ area: r.area, notes: r.notes })),
}
