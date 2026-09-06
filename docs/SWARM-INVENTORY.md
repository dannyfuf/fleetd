## 1. Domain model

### Identity and validation

| Type | Actual constraint / construction | Meaning |
| --- | --- | --- |
| `ContextId` | `z.string().regex(/^[a-z0-9][a-z0-9-]*$/)` | Persisted context key. Creation lowercases the name, replaces non-`[a-z0-9]` runs with `-`, collapses/trims `-`. |
| `RepoId` | `z.string().regex(/^[^/\s]+\/[^/\s]+$/)` | Exactly `owner/name`. |
| `WorktreeId` | `z.string().regex(/^[^/\s]+\/[^/\s]+#[^\s#]+$/)` | Exactly `owner/name#slug`; globally unique across hosts. |
| `HostId` | `/^[a-z0-9-]+$/` plus `id !== "local"` | Remote-host key; `local` is reserved. |
| slug | nonempty, not `.`/`..`, `slugify(slug) === slug`, starts `[a-z0-9]` | Filesystem/id component. `slugify`: lowercase; `/` and non-`[a-z0-9._-]` → `-`; collapse/trim `-`. |
| branch | `validateBranch` rejects empty, `@`, leading `-`, trailing `/`/`.`, empty/dot-leading/`.lock` components, control/space, `..`, `@{`, `[`, `\`, `[~^:?*]`; slug beginning `.hot` is reserved | Git branch in the independent copy. |
| local session | `` `${repoName.replace(/[.:]/g, "-")}/${slug.replace(/[.:]/g, "-")}` `` | Example `payroll/feat-payroll-fix`. |
| proxy session | same `sessionName(hostId, remoteSession)` | Example `devbox/payroll/feat-payroll-fix`. |

### Persisted entities (`src/core/types.ts`)

```ts
type Context = {
  id: ContextId;
  name: string;
  owners: string[];
  createdAt: string; // ISO datetime
};

type RepoHooks = { prepare: string[]; postCreate: string[] };

type Repo = {
  id: RepoId; owner: string; name: string; url: string;
  contextId: ContextId; defaultBranch: string; path: string;
  clonedAt: string; // ISO datetime
  hooks: RepoHooks; // default {prepare: [], postCreate: []}
};

type CloneJob = {
  id: RepoId; owner: string; name: string; url: string;
  contextId: ContextId; defaultBranch: string; path: string;
  stagingPath: string; logPath: string; pid?: number;
  startedAt: string;
  status: "starting" | "cloning" | "failed";
  error?: string;
};

type Worktree = {
  id: WorktreeId; repoId: RepoId; slug: string; branch: string;
  baseRef: string; path: string; session: string; host?: string;
  createdAt: string; lastOpenedAt?: string;
};

type HostConfigEntry = {
  ssh: string;
  swarmCommand: string; // default "swarm"
};
type Host = HostConfigEntry;
```

- **Context** groups repos and supplies GitHub owners. Delete cascades repos/worktrees/sessions.
- **Repo** is a pristine full clone at `reposDir/<owner>/<name>`; only fetch/reset/clean occurs there. `url` is the selected SSH/HTTPS clone URL.
- **Worktree** is not `git worktree`; it is a full independent clonefile/reflink copy. `host` absent means local. Remote mirrors store authoritative remote `path`/`session`, set `host`, and preserve locally tracked `lastOpenedAt` across sync.
- **Prepared-copy slot** is unregistered, based on pristine base, and has `prepare` hooks attempted. Slot 0 is `.hot`; `n >= 1` is `.hot.<n>`; `hotPoolSize: 0` disables it.
- **Agent session** uses `type AgentName = "claude" | "opencode"`; it is runtime-only `swarm-agent-<AgentName>`, rooted at `config.reposDir`, and not in state.

### Runtime session/status types

```ts
type SessionState = "none" | "detached" | "attached" | "unknown";
interface TmuxPane { id: string; pid: number; currentCommand: string; currentPath: string }
interface TmuxWindow { session: string; index: number; name: string; active: boolean; panes: TmuxPane[] }
interface TmuxSession {
  name: string; attached: boolean; windows: number;
  createdAt: number; lastActivityAt: number;
}
interface WorktreeStatus {
  worktreeId: WorktreeId;
  session: SessionState;
  windows: Array<{ index: number; name: string; command: string; keepAlive: string[] }>;
  running: string[];
}
```

`command` is the first pane's `pane_current_command`; `keepAlive` holds matched rule labels including dynamic `:<port>`. `unknown` means observation failed (especially remote offline), unlike successful `none`.

### Pull-request and inspection facts

```ts
type PrTab = "mine" | "review";
type PrChecks = "pass" | "fail" | "pending" | "none";
type PrReviewDecision = "approved" | "changes_requested" | "review_required" | "none";
type PrState = "draft" | "ci_fail" | "changes" | "ci_pending" | "approved" | "review";

type PullRequest = {
  repoId: RepoId; number: number; title: string; url: string; author: string;
  headRefName: string; baseRefName: string; isDraft: boolean;
  isCrossRepository: boolean; headRepo?: RepoId;
  reviewDecision: PrReviewDecision; checks: PrChecks;
  additions: number; deletions: number; labels: string[]; updatedAt: string;
};

type PrRepoSlice = {
  prs: PullRequest[]; fetchedAt?: string; error?: string; loading: boolean;
};

type InspectionPullRequest = {
  number: number; state: "OPEN" | "MERGED" | "CLOSED"; url: string;
  baseRefName: string; headRefOid: string;
};

type WorktreeInspection = {
  worktreeId: WorktreeId; repoId: RepoId; host: string; path: string;
  branch: string; baseRef: string; head: string | null; targetBranch: string;
  upstream: string | null; ahead: number | null; behind: number | null;
  upstreamGone: boolean; dirty: boolean; mergedIntoTarget: boolean;
  uniqueCommits: number | null; published: boolean; merged: boolean;
  pr: InspectionPullRequest | null; session: SessionState; running: string[];
  inspectedAt: string; warnings: string[]; error: string | null;
};
```

- `PullRequest.url` validates as exactly `https://github.com/<owner>/<name>/pull/<number>`; null GitHub author maps to `ghost`.
- PR-state priority: draft → CI fail → changes requested → CI pending → approved → review.
- PR/worktree match: `baseRef === "pull/<number>/head"`, or same-repo `branch === headRefName`. Same-repo local branch is `headRefName`; cross-repo is `pr/<number>`.
- `mergedIntoTarget` is raw `HEAD` ancestry into `origin/<targetBranch>`. `published` means `refs/remotes/origin/<branch>` exists (a matching gone upstream also sets it).
- `merged` means a `MERGED` PR whose `headRefOid` equals or contains local `head`, **or** `mergedIntoTarget && published`. Commits added after PR merge therefore fail closed.

### `state.json` exact schema

```ts
type State = {
  version: 1;
  contexts: Context[];
  repos: Repo[];
  clones: CloneJob[]; // default [] when missing
  worktrees: Worktree[];
  activeContextId?: ContextId;
};
```

Default is exactly `{ "version": 1, "contexts": [], "repos": [], "clones": [], "worktrees": [] }`; `activeContextId` is omitted. Load/save is Zod-validated. Invalid disk state is renamed to `state.json.broken-<epochms>` and raises `validation`.

### `config.json`: every field and default

Missing fields deep-merge with `defaultConfig(SWARM_HOME)`. `~`/`~/` expansion applies only to `reposDir`/`worktreesDir`, then both resolve absolute. Unknown keys are stripped by Zod.

| JSON path | Type / validation | Exact default | Meaning |
| --- | --- | --- | --- |
| `version` | literal `1` | `1` | Schema version. |
| `reposDir` | string, resolved absolute | `<SWARM_HOME>/repos` | Base clones. |
| `worktreesDir` | string, resolved absolute | `<SWARM_HOME>/worktrees` | Slots, attempts, worktrees. |
| `hosts` | record keyed by `HostId` | `{}` | Remote hosts. |
| `hosts.<id>.ssh` | nonempty string | none | SSH destination. |
| `hosts.<id>.swarmCommand` | nonempty string | `"swarm"` | Remote command prefix. |
| `defaultHost` | `"local" \| HostId`, configured if remote | `"local"` | Create-dialog placement. |
| `hotPoolSize` | nonnegative integer | `1` | Slots/repo; 0 disables. |
| `hotFreshnessMs` | nonnegative integer | `60000` | Fresh-marker age. |
| `hotRefreshIntervalMs` | nonnegative integer | `300000` | Periodic refresh; 0 disables. |
| `agent` | `"claude" \| "opencode"` | `"claude"` | Selected agent. |
| `agentCommands.claude` | nonempty string | `"claude"` | Full shell command typed into pane. |
| `agentCommands.opencode` | nonempty string | `"opencode"` | Same. Entries default independently. |
| `windows` | `{name:nonempty string,command:nonempty string}[]` | `[ {"name":"nvim","command":"nvim ."}, {"name":"cc","command":"{agent}"}, {"name":"lg","command":"fleet://lazygit"} ]` | Ordered windows; `{agent}` replaced by selected configured command. A `fleet://` command names a Fleet-provided surface instead of a program (only `fleet://lazygit` exists; an unknown one is a validation error). **Fleet divergence**: the third default is the native git pane, not the `lazygit` binary; an imported swarm `lazygit` window is upgraded to it, while a `lazygit` written into Fleet's own `config.json` is left alone. Schema allows `[]`, mount does not. |
| `sleep.enabled` | boolean | `true` | False keeps every window. |
| `sleep.keepAlive` | rules below | four defaults below | Window-preservation rules. |
| `sleep.keepAlive[].id` | string | below | Stable settings id. |
| `sleep.keepAlive[].label` | string | below | Report label. |
| `sleep.keepAlive[].kind` | `"process" \| "listening-port"` | below | Match mode. |
| `sleep.keepAlive[].pattern` | string | `""` at rule-schema level | Case-insensitive full-command regex; ignored for port. |
| `sleep.keepAlive[].enabled` | boolean | `true` | Active flag. |
| `sleep.graceMs` | integer; negative schema-valid, runtime clamps 0 | `2000` | Wait after `:qa`. |
| `discoveredWatches.enabled` | boolean | `true` | Enables read-only daemon process discovery. |
| `discoveredWatches.intervalMs` | nonnegative integer; runtime minimum 500 | `2000` | Process snapshot cadence. |
| `discoveredWatches.processes` | `{id,pattern,enabled}[]` | four agent rules below | Full-command regex candidates; invalid expressions are skipped. |
| `github.cacheTtlSeconds` | integer | `3600` | Repo cache TTL. |
| `github.prTtlSeconds` | integer | `90` | PR cache TTL; omitted from README sample. |
| `github.cloneProtocol` | `"ssh" \| "https"` | `"ssh"` | URL choice. |
| `ui.statusRefreshMs` | integer | `2000` | Local polling; runtime minimum 500 ms. |
| `ui.remoteStatusRefreshMs` | positive integer | `10000` | Per-host polling; runtime minimum 500 ms. |

```json
"keepAlive": [
  { "id": "claude", "label": "claude", "kind": "process", "pattern": "(^|/)claude( |$)", "enabled": true },
  { "id": "opencode", "label": "opencode", "kind": "process", "pattern": "(^|/)opencode( |$)", "enabled": true },
  { "id": "codex", "label": "codex", "kind": "process", "pattern": "(^|/)codex( |$)", "enabled": true },
  { "id": "servers", "label": "server", "kind": "listening-port", "pattern": "", "enabled": true }
]
```

Fleet additionally defaults `discoveredWatches.processes` to
`codex-companion\.mjs task-worker`, `(^|/)codex( |$)`, `(^|/)claude( |$)`, and
`(^|/)opencode( |$)`, all enabled. This is a Fleet daemon extension, not a Swarm
sleep-policy field.

Legacy normalization: if no window contains `{agent}`, the first exact command among `cc`, `claude`, `opencode` becomes `{agent}`.

### Marker/cache/trash/log entities

| Record | Exact shape/content | Lifecycle |
| --- | --- | --- |
| `.git/swarm-hot.json` | `{fetchedAt:string(ISO),defaultBranch:string,sha:string(40..64 hex),prepareFingerprint:string(64 hex)}` | Atomic after preparation; fingerprint = SHA-256 of `JSON.stringify(orderedPrepareCommands)`. |
| `.git/swarm-creating.json` | `{id:string,repoId:RepoId,branch:string,baseRef:string,createdAt:string(ISO)}` | Written just before final rename, removed after state commit. |
| repo cache | `{fetchedAt:string,repos:RemoteRepo[]}` | Invalid ignored; stale cache is fallback when refresh fails. |
| PR cache | `{fetchedAt:string(ISO),prs:PullRequest[]}` | Invalid ignored. |
| state lock | `<pid>\n` | Exclusive `state.json.lock`; 3 s timeout, 25 ms retry, 1 s fresh-empty grace; stale owner reclaimed. |
| node cache | absolute executable + newline | Launcher trusts when executable. |
| startup profile | one JSON line `{processStartMs:0,entries:[{name,startMs,durationMs?}]}` | Only with `SWARM_STARTUP_PROFILE`. |
| structured log | JSONL `{ts,level:"info"\|"warn"\|"error",scope,msg,data?}` | Append-only `swarm.log`; logger errors swallowed. |
| clone/hot logs | raw child stdout/stderr | Per-clone and per-slot. |
| trash | renamed repo/worktree/attempt then detached `rm -rf` | Rename gives fast logical deletion and rollback before state commit. |

`RemoteRepo = {owner,name,fullName,description,sshUrl,isPrivate,updatedAt,defaultBranch}`; null description → `""`, missing default branch → `"main"`.

## 2. Filesystem layout under ~/.swarm

`SWARM_HOME` defaults `$HOME/.swarm`; configurable directories may live elsewhere.

| Path pattern | Contents/invariant |
| --- | --- |
| `$SWARM_HOME/config.json` | Pretty validated Config; created first load; atomic temp+rename. |
| `$SWARM_HOME/state.json` | Pretty validated State; absent means default. |
| `$SWARM_HOME/state.json.lock` | Cross-process transaction PID lock. |
| `$SWARM_HOME/state.json.broken-<epochms>` | Quarantined invalid state. |
| `${reposDir}/<owner>/<name>/` | Pristine base. |
| `${reposDir}/<owner>/<name>.staging-<pid>-<uuid>/` | Detached repo clone attempt. |
| `${worktreesDir}/<owner>/<name>/<slug>/` | Published copy. |
| `${worktreesDir}/<owner>/<name>/<slug>.creating-<uuid>/` | Private create attempt. |
| `${worktreesDir}/<owner>/<name>/.hot` / `.hot.<n>` | Complete slots 0 / n≥1. |
| `${worktreesDir}/<owner>/<name>/.hot.staging` / `.hot.<n>.staging` | Incomplete worker staging. |
| `${worktreesDir}/<owner>/<name>/.hot[.<n>].staging.pid` | Worker PID. |
| `<copy>/.git/swarm-hot.json` | Freshness marker. |
| `<published>/.git/swarm-creating.json` | Publish intent. |
| `<worktree>/.git/swarm-post-create-<uuid>.log` | Temporary tab records; removed after runner. |
| `$SWARM_HOME/trash/<epochms>-<name-or-slug>[-<uuid>]` | Renamed deletion/recovery target. |
| `$SWARM_HOME/cache/github/<owner>.json` | Repo cache. |
| `$SWARM_HOME/cache/github/prs/<owner>/<name>/<mine|review>.json` | PR cache. |
| `$SWARM_HOME/cache/ssh/%C` | OpenSSH control sockets; directory mode `0700`. |
| `$SWARM_HOME/cache/node-bin` | Resolved Node. |
| `$SWARM_HOME/logs/swarm.log` | JSONL app + raw post-create output. |
| `$SWARM_HOME/logs/clone-<owner>-<name>-<uuid>.log` | Clone output. |
| `$SWARM_HOME/logs/hot-copy-<owner>-<name>[-<slot>].log` | Slot output; slot 0 no suffix. |
| `$SWARM_HOME/tmux.local.conf` | Optional override sourced by tmux config. |

`.hot*` is excluded from normal directory listings. Recursive removal accepts only strict descendants (never roots) of trash, reposDir, worktreesDir.

### Copy-on-write strategy

1. macOS synchronous path calls libc `clonefile(src,dest,0)` via `node:ffi` and `/usr/lib/libSystem.B.dylib`.
2. Partial clonefile destination is removed before fallback; pre-existing destination is never overwritten.
3. macOS fallback/detached copy: `cp -Rc <src> <dest>`.
4. Linux: `cp -R --reflink=auto <src> <dest>`.
5. Other: `cp -R <src> <dest>`.
6. Repo clones use detached `git clone`, not filesystem copy.

### Slot build, claim, refresh

Build/replenish:

1. Per-RepoId in-process mutex serializes pool inspection/base refresh/launch; deletion blocks new work.
2. Read `.staging.pid`, verify alive and (unless just launched) that `ps` command contains exact staging path. Poll every 100 ms with unref'ed timers; clean dead/invalid staging/PID.
3. Remove slots/staging/PIDs numbered `>= hotPoolSize` after any worker ends.
4. Pick lowest absent configured slot; if none, return.
5. Fetch/prune pristine base, resolve default, compare HEAD/origin SHA/dirty, and checkout/reset/clean only if needed.
6. Detached shell worker copies base→staging, runs ordered prepare hooks warning-only, writes `.git/swarm-hot.json.tmp-$$`, renames marker, asserts final slot absent, renames staging→slot. Trap removes failed staging and always PID; log is per-slot.

Claim/create:

1. Preflight state/path/id/session conflicts and reclaim only a destination containing publish intent.
2. Await shared refresh, preparation, and detached workers; preparation failure warns and falls back.
3. Make `${destination}.creating-<uuid>`.
4. Under repo mutex try rename slot 0, 1, … to attempt. `ENOENT` means another creator won. Successful claim emits `prepared-copy-claimed` for immediate refill.
5. If none exists, clone pristine base→attempt.
6. Outside mutex refresh attempt, checkout/create branch, rerun prepare hooks on fallback, reset, or fingerprint mismatch.
7. Publish/register atomically; conflict trashes only losing UUID attempt; other failure detached-removes it.

Freshness/ref rules:

1. Fresh means marker age `>=0 && < hotFreshnessMs`, exact fingerprint, default remote branch present, and current remote-tracking SHA equals marker SHA.
2. Named branch fetches exact refspecs for default, requested `origin/<base>`, and requested branch. Combined failure retries mandatory base refs; missing requested branch is allowed, missing selected nondefault base is fatal.
3. Stale unnamed path tries narrow default fetch then full fetch+prune.
4. Relist refs and reset only if HEAD differs/dirty.
5. Forced refresh handles existing slots sequentially; with no slot refreshes base. Fingerprint change deletes/rebuilds through staging, removing ignored old-hook output. Other refresh removes marker, refreshes in place, reruns hooks only after reset, then rewrites marker.
6. `skipIfFresh` avoids Git I/O only when at least one slot exists and every existing slot is fresh/fingerprint-matching.

### Publish intent and atomic recovery

1. Enter `state.json` transaction and repeat id/session/registered-path/final-path checks.
2. Atomically write attempt `.git/swarm-creating.json`.
3. Rename attempt→canonical slug.
4. Append Worktree/update repo default; validate; write `state.json.tmp-<pid>-<uuid>` exclusively; rename→state while lock held.
5. Remove intent. State failure after publication moves canonical path to trash and detached-removes it.
6. Startup scans every registered repo root before hydration. Missing marker (including `.git` pointer `ENOTDIR`) is skipped. Already registered path loses marker.
7. Valid recovery requires exact repo/id/canonical path/nonempty base and checked-out branch; append transactionally then remove marker.
8. Invalid/mismatched intent is trashed. Canonical unregistered intent can also be reclaimed during create preflight.

Atomic text write is exclusive temp + same-filesystem rename + best-effort temp unlink. Repo/worktree delete renames to trash inside transaction, restores original if save fails, then detached-removes after commit.

## 3. Operations / use cases

### CLI surface and envelopes

| Invocation | Inputs/defaults | Success output |
| --- | --- | --- |
| `swarm` | none | TUI; exit 0 on quit/open; updater requests 75 for launcher restart. |
| `swarm create <owner/name> <slug> [--branch <name>] [--base <ref>] [--host <id>] [--url <url>] [--default-branch <name>] [--hooks <json>] [--json]` | branch=slug; base=`origin/<repo default>`; omitted host=local; hooks=`{"prepare":[],"postCreate":[]}` | `{protocol:1,created:boolean,worktree:Worktree}`; human `Created <id>` / `Existing <id>`. |
| `swarm open <owner/name#slug\|repo/slug>` | matches id, stored session, or remote proxy name | No envelope; open/attach/switch. |
| `swarm delete <id>... [--json]` | one+ IDs | `{protocol:1,ok,results:[{worktreeId,ok,reason?}]}`; continues failures; exit 1 if any fail. |
| `swarm prune [--dry-run] [--no-fetch] [--kill-sessions] [--repo <owner/name>] [--json]` | all/repo; fetch on by default | `{protocol:1,dryRun,deleted:string[],skipped:[{worktreeId,reason,merged,dirty,uniqueCommits,running}]}`. Dry-run `deleted` means would-delete. |
| `swarm inspect [id...] [--fetch] [--repo <owner/name>] [--json]` | all default; fetch off | `{protocol:1,worktrees:WorktreeInspection[]}`. |
| `swarm kill <id> [--json]` | one ID | `{protocol:1,ok:true}`; human `Killed <id>`. |
| `swarm sleep [session] [--json]` | current client session default | `{protocol:1,kept:[{window,reason}],closed:string[],sessionKilled:boolean}` or pretty report. |
| `swarm agent [claude\|opencode]` | `config.agent` default | Nested tmux attach; no JSON. |
| `swarm status [--json]` | local worktrees only | `{protocol:1,statuses:WorktreeStatus[]}`; human `<id> <session>` lines. |
| `swarm path <id>` | local only | Resolved absolute plain path. |
| `swarm list [--json]` | none | `{protocol:1,version:"swarm <version>",repos:Repo[],worktrees:Worktree[]}`; human counts. |
| `swarm doctor` | none | Plain `CHECK STATUS DETAIL`; exit 1 on any fail. |
| `swarm --version`/`-v` | none | `swarm 0.1.0+<sha>` or `+dev`. |
| `--help`/`-h`, `<command> --help\|-h` | exact supported form | Usage/help. |

Any JSON-mode failure: `{"protocol":1,"error":{"kind":"<not-found|conflict|git|tmux|fs|github|remote|validation|cancelled|unsupported|unknown>","message":"<single-line>"}}`. Duplicate flags, missing values, malformed IDs/slugs/hooks, extra args, unknown options are validation errors.

### `create`

Preconditions/idempotency:

- Slug must be canonical; branch passes `validateBranch`.
- Existing id returns `created:false` only if every explicitly supplied `--branch`/`--host` matches. Omitted branch/host accepts recorded values; hooks do not rerun; base/url/default hint are not compared. Otherwise conflict.
- Unregistered repo requires `--url`. Context choice: owner-containing context, active, first, else create `{name:owner,owners:[owner]}`.
- Existing repo has supplied hooks persisted. New detached clone is reconciled every 100 ms until Repo/failed.

Algorithm:

1. Register/clone repo; base defaults `origin/<defaultBranch>`.
2. Claim/fallback private attempt per section 2; base itself is never refreshed on interactive claim path.
3. If `origin/<branch>` exists: `git checkout <branch>`; else `git checkout -b <branch> <baseRef>`.
4. PR create (TUI): fetch head; `git checkout -B <localBranch> refs/swarm/pulls/<n>/head`; persist `baseRef:"pull/<n>/head"`.
5. Publish intent, canonical rename, state append. Visibility precedes post-create hooks.
6. CLI awaits post-create runner. TUI schedules separate operation and can open immediately.
7. Claim queues one refill; fallback/no claim queues `hotPoolSize` attempts.

Remote exact argv: `create <repo.id> <slug> [--branch <branch>] --base <baseRef> --url <repo.url> --default-branch <repo.defaultBranch> --hooks <JSON.stringify(repo.hooks)> --json`; it never forwards `--host`, preserves omission of branch, has no client timeout, then syncs host.

Prepare hooks: ordered `sh -c <command>` in copy, continue/warn on nonzero. Post-create: one detached shell sequence, ordered, continue/warn, append output/start/end/exit/duration to `swarm.log`, never roll back.

### `open`

1. Resolve id/session/proxy; capture current client session.
2. Ensure local session/windows or valid one-pane remote SSH proxy.
3. Transactionally update `lastOpenedAt`.
4. Inside tmux `switch-client`; outside attach with inherited stdio.
5. Default confirms target still exists, finds previous registered worktree, sleeps it. Previous-sleep errors warn only. `O` sets `sleepPrevious:false`.

Unknown target/host, tmux/setup, or touch errors fail. Half-created new sessions are killed after mount failure.

### `delete`

- Unconditional: ignores dirty, commits, merge/PR, attachment, running commands.
- Local: validate canonical path; kill session; rename to `trash/<epochms>-<slug>` inside state transaction; unregister/commit; detached `rm -rf`. Restore rename if state save fails.
- Remote: invoke `delete <id> --json` accepting valid nonzero envelope; success kills proxy and mirror. Multi-delete sequentially records reasons and continues.
- Repo/context cascade coordinates pool work, deletes children, trashes base as `<epochms>-<repo.name>`, unregisters, detached-removes.

### `inspect`

1. Strictly resolve named IDs, apply repo filter, group hosts.
2. Remote `inspect <ids...> [--fetch] --json`; unreachable/omitted becomes per-worktree error.
3. `--fetch`: distinct local repos concurrently `git fetch --prune origin`; failure adds `fetch failed`.
4. One status snapshot.
5. Worktrees concurrently: path; latest PR; HEAD/upstream/gone/dirty; divergence; published ref; target ref; unique count; ancestry; merged.
6. Exact soft warnings: `fetch failed`, `gh unavailable`, `no upstream`, `upstream gone`, `ahead/behind unavailable`, `published status unavailable`, `target ref unavailable`, `target ref missing`, `unique commit count unavailable`, `target comparison failed`, `pull request head comparison failed`. Missing repo/path/fundamental Git state sets `error`.

### `prune`

1. One inspection; fetch unless `--no-fetch`.
2. Reject error, dirty, attached/unknown session, unknown required unique count, or unmerged.
3. Default also rejects running labels with `tmux session has running commands: ...`.
4. `--kill-sessions` allows running only because attached/unknown remain rejected and requires known unique count; deletion hard-kills session.
5. Dry-run does no delete but lists eligible IDs in `deleted`. Apply deletes sequentially; failure becomes skipped.

### Other commands

| Use case | Algorithm / side effects / errors |
| --- | --- |
| `kill` | Exact id; local kill-if-present or remote `kill <id> --json`; remote also kills local proxy. |
| `sleep` | Resolve explicit/current session; local policy in section 4; remote `sleep <remoteSession> --json`, preserving proxy. Error if no current/matching session. |
| `agent` | Create session if absent, first window agent name at reposDir, send configured command + Enter, attach via invoking socket with `TMUX` removed. Reopen preserves terminal. |
| `status` | Best-effort config+tmux sessions+panes+ps concurrently, ≤1 lsof; one status/local record. Failures log/degrade. |
| `path` | `path.resolve`, refuse remote. |
| `list` | State load only; no clone reconcile/remote sync. Resolve local paths. |
| `doctor` | Parallel 5 s `tmux -V`, `git --version`, `gh auth status`, macOS `cp -c` or other `cp --help`; check Node≥26.4, tmux≥3.2, auth, copy. Per host direct SSH `true`, then `list --json` 5 s with protocol/version. |

TUI-only: context create/update(service only)/switch/delete cascade; repo search/clone/assign/delete; PR mine/review/filter/refresh/browse/copy/create-open; updater requires clean `main`, runs `git pull --ff-only origin main`, pinned Node/npm install/build, caches Node and exits 75.

## 4. tmux control layer

### Default fixed three-window layout

`config.windows` can replace this default. Every local window cwd is `worktree.path`.

| Order/name | Exact typed command |
| --- | --- |
| 0 `nvim` | `nvim .` |
| 1 `cc` | `{agent}` → selected `agentCommands[agent]` (default `claude`) |
| 2 `lg` | `lazygit` (**Fleet**: `fleet://lazygit`, a native pane with no PTY — see `docs/ARCHITECTURE.md` "Native tabs"; swarm's `lazygit` is upgraded on import) |

Mount creates first window/session, types command, appends only missing **names**, swaps named windows into configured order, selects original minimum index. Extra or wrong-command windows remain.

### Every tmux adapter command

All adapter runs are `tmux -u ...`; add `LC_CTYPE=C.UTF-8` when configured locale is not UTF-8. Exact session target is `=<name>`.

| Operation | Arguments after `tmux -u` |
| --- | --- |
| current | `display-message -p #{client_session}` |
| sessions | `list-sessions -F #{session_name}\t#{session_attached}\t#{session_windows}\t#{session_created}\t#{session_activity}` |
| one session panes | `list-panes -t =<session> -s -F #{session_name}\t#{window_index}\t#{window_name}\t#{window_active}\t#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_current_path}` |
| all panes | same format, `list-panes -a -F ...` |
| exists | `has-session -t =<session>` |
| create | `new-session -d -s <name> -n <windowName> [-c <cwd>] [<command>]` |
| add window | `new-window -d -t =<session>: -n <name> -c <cwd> -P -F #{window_index}` |
| send | `send-keys -t <target> <keys...> [Enter]` |
| reorder | `swap-window -d -s =<session>:<a> -t =<session>:<b>` |
| select | `select-window -t =<session>:<index>` |
| close | `kill-window -t =<session>:<index>` |
| kill | `kill-session -t =<session>`; tolerant form ignores not-found text |
| proxy option | `set-option -t =<session>: <name> <value>` |
| switch | `switch-client -t =<session>` |
| attach | `tmux -u attach-session -t =<session>` via inherited stdio |
| message | `display-message <message>` |

Remote proxy `<host>/<remote session>` has one `ssh` window. Reuse requires exactly one pane with `currentCommand === "ssh"`; else replace. Set `remain-on-exit on`, `detach-on-destroy off`. Agent sessions are `swarm-agent-*`; `C-q` detaches nested client only there.

### Sleep policy

1. Missing session returns empty report. Disabled policy keeps all with reason `sleep disabled`.
2. List windows, one `ps -axo pid=,ppid=,command=`, descendant trees, ≤1 `lsof -nP -iTCP -sTCP:LISTEN -a -p <comma-pids> -F pn`.
3. Process rules use case-insensitive full command regex; invalid logs/skips. Ports produce sorted `:<port>` labels.
4. Walk windows descending. Any match keeps entire window with labels joined `, `.
5. Unmatched vim/nvim panes get `Escape`, `:qa`, Enter; find editor PID and poll 100 ms up to `max(0,graceMs)`.
6. Still alive → keep `unsaved changes`; otherwise close.
7. No kept windows → kill session once, `sessionKilled:true`; otherwise kill closable windows.

Switch sleeps prior only after target switch and confirmed liveness. `O` skips. Remote sleep preserves proxy.

### `tmux/tmux.conf`

| Setting/binding | Exact behavior |
| --- | --- |
| root | Empty `$SWARM_ROOT` → `$HOME/buk/swarm`. |
| reload | unbind `r`; prefix `r` sources config then displays `swarm tmux config reloaded`. |
| prefix | `C-s`; old default not explicitly unbound. |
| UI | mouse on; status top; Catppuccin rounded window status; left empty/right directory. |
| copy | vi mode; `v` select; `y`/mouse pipe to pbcopy→wl-copy→xclip→discard; prefix `P` paste. |
| nav | vim-tmux-navigator `C-h/C-l/C-k/C-j`. |
| clear | prefix `k`: `C-l`, `sleep .3s`, clear history. |
| popup | prefix `s`: `display-popup -E -w 90% -h 85%` running `"$SWARM_ROOT/bin/swarm"`, failure waits Enter. |
| agents | prefix `a` / `A`: same popup running `swarm agent claude` / `opencode`; root `C-q` detaches only `swarm-agent-*`, else sends C-q. |
| persistence | TPM, Catppuccin, vim-tmux-navigator, resurrect, continuum; restore on; save 10 min; capture contents; nvim strategy `session`; restore exactly `nvim lazygit`, not agents. |
| override | source `~/.swarm/tmux.local.conf` after TPM. |

TPM supplies prefix `I` plugin install; project file does not bind it directly.

## 5. TUI

### Views/layout

- Full terminal OpenTUI/React frame; popup 90%×85%. Header: numbered context tabs; attached/live, detached/sleeping, unknown/offline, review count.
- Main: left repos width 16..26 at ~24%; right worktrees. Worktrees pane and `All` pseudo-repo default. Body `max(1,height-7)`; detail 4 rows if body≥13, 3 if ≥8, else none; scrolloff 2.
- Footer priority: newest operation → newest of ≤3 3.2 s toasts → persistent error → loading → filter help → hints. Dialog ghosts base and owns keys; global Ctrl-C still quits.
- Modal model: normal mode owns vi keys (`j/k`, `h/l`, `gg/G`, `ctrl-d/u`); `/` enters a real insert-mode filter input; `:` opens the fuzzy command palette; dialog text fields consume printable input; `g` is an untimed one-key prefix for `gg`, `gt`, or `gT`, and an unrelated next key falls back to its ordinary normal-mode action.
- Exact empty copy includes `no contexts`, `No contexts yet.` + `N create your first context`, `No repos in <context>.` + `n clone one`, `No worktrees [for <repo>] yet.`, `Nothing matches “<filter>”.`, `No open PRs authored by you in <scope>.`, and `No PRs waiting for your review in <scope>.`.

| Screen | Exact displayed fields |
| --- | --- |
| Repos | `REPOS` count; All/repo cursor; disambiguated owner/name; worktree count; aggregate session glyph. Clone row spinner/failed. Detail variants: active context+owners+counts; repo name/owner/default/path/worktree/live counts/prepare/post hooks/url; clone status/path/log/error. |
| Worktrees | Header repo/all/filter/count/scroll. Row session glyph/spinner, optional repo prefix, branch, `@host`, running, optional `#n <state>` badge, relative opened-or-created. Detail branch/host/repo/base/PR, local or host:path, windows+keepAlive/offline error, session, created/opened. |
| PR `MINE` | Scope selected repo or active-context repos; tab counts/loading/fetch age. Row local presence glyph, number, title, optional branch/repo, derived state, update age. Detail target, additions/deletions, worktree path/session or proposed destination+pull ref/fork, checks/review/labels/age/url. |
| PR `REVIEW` | Same, excludes keys also in mine; includes author when width permits. |
| Startup | Bordered `swarm`, `Loading workspace…` before runtime/full App import. |

PR columns: number 6; title flexible; author 12 at width≥70/16 at≥130; branch at≥90; repo prefix at≥110; state 8; time 7. Worktree running 0/10/14/18 by width; PR badge 15.

### Dialogs and keys

| Dialog | Data | Exact keys |
| --- | --- | --- |
| Create | branch+slug preview; optional sorted host; fuzzy base (6); fetching indicator; initial bases default + old baseRef/origin branch; free text allowed | Esc; Tab/S-Tab fields; Enter; ↓/Ctrl-N, ↑/Ctrl-P base; host ←/→. |
| Clone | 150 ms debounce, 8 remote results: privacy/fullName/description/age | Esc; ↓/Ctrl-N, ↑/Ctrl-P; Enter. Text consumes j/k; close/query change aborts search. |
| Confirm | danger/body | Enter/y confirm; Esc/n/q cancel. |
| Context | name/id preview/comma owners | Esc; Tab/S-Tab; Enter. |
| Assign | context name/owners/current | Esc; j/k, arrows, Ctrl-N/P; Enter. |
| Settings | editable agent/command, sleep toggle/rule toggles; read-only grace/windows/hosts/protocol | Esc; arrows/Ctrl-N/P (j/k except command input); Space; agent ←/→; Enter save. |
| Palette | fuzzy first 10 valid commands + contexts, label/keys | Esc; arrows/Ctrl-N/P; Enter. Text consumes j/k. |
| Help | normal/PR/filter/dialog/tmux keys + version | Esc/Enter/q/? close. |

### Complete main normal keymap

| Key | Action |
| --- | --- |
| `j`/`k`, `↓`/`↑` | move |
| `gg`/`G` | top/bottom |
| `ctrl-d`/`ctrl-u` | half page |
| `h`,`←`,`S-Tab` / `l`,`→`,`Tab` | repos/worktrees focus |
| `Enter`,`o` | repo→worktrees; worktree open+quit |
| `O` | open+quit, keep prior |
| `n` | clone dialog in repos; create in worktrees |
| `N` | new context |
| `d` / `D` | delete selected / active context |
| `s` / `K` | sleep / kill confirmation |
| `m` | move repo |
| `r` / `U` | refresh / update-restart |
| `/` / `:` / `,` | filter / palette / settings |
| `gt`/`gT`, `1`–`9` | next/previous/nth context |
| `b` | browser PR if present |
| `y` | copy local path or `host:path` |
| `p` | PR screen (code/help; absent KEYMAP.md) |
| `?` | help |
| `q` | quit |
| `Esc` | clear retained filter, else quit |
| `ctrl-c` | global quit |

### Complete PR normal keymap

| Key | Action |
| --- | --- |
| navigation keys above | move/top/bottom/half-page |
| `Tab`,`l`,`→` / `S-Tab`,`h`,`←` | next/previous tab (both toggle) |
| `Enter`,`o` / `O` | open/create PR worktree, sleeping/keeping previous |
| `b` / `y` | browser / copy URL |
| `r` | force both tabs |
| `/`,`:`,`,`,`?`,`U` | filter/palette/settings/help/update |
| `gt`/`gT`,`1`–`9` | context/rescope |
| `p`,`q` | back main |
| `Esc` | clear PR filter else back |
| `ctrl-c` | quit popup |

Filter mode: printable/Backspace edit real input; Ctrl-N/↓ and Ctrl-P/↑ select; Enter opens; first Esc exits input retaining filter, second clears; Ctrl-C quits.

### UI async/progress/cancellation pain point

| Behavior | Current code |
| --- | --- |
| Progress | `runOperation` covers creates/deletes/clone launch/hot prep/post-hooks/PR create/update. 16 ms log batching, last 200 lines; only latest operation in footer. |
| Background | startup reconcile/status/remote sync/pool fill/PR loads; timers; clone poll; create-dialog prefetch; detached clone/hooks/remove. UI keyboard keeps rendering. |
| No operation UI | refresh, PR refresh, open, sleep, kill execution, context switch/tab loads, clipboard/browser/status sync. They can overlap without duplicate suppression. |
| Popup close | `dispose()` aborts controller hot prep, periodic/create-dialog refresh; `Shell.run` SIGTERM then SIGKILL after 500 ms; releases unref'ed polls. |
| Survives close | detached clone, hot worker, post-create runner, trash remove; ignored stdin/new process group/file logs/unref. |
| Dialog close | clone search aborts; create-dialog refresh does **not** abort, stale generation only discards UI result. |
| Hazard | foreground create/delete/open/update/remote calls lack lifecycle signals. Process exit can interrupt them; pre-marker `.creating-<uuid>` is skipped by startup recovery. |
| Staleness | any active Operation pauses local status timer; long post-create hook freezes local status indicators. Remote timers continue. |

## 6. Background job model

| Job | Scheduling/concurrency | Lock/persistence | Cancellation/completion | Logging |
| --- | --- | --- | --- | --- |
| Repo clone | Persist starting job; detached clone; controller reconcile startup/immediate/every 2 s; CLI create polls 100 ms; no global clone limit | unique staging; rename then Repo append/job remove | survives popup; live PID pending; dead valid `.git` promotes; invalid fails/cleans | unique clone raw log + structured result |
| Pool startup fill | `prepareReposWithLimit(...,2)` = max 2 repo workers; each queues `max(1,hotPoolSize)` sequential attempts | per-repo mutex + staging/PID | background signal cancels launch-side work/poll; detached worker survives; same-repo calls share promise | operation + structured + raw slot log |
| Claim refill | claim event queues one; fallback queues pool size; counts coalesce; one task loop/repo | same | same | success toast hidden; footer operation |
| Prepared refresh | one queue/repo: active skip/forced; skip joins active; forced behind skip creates one queued forced; slots sequential | repo mutex | interest-set AbortController; caller abort detaches; shared cancels only last; `awaitPendingRefresh` includes queued forced | warnings |
| Periodic refresh | interval when pool/interval nonzero; one global in-flight; repos sequential, skip-if-fresh | same | popup abort | warnings |
| Local status | `max(500,statusRefreshMs)`; one in-flight; timer skips if busy/any operation | read-only | no signal; no piling | best-effort errors |
| Remote status | one timer and in-flight per host, `max(500,remoteStatusRefreshMs)` | mirror read | 30 s; failure → unknown | dedup host error |
| PR fetch | tabs concurrent; prior same repo/tab aborted; global GitHub concurrency 4; cache first | memory slices + atomic cache | same-key generation cancel; no controller-wide dispose | 120-char error, toast once |
| Inspect | repos fetch concurrent; worktrees concurrent; host groups concurrent | read-only | no CLI signal; remote 30 s | warning/error fields |
| Prune/delete | inspect once, deletes sequential; trash removal detached | state lock across kill/rename | no signal | per-result + structured |
| Post-create | one detached runner/worktree; hooks sequential; TUI tracks exit, CLI awaits | registration first; temp records | survives popup; no cancel; unref child means promise doesn't hold loop | raw `swarm.log` + parsed steps |

Locking:

- `state.json.lock`: full cross-process load-modify-save, 3 s timeout; in-process save chain; fallback test ports use promise chain.
- `repoMutexes[RepoId]`: claims, preparation launch, slot refresh, base refresh/reset, fallback clone. Private post-claim Git/hook work and detached worker do not hold it.
- `deletingRepos`: blocks new work; aborts preparation/refresh, waits promises/workers, removes current/historical slot/staging/PID under mutex, then cascade.
- `inFlightTargets`: only `runOperation`; duplicate shows info unless disabled.

## 7. External tool integration

### Environment/runtime

| Variable | Behavior |
| --- | --- |
| `SWARM_HOME` | storage root, default `$HOME/.swarm`; launcher accepts empty-but-set |
| `HOME` | fallback, tilde expansion, nvm/nodenv discovery |
| `SWARM_ROOT` | tmux checkout; empty → `$HOME/buk/swarm` |
| `SWARM_INSTALL_ROOT` | launcher exports repo root; updater root override |
| `SWARM_NODE` | existing executable wins without version probe |
| `SWARM_CWD` | original PWD exported; TypeScript does not read it |
| `SWARM_STARTUP_PROFILE` | output timing JSON path |
| `TMUX` | in-tmux/socket; agent parses `socket,pid,index`, uses `-S`, deletes from nested env |
| `NVM_DIR` | updater, else `$HOME/.nvm` |
| `PATH` | Node discovery; updater prepends pinned bin; popup does not source interactive startup |
| locale vars | absent non-UTF8 → tmux `LC_CTYPE=C.UTF-8` |

Launcher:

- Built: `<node> --experimental-ffi --no-warnings=ExperimentalWarning dist/swarm.mjs "$@"`.
- Source: `<node> --experimental-ffi --no-warnings=ExperimentalWarning --import tsx src/main.ts "$@"`.
- Exit 75: restore original cwd and `exec "$0" "$@"`.
- Node order: SWARM_NODE; cached executable; compatible version-visible PATH nvm/nodenv; highest compatible installed nvm/nodenv; opaque PATH after probe. Minimum 26.4.

### Exact Git commands

| Purpose | Invocation |
| --- | --- |
| clone | `git clone --progress <url> <staging>` detached |
| fetch | `git fetch [--prune] origin` |
| refs | `git fetch <remote> <refs...>`; branch refspec `+refs/heads/<b>:refs/remotes/origin/<b>` |
| origin HEAD | `git symbolic-ref refs/remotes/origin/HEAD`; repair `git remote set-head origin --auto` |
| remote exists/list | `git show-ref --verify --quiet refs/remotes/origin/<b>`; `git for-each-ref --format=%(refname:short) refs/remotes/origin` |
| reset | `git checkout -B <b> origin/<b>`; `git reset --hard origin/<b>`; `git clean -fd` |
| checkout | `git checkout -b <b> <from>`; `git checkout -B <b> <from>`; `git checkout <b>` |
| PR | `git fetch origin +refs/pull/<n>/head:refs/swarm/pulls/<n>/head` |
| revision/branch | `git rev-parse --verify <ref>`; existence adds `--quiet`; `git branch --show-current` |
| upstream | `git for-each-ref --format=%(upstream:short)%00%(upstream:track) refs/heads/<b>` |
| divergence/count | `git rev-list --left-right --count <upstream>...HEAD`; `git rev-list --count <target>..HEAD` |
| ancestry | `git merge-base --is-ancestor <ancestor> <descendant>` |
| dirty | `git status --porcelain --untracked-files=normal` |
| update | `git rev-parse --is-inside-work-tree`; `git branch --show-current`; `git status --porcelain`; `git pull --ff-only origin main` |
| build version | `git rev-parse --short HEAD` |

Default branch order: valid origin/HEAD; repair/reread; valid hint; main; master; first remote; hint even without remotes; local symbolic HEAD; main.

### Exact `gh` commands

| Purpose | Invocation |
| --- | --- |
| viewer | `gh api user --jq .login` |
| repos | `gh repo list <owner> --limit 1000 --json name,owner,nameWithOwner,description,sshUrl,isPrivate,updatedAt,defaultBranchRef` |
| open PR by branch | `gh pr list --repo <id> --state open --head <branch> --limit 1 --json <PR_FIELDS>` |
| latest inspect PR | `gh pr list --repo <id> --head <branch> --state all --limit 100 --json number,state,url,baseRefName,headRefOid,updatedAt` |
| mine | `gh pr list --repo <id> --state open --limit 100 --json <PR_FIELDS> --author @me` |
| review | same, `--search user-review-requested:@me` |
| doctor | `gh auth status` |

`PR_FIELDS` exactly: `number,title,url,author,headRefName,baseRefName,isDraft,isCrossRepository,headRepository,headRepositoryOwner,reviewDecision,statusCheckRollup,additions,deletions,labels,updatedAt`.

### Other tools

| Tool | Exact integration |
| --- | --- |
| SSH batch | `ssh -o BatchMode=yes -o ConnectTimeout=5 -o ControlMaster=auto -o ControlPath=<home>/cache/ssh/%C -o ControlPersist=120 -- <dest> <single POSIX-quoted remote command>`; embedded `'` becomes `'"'"'` |
| SSH proxy | interactive `ssh -t` with common options (no BatchMode), `-- <dest> <swarmCommand> open '<id>'`, wrapped by `sh -c` failure/read prompt |
| tmux | all exact commands in section 4 |
| nvim/vim | default `nvim .`; sleep detects `vim`/`nvim`, sends `Escape :qa Enter`; resurrect nvim strategy `session` |
| lazygit | default `lazygit`; resurrect list |
| claude | default `claude`; cc/agent popup; keep regex `(^\|/)claude( \|$)` |
| opencode | default `opencode`; agent popup; regex `(^\|/)opencode( \|$)` |
| codex | not launched; sleep-only regex `(^\|/)codex( \|$)` |
| agent command | returned as one-element `[command]`, sent as one tmux key-string + Enter for pane shell parsing; nested attach is `tmux [-S <socket parsed from TMUX>] attach-session -t swarm-agent-<agent>` with `TMUX` deleted from the child environment |
| process/port | `ps -axo pid=,ppid=,command=`; `lsof -nP -iTCP -sTCP:LISTEN -a -p <pids> -F pn` |
| clipboard | mac `pbcopy`; else `which wl-copy` then `wl-copy`, fallback `xclip -selection clipboard` |
| browser | detached `open <github-url>` mac, `xdg-open` other; only HTTPS github.com |
| Node install | `bash -c '. "$NVM_DIR/nvm.sh" && nvm install'` at install root |
| package/build | absolute pinned `npm ci` if lock else `npm install`; then `npm run build` |
| remove | detached `rm -rf <validated descendant>` |

Remote list/status/inspect/delete/kill/sleep timeout 30,000 ms; create unbounded. Exit 255/124 = unreachable. Protocol must equal 1.

## 8. Testing approach in src/testing

Ports/adapters architecture: core types/interfaces/pure helpers → adapters/services → controller → UI; UI imports core, never adapters/services; `runtime.ts`/`main.ts` compose.

| Support | Model/capture |
| --- | --- |
| `fakeShell.ts` | predicate rules, ShellResults, exact normal/detached/logged/exec calls; unmatched 127 |
| `fakeFiles.ts` | absolute in-memory path/text prefix clone/move/remove, synthetic hot publication, calls/removals |
| `fakeGit.ts` | branches/default/revisions/dirty/upstream/divergence/ancestry + calls |
| `fakeGithub.ts` | repos/PR/cache/failures + calls |
| `fakeTmux.ts` | mutable sessions/windows/panes/attachment/keys/options/kills + calls; final window removes session |
| `fakeProcess.ts` | snapshot/descendants/ports/liveness/opened URLs |
| `fakeRemoteHost.ts` | rule results/errors + exact host/args/options |
| `fakeClipboard.ts` | copied strings |
| `fakeUpdater.ts` | roots and steps |
| `fakeController.ts` | UI store mutations, timed ops, captured create/config/yank/browse/prefetch |
| `memoryState.ts` | structured clones, AsyncLocalStorage nested transactions, serialized chain, snapshots |
| `memoryConfig.ts` | cloned config/save history |
| `fixedClock.ts` | deterministic now/manual intervals |
| `nullLogger.ts` | `{level,scope,message,data?}` capture/scopes |
| `fixtures.ts` | canonical entities/builders |

- Adapter tests inject FakeShell and assert exact argv/cwd/env; filesystem/state use temp dirs where atomicity matters.
- Service tests assert domain result and side-effect order. Worktree coverage includes concurrency, cancellation, recovery, stale PIDs, slot ordering, ref fallback, hooks, rollback, remote.
- `worktrees.integration.test.ts` is real filesystem/Git seam.
- Reducer/selectors/keymap are pure-tested; controller separately orchestrated; UI screen/OpenTUI uses FakeController; formatting/layout pure tests.
- CLI tests exact parser/envelopes/human output/idempotency/exit/remote/doctor.
- Commands: `node --experimental-ffi --import tsx --test "src/**/*.test.ts" "src/**/*.test.tsx"`; `tsc --noEmit`; Biome.
- Rust should preserve traits equivalent to Shell, Files, Git, Github, Tmux, Process, State, Config, Clipboard, RemoteHost, Clock, Updater, Lifecycle and deterministic fake call logs.

## 9. Gaps and pain points noticed in the code

- README config omits `github.prTtlSeconds:90`; architecture App excerpt predates PR state/actions; KEYMAP.md omits code-defined `p` and PR-screen keys.
- Generic docs imply Tab in every dialog; only create/context use it. Text-field lists use arrows/Ctrl-N/P, not j/k.
- TUI `d` is unconditional with only tmux summary; safe inspect/prune exists CLI-only. No TUI safety view.
- Popup quit lacks operation cancellation handshake for create/delete/open/update/remote. Pre-marker `.creating-<uuid>` interrupted early is skipped forever by recovery; no stale-attempt cleanup.
- Create-dialog close does not cancel forced fetch; generation only discards result.
- Any Operation pauses local status; long post-create hook freezes indicators. Only latest concurrent op appears in footer; `hot-copy:<repo>` target does not match repo row.
- Refresh/PR refresh/open/sleep/kill/context/clipboard/browser/host sync have no Operation progress or duplicate suppression.
- Schema permits `windows:[]` (mount fails later) and negative `sleep.graceMs` (clamped silently).
- Settings cannot edit grace/rule definitions/windows/hosts/protocol/pool/timers/status intervals; many require JSON.
- Context update/edit dialog exists but no normal binding opens edit.
- Hook failures warn only; no persisted degraded fact despite “ready”.
- Repo discovery cap 1000, PR cap 100; no pagination.
- Remote create has no timeout; can hang indefinitely.
- Non-mac doctor requires reflink, but non-Linux/non-mac adapter uses plain `cp -R`.
- Local status failures can show existing session as `none`; only remote failure uses safer `unknown`.
- Existing sessions repaired only by window name; wrong cwd/command, duplicates, extras remain.
- State lock timeout 3 s, but destructive transactions await tmux/renames/cascades while holding it; concurrent writers may time out.
- `list --json` does not reconcile clone jobs or sync hosts, so agent-driving view can be stale.
