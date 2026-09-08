# 0009 — Jira through `acli`, not through the REST API

**Adopted.** The `"jira"` backend drives the Atlassian CLI (`acli` 1.3.18) through the daemon's
`Shell` adapter. There is no HTTP client, no API token in Fleet's config, and no OAuth flow of our
own: `acli jira auth status` reports the machine's single active account, and every call is an
argv. The backend lives entirely in `crates/fleet-daemon/src/adapters/board/jira/`; outside that
directory the word "jira" appears only in tests and doc comments.

- **A REST client was rejected.** It would have made Fleet responsible for storing an Atlassian
  credential, refreshing it, and tracking API changes — for a feature whose users already have
  `acli` authenticated. The cost is `acli`'s limits, and those are cheaper than a secret store.
- **Bulk fetch was rejected because `acli` does not have it.** `search` returns only `key, summary,
  status, assignee, priority, issuetype, description, labels`; everything else — `updated`,
  `duedate`, `parent`, comments, custom fields — needs one `view --fields … --json` per key. The
  backend therefore fetches per key with bounded concurrency, and pulls incrementally with a JQL
  `updated >= "-<n>m"` window because there is no cursor pagination and no page metadata.
- **Id-keyed statuses were rejected.** `transition --status <name>` moves by status *name* and
  transitions cannot be listed, so the board's status map is name-keyed and `adopt_schema` matches
  on the name the remote reports.
- **A generic markdown crate was rejected for ADF.** Descriptions and comment bodies are Atlassian
  Document Format JSON, not text. `jira/adf.rs` is a pure, fixture-tested converter in both
  directions; nothing else in the workspace needs to know ADF exists.
- **Silent field loss was rejected.** `acli edit` can change only summary, description, assignee and
  labels; priority, due date, parent and story points are not writable post-create. The backend
  declares those in `BackendSchema.readonly_fields`, and the core refuses a local edit to one of
  them rather than letting it become a change that can never be pushed. `parent_id` is the single
  exemption, on create only, because `acli create` can carry the hierarchy that `edit` cannot.

Consequences: a failing `acli` call reports the subcommand it ran and never its argv, because that
text is persisted in `board.sync.last_error` and printed by the job log, the CLI and the app — and
the argv carries issue summaries and assignee addresses (`adapters/shell.rs::subcommand`). The full
command line goes to the debug log only. `docs/BOARD-JIRA.md` is the prose half of this contract,
including the verified `acli` command reference, and changes with the backend.
