# Fleet UX proposal — lens: **background awareness and safety**

> Scope of this lens: make background jobs, session liveness, keep-alive processes and
> destructive actions legible and safe, without nagging. Every placement decision below is
> argued from that lens; where another lens would legitimately choose differently it is noted
> in **Open questions**.
>
> References: `docs/ARCHITECTURE.md`, `docs/KEYMAP.md`, `docs/SWARM-INVENTORY.md` (cited as
> §1 domain model, §3 operations, §4 sessions/sleep, §5 TUI, §6 jobs, §9 pain points).

---

## 0. Measurement conventions used throughout

| Thing | Value |
| --- | --- |
| Base grid | 4 px |
| UI font | SF Pro Text 13 px / line 18 px |
| Mono font (terminal, paths, branches, log tails) | SF Mono 12.5 px → cell **7.5 × 18 px**; `1 ch = 7.5 px` |
| Default window | 1280 × 800 logical px; minimum 960 × 600 |
| Row height, all lists | 26 px (icon 16 px, 5 px vertical padding) |
| Icon set | Lucide, 16 px stroke 1.5 (14 px inside chips, 12 px inside status bar) |

Widths below are given in px, with the character equivalent in parentheses where the content
is monospaced, so the swarm width breakpoints in §5 (running 0/10/14/18 chars, PR badge 15,
title flexible, state 8, time 7) survive the port.

---

## 1. Design principles derived from the lens

1. **A job is never owned by a surface.** Anything that can take >300 ms is a daemon `Job`
   with an id, a log path and a cancel token (`ARCHITECTURE.md` §Jobs). The UI only ever
   *observes* it. Closing a dialog, leaving a screen, or quitting the app never cancels it, and
   the UI must say so at the exact moment the user would fear otherwise (dialog close, quit).
   This directly retires §9's "popup quit lacks operation cancellation handshake" and
   "create-dialog close does not cancel forced fetch, stale generation only discards result".
2. **A job is shown where its result lands, and counted once globally.** Inline progress on the
   row that will change (repo row cloning, worktree row being created, PR tab header
   refreshing) + a single `⟳ n` counter in the top-right chrome. Never a modal progress
   dialog, never a second copy of the same progress in the status bar. This retires §5's
   "only the latest operation appears in the footer" and §6's "`hot-copy:<repo>` target does
   not match the repo row".
3. **Absence of information is its own state and never renders as good news.** `unknown`
   (observation failed, §1 `SessionState`) gets an amber `circle-help`, never the dim glyph
   used for `none`; a fact the daemon could not compute (`ahead: null`, `uniqueCommits: null`,
   warnings `gh unavailable` / `fetch failed`) prints as `—` plus the warning, never as `0`.
   Confirmations escalate (`y` → `Y`) when a decisive fact is unknown.
4. **Confirmation cost is proportional to what is actually at risk.** The same `d` shows a
   one-line confirm when the inspection says clean + merged + no session, and a facts panel
   when it does not. The dialog states facts (§1 `WorktreeInspection`, §4 keep-alive labels),
   never adjectives, and always stamps their freshness. Safety comes from the *facts*, not
   from friction.
5. **Errors are sticky and actionable; successes are transient.** A failed job stays in the
   Jobs panel with `R retry` and `y copy log path` and turns the global counter red until seen.
   Successes get at most a 3.2 s toast, and only when the row that changed is not on screen.
   Nothing blinks, nothing re-announces itself, nothing asks to be dismissed twice.

---

## 2. Global layout and persistent chrome

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ ●1 buk    2 personal    3 oss                                  ⟳2   ⚑3   ◍           │ 34 px  context bar
├───────────────┬──────────────────────────────────────────┬───────────────────────────┤
│ REPOS      12 │ WORKTREES   buk/payroll             8    │ DETAIL                    │ 26 px  pane headers
│               │                                          │                           │
│ ▸ All      23 │ ◉ feat-payroll-fix  ⚙claude ⌁:3000 #128✓ │ branch  feat-payroll-fix  │
│ ▸ payroll   8 │ ○ fix-tax-rounding                   2h  │ base    origin/main       │
│ ⟳ ledger    · │ ☾ spike-cache                        1d  │ path    ~/.fleet/wt/…     │
│ ▸ web       5 │ ? api-poc  @devbox                   3d  │ session attached · 3 term │
│               │ ⟳ new-slug  copying files…               │ checked 14s ago           │
│    240 px     │        flex, min 420 px                  │        360 px  (i)        │
├───────────────┴──────────────────────────────────────────┴───────────────────────────┤
│ buk › worktrees  ⟳ clone ledger 61%        n new  d delete  J jobs            ◍ 12ms │ 24 px  status bar
└──────────────────────────────────────────────────────────────────────────────────────┘
   Proportions at 1280 px with detail open: 240 / 676 / 360 (19% / 53% / 28%).
   Detail closed (`i`): 240 / 1036. Below 1100 px the detail panel becomes an overlay
   on the right half (min 320 px) so the worktree list never drops under 420 px.
```

### Persistent chrome

| What | Where | Why here |
| --- | --- | --- |
| Context tabs `●1 buk  2 personal` | Context bar, left, 34 px tall | Contexts scope *everything* below (repos, PR scope, §3 cascade delete). Top-left = first read, and `1`–`9`/`gt` are already muscle memory. The dot marks the active one so the number is not the only signal. |
| Jobs counter `⟳ 2` | Context bar, right, 56 px slot | One global truth for "is something happening" (principle 2). Top-right is the classic ambient-status corner, far from the cursor so it never competes with the list. Turns `⚠ 1` red-amber when a job failed and unseen. Hidden entirely at 0 running / 0 unseen failures — no spam. |
| Review counter `⚑ 3` | Context bar, right of jobs | §5 header already carries a review count; it is a *pull* signal (work waiting on the user), so it belongs with the other ambient counters, not in the PR screen only. |
| Daemon dot `◍` | Context bar, far right, 8 px dot | Liveness of the thing that owns all background work. Always present but dot-only when healthy (green), expands to `◍ fleetd reconnecting 3s` amber / `◍ fleetd down` red. A safety lens requires a persistent indicator; the no-spam rule limits it to 8 px until it matters. |
| Breadcrumb `buk › worktrees` | Status bar, left | Answers "where am I / what will `n` create" in one glance; mode (`FILTER`, `SCROLL`) replaces it when modal. |
| Active-job ticker `⟳ clone ledger 61%` | Status bar, center-left | The one line of *detail* about background work when its row is off-screen. Single slot, newest job wins, with `+2` suffix if more are running; the counter in the corner remains authoritative. |
| Contextual key hints | Status bar, right of center | Discoverability without a permanent cheat sheet; only the 3 keys that act on the current selection. |
| Toast stack | Above status bar, right, 320 px wide, max 3, 3.2 s | §5 footer priority already caps toasts at 3 × 3.2 s. Right-aligned so it never covers the list cursor (left) or the detail values. |
| Sticky error slot | Status bar, replaces ticker, red, until `!` or `Esc` | Errors must not expire (principle 5). Retires §9 "PR fetch error toast once". |

---

## 3. Screens

### 3.1 Hub — Repos rail

**Purpose:** "Which repo am I working in, and is anything happening to it right now?"

```
┌ REPOS                 12 ┐   240 px
│ ▸ All                 23 │   16 icon | 8 | name flex | count 3ch=22 | glyph 16 | 8
│ ◉ payroll              8 │
│ ⟳ ledger      cloning  · │   ← clone job inline, count replaced by spinner
│ ✕ old-api      failed  · │   ← failed clone stays until dismissed
│ ☾ web                  5 │
└──────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Pane title + total | `REPOS` + repo count right-aligned | Header row, 26 px | Count is a scope fact, not a row; keeping it in the header saves a row per repo | §5 Repos: "`REPOS` count" |
| Aggregate session glyph | Worst-of the repo's worktree states (`unknown` > `attached` > `detached` > `sleeping` > `none`) | Row, left, 16 px | Leftmost = scanned first; the rail's job under this lens is "where is live work?" | §5 "aggregate session glyph"; §1 `SessionState` |
| Repo name | `name`, disambiguated to `owner/name` only on collision | Row, flex, ellipsis-middle | Names are what the user thinks in; owner only when ambiguous | §5 "disambiguated owner/name" |
| Worktree count | integer, right-aligned 22 px (3 ch) | Row, right | Right edge = numeric column, easy vertical scan | §5 "worktree count" |
| Clone-job state | `⟳ cloning 61%` / `✕ failed` replacing the count | Row, right, replaces count | The count is meaningless while the repo does not exist yet; reusing the slot avoids a second column | §1 `CloneJob.status`; §6 Repo clone |
| `All` pseudo-repo | pinned first, total worktrees | Row 1 | Default scope (§5) and the fastest path to "everything live" | §5 "All pseudo-repo default" |

**Intentionally omitted:** clone URL, default branch, path, hook lists, `clonedAt` (all in the
detail panel — the rail is a switcher, not a record); per-repo prepared-pool state (moves to
detail, it is never actionable from the rail); progress bars (a 3-digit percent in 22 px beats
a bar that cannot show a number).

**States:** *empty* → `No repos in <context>.` + dim `n clone one` (verbatim §5). *loading*
(startup reconcile) → rail renders immediately from `state.json`, no skeleton; the ticker says
`⟳ reconciling`. *clone running* → row present from the moment the `CloneJob` is persisted
(§6 "persist starting job"), spinner + percent. *clone failed* → `✕` + `failed`; row is
selectable; `Enter` opens the Jobs panel focused on that job, `d` dismisses the failed clone
(§1 `CloneJob.error`). *deleting* → row dims to 40 % with `⟳ deleting`, stays until the state
transaction commits (§3 delete: rename-then-detached-`rm -rf`).

**Icons:** `folder-git-2` (repo, only in detail header), session glyphs per §3.9,
`loader-circle` (spin, clone), `circle-x` (clone failed), `zap` (prepared copies ready, detail
only).

**Keyboard:** `j/k` move · `Enter`/`o`/`l` select → focus worktrees · `n` clone · `d` delete
(confirm, cascades) · `m` move to context · `i` toggle detail · `J` jobs.

---

### 3.2 Hub — Worktrees list

**Purpose:** "Which of my copies is alive, which has work running in it, and which is safe to
throw away?"

```
 WORKTREES   buk/payroll                              8       ← header: scope, filter, count
┌───────────────────────────────────────────────────────────┐  676 px flex
│ ◉ feat-payroll-fix        ⚙claude ⌁:3000   #128 ci ✓   2m │
│ ○ fix-tax-rounding                                     2h │
│ ☾ spike-cache             ✎ unsaved                    1d │
│ ? api-poc  @devbox                        offline      3d │
│ ⟳ new-slug                copying files…                  │
│ ▲ hooks-failed            ⚠ hooks failed               5m │
└───────────────────────────────────────────────────────────┘
  16   flex(min 180)        running 0/75/105/135 px   108   52
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session glyph | `◉ ○ ☾ · ?` (attached / detached / sleeping / none / unknown) | Row, col 0, 16 px | The single most safety-relevant bit; left edge, aligned down the whole list for scanning | §1 `WorktreeStatus.session`, §5 "row session glyph/spinner" |
| Job spinner | replaces the glyph while a job targets this worktree | col 0 | Same column = one "what is this row doing" slot; no layout shift | §6 create/delete/hooks jobs |
| Repo prefix | `payroll/` shown only in `All` scope | col 1, prefix | Redundant when a repo is selected; costs 8 ch otherwise | §5 "optional repo prefix" |
| Branch / slug | `branch` (mono), ellipsis-middle | col 1, flex, min 180 px | The user's own name for the work; the widest, most-read field | §1 `Worktree.branch`; §5 row |
| Host chip | `@devbox` amber if the host is unreachable | after branch, 72 px | Adjacent to the name because it changes what every action means (remote delete/kill are RPCs) | §1 `Worktree.host`; §3 remote ops |
| Keep-alive chips | `⚙claude`, `⌁:3000`, `✎ unsaved` — max 3, `+n` overflow | right-of-center, 0 / 75 / 105 / 135 px by window width | These are exactly the things that make sleeping/killing/deleting destructive; they sit next to the state glyph they qualify | §4 sleep policy labels; §1 `WorktreeStatus.running` + `windows[].keepAlive` |
| Degraded chip | `⚠ hooks failed` | same slot, wins over keep-alive chips | A worktree that says "ready" but whose post-create hooks failed is a trap | §9 "Hook failures warn only; no persisted degraded fact despite 'ready'" |
| PR badge | `#128 ci ✓` — number + state icon, 108 px (15 ch) | right of chips | Ties the copy to review reality without opening the PR screen | §5 "optional `#n <state>` badge", badge width 15 |
| Age | relative `lastOpenedAt ?? createdAt`, 52 px (7 ch) | far right | Sorting/recency cue; right edge is the conventional timestamp gutter | §1 `Worktree.lastOpenedAt`; §5 "relative opened-or-created" |

**Intentionally omitted:** base ref, absolute path, window list, `ahead/behind`, dirty flag
(all in detail / in the delete confirm — the list is a chooser, and dirty state on every row
would nag on every row); a "safe to delete" column (it is a *derived judgement* that must be
re-computed with fresh facts at the moment of deletion, not cached in a list, §3 `inspect`);
progress percentages for create (phases are more honest than percentages: `copying files…`,
`checking out…`, `hooks 2/3`).

**States:**

| State | Rendering |
| --- | --- |
| Empty | `No worktrees for <repo> yet.` + `n create one` (verbatim §5) |
| Filtered empty | `Nothing matches “<filter>”.` (verbatim §5) |
| Loading | list renders from state instantly; the session column shows `?` for at most one poll interval, then real states — never a blank column |
| Job running (create) | `⟳` + phase text in the chips slot; row is already selectable and `Enter` opens it once the session exists (§3 create step 6: TUI can open before post-create hooks) |
| Job running (delete) | row dims to 40 %, `⟳ deleting`, non-selectable; disappears on state commit |
| attached | `circle-dot` accent-green + optional `1/3` terminal hint in detail |
| detached | `circle` blue — process alive, nobody watching |
| sleeping | `moon` slate — session kept only by keep-alive rules, or all terminals closed by the sleep policy |
| none | `dot` 30 % opacity — no session at all |
| unknown | `circle-help` amber + tooltip `status unavailable` (never grey) |
| Host offline | host chip amber + `offline` in the chips slot; session forced to `unknown`, never `none` | 
| Error (inspect) | `triangle-alert` amber prefix on the age column + the exact warning string in detail |

**Icons:** `circle-dot`, `circle`, `moon`, `dot`, `circle-help`, `loader-circle`, `bot`
(agent keep-alive), `server` (listening port), `file-pen-line` (unsaved editor),
`triangle-alert` (degraded), `git-pull-request` family (PR badge, §3.4), `cloud-off`
(unreachable host).

**Keyboard:** `j/k`, `gg/G`, `ctrl-d/u` · `Enter`/`o` open (sleeps previous) · `O` open
keeping previous · `n` create · `d` delete (facts confirm) · `x` prune repo (dry run) · `s`
sleep · `K` kill (confirm) · `I` inspect (job) · `y` copy path · `i` detail · `/` filter.

---

### 3.3 Hub — Detail panel

**Purpose:** "Everything I need before I act on this row — and how old that knowledge is."

```
┌ DETAIL ─────────────────────────── 360 px ┐
│ feat-payroll-fix                          │  title 15 px semibold
│ buk/payroll  ·  local                     │
│                                           │
│ branch    feat-payroll-fix                │  label col 72 px, value flex mono
│ base      origin/main                     │
│ path      ~/.fleet/worktrees/buk/…/feat…  │  y copies
│                                           │
│ SESSION                        ◉ attached │  section header 11 px caps
│  1 nvim    ✎ unsaved changes              │
│  2 cc      ⚙ claude                       │
│  3 lg      —                              │
│                                           │
│ SAFETY                    checked 14s ago │
│  dirty          12 files                  │
│  unique commits 2                         │
│  published      no                        │
│  PR            #128 open · ci pass        │
│  ⚠ gh unavailable                         │
└───────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Identity block | slug, `repoId`, `local`/`@host` | Top | Confirms the target of every key you are about to press | §1 `Worktree` |
| `path` | tilde-collapsed, mono, middle-ellipsis | Identity block | `y` copies it (§3 `path`); the value must be visible to trust the copy | §3 `path`, §5 detail |
| SESSION section | per-terminal `index name` + keep-alive label | Middle | Answers "what will I lose if I sleep/kill" *before* the confirm dialog opens | §1 `WorktreeStatus.windows[].keepAlive`; §4 policy |
| SAFETY section | `dirty`, `uniqueCommits`, `published`, `merged`, `ahead/behind`, PR | Bottom | Same five facts the delete/prune confirm will quote — the confirm is then never a surprise | §1 `WorktreeInspection`; §3 `inspect` |
| Freshness stamp | `checked 14s ago` (amber >2 min, red on error) | SAFETY header, right | A fact without an age is not a fact under this lens | §1 `WorktreeInspection.inspectedAt` |
| Warnings | exact strings: `fetch failed`, `gh unavailable`, `no upstream`, `upstream gone`, `ahead/behind unavailable`, `published status unavailable`, `target ref unavailable`, `target ref missing`, `unique commit count unavailable`, `target comparison failed`, `pull request head comparison failed` | Below SAFETY | Verbatim strings are greppable and match the CLI | §3 `inspect` step 6 |
| Null facts | `—` never `0` | value column | Principle 3 | §1 `ahead/behind/uniqueCommits: null` |

**Intentionally omitted:** `createdAt` in absolute form (relative in list is enough; absolute
on hover), raw hook command lists for worktrees (repo-level fact), full log tails (Jobs panel).

**States:** *never inspected* → SAFETY shows `not checked · I to check` and delete escalates to
`Y` (principle 3). *inspect running* → section header shows `⟳ checking…`, old values dimmed
but still readable (never blanked). *inspect error* → red `error: <message>` + `I retry`.
*repo selected* → panel switches to repo variant (name/owner/default branch/path/worktree
count/live count/prepare hooks/post-create hooks/url/prepared copies `1/1 ready · refreshed
2m ago`, §5 + §2 pool). *clone job selected* → status/path/log path/error + `Enter` to jobs.

**Icons:** `clock` (freshness), `file-diff` (dirty), `git-commit-horizontal` (unique commits),
`cloud-upload` (published), `git-merge` (merged), `triangle-alert` (warning), `circle-x`
(error), `zap` (prepared copies).

---

### 3.4 Pull requests screen (Mine / Review)

**Purpose:** "What needs me, and do I already have a copy of it running?"

```
 MINE  7          REVIEW  3                    fetched 42s ago  ⟳       ← tabs + freshness
┌────────────────────────────────────────────────────────────────────────┐
│ ◉ #128  Fix payroll rounding drift      feat-payroll-fix   ci ✓    2m  │
│ ·  #131  Bump ledger deps               deps/ledger        draft   1h  │
│ ○ #119  Cache spike                     spike-cache        changes 3h  │
│ ✕ ─ could not load review tab: gh: HTTP 502 …                      r  │
└────────────────────────────────────────────────────────────────────────┘
 20   44    flex (title)                  140 (≥900)   84     52
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Tab strip + counts | `MINE n` / `REVIEW n` | Top-left | Two disjoint queues (§5: review excludes keys in mine); counts are the "is there work" signal | §1 `PrTab`; §5 PR views |
| Freshness + spinner | `fetched 42s ago` / `⟳ refreshing` | Tab row, right | GitHub data is cached (`prTtlSeconds: 90`); an unstamped list invites acting on stale review state | §1 `PrRepoSlice.fetchedAt/loading`; §1 config `github.prTtlSeconds` |
| Local-presence glyph | session glyph if a worktree matches, `·` if not | col 0, 20 px | "Do I already have this checked out and running?" is the first question on a PR row | §5 "row local presence glyph"; §1 PR/worktree match rule |
| Number | `#128`, mono, 44 px (6 ch) | col 1 | §5 fixed width 6; stable left rail for scanning | §5 PR columns |
| Title | flex, ellipsis-end | col 2 | The only human-readable field | §1 `PullRequest.title` |
| Branch | `headRefName`, shown ≥900 px | col 3, 140 px | Ties the PR to the worktree that would be created | §5 branch at ≥90 ch |
| Author | 96 px, Review tab only, ≥1000 px | col 4 | Only meaningful when the PR is not yours | §5 "includes author when width permits" |
| State | icon + word, 84 px (8 ch) | col 5 | Derived priority state: draft → ci fail → changes → ci pending → approved → review | §1 `PrState` priority |
| Age | `updatedAt` relative, 52 px (7 ch) | col 6 | §5 time 7 | §1 `PullRequest.updatedAt` |

**Intentionally omitted:** additions/deletions, labels, base branch, checks/review breakdown,
URL (all in the detail panel, §5 PR detail); avatars (no signal, costs a column); merge
buttons (Fleet never mutates GitHub).

**States:** *loading with cache* → old rows stay, header shows `⟳ refreshing · fetched 4m ago`
— stale-with-a-stamp beats a spinner over an empty list. *loading cold* → 6 skeleton rows.
*error* → a single sticky row `✕ <error, 120 chars> · r retry` at the top of the affected tab,
cached rows below it still usable (§6: "120-char error, toast once" becomes a persistent row).
*empty* → `No open PRs authored by you in <scope>.` / `No PRs waiting for your review in
<scope>.` (verbatim §5). *creating a worktree from a PR* → the row's presence glyph becomes
`⟳` and stays until the create job publishes; leaving the screen does not stop it.

**Icons:** `git-pull-request-draft` (draft, slate), `circle-x` (ci fail, red),
`message-square-warning` (changes requested, amber), `clock` (ci pending, amber),
`circle-check` (approved, green), `git-pull-request` (review, blue), `git-merge` (merged, in
detail only), plus the session glyph set for local presence.

**Keyboard:** `Tab`/`S-Tab`, `h/l` switch tab · `Enter`/`o` open-or-create the worktree · `O`
same, keeping previous awake · `b` browser · `y` copy URL · `r` force refresh both tabs (job)
· `p`/`q` back.

---

### 3.5 Workspace

**Purpose:** "Work in the terminal, while never losing track of what is running here and
elsewhere."

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ◉ payroll/feat-payroll-fix   ⌥feat-payroll-fix   ⚙claude ⌁:3000   ⟳1     z   │ 30 px header
├──────────────────────────────────────────────────────────────────────────────┤
│ 1 nvim ✎   2 cc ⚙   3 lg   4 test ✕1                                   +     │ 28 px tabs
├──────────────────────────────────────────────────────────────────────────────┤
│ $ pnpm test                                              ┌──────────────────┐│
│ …                                                        │ SCROLL  −240/12k ││ pill, top-right
│                                                          └──────────────────┘│
│                            terminal grid, 7.5 × 18 cells                     │
├──────────────────────────────────────────────────────────────────────────────┤
│ ⚠ process exited (1) · r restart · ctrl-s x close                            │ 22 px, only on exit
└──────────────────────────────────────────────────────────────────────────────┘
   ctrl-s pressed → replaces the bottom strip for 1 key:
│ ctrl-s  s hub · 1-9 tab · c new · x close · [ scroll · ] paste · a agent · J jobs │
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session glyph + name | `◉ payroll/feat-payroll-fix` | Header, left | Confirms *which* copy your keystrokes are hitting — the single highest-stakes fact in the app | §1 session naming; §1 `SessionState` |
| Branch chip | `⌥feat-payroll-fix` | Header, after name | Session name ≠ branch when slug and branch differ; typing `git push` deserves certainty | §1 `Worktree.branch` |
| Keep-alive chips | `⚙claude`, `⌁:3000`, `✎unsaved` | Header, center-right | The processes that will be *kept* by sleep and *killed* by kill; visible before you press either | §4 sleep policy |
| Jobs counter | `⟳1` | Header, right | Background work continues while you are in a terminal; the Hub chrome is not visible here, so the counter must be | ARCHITECTURE §Jobs |
| Zoom affordance | `z` hint | Header, far right | `ctrl-s z` hides header+tabs; the hint is the only way to discover the way back | KEYMAP `ctrl-s z` |
| Tab strip | `index name status` per terminal | Below header | Ordered, numbered, matching `ctrl-s 1-9`; §4 default layout `nvim｜cc｜lg` | §4 three-window layout; §1 `Terminal` |
| Per-tab status glyph | `⚙` running keep-alive, `✎` unsaved editor, `✕n` exited with code, nothing when idle | Inside tab, right of name | Answers "which tab is doing something" without switching to it | §1 `Terminal.foreground_command/status`; §4 keep-alive |
| Exit strip | `⚠ process exited (1) · r restart · ctrl-s x close` | Bottom, 22 px, only when the tab's command exited | tmux's `remain-on-exit` made this recoverable; Fleet must not silently swallow a crashed dev server | §4 `remain-on-exit on` |
| Scroll pill | `SCROLL −240/12 000` + `v select · y yank · Esc exit` on the second line while selecting | Terminal area, top-right, 176 × 22 px | Must be visible but must not cover the prompt (bottom-left) or shift the grid | §5 copy-mode; ARCHITECTURE `viewport{scrollback_len, offset}` |
| Prefix hint | one row of the 12 prefix keys | Bottom strip, replaces exit strip for one key | Discoverability exactly when needed, gone immediately after — the anti-nag form of a cheat sheet | KEYMAP Prefix mode |
| Disconnect veil | 60 % scrim + `◍ fleetd disconnected — reconnecting 3s` | Over the grid | The grid is a *mirror*; a frozen mirror that looks live is the worst possible failure | ARCHITECTURE client mirror; §3.11 |

**Intentionally omitted:** a permanent key cheat sheet (prefix hint covers it), a clock, CPU/mem
gauges, per-tab PIDs (detail/`ctrl-s ?`), breadcrumbs to the Hub (the session name *is* the
breadcrumb), a scrollbar (the pill gives position; a scrollbar would fight the alt-screen).

**States:** *attached* (this client) `circle-dot` green · *detached* (running, another client
or none attached) `circle` blue · *sleeping* header dims and shows `☾ slept · Enter to wake`
over the grid, with the kept terminals listed and the reason (`unsaved changes`, `claude`,
`:3000`, `sleep disabled`) per §4 step 6 · *unknown* `circle-help` amber + veil · *terminal
exited* exit strip · *job running* `⟳n` in header, `ctrl-s J` opens the panel over the terminal
without detaching it.

**Icons:** `circle-dot`/`circle`/`moon`/`circle-help` (session), `square-terminal` (new tab),
`bot` (agent), `server` (port), `file-pen-line` (unsaved), `circle-x` (exited), `arrow-up-down`
(scroll mode), `unplug` (disconnected), `maximize-2` (zoom hint).

**Keyboard:** all keys to the PTY except `ctrl-s`; then `s` hub · `1-9` tab · `h/l`, `p/n` tab
cycling · `c` new · `x` close (confirm if keep-alive) · `,` rename · `[` scroll · `]` paste ·
`a`/`A` agent session · `z` zoom · `J` jobs · `?` help · `Esc` cancel prefix.

---

### 3.6 Jobs panel

**Purpose:** "What is the daemon doing, what did it fail at, and what can I do about it?" —
the single place background work is fully legible.

```
        ┌ JOBS ───────────────────────────────────────────── 880 × 560 ────────┐
        │ ⟳ 2 running   ✕ 1 failed   ✓ 5 done             f filter   D dismiss │ 32 px
        ├──────────────────────────────┬───────────────────────────────────────┤
        │ ⟳ clone   buk/ledger   0:42 ▸│ ~/.fleet/logs/jobs/j-8f3c.log         │ 26 px
        │   Receiving objects 61%      │ remote: Enumerating objects: 128421    │
        │ ⟳ hooks   payroll#feat 0:08  │ Receiving objects:  61% (4.2/6.9 MiB)  │
        │   pnpm install (2/3)         │ ▌                                      │
        │ ✕ prs     review       1m    │                                        │
        │   gh: HTTP 502 upstream …    │   last 200 lines · follow on           │
        │ ✓ create  payroll#fix  12s   │                                        │
        │ ⊘ prune   buk/web      —     │                                        │
        │        320 px                │              flex 560 px               │
        ├──────────────────────────────┴───────────────────────────────────────┤
        │ Enter follow · c cancel · R retry · y copy log path · Esc close       │ 24 px
        └──────────────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Summary counts | running / failed / done | Header, left | The panel opens on `J` from anywhere; the header answers the question before the eyes reach the list | ARCHITECTURE `JobManager` |
| Job row line 1 | status icon · kind · target · elapsed | List, left column | Kind+target identify a job the way the user thinks of it ("the ledger clone"), not by id | §6 job table (clone, pool fill, refresh, status, PR fetch, inspect, prune/delete, post-create) |
| Job row line 2 | last progress line, 1 line, mono, dimmed | Under line 1 | §6 already keeps "last progress line"; two-line rows let the list stay scannable while showing live output | ARCHITECTURE `Job.last progress line` |
| Log tail | last 200 lines, follow-on, mono 12.5 px | Right pane | §5 "16 ms log batching, last 200 lines" — the existing batching budget maps 1:1 | §5 progress; ARCHITECTURE `logs/jobs/<id>.log` |
| Log path | `~/.fleet/logs/jobs/<id>.log` | Right pane header | Makes the failure survivable outside the app; `y` copies it | ARCHITECTURE FLEET_HOME layout |
| Cancel | `c` (only when `cancellable`) | Footer + per-row | Explicit cancel is the *only* thing that stops a job (guiding rule 1) | ARCHITECTURE cancellation tokens |
| Retry | `R` on failed jobs | Footer | Recoverable errors (principle 5); re-runs with identical parameters | §9 "no retry path" |
| Dismiss | `D` clears finished/failed rows | Header right | Bounded list; failures must be dismissed *deliberately*, never auto-expire | principle 5 |
| Filter | `f` cycles all → running → failed | Header right | A long-lived daemon accumulates jobs; a single-key cycle beats a dropdown | §6 many concurrent job kinds |

**Intentionally omitted:** job ids in the row (only in the log path and on `y`), queue
positions, per-job start timestamps (elapsed is what matters live; absolute on hover), a
progress bar (percent is inside the progress line when the tool emits one), pool-refresh
"skip" jobs (noise: they are logged, not listed, per §6 "skip-if-fresh").

**States:** `queued` `clock` slate · `running` `loader-circle` spin accent · `cancelling`
`circle-stop` amber · `cancelled` `circle-slash` slate · `succeeded` `circle-check` green,
auto-collapses to one line after 10 s and auto-dismisses after 60 s · `failed` `circle-x` red,
never auto-dismissed, keeps the global counter red until the panel is opened · *empty* →
`Nothing running. Fleet keeps jobs in the daemon, so they survive this window.` (a one-time
teaching line, this is the place for it).

**Keyboard:** `J`/`Esc` toggle · `j/k` select · `Enter` follow log (focus right pane, `j/k`
scrolls it, `G` re-enables follow) · `c` cancel · `X` cancel all cancellable (confirm) · `R`
retry · `y` copy log path · `D` dismiss finished · `f` filter · `gg/G`.

---

### 3.7 Dialog: Create worktree

**Purpose:** "Make a copy from *this* base, and tell me honestly how long it will take."

```
┌ New worktree — buk/payroll ─────────────────────── 640 × 380 ─┐
│ branch   feat-payroll-fix▌                                    │  field 1
│ slug     feat-payroll-fix          (auto)                     │  derived preview
│ host     ◂ local ▸                                            │  only if hosts configured
│                                                               │
│ base     origin/main▌                            ⟳ fetching   │  field 2 + fuzzy list
│   ▸ origin/main                                               │
│     origin/release-24                                         │
│     origin/feat-payroll-fix        (previous base)            │
│     … 3 more                                                  │
│                                                               │
│ ⚡ prepared copy ready — create takes ~2 s                     │  expectation line
│ hooks: pnpm install · pnpm build          (run in background) │
│                                                               │
│ Esc cancel · Tab field · Ctrl-N/P base · Enter create         │
└───────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| branch input | free text, validated live | Field 1 | It is what the user came to type | §1 `validateBranch` |
| Validation line | exact rule that failed, e.g. `branch cannot contain “..”` | Under branch, red | Fail before a job starts, not after | §1 branch rules |
| slug preview | derived, `(auto)`, editable on `Tab` | Field 2 | The slug becomes the path and the id; showing it prevents surprise ids | §1 slug rules; §5 Create dialog |
| host cycler | `◂ local ▸`, hidden when `hosts` is empty | Field 3 | Placement changes every later operation; hidden when it cannot vary | §1 `defaultHost`; §5 optional host |
| base fuzzy list | 6 rows, default + previous `baseRef` + origin branches | Below base input | §5 create dialog exactly | §5 Create; §3 create step 1 |
| Fetch indicator | `⟳ fetching` in the base row | Right of base label | The prefetch is a job; if you close the dialog it keeps running and the next open is instant (retires §9 "create-dialog close does not cancel forced fetch") | §9; guiding rule 1 |
| Expectation line | `⚡ prepared copy ready — create takes ~2 s` **or** `no prepared copy — first create copies the repo (~40 s), it runs in the background` | Above the key hints | Under this lens, the pool is invisible plumbing whose *only* user-visible consequence is latency; say the latency | §2 pool claim/fallback; §6 claim refill |
| Hooks preview | prepare + post-create commands, one line, `(run in background)` | Bottom | Post-create hooks run detached and can fail *after* the worktree looks ready; naming them here makes the later `⚠ hooks failed` chip intelligible | §3 create step 6; §9 hook failures |

**Intentionally omitted:** progress UI for the create itself (the dialog closes on `Enter`;
progress belongs on the row, principle 2), advanced git options, a "wait for hooks" checkbox.

**States:** *base list loading* → previous list stays + `⟳`. *validation error* → red line,
`Enter` blocked. *duplicate id* → `A worktree buk/payroll#feat-x already exists · Enter opens
it` (§3 create idempotency: existing id returns `created:false`). *submitted* → dialog closes
immediately, a `⟳ creating` row appears in the list; **on close, if a fetch or prefetch job is
still running, a 3.2 s toast says `base fetch still running · J`**.

**Keyboard:** `Tab`/`S-Tab` fields · `Ctrl-N`/`Ctrl-P` or `↓`/`↑` base list · `←`/`→` host ·
`Enter` create · `Esc` cancel (jobs keep running).

---

### 3.8 Dialog: Clone repo

```
┌ Clone repository ───────────────────────── 640 × 340 ─┐
│ search  payroll▌                            ⟳         │
│  ▸ 🔒 bukhr/payroll        Payroll core     2d        │
│    🔒 bukhr/payroll-api    …                3w        │
│    🌐 acme/payroll-ui      …                1y        │
│  … 5 more                                             │
│ context  buk                       protocol  ssh      │
│ Clones in the background — you can keep working.      │
│ Esc cancel · Ctrl-N/P · Enter clone                   │
└───────────────────────────────────────────────────────┘
```

Element highlights: privacy glyph (`lock` / `globe`) at col 0 (§5 "privacy/fullName/
description/age"); 8 results max, 150 ms debounce (§5); target context shown because a clone
is filed into a context and later cascades on context delete (§1 delete cascade); the
background line is the anti-anxiety statement that makes `Esc` safe (§6 "survives popup").

**States:** *searching* `⟳` in the input row; *no results* `Nothing matches “<query>”.`;
*rate-limited/error* red row with the gh error + `r retry`; *submitted* → dialog closes, repo
row appears immediately with `⟳ cloning` (§6 "persist starting job" before the child starts).

---

### 3.9 Dialog: Confirm (delete / prune / kill / close terminal / quit)

**Purpose:** "Show me exactly what I am about to lose, with the age of that knowledge."

Two sizes, chosen by facts — this is principle 4 made literal.

**Compact** (all decisive facts are known *and* benign):

```
┌───────────────────────────────────────────────────────┐
│ Delete buk/payroll#fix-tax-rounding?                  │
│ ✓ clean   ✓ merged into main   ✓ no session           │
│ checked 8s ago · trash, then removed in background    │
│                                    y delete · n cancel│
└───────────────────────────────────────────────────────┘
```

**Expanded** (any risk fact is true, or any decisive fact is unknown):

```
┌ Delete worktree ──────────────────────────── 560 px ─┐
│ buk/payroll#feat-payroll-fix                         │
│                                                      │
│ ⚠ 12 uncommitted files                               │
│ ⚠ 2 commits not on any remote                        │
│ ⚠ session attached · claude, server :3000            │
│ ✓ PR #128 open (not merged)                          │
│ ⚠ unique commit count unavailable (gh unavailable)   │
│                                                      │
│ checked 3m ago · I re-check                          │
│ Deleting kills the session and moves the copy to     │
│ trash; commits that exist only here are lost.        │
│                                                      │
│                     Y delete · n cancel · I re-check │
└──────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Target id | full `WorktreeId` | Title area | Deleting the wrong copy is the top failure mode | §1 `WorktreeId` |
| Facts list | one line each: dirty (file count), unique commits, published, merged/PR, session + running labels | Body, ⚠ risks first, ✓ safe after | Ordered by risk so the eye lands on the reason to stop | §1 `WorktreeInspection`; §3 `delete` is unconditional today |
| Unknown facts | `⚠ <exact warning string>` | With the facts | Principle 3; also explains why the key is `Y` | §3 inspect warnings |
| Freshness | `checked 3m ago` + `I re-check` | Above the consequence line | The confirm is only as good as the inspection behind it | §1 `inspectedAt` |
| Consequence sentence | plain future tense, states irreversibility and background-ness | Above keys | Users confirm the *sentence*, not the title | §3 delete algorithm (kill session → trash rename → detached rm) |
| Key row | `y`/`Y` confirm · `n` cancel · action key | Bottom right | Matches KEYMAP confirm convention | KEYMAP dialogs |

Escalation rule: `y` confirms when every decisive fact (`dirty`, `uniqueCommits`, `published`,
`session`) is known; **`Y` (shift) is required when any is unknown or the inspection errored**
— consistent with KEYMAP's "uppercase = stronger variant".

**Exact wording per action**

| Action | Title | Consequence line | Key |
| --- | --- | --- | --- |
| Delete worktree (clean) | `Delete buk/payroll#fix-tax-rounding?` | `Moves the copy to trash, then removes it in the background.` | `y` |
| Delete worktree (risky) | `Delete worktree` | `Deleting kills the session and moves the copy to trash; commits that exist only here are lost.` | `y` / `Y` |
| Delete repo | `Delete repository buk/payroll?` | `Also deletes 8 worktrees and their sessions. The base clone and every copy go to trash.` | `Y` |
| Delete context | `Delete context “buk”?` | `Also deletes 4 repositories, 23 worktrees and every session in them.` | `Y` |
| Prune | `Prune buk/payroll — 3 of 8 worktrees` | `Deletes the 3 listed below. The 5 skipped ones are kept, with the reason shown.` | `y` |
| Kill session | `Kill session payroll/feat-payroll-fix?` | `Kills 3 terminals at once. nvim has unsaved changes; claude and the server on :3000 are killed too. Nothing is saved.` | `Y` when unsaved/keep-alive present, else `y` |
| Sleep (no confirm) | — | toast: `slept · kept 1 window (unsaved changes)` | none |
| Close terminal | `Close terminal 2 “cc”?` | `claude is running in it and will be killed.` | `y` |
| Quit app | `Quit Fleet?` | `2 jobs and 3 sessions keep running in fleetd and will be here when you come back.` | `y` |
| Quit + stop daemon | `Stop fleetd and quit?` | `Kills 3 sessions (nvim in payroll/feat-payroll-fix has unsaved changes) and cancels 2 running jobs, including the clone of buk/ledger.` | `Y` |

**Prune dialog** deserves its own body, because it is a *multi-target* confirm:

```
┌ Prune buk/payroll — 3 of 8 ─────────────────────────── 720 px ─┐
│ DELETE                                                         │
│  ✓ fix-tax-rounding    merged · clean · no session             │
│  ✓ chore-deps          merged · clean · no session             │
│  ✓ spike-cache         merged · clean · slept                  │
│ KEEP                                                           │
│  ⚠ feat-payroll-fix    2 unique commits, not merged            │
│  ⚠ api-poc             session attached                        │
│  ⚠ hotfix-vat          12 uncommitted files                    │
│  ? devbox/ledger-sync  status unknown (host offline)           │
│  ⚠ old-spike           unique commit count unavailable         │
│ dry run · fetched 6s ago                y prune 3 · n cancel   │
└────────────────────────────────────────────────────────────────┘
```

Reasons are the verbatim skip reasons from §3 prune (error, dirty, attached/unknown session,
unknown unique count, unmerged, `tmux session has running commands: …` → reworded to
`session has running commands: claude, :3000`).

**Intentionally omitted from all confirms:** "Do not ask again" checkboxes (a confirm that can
be disabled per-action is a confirm nobody reads; the *compact* form is the real answer to
fatigue), a countdown/disabled-button delay, an undo *promise* the daemon cannot keep
(recovery is the real trash directory, see amendments).

---

### 3.10 Dialogs: Context, Assign, Settings, Help

**New / Edit context** (`480 × 220`): `name` → live `id` preview (`buk-payroll`, §1 slugify),
`owners` comma list (feeds PR scope). Delete lives *inside* Edit (`ctrl-d`, expanded confirm),
not on a top-level key — see amendments. Why: a context delete cascades repos + worktrees +
sessions (§1), the single most destructive action in Fleet, and today it sits on `D`, one
shift away from `d`.

**Assign repo to context** (`420 × 300`): list of contexts with `name`, owners, worktree
count, current one marked `●`. Footer: `Moves the repo record only — nothing on disk changes,
sessions keep running.` (Reassurance is warranted precisely because the action *sounds*
destructive; §3 TUI-only assign.)

**Settings** (`720 × 560`, sectioned, `Space` toggles, `←/→` cycles, `Enter` saves):

| Section | Fields | Why this lens cares |
| --- | --- | --- |
| Agent | `agent` (claude/opencode), `agentCommands.*` | §1 config; determines the `⚙` keep-alive chip |
| Sleep | `sleep.enabled`, `sleep.graceMs` (editable — §9 says it is not today), per-rule toggle with **live match count**: `claude — matching 2 processes now` | The sleep policy is the safety policy; a rule you cannot see matching is a rule you cannot trust (§4) |
| Keep-alive rules | id, label, kind, pattern (read-only pattern in v1), enabled | §1 `sleep.keepAlive[]` |
| Jobs & warnings | `warn before quitting with running jobs` (default on), `keep finished jobs for` (60 s) | KEYMAP: quit "with a confirm only if a job is running and the user asked to be warned" |
| Pool | `hotPoolSize`, `hotFreshnessMs`, `hotRefreshIntervalMs` + `prepared copies: 1/1 ready` | The pool is the create-latency story of §3.7 |
| GitHub | `cacheTtlSeconds`, `prTtlSeconds`, `cloneProtocol` | Explains PR freshness stamps (§3.4) |
| Read-only | `windows[]`, `hosts`, dirs, protocol version, daemon pid/uptime/socket | §5 read-only set + daemon liveness |

**Help** (`760 × 620`): the KEYMAP tables grouped by mode, plus a top block **"What keeps
running"**: `Jobs and sessions live in fleetd. Closing a dialog, leaving a screen or quitting
Fleet (ctrl-q) never stops them. Only c in the Jobs panel, K, and ctrl-shift-q stop things.`
Under this lens that paragraph is the single most valuable sentence in the app.

---

### 3.11 Command palette, Filter bar, Toasts

**Palette** (`:`, `640 × 420`, centered, top third): fuzzy over commands + contexts + repos +
worktrees + jobs. Rows: `icon · label · target · keys`. Jobs are searchable (`cancel clone
ledger`) so a background action is reachable without learning the panel. First 10 results
(§5). Destructive commands are prefixed with `triangle-alert` and still route through the
confirm dialog — the palette never bypasses a confirm.

**Filter bar** (`/`): replaces the pane header with `/ payroll▌   8 of 23`, mono, 26 px. Live
count is the feedback. First `Esc` exits input keeping the filter (header keeps showing
`/payroll  8 of 23` dimmed), second `Esc` clears — verbatim §5 semantics. Why in the header
and not the footer: the filter scopes the list *below* it, and swarm's footer is already the
job/error surface.

**Toasts**: bottom-right, 320 px, ≤3, 3.2 s, `icon · text · optional key`. Allowed contents:
background success whose row is off-screen (`✓ cloned buk/ledger · Enter opens`), clipboard
(`✓ path copied`), sleep report (`☾ slept · kept 1 window (unsaved changes)`), and the
dialog-close reassurance (`⟳ base fetch still running · J`). **Never** errors (they are sticky,
principle 5), never progress (that is the ticker/rows), never confirmations of things visible
on screen.

---

### 3.12 First run and empty states

```
┌──────────────────────────── first run ───────────────────────────┐
│                          ⛵  Fleet                                │
│         Copies, sessions and PRs — all owned by fleetd.          │
│                                                                  │
│   N   create your first context                                  │
│   n   clone a repository                                         │
│   ?   keymap                                                     │
│                                                                  │
│   ◍ fleetd running · 0.1.0 · ~/.fleet                            │
│   Found ~/.swarm — I import contexts, repos and worktrees        │
│   without touching them.                    i import · Esc skip  │
└──────────────────────────────────────────────────────────────────┘
```

Empty strings elsewhere are verbatim from §5: `no contexts`, `No contexts yet.` +
`N create your first context`, `No repos in <context>.` + `n clone one`,
`No worktrees [for <repo>] yet.`, `Nothing matches “<filter>”.`, `No open PRs authored by you
in <scope>.`, `No PRs waiting for your review in <scope>.` The import offer appears only when
`~/.swarm/state.json` exists (`fleet import --from-swarm`, ARCHITECTURE) and states
non-destructiveness explicitly.

---

### 3.13 Daemon-unavailable and degraded states

Three distinct situations, three distinct treatments — conflating them is the failure mode.

| Situation | Surface | Exact text | Recovery keys |
| --- | --- | --- | --- |
| **Cold start, daemon not yet up** | Full-screen, centered, no chrome | `Starting fleetd…` + spinner; after 3 s adds the socket path | none (auto-spawn, ARCHITECTURE) |
| **Daemon will not start** | Full-screen | `fleetd could not start.` / `<last 3 lines of ~/.fleet/logs/fleetd.log>` / `The socket ~/.fleet/fleetd.sock is stale.` | `r` retry · `L` open log · `D` run doctor · `ctrl-q` quit |
| **Daemon died while attached** | Sticky red top banner (34 px) + veil over terminal grids only | `◍ fleetd stopped — reconnecting (3s)`. On reconnect: `◍ fleetd restarted. Terminal sessions did not survive; worktrees, jobs and state are intact.` (PTYs do not survive a daemon restart, ARCHITECTURE §Sessions) | `r` reconnect now · `Esc` dismiss banner (dot stays amber) |

Lists remain readable from the last snapshot with every session glyph forced to `?` (`unknown`,
never `none`) and a `stale · <age>` stamp in the pane header — principle 3 at the app level.
`doctor` results (tmux/git/gh/cp/Node checks, §3 doctor) render as a compact `CHECK STATUS
DETAIL` table inside the same screen.

---

### 3.14 Quit with running work

`ctrl-q` (quit app, daemon keeps running) — confirm **only** if `warn before quitting with
running jobs` is on *and* something is running:

```
┌ Quit Fleet? ──────────────────────────────── 520 px ─┐
│ These keep running in fleetd:                        │
│   ⟳ clone buk/ledger            61%                  │
│   ⟳ hooks payroll#feat-payroll  pnpm install         │
│   ◉ 3 sessions, 7 terminals                          │
│ They will be here when you come back.                │
│                            y quit · n cancel · J jobs│
└──────────────────────────────────────────────────────┘
```

`ctrl-shift-q` (quit app **and** stop daemon) — always confirms, and inverts the tone because
everything listed dies:

```
┌ Stop fleetd and quit? ────────────────────── 560 px ─┐
│ ⚠ Killed now:                                        │
│   ◉ payroll/feat-payroll-fix   nvim ✎ unsaved,       │
│                                claude, server :3000  │
│   ○ web/spike-cache            2 terminals           │
│ ⚠ Cancelled now:                                     │
│   ⟳ clone buk/ledger  61%   (restartable)            │
│   ⟳ hooks payroll#feat-payroll-fix  (not restartable)│
│ Worktrees, repos and state on disk are untouched.    │
│                     Y stop and quit · n cancel       │
└──────────────────────────────────────────────────────┘
```

Cancellable vs. not is taken from the job's cancel token (ARCHITECTURE `JobManager`); a
detached post-create runner (§6 "no cancel") is labelled `not restartable` rather than pretending.

---

## 4. Keystroke counts — 10 most frequent tasks

"Min" = cursor already on the target. "Typical" = target visible in the same list, ≤3 rows away.
Text typed into inputs is counted separately as `+N chars`.

| # | Task | Sequence | Min | Typical |
| --- | --- | --- | --- | --- |
| 1 | Open worktree (Hub → Workspace) | `Enter` | **1** | `j j Enter` = 3 |
| 2 | Create worktree from a branch name | `n` `<branch>` `Enter` | **2** + chars | 2 + 14 chars |
| 3 | Create worktree from a PR | `p` `j` `Enter` | **2** (`p Enter`) | 3 |
| 4 | Switch between two sessions | `ctrl-s s` `k` `Enter` | 4 | 4 · **2 with proposed `ctrl-s Tab`** |
| 5 | Jump to terminal tab 3 | `ctrl-s 3` | **2** | 2 |
| 6a | Sleep current session (from Hub) | `s` | **1** | 3 (`j j s`) |
| 6b | Kill session | `K` `y` (`Y` if unsaved) | **2** | 4 |
| 7a | Delete worktree | `d` `y` | **2** | 4 |
| 7b | Prune a repo | `x` (review) `y` | **2** | 2 |
| 8 | Refresh PRs | `r` (on PR screen) | **1** | 2 (`p r`) |
| 9 | Check a background job | `J` (`ctrl-s J` from terminal) `Esc` | **2** | 3 from terminal |
| 10 | Copy worktree path | `y` | **1** | 3 |

Design consequence: every safety-relevant action is ≤2 keystrokes from its target, and the
confirm is exactly one of those two. Nothing safety-relevant is cheaper than a job check (`J`
= 1), which is the ratio this lens wants.

---

## 5. Proposed keymap amendments (KEYMAP.md not edited)

| # | Binding | Where | Proposal | Rationale (lens) |
| --- | --- | --- | --- | --- |
| A1 | `` ` `` | Normal, anywhere | Jump to the **last session** (alternate-file semantics from vim) | Task 4 drops 4 → 1; the most frequent "context switch" today costs a Hub round trip |
| A2 | `ctrl-s Tab` | Prefix | Same, from inside a terminal | Symmetric with A1; `ctrl-s s` remains "go to Hub" |
| A3 | `D` unbound at top level; context delete moves **inside** the Edit-context dialog (`E`, then `ctrl-d`) | Hub | `D` is one shift from `d`, and it cascades repos + worktrees + sessions (§1) | The most destructive action must not be adjacent to the most common one |
| A4 | `E` | Hub (context bar focused, or any Hub screen) | Edit active context (name, owners, delete) | §9: "Context update/edit dialog exists but no normal binding opens edit" |
| A5 | `Y` | Confirm dialogs | Required instead of `y` when any decisive safety fact is unknown or the inspection errored | Principle 3 + KEYMAP's own "uppercase = stronger variant" |
| A6 | `!` | Normal + Workspace | Show/focus the sticky error (last failed job) and offer `R retry` | Errors must be re-reachable after the status-bar slot is dismissed |
| A7 | `u` | Hub worktrees | **Undo last delete** while the trash entry still exists (§3 delete renames to `trash/<epochms>-<slug>` before a detached `rm -rf`) | A real, cheap safety net that the current architecture already almost provides |
| A8 | `c` / `X` / `R` / `D` / `f` | Jobs panel | cancel / cancel-all / retry / dismiss finished / filter | Cancel must exist and must be the *only* thing that stops a job |
| A9 | `ctrl-s w` | Prefix | Sleep this session and go to Hub | "Finish here safely" is currently two steps (`ctrl-s s` then `s`) and the intermediate state is easy to forget |
| A10 | `ctrl-s r` | Prefix | Restart the exited command in the current terminal | Pairs with the exit strip (§3.5); today a crashed dev server needs manual retyping |
| A11 | `I` | PR screen | Inspect the local worktree matching the selected PR | Makes "is my copy of this PR safe to delete?" answerable from where the question arises |
| A12 | `g j` | Normal | Go to the next running job's target row (worktree/repo) | Connects the global counter to the row that is changing without opening the panel |

---

## 6. Open questions

1. **Sleeping vs. detached as a first-class state.** §1 only has `none|detached|attached|
   unknown`; Fleet's sleep keeps a session alive when keep-alive rules match. Should `sleeping`
   be a real `SessionState` variant in `fleet-core` (my assumption), or a derived UI state from
   "session exists ∧ no terminals ∧ last activity > N"?
2. **Trash retention / undo (A7).** How long does `trash/<epochms>-<slug>` survive before the
   detached `rm -rf` runs — immediately (as today), or after a grace period that makes `u`
   meaningful? This is the single biggest safety upgrade available and it is a daemon policy
   question, not a UI one.
3. **Auto-inspect cadence.** Confirms are only as good as `inspectedAt`. Should the daemon
   re-inspect the *selected* worktree opportunistically (debounced 400 ms after the cursor
   settles) so confirms are always fresh, at the cost of background `git`/`gh` traffic?
4. **Escalation strictness (A5).** Is `Y` enough when facts are unknown, or should the truly
   irreversible cases (context delete, repo delete) require typing the id? My instinct is no —
   typed confirmations train people to type — but it is a product call.
5. **Failed-job counter persistence.** Should a red `⚠` counter survive an app restart (jobs
   live in the daemon), or reset on each launch? Surviving is more honest; resetting is quieter.
6. **Where do agent sessions (`ctrl-s a/A`, `swarm-agent-*`) appear in the Hub?** They are
   runtime-only and not in state (§1). Proposal: a pinned "Agents" row at the bottom of the
   repos rail showing live agent sessions; unresolved whether they also deserve session glyphs
   in the context bar.
7. **Keep-alive chip truthfulness on remote hosts.** Remote status is polled every
   `remoteStatusRefreshMs` with a 30 s timeout (§6); the chips can be up to ~40 s stale. Do we
   dim remote chips and stamp them, or hide chips entirely for remote worktrees?
8. **Ticker vs. silence.** The status-bar job ticker (§2) is the one place this lens risks
   nagging. Alternative: show it only for jobs the user personally started this session, and
   leave periodic/pool/status jobs to the counter alone.
9. **Zoom (`ctrl-s z`) hides the session header** — including the keep-alive chips and the job
   counter. Should zoom keep a 1-line collapsed strip, or is the `z` hint enough?
