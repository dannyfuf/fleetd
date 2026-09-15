### R1-1 | P1 | crates/fleet-app/src/bridge/requests.rs:93
- Problem: A mutation is considered settled by any snapshot or an unconditional 250 ms expiry, without proving that the mutation’s own snapshot was applied.
- Impact: `await idle` can return against stale daemon-derived UI state, producing flaky failures or false-green assertions after mutations.
- Fix: Correlate mutation replies and snapshots with revisions, clearing only claims covered by the applied revision; treat expiry as an explicit synchronization failure rather than successful settlement.
- Evidence: `requests.rs:93-103,113-122` arms the expiry; `state/harness.rs:258-262,337-338` clears claims without correlation; `shell/root/events.rs:52-63` clears every claim on any snapshot; `fleet-daemon/src/server/broadcast.rs:88-128` permits an older coalesced snapshot to publish while a newer revision schedules another. This violates `docs/TESTING-HARNESS.md:180-182`.

### R1-2 | P2 | crates/fleet-harness/src/scenario.rs:377
- Problem: Teardown failures are collected only after the scenario outcome is fixed, so they are omitted from the failed journal entry and report verdict.
- Impact: A run whose daemon, Fleet process, or display lane fails during teardown returns an error but leaves a Passed report and deletes `home/`, destroying evidence from a failed run.
- Fix: Fold teardown problems into `outcome` before writing the failed event and report or deciding whether to remove `home/`.
- Evidence: `scenario.rs:377` collects teardown problems, `:392-408` journals and renders using only `outcome`, `:414-420` removes `home/`, and only `:426-429` converts the teardown problem into an error. The evidence-retention contract is `docs/TESTING-HARNESS.md:400,406-408`.

### R1-3 | P2 | crates/fleet-harness/src/scenario.rs:107
- Problem: Directory runs use a requested `--run-dir` as the suite root without creating or atomically claiming it.
- Impact: A normal request for a new suite directory fails every child with `ENOENT`; using an existing directory allows concurrent suites to share and overwrite its `report.md`.
- Fix: Atomically claim and create the suite root before constructing child paths, applying the same collision policy as default suite roots.
- Evidence: `scenario.rs:107-110` merely copies the requested path and `:121-123` immediately creates a child beneath it, while `rundir.rs:79-87,251-260` uses non-recursive `create_dir` for child claims. `docs/TESTING-HARNESS.md:387-390` requires suite roots to follow the atomic-claim rule.

### R1-4 | P3 | crates/fleet-harness/src/scenario.rs:870
- Problem: Capture, baseline decoding, or baseline I/O errors occurring after a successful `shot` response discard that app response from the journal.
- Impact: Failed screenshot runs lose the settled geometry and correlated app exchange precisely when the journal is needed for diagnosis.
- Fix: Return a structured shot outcome containing both the original response and any runner-side failure, so the command exchange is recorded before the step is marked failed.
- Evidence: `scenario.rs:861` receives the response, but `:870-882` propagates later failures with `?`; `journal_outcome` consequently takes its `Err` branch at `:612-624` and records only a runner error instead of the command exchange handled at `:594-601`. This contradicts `docs/TESTING-HARNESS.md:484-486`.