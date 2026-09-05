# Fleet UX proposal — lens: **Glanceability & Minimalism**

> Scope: every screen in `docs/ARCHITECTURE.md` §Client, obeying `docs/KEYMAP.md` and preserving
> every behavior in `docs/SWARM-INVENTORY.md`. This document proposes; it does not edit KEYMAP.md.
> Numbers are real: px at the default window **1280×800** (min **900×560**), 4px base unit.

---

## 1. Design principles derived from the lens

1. **One glyph beats one word; one word beats one sentence.** Any fact that has ≤5 possible values
   becomes a 16px Lucide glyph in a fixed column, so the eye reads a *shape pattern* down the list
   instead of parsing text. Text is spent only on the two things that are unbounded: branch names
   and PR titles.
2. **Zero-suppression.** A count of 0, an empty label, a "none", a "no error", a "not remote" —
   none of them render. Chrome appears only when it carries information. A calm screen therefore
   *means* "nothing needs you", which is itself the most valuable glanceable fact.
3. **Four colors, all semantic, never decorative.** `green` = healthy/done/approved,
   `amber` = needs attention/in-flight, `red` = broken/destructive, `blue` = *only* the cursor and
   focus ring. Everything else is one of three neutrals. Draft, muted, disabled and "not applicable"
   are all rendered by *lowering contrast*, never by adding a hue.
4. **Progressive disclosure with a stable frame.** The detail panel is closed by default (`i`),
   filter replaces the list header in place, dialogs are the only layer that ghosts the base, and
   the Jobs panel docks to the right instead of covering the list. Nothing the user opens ever
   reflows what they were already looking at — position is memory.
5. **Cut anything that does not change the next keystroke.** Every element in the tables below had
   to answer: *which decision does this change?* Timestamps that only say "recently", IDs the user
   never types, paths the user copies instead of reads, "success" toasts for things that visibly
   succeeded — all removed. The full data still exists behind `i`, `I`, `J` and the palette.

---

## 2. Global layout

### 2.1 Hub frame (proportions at 1280×800)

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ ▍buk  1   personal  2   oss  3                            ⟳2   ◉3   ◔5         │ 36px  context bar
├──────────────┬─────────────────────────────────────────────────────────────────┤
│ REPOS     6  │ WORKTREES                                                12     │ 30px  pane headers
│──────────────│─────────────────────────────────────────────────────────────────│
│ ◉ All     12 │ ◉ feat/payroll-fix          buk/payroll   claude   #412 CI    2h │
│   payroll  5 │ ☾ fix/rut-validator         buk/payroll            #408 Appr  1d │ 30px  rows
│ ◉ fleetd   3 │   spike/gpui-vt             dannyfuf/fl                     3d   │
│   www      2 │ ● chore/deps                buk/www                          5d  │
│   infra    1 │                                                                  │
│   ⟳ nixos   │                                                                  │
│              │                                                                  │
│  240px       │                        flex (1039px)                             │
├──────────────┴─────────────────────────────────────────────────────────────────┤
│ buk › payroll › feat/payroll-fix                       ⟳ pool payroll  40%      │ 26px  status bar
└────────────────────────────────────────────────────────────────────────────────┘
                                          ┌──────────────────────────┐
                                          │ ✕ clone nixos failed  ⏎ │  toast layer, bottom-right
                                          └──────────────────────────┘
```

With the detail panel open (`i`) the worktrees list shrinks to 699px and a 340px panel is inserted
on the right; the repos rail never moves.

### 2.2 Persistent chrome

| What | Where | Size | Why here |
| --- | --- | --- | --- |
| Context tabs (numbered 1–9) | Top-left, x=12 | 36px tall, tab = text+8px pad | Contexts are the outermost coordinate; `1`–`9` / `gt` are the cheapest jump in the app, so the digits must be visible while you press them. Top-left is the first place a left-to-right reader lands. |
| Live chips (`⟳ jobs`, `◉ awake`, `◔ review`) | Top-right of context bar | 22px tall pills, 8px gap | The three "is anything happening without me?" counters. Top-right is the OS-standard status corner (menu bar extras), far from the cursor, so they never compete with the list. Hidden at 0. |
| Repos rail | Left, fixed 240px (drag 200–320, remembered) | full height | Second coordinate. Narrow and left because it is a *filter*, not content: you scan it once and then live in the list. |
| Pane header (`REPOS 6` / `WORKTREES 12`) | Top of each pane | 30px, 11px uppercase, tracking .06em, `fg.faint` | Names the pane and carries its only aggregate (count). Doubles as the filter bar host, so filtering costs zero layout shift. |
| Status bar | Bottom, full width | 26px | One breadcrumb (left) + one in-flight job line (right). Bottom edge = lowest-priority glance target, correct for context you already know. |
| Toast layer | Bottom-right, above status bar, 320px wide | max 2 stacked, 3.2s | Only for events with no other home. Bottom-right so it never covers the cursor row or the branch column. |
| Focus ring | 2px `blue` inset on focused pane, 2px left bar on cursor row | — | The only blue in the app: blue always answers "where am I?" and never "how is it going?". |

### 2.3 Tokens

| Token | Value (dark) | Value (light) | Use |
| --- | --- | --- | --- |
| `bg` | `#0E1013` | `#FBFBFC` | app ground |
| `bg.raised` | `#16181D` | `#FFFFFF` | rails, panels, dialogs |
| `bg.row.sel` | `#1E2430` | `#EDF2FB` | cursor row |
| `border` | `#22262E` | `#E3E5E9` | 1px hairlines only |
| `fg` | `#E6E8EB` | `#16181D` | branch, title |
| `fg.muted` | `#8A9099` | `#6B7280` | repo, host, age, counts |
| `fg.faint` | `#5A6069` | `#9CA3AF` | draft, disabled, key hints |
| `green` | `#3FB950` | `#1A7F37` | attached, pass, approved, done |
| `amber` | `#D29922` | `#9A6700` | running, pending, dirty, unknown |
| `red` | `#F85149` | `#CF222E` | failed, changes requested, danger |
| `blue` | `#58A6FF` | `#0969DA` | cursor / focus only |

Type: UI **SF Pro Text 13/18**; data (branch, path, sha, terminal) **SF Mono 12.5/18**;
labels **11/14 uppercase, tracking .06em**. Row height **30px** everywhere (lists, tabs, palette
rows are 34px because they carry a key hint).

### 2.4 Status glyph vocabulary (used identically on every screen)

| State (`SessionState` §1) | Lucide icon | Color | Tooltip / detail wording |
| --- | --- | --- | --- |
| `attached` | `circle-dot` | green | `attached` |
| `detached`, awake | `circle` | fg | `running, detached` |
| `detached`, slept | `moon` | fg.muted | `sleeping — 2 windows kept` |
| `none` | *(blank 16px)* | — | *(nothing rendered)* |
| `unknown` | `circle-help` | amber | `unknown — host offline` |
| clone job in flight (`CloneJob.status`) | `loader-circle` (spin) | amber | `cloning…` |
| clone failed | `circle-x` | red | `clone failed` |

Absence-as-signal is deliberate: a worktree with no session shows **nothing**, so the eye counts
live work by counting glyphs.

---

## 3. Screens

---

### 3.1 Hub — Context bar

**Purpose:** *Which slice of the world am I in, and is anything moving in it?*

```
 ▍buk  1    personal  2    oss  3                              ⟳2    ◉3    ◔5
 └ 2px blue underline on active
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Context tab | `Context.name` + faint index digit | left, 12px pad, 16px gap | matches `1`–`9` binding | `State.activeContextId`, op *context switch* |
| Active marker | 2px `blue` underline, full tab width | under active tab | color = "where am I" | — |
| Overflow | `+3` faint chip after tab 9 | end of tab row | >9 contexts are palette-reachable only | `gt`/`gT` still cycle all |
| Jobs chip | `loader-circle` + running count | right, 12px from edge | OS status corner | §6 job model; `J` opens |
| Awake chip | `circle-dot` + count of `attached`+`detached` sessions | right of jobs | one number for "machines I left running" | `WorktreeStatus.session` |
| Review chip | `eye` + count of `review` tab PRs | rightmost | the only *inbound* obligation in the app | PR tab `review`, §5 |

**Intentionally omitted:** context `owners` (only matters in the Context dialog and PR scoping —
shown in detail panel), `createdAt`, repo counts per context (the rail already counts them),
app version (Help + palette), a settings/help gear (`,` / `?`), a window title bar chrome row
(the GPUI window uses a 36px unified titlebar that *is* this context bar; traffic lights sit at
x=12–72, tabs start at x=84).

**States:** loading → chips absent, tabs render from cached snapshot. Error (daemon) → see §3.12.
No contexts → tab row replaced by faint `no contexts` + `N create your first context`.

**Icons:** `loader-circle`, `circle-dot`, `eye`.

**Keyboard:** `1`–`9` jump, `gt`/`gT` cycle, `N` new, `D` delete (confirm), proposed `e` edit.

---

### 3.2 Hub — Repos rail

**Purpose:** *Scope the worktree list, and is any repo unhealthy or still cloning?*

```
┌──────────────┐
│ REPOS     6  │  30px
├──────────────┤
│▌◉ All     12 │  30px  ← "All" pseudo-repo, default cursor
│ ◉ payroll  5 │
│   fleetd   3 │
│   www      2 │
│   infra    1 │
│ ⟳ nixos      │  clone in flight
└──────────────┘
   240px
```

Row internals: `[12px pad][glyph 16][8][name flex, truncate-middle][8][count 24 right][12px pad]`.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| `All` pseudo-repo | literal `All` + total worktree count | row 0, pinned | swarm's default; the most common scope | TUI §5 "All pseudo-repo default" |
| Aggregate session glyph | `circle-dot` green if any session `attached`; else `circle` if any awake; else blank | left of name | one shape tells you which repos hold live work | §5 Repos "aggregate session glyph" |
| Repo name | `Repo.name`; `owner/name` **only** when two repos share a name | center flex | you think in repo names, not slugs | §5 "disambiguated owner/name" |
| Worktree count | integer, right-aligned, `fg.muted` | right | sizes the jump you are about to make | §5 "worktree count" |
| Clone row | `loader-circle` spin + name, count column empty | in sort position | a repo being born must be visible where it will live | `CloneJob`, §5 "Clone row spinner/failed" |
| Clone failed row | `circle-x` red + name + faint `failed` | same | failure must not disappear silently | `CloneJob.status = failed`, `.error` |

**Intentionally omitted:** repo `url`, `path`, `defaultBranch`, `clonedAt`, hook counts, private
lock icon, owner avatar. All are in the detail panel (`i`); none of them changes which repo you
select. Also omitted: a "live count" badge separate from the aggregate glyph — two numbers per row
is exactly the spam this lens forbids.

**States:** empty → `No repos in <context>.` + faint `n clone one` (exact swarm copy).
Filtered-empty → `Nothing matches "<filter>".` Loading → rail renders from state.json instantly;
discovery refresh shows the `⟳` chip in the context bar only.

**Icons:** `circle-dot`, `circle`, `loader-circle`, `circle-x`, `folder-git-2` (detail header only).

**Keyboard:** `j`/`k`, `gg`/`G`, `Enter`/`o`/`l` → focus worktrees, `n` clone, `d` delete (confirm,
cascades), `m` move context, `i` detail, `0` proposed → jump to `All`.

---

### 3.3 Hub — Worktrees list

**Purpose:** *Which of my branches is alive, which needs attention, which can I throw away?*

```
 WORKTREES                                                              12
 ────────────────────────────────────────────────────────────────────────
▌◉ feat/payroll-fix        buk/payroll     ⚡claude, :3000   #412 CI     2h
 ☾ fix/rut-validator       buk/payroll                       #408 Appr   1d
   spike/gpui-vt  ☁devbox  dannyfuf/fleetd                              3d
 ● chore/deps ✎            buk/www                                      5d
```

Column budget, list width **W = 1039px** (detail closed), padding 16/16 → 1007 usable:

| # | Column | Width | Align | Shown when |
| --- | --- | --- | --- | --- |
| 1 | session glyph | 16 | center | always |
| 2 | gap | 10 | | |
| 3 | branch (`Worktree.branch`) + `✎` dirty + `☁host` | flex, **min 220** | left | always |
| 4 | gap | 12 | | |
| 5 | repo `owner/name` | 150, truncate-head | left | only when scope = `All` **and** W ≥ 820 |
| 6 | gap | 12 | | |
| 7 | running keep-alive labels | 120, ellipsis | left | W ≥ 900 (else collapses to `⚡n`, 28px) |
| 8 | gap | 12 | | |
| 9 | PR badge `#n` + state | 92 | left | W ≥ 700 and `pr` present |
| 10 | gap | 12 | | |
| 11 | age | 52 | right | W ≥ 640 |

Below 640px only columns 1, 3, 9 survive. (This mirrors swarm's char-cell rules: running 0/10/14/18,
PR badge 15, at 7.5px/char.)

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session glyph | §2.4 table | col 1 | leftmost = first read; it decides `Enter` vs `s` vs `d` | `WorktreeStatus.session` |
| Branch | `Worktree.branch` (not `slug`, not `id`) | col 3 | the only string the user actually thinks in | `Worktree.branch`; slug is derivable |
| Dirty mark | `✎` (`file-pen`, 12px, amber) suffixed to branch | inline after branch | dirty is a *property of the branch*, so it rides with it instead of buying a column | `WorktreeInspection.dirty`, op *inspect* |
| Host chip | `cloud` + `Host` id, `fg.muted` | inline after dirty | absent for local (the 95% case) — zero-suppression | `Worktree.host`, §1 remote mirrors |
| Repo prefix | `owner/name` | col 5 | disambiguates only in `All` scope | §5 "optional repo prefix" |
| Running labels | `⚡` (`zap`) + `keepAlive` labels joined `, ` | col 7 | tells you *why* sleep will refuse to close windows | `WorktreeStatus.windows[].keepAlive`, §4 sleep policy |
| PR badge | `#412` + state word (§2.4/§3.5) | col 9 | the single fact that decides "is this branch done?" | `InspectionPullRequest`, §5 "optional `#n <state>` badge" |
| Age | relative `lastOpenedAt ?? createdAt`, 1 unit (`2h`, `3d`, `5w`) | col 11 | recency is the natural sort you verify visually | `Worktree.lastOpenedAt` / `createdAt` |
| Sort | `lastOpenedAt` desc, then `createdAt` desc | — | MRU means row 0 is almost always the right answer, so `Enter` alone is often the whole task | op *open* step 3 |

**Intentionally omitted:** `WorktreeId` (never typed in the GUI), `path` (`y` copies it; detail shows
it), `baseRef`, `session` name string, window list, `ahead/behind`, `uniqueCommits`, `published`,
`mergedIntoTarget`, absolute timestamps, PR title, PR author, additions/deletions. Rationale: none of
them change which row you press `Enter` on; every one of them appears in the detail panel or the
delete/prune confirmation, i.e. exactly at the moment they *do* change a decision.

**States**
- *empty*: `No worktrees yet.` / `No worktrees for <repo> yet.` + faint `n create one`.
- *filter-empty*: `Nothing matches "<filter>".`
- *loading (cold)*: rows render from `state.json` immediately with glyph column blank; the glyph
  column fades in when the first `WorktreeStatus` arrives. No skeletons — the data is local.
- *job running on a row*: the glyph column is replaced by `loader-circle` (spin, amber) and the age
  column by the job's short kind (`create`, `prune`, `hooks`); the row is never disabled.
- *error on a row* (inspect/status failed): glyph = `circle-help` amber; detail panel shows
  `WorktreeInspection.error` verbatim. The row stays operable.
- *session*: attached `circle-dot` green / detached `circle` / sleeping `moon` / none blank /
  unknown `circle-help` amber.
- *PR*: see §3.5 badge table; a merged PR shows `git-merge` green + `Merged` — the strongest hint
  that this row is prune-able.

**Icons:** `circle-dot`, `circle`, `moon`, `circle-help`, `file-pen`, `cloud`, `zap`,
`git-pull-request`, `git-pull-request-draft`, `git-merge`, `loader-circle`.

**Keyboard:** `j`/`k`/`gg`/`G`/`ctrl-d`/`ctrl-u`; `Enter`/`o` open (sleeps previous), `O` open keeping
previous, `n` create, `d` delete, `x` prune repo, `s` sleep, `K` kill, `I` inspect, `i` detail,
`y` copy path, `b` browser, `/` filter, `p` → PRs.

---

### 3.4 Hub — Detail panel (`i`, closed by default)

**Purpose:** *Everything I deliberately kept out of the row, on demand, in one 340px column.*

```
┌────────────────────────────────────┐
│ ⑂ feat/payroll-fix                 │ 34  title
│   buk/payroll · origin/main        │ 20  subtitle
├────────────────────────────────────┤
│ ◉  attached · nvim, cc ⚡claude, lg │
│ ⇡3 ⇣0 · 3 unique · dirty           │
│ ⇱  #412 open · CI failing          │
│ ~/…/worktrees/buk/payroll/feat-…   │ ← y
│ opened 2h ago · created 5d ago     │
└────────────────────────────────────┘
      340px, 12px padding
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `git-branch` + `branch` | row 1 | echoes the cursor row so the eye does not re-search | — |
| Subtitle | `repoId` · `baseRef` | row 2, `fg.muted` | provenance, read once | `Worktree.repoId`, `.baseRef` |
| Session line | glyph + state word + window names with keep-alive labels | block 1 | this is the *only* place window names exist; needed before `s`/`K` | `WorktreeStatus.windows[]`, §4 |
| Divergence line | `⇡ahead ⇣behind · N unique · dirty` (missing parts omitted) | block 1 | the safety facts for `d` | `WorktreeInspection.ahead/behind/uniqueCommits/dirty` |
| PR line | `#n state · checks/review` | block 1 | — | `InspectionPullRequest`, `PrChecks`, `PrReviewDecision` |
| Path | middle-truncated, mono 12px, `fg.muted` | block 2 | you copy it (`y`) far more than you read it | `Worktree.path`; op `path` |
| Times | `opened <rel> · created <rel>` | last | low priority by definition | `lastOpenedAt`, `createdAt` |
| Warnings | amber `triangle-alert` + each `warnings[]` string verbatim | above times, only if any | swarm's 11 soft-warning strings are diagnostic gold and must not be paraphrased | op *inspect* step 6 |
| Stale marker | faint `inspected 4m ago · I refresh` | footer, only if >60s old | prevents trusting stale safety facts | `inspectedAt` |

Repo cursor variant: name/owner/`defaultBranch`/`path`/worktree+live counts/`hooks.prepare`
(count + first command)/`hooks.postCreate`/`url`. Clone-job variant: status, staging path, log path
(`Enter` opens it in the Jobs panel), `error` in red. Context variant (rail header focused): name,
`owners` joined, repo + worktree counts.

**Intentionally omitted:** `WorktreeId`, `session` name, `host.ssh`, `swarmCommand`, `createdAt`
absolute ISO, PR labels, additions/deletions (those live on the PR screen where they are compared
across rows).

**Keyboard:** `i` closes; `Tab`/`l` focuses the panel so `j`/`k` scroll it when it overflows;
`y` still copies the path from anywhere.

---

### 3.5 Pull requests screen (`p`)

**Purpose:** *What am I waiting on, and what is waiting on me — and can I start on it in one key?*

```
 ▍buk  1   personal  2   oss  3                                ⟳2  ◉3  ◔5
 ────────────────────────────────────────────────────────────────────────
  MINE 7      REVIEW 5                                    fetched 40s ago
 ────────────────────────────────────────────────────────────────────────
▌◉ #412  Fix RUT validation on payroll import     feat/rut…   CI      2h
   #408  Bump lazygit to 0.44                     chore/deps  Appr    1d
   #401  Draft: gpui vt spike                     spike/gpu…  Draft   3d
      ↑ local-presence glyph
```

Columns at W = 1039 (list-only; detail panel behaves as §3.4):

| # | Column | Width | Shown when |
| --- | --- | --- | --- |
| 1 | local-presence glyph | 16 | always |
| 2 | number `#1234` | 52, right | always |
| 3 | title | flex, min 240 | always |
| 4 | author (`REVIEW` tab) | 108 | W ≥ 720 |
| 5 | `headRefName` | 160, truncate-tail | W ≥ 900 |
| 6 | repo `owner/name` | 130 | W ≥ 1100 **and** scope = context |
| 7 | state badge | 92 | always |
| 8 | age | 52, right | W ≥ 640 |

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Tabs `MINE` / `REVIEW` | label + count; active = 2px blue underline | row under context bar, left | `Tab`/`h`/`l` toggle them; counts answer "how much is queued" without entering | `PrTab`, §5 "tab counts" |
| Fetch age | `fetched 40s ago` / `fetching…` / red `gh unavailable` | same row, right | trust marker for cached data; right side because you check it only when surprised | `PrRepoSlice.fetchedAt/loading/error`, `github.prTtlSeconds` |
| Local-presence glyph | `circle-dot`/`circle`/`moon` if a matching worktree exists, else blank | col 1 | decides whether `Enter` *opens* or *creates* — the highest-value bit on this screen | §5 "local presence glyph"; match rule §1 |
| Number | `#{number}` | col 2 | the handle you say out loud | `PullRequest.number` |
| Title | `PullRequest.title`, single line, tail-truncated | col 3 | — | — |
| Author | `login`, `fg.muted` | col 4 | only meaningful in `REVIEW` | `PullRequest.author` (null → `ghost`) |
| Branch | `headRefName` mono, `fg.muted` | col 5 | ties the PR to the branch you will get | `PullRequest.headRefName` |
| State badge | one icon + one word, priority draft→ci_fail→changes→ci_pending→approved→review | col 7 | collapses 3 fields into 1 glance | `PrState` priority, §1 |
| Age | relative `updatedAt` | col 8 | staleness of the *PR*, not the fetch | `PullRequest.updatedAt` |

**PR state badge (exact text ≤8 chars, exact icon, exact color):**

| `PrState` | Icon | Color | Text |
| --- | --- | --- | --- |
| `draft` | `git-pull-request-draft` | fg.faint | `Draft` |
| `ci_fail` | `circle-x` | red | `CI fail` |
| `changes` | `message-square-warning` | amber | `Changes` |
| `ci_pending` | `loader-circle` (spin) | amber | `CI ···` |
| `approved` | `circle-check` | green | `Approved` |
| `review` | `eye` | fg.muted | `Review` |
| merged (inspection) | `git-merge` | green | `Merged` |
| cross-repo fork | `git-fork` prefix on the branch column | fg.muted | — |

**Intentionally omitted from rows:** `additions`/`deletions` (a two-number diff stat is noise in a
list; it lives in the detail panel as `+128 −34`), `labels` (detail panel; they never decide which
PR you open), `baseRefName` (detail), `isCrossRepository` (folded into the `git-fork` prefix),
`checks`+`reviewDecision` as separate columns (they *are* the badge), `url` (`y`/`b`).

**States**
- *empty MINE*: `No open PRs authored by you in <scope>.`  *empty REVIEW*: `No PRs waiting for your review in <scope>.`
- *loading*: tab count replaced by `loader-circle`; previously cached rows stay on screen at full
  opacity (cache-first, per §6 "cache first"). Never a blank screen on refresh.
- *error*: the fetch-age slot turns red with the 120-char error, truncated to the slot with the full
  string in the detail panel; **stale rows remain visible** — losing data to show an error is spam.
- *job running*: creating a worktree from a PR turns that row's presence glyph into a spinner and
  writes `create <repo>#<slug>` into the status bar; the screen stays fully navigable.

**Keyboard:** `Tab`/`S-Tab`/`h`/`l` tabs, `j`/`k`/`gg`/`G`, `Enter`/`o` open-or-create (sleeps
previous), `O` keep previous, `b` browser, `y` copy URL, `r` force refresh both tabs, `/` filter,
`i` detail, `p`/`q` back.

---

### 3.6 Workspace

**Purpose:** *Be a terminal. Tell me only which terminal I am in and whether the session is healthy.*

```
┌────────────────────────────────────────────────────────────────────────┐
│ ⑂ feat/payroll-fix   buk/payroll                    ◉  ⚡claude, :3000  │ 30px header
├────────────────────────────────────────────────────────────────────────┤
│  1 nvim  │ 2 cc ● │ 3 lg  │ +                                          │ 30px tabs
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│  terminal cell grid, 8px padding, SF Mono 12.5/18                      │
│                                                        ┌─────────────┐ │
│                                                        │ SCROLL 1240 │ │ scroll pill
│                                                        └─────────────┘ │
│                                                                        │
├────────────────────────────────────────────────────────────────────────┤
│ ^s  s hub  1-9 tab  c new  x close  [ scroll  ] paste  a agent  J jobs │ 22px prefix hint
└────────────────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session header | `git-branch` + branch (fg) + `repoId` (muted) + `cloud host` if remote | top-left | one line answering "am I in the right worktree?" — the #1 terminal error | `Session.kind = Worktree(id)` |
| Session state + keep-alive | glyph + `⚡` labels | top-right | mirrors the Hub row so the two screens read identically | `WorktreeStatus`, §4 |
| Tab strip | `index name` per `Terminal`, min 84 / max 200px, auto | under header | tabs are the second coordinate; directly above the content they switch | `Session.terminals`, `active_terminal` |
| Active tab | fg text + 2px blue bottom border | — | blue = "where am I" | — |
| Tab activity dot | 6px amber `●` on inactive tab with output since last visit | right inside tab | the only background-activity signal; prevents polling tabs by hand | `Terminal.status`, dirty-row events |
| Tab exited mark | tab label at `fg.faint` + `circle-x` 12px | — | a dead command must not look alive | `Terminal.status` |
| `+` button | `plus` glyph tab | end of strip | mouse parity for `ctrl-s c` | keymap `ctrl-s c` |
| Terminal area | cell grid, 8px pad, no border | fills | maximum rows; chrome ≤ 82px total | — |
| Scroll pill | `SCROLL <offset>` + `SEL` when selecting, 11px, `bg.raised`, amber left bar | overlay top-right, 12px inset | during scroll the eyes are on content; top-right is vim's ruler position and never covers the prompt (bottom-left) | `viewport{scrollback_len, offset}`; keymap Scroll mode |
| Prefix hint | one 22px strip listing 8 prefix keys, mono 11px, `fg.faint` | bottom, full width, appears the instant `ctrl-s` is held, vanishes on the next key | it exists for a single keystroke, so it costs nothing; bottom edge keeps the cursor line visible | keymap `ctrl-s` one-shot prefix |
| Jobs chip | inherited from context bar, drawn in the header right when the header is visible | top-right | background work must be visible from inside a terminal too | §6 |

`ctrl-s z` (zoom) hides header + tab strip; a 2px amber bar on the window's top edge remains as the
only reminder that chrome is hidden.

**Intentionally omitted:** window title from the PTY (`FrameUpdate.title` is used for the *tab* name
only when the terminal was never renamed), shell PID, `foreground_command`, cwd, cols×rows readout,
a mode indicator for Terminal mode (being in a terminal is self-evident), a scrollback percentage
bar, connection latency. All of these are inspectable via `J` / `,` / the palette.

**States:** attached (normal) · detached (impossible here — attaching is what this screen is) ·
terminal exited → grid frozen at last frame + centered faint `process exited · ^s x close · ^s c new`
· daemon lost → grid dims to 55% and a 26px amber banner replaces the header:
`reconnecting to fleetd…` (see §3.12) · job running → chip in header.

**Icons:** `git-branch`, `cloud`, `circle-dot`, `zap`, `plus`, `circle-x`, `square-terminal` (palette
entries only), `bot` (agent session header).

**Keyboard:** all keys to the PTY; `ctrl-s` then `s / 1-9 / h l p n / c / x / , / [ / ] / a A / z / J / ? / Esc`.

---

### 3.7 Jobs panel (`J`)

**Purpose:** *What is the daemon doing for me, is it stuck, and can I read the log?*

Right-docked sheet, **440px** wide, full height between context bar and status bar, `bg.raised`,
1px left border, 160ms slide. The list behind stays fully visible and readable — a centered modal
would hide exactly the rows the jobs are about.

```
                                    ┌────────────────────────────────────┐
                                    │ JOBS                    3 running  │ 30
                                    ├────────────────────────────────────┤
                                    │▌⟳ clone  nixos                 40% │ 44
                                    │   Receiving objects: 40% (81/202)  │
                                    │ ⟳ pool   buk/payroll               │
                                    │   running prepare hook 2/3         │
                                    │ ⟳ hooks  buk/payroll#feat-rut      │
                                    │   npm ci                           │
                                    ├────────────────────────────────────┤
                                    │ ✓ prune  buk/www        deleted 2  │ 30  (done, 60s)
                                    │ ✕ fetch  dannyfuf/fleetd           │
                                    │   gh: HTTP 502                     │
                                    └────────────────────────────────────┘
                                    │ ⏎ log   c cancel   f follow   Esc  │ 26
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Header | `JOBS` + `<n> running` | top | the count is why you opened it | §6 |
| Kind | fixed 7-char slug: `clone`, `pool`, `hooks`, `prune`, `create`, `delete`, `fetch`, `prs`, `inspect`, `update` | col 1, 56px | fixed width makes the column scannable as a shape | §6 job table |
| Target | `RepoId` or `WorktreeId` | col 2, flex | which of my things is affected | `Job.target` |
| Progress | percent when parseable, else nothing | col 3, 40px right | — | `Job.status`, last progress line |
| Progress line | last stdout line, 1 line, `fg.muted` 11px, tail-truncated | row 2 of the item | the single most reassuring artifact for a long job | §6 "last progress line" |
| Status glyph | `loader-circle` amber / `circle-check` green / `circle-x` red / `clock` faint (queued) | col 0, 16px | — | `Job.status` |
| Done items | collapse to a single 30px line, auto-hide 60s after completion | below running | finished work must decay, not accumulate | — |
| Failed items | stay until dismissed (`d`), red glyph, error line kept | pinned above done | the only sticky state | — |
| Footer hints | `⏎ log  c cancel  f follow  Esc` | bottom of sheet | this is a low-frequency screen; hints here are worth their 26px | keymap |

`Enter` opens `logs/jobs/<id>.log` inside the sheet (the sheet widens to 640px, mono 11.5px, tail-
following, `f` toggles follow, `Esc` returns). Cancel (`c`) asks nothing for cancellable jobs; the
job flips to `cancelling…` then disappears.

**Intentionally omitted:** job `id`, `startedAt`/`finishedAt` absolute times (a running job shows
`4m` elapsed, right-aligned, only after 30s), per-job PID, concurrency/queue position, log path
string (`Enter` opens it), a chart or progress bar for non-percent jobs.

**States:** empty → centered faint `Nothing running.` + `Fleet keeps jobs alive when you close this.`
(that sentence is the product promise; it earns its pixels once). Daemon down → `Jobs unavailable —
fleetd is not running.`

**Icons:** `loader-circle`, `circle-check`, `circle-x`, `clock`, `download` (clone), `package` (pool),
`terminal` (hooks), `trash-2` (prune/delete), `refresh-cw` (fetch/prs), `arrow-up-circle` (update).

**Keyboard:** `J` toggle, `j`/`k`, `Enter` log, `c` cancel, `f` follow, `d` dismiss finished, `Esc` close.

---

### 3.8 Dialogs — shared frame

All dialogs: centered, `bg.raised`, 12px radius, 1px `border`, shadow `0 16px 48px rgba(0,0,0,.45)`,
backdrop = base screen at 45% opacity with a 8px blur (swarm's "ghosts base"). Header 44px:
icon + title, no close button (`Esc`). Footer 44px: left = contextual hints in `fg.faint` mono 11px,
right = primary action label only (`⏎ Create`). **No OK/Cancel button pair anywhere** — the hint row
says the keys, and this is a keyboard app.

| Dialog | Width × height | Icon |
| --- | --- | --- |
| Create worktree | 560 × auto (≈380) | `git-branch-plus` |
| Clone repo | 560 × 420 | `download` |
| Confirm | 480 × auto (≈220) | `triangle-alert` / `trash-2` |
| New / Edit context | 460 × 240 | `boxes` |
| Assign repo to context | 460 × 340 | `arrow-right-left` |
| Settings | 720 × 560 | `settings-2` |
| Help | 880 × 620 | `circle-question` |

---

#### 3.8.1 Create worktree (`n`)

**Purpose:** *Name a branch, pick a base, go — with the filesystem consequence visible before I commit.*

```
┌──────────────────────────────────────────────────────────┐
│ ⑂+ New worktree · buk/payroll                            │
├──────────────────────────────────────────────────────────┤
│ Branch                                                   │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ feat/rut-validator                                  ▏│ │ 36
│ └──────────────────────────────────────────────────────┘ │
│ → buk/payroll#feat-rut-validator                         │ 18 faint
│                                                          │
│ Base                          ⟳ fetching                 │
│ ┌──────────────────────────────────────────────────────┐ │
│ │▌origin/main                                  default │ │ 28 × 6
│ │ origin/release-2026                                  │ │
│ │ origin/feat/payroll-import                           │ │
│ └──────────────────────────────────────────────────────┘ │
│                                                          │
│ Host   ◂ local ▸                                         │ 28  (only if hosts configured)
├──────────────────────────────────────────────────────────┤
│ ⇥ field   ⌃n/⌃p base   esc cancel          ⏎ Create      │
└──────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `New worktree · <repoId>` | header | the repo is already chosen; restating it prevents the #1 mistake | op *create* |
| Branch input | free text, `validateBranch` live | first field, auto-focused | the only required input; typing must start on open | op *create* precondition |
| Slug preview | `→ <repoId>#<slugify(branch)>` faint | under input | shows the id and the directory name you will get, without a second field | `WorktreeId` construction, slug rules §1 |
| Validation | inline red line replacing the preview: exact rule text, e.g. `branch cannot contain "..."` | same slot | zero layout shift | `validateBranch` reject list §1 |
| Base list | up to **6** rows: default `origin/<defaultBranch>` first, then old `baseRef`, then `origin/*` fuzzy-filtered by the branch text | second field | 6 is swarm's number and fits without scrolling | Create dialog §5 |
| `default` tag | faint right-aligned on the default row | — | one word instead of a separate "use default" control | — |
| Fetch indicator | `loader-circle` + `fetching` at the section's right | — | the list may grow under you; say so | §5 "fetching indicator" |
| Free text base | typing in the base row accepts any ref | — | swarm allows it | §5 "free text allowed" |
| Host selector | `◂ local ▸`, only when `config.hosts` is non-empty | third field | zero-suppressed for the local-only majority | `defaultHost`, `Host` |
| Pool hint | faint `hot slot ready` (green `zap`) or `no slot — full copy` (amber) at the footer left | footer | predicts whether this takes 2s or 40s — it changes whether you wait or switch away | prepared-copy pool §1, §6 |

**Intentionally omitted:** slug as a separate editable field (derived; editable via palette command
`create with custom slug` for the rare case), `--url`, `--default-branch`, `--hooks` (CLI-only),
a "run post-create hooks" checkbox (always on), a base-ref preview of commits, target path.

**States:** submitting → the dialog **closes immediately** and the work becomes a job (`create`) in
the status bar + Jobs panel; conflict (existing id with different branch/host) → dialog reopens with
the exact conflict message in red at the footer; fetching → indicator only, never blocks `Enter`.

**Keyboard:** `Tab`/`S-Tab` fields, `ctrl-n`/`ctrl-p` or `↓`/`↑` base list, `←`/`→` host, `Enter`, `Esc`.

---

#### 3.8.2 Clone repo (`n` in repos)

**Purpose:** *Find a GitHub repo by typing a few letters and get it cloning.*

```
┌──────────────────────────────────────────────────────────┐
│ ⤓ Clone repo · into "buk"                                │
├──────────────────────────────────────────────────────────┤
│ ┌──────────────────────────────────────────────────────┐ │
│ │ 🔍 payroll                                          ▏│ │ 36
│ └──────────────────────────────────────────────────────┘ │
│▌🔒 bukhr/payroll                                     2h  │ 34 × 8
│    Nómina y remuneraciones                               │
│    bukhr/payroll-legacy                              1y  │
│    Archived import pipeline                              │
└──────────────────────────────────────────────────────────┘
│ ⌃n/⌃p  esc cancel                             ⏎ Clone    │
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| Target context | `into "<context.name>"` in the header | header | clone destination is invisible otherwise and is a real mistake source | `Repo.contextId` |
| Search input | 150ms debounce | top | — | §5 Clone: "150 ms debounce" |
| Results | **8** rows max, 2 lines: `lock`(private) + `fullName` + relative `updatedAt`; description in `fg.muted` 11px | list | 8 is swarm's cap | §5 "8 remote results" |
| Empty description | row collapses to 22px single line | — | zero-suppression | `RemoteRepo.description` null → `""` |
| Searching | `loader-circle` replacing the magnifier | in input | in-place, no reflow | — |

**Intentionally omitted:** stars/forks/language, clone protocol picker (`github.cloneProtocol`
belongs in Settings), the full ssh URL, owner avatars, "clone into a different context" (that is
`m` after the fact).

**States:** empty query → faint `Type to search GitHub.` No results → `No repos match "<q>".`
`gh` failure → red one-liner in the results area, input stays live. On `Enter` the dialog closes
instantly and a `clone` job appears in the rail and the Jobs panel (survives closing the app — §6).

---

#### 3.8.3 Confirm (delete / prune / kill / close terminal)

**Purpose:** *Show me exactly what I will lose, in facts, not adjectives.*

Delete worktree — the safety facts come from `inspect`, which swarm never showed in the TUI (§9 gap):

```
┌────────────────────────────────────────────────┐
│ 🗑 Delete feat/payroll-fix?                    │
├────────────────────────────────────────────────┤
│ ✎  uncommitted changes                         │
│ ⑂  3 commits not on origin/main                │
│ ◉  session attached · claude, :3000 running    │
│ ⇱  #412 open                                   │
│                                                │
│ The copy is moved to trash and removed.        │
├────────────────────────────────────────────────┤
│ n cancel                          y  Delete    │
└────────────────────────────────────────────────┘
```

Only the *true* facts render; a clean, sessionless, merged worktree shows a single green line
`✓ merged into origin/main · nothing to lose` and the primary label stays `Delete`.

| Variant | Title (exact) | Body (exact) | Primary (exact) |
| --- | --- | --- | --- |
| Delete worktree | `Delete <branch>?` | fact list above + `The copy is moved to trash and removed.` | `y  Delete` |
| Delete repo | `Delete buk/payroll?` | `4 worktrees and 2 sessions are deleted with it.` + `The clone is moved to trash.` | `y  Delete repo` |
| Delete context | `Delete context "buk"?` | `3 repos, 12 worktrees and 3 sessions are deleted with it.` | `y  Delete context` |
| Prune | `Prune 4 of 12 worktrees in buk/payroll?` | two lists: `Delete` (branch + `merged` / `no unique commits`) and `Keep` (branch + exact skip reason from `prune`, e.g. `tmux session has running commands: claude`) | `y  Prune 4` |
| Kill session | `Kill session for <branch>?` | `3 windows are killed: nvim, cc (claude), lg.` + `Unsaved work in them is lost.` | `y  Kill` |
| Close terminal | `Close "cc"?` | `claude is still running in it.` | `y  Close` |
| Quit + daemon | see §3.13 | — | — |

**Intentionally omitted:** a "don't ask again" checkbox (destructive confirmations must not be
disable-able), an "are you sure?" second step, icon-only danger banners, the worktree path,
the `WorktreeId`.

**States:** the prune variant is preceded by a `prune --dry-run` job; while it runs the dialog shows
`loader-circle` + `checking 12 worktrees…` and `y` is inert (not disabled-looking — just does nothing
until facts exist). If `inspect` failed for a row, the delete dialog shows amber
`inspect failed — safety unknown` and the primary becomes `y  Delete anyway`.

**Keyboard:** `y`/`Enter` confirm, `n`/`Esc`/`q` cancel. Nothing else is bound, so muscle memory
cannot misfire.

---

#### 3.8.4 New / Edit context

```
┌────────────────────────────────────────────┐
│ ⬚ New context                              │
├────────────────────────────────────────────┤
│ Name    ┌────────────────────────────────┐ │
│         │ Buk HR                        ▏│ │
│         └────────────────────────────────┘ │
│         → buk-hr                            │  faint id preview
│ Owners  ┌────────────────────────────────┐ │
│         │ bukhr, dannyfuf                 │ │
│         └────────────────────────────────┘ │
│         GitHub orgs/users used to scope PRs │  faint, one line
├────────────────────────────────────────────┤
│ ⇥ field   esc cancel              ⏎ Create │
└────────────────────────────────────────────┘
```

`Name` → `ContextId` preview under the field (`Context.id` slugify rules §1). `Owners` is comma-
separated; the one-line explainer is the only piece of teaching copy in the app, because `owners`
is the single field whose purpose is not guessable. Edit variant (proposed binding `e`, closing
inventory gap §9 "no normal binding opens edit") shows the id as read-only faint text instead of a
preview. **Omitted:** `createdAt`, repo list, a color/emoji picker.

---

#### 3.8.5 Assign repo to context (`m`)

```
┌────────────────────────────────────────────┐
│ ⇄ Move buk/payroll                         │
├────────────────────────────────────────────┤
│▌ buk          bukhr                current │
│  personal     dannyfuf                     │
│  oss          zed-industries, ghostty-org  │
├────────────────────────────────────────────┤
│ ⌃n/⌃p  esc cancel                 ⏎ Move   │
└────────────────────────────────────────────┘
```

Rows: `Context.name` (fg) + `owners` joined (fg.muted, truncate) + faint `current` tag on the
present one. Owners are shown here specifically because they are the reason a repo belongs to a
context. **Omitted:** repo counts, ids, created dates.

---

#### 3.8.6 Settings (`,`)

720×560, two columns: a 180px section rail (`General`, `Sleep`, `Windows`, `GitHub`, `Hosts`,
`About`) and a 540px pane. Editable fields carry a normal-contrast value; read-only facts are
`fg.muted` with no input chrome — the *absence of a box* is how "you cannot edit this here" is said,
instead of a disabled style.

| Section | Rows |
| --- | --- |
| General | `Agent  ◂ claude ▸` · `Claude command  [claude]` · `OpenCode command  [opencode]` · `Hot pool size  ◂ 1 ▸` |
| Sleep | `Sleep on switch  [x]` · rule list: `[x] claude`, `[x] opencode`, `[x] codex`, `[x] server (listening ports)` · read-only `Grace 2000 ms` |
| Windows | read-only ordered list `1 nvim — nvim .` / `2 cc — {agent}` / `3 lg — lazygit` |
| GitHub | `Clone protocol ◂ ssh ▸` · read-only `Repo cache 3600s` · `PR cache 90s` |
| Hosts | read-only per host: `devbox — ssh danny@devbox — swarm` |
| About | `Fleet 0.1.0+<sha>` · `fleetd running · pid 4211 · up 3h` · `Config ~/.fleet/config.json` |

Read-only rows show a faint trailing `edit in config.json` **once per section**, not per row.
`Sleep` rules explain themselves through their labels; no per-rule regex is shown (it is in
config.json) — that is the biggest single cut versus swarm's settings screen.

**States:** saving is synchronous and silent; a failed write shows a red footer line with the exact
error and keeps the dialog open. **Omitted:** timers/intervals, protocol version, `reposDir`/
`worktreesDir` (About shows `FLEET_HOME` only), import-from-swarm (a palette command).

---

#### 3.8.7 Help (`?`)

880×620, three columns × ~14 rows, grouped by *mode* because the app is modal:
`Hub` · `Worktrees & PRs` · `Terminal (^s)` · `Scroll` · `Dialogs & filter`. Keys in a 68px mono
`fg` column, action in `fg.muted`. Version + `fleetd` status in the footer. Context-sensitive:
opening `?` inside a terminal scrolls to the `Terminal (^s)` column and dims the others to 55%.
**Omitted:** prose, links, a search field (the palette *is* the searchable surface).

---

### 3.9 Command palette (`:`)

**Purpose:** *Do the thing I know the name of but not the key — and reach everything the keymap does not bind.*

640px wide, top-anchored at y=120 (thinking position, not screen center), 44px input, up to **10**
rows × 34px.

```
┌──────────────────────────────────────────────────────────────┐
│ : prune                                                     ▏│ 44
├──────────────────────────────────────────────────────────────┤
│▌🗑 Prune worktrees · buk/payroll                          x  │ 34
│  🗑 Prune all repos in buk                                   │
│  ⬚ Switch to context: personal                            2  │
│  ⤓ Clone repo                                             n  │
└──────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| Icon | the same Lucide glyph the action uses elsewhere | col 1, 16px | teaches the icon language | — |
| Label | verb-first, with `· <target>` when the action is scoped to the cursor | col 2 | you search by verb | §5 palette |
| Key hint | the bound key, right-aligned, `fg.faint` mono | col 3 | the palette's job is to make itself unnecessary | §5 "label/keys" |
| Contexts | `Switch to context: <name>` entries mixed in | inline | swarm mixes commands and contexts | §5 "commands + contexts" |
| Cap | first 10 fuzzy matches | — | — | §5 "first 10 valid commands" |

Only *valid* commands render (no disabled rows). **Omitted:** categories/section headers,
descriptions, recently-used section, fuzzy match highlighting beyond a subtle `fg` weight bump.

---

### 3.10 Filter bar (`/`)

**Purpose:** *Narrow this list without moving it.*

The filter **replaces the pane header in place** — 30px, same row, no overlay, no reflow:

```
 WORKTREES                                                    12       ← normal
 🔍 payroll▏                                                 3/12  esc ← filtering
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| `search` icon | 14px `fg.muted` | left, replaces the `WORKTREES` label | signals mode without a mode word | §5 filter is a real insert-mode input |
| Query | live text, blue caret | inline | — | — |
| Match count | `3/12` | right | tells you whether to keep typing | — |
| `esc` hint | faint, right of the count | right | the two-stage Esc is non-obvious | §5 "first Esc retains, second clears" |
| Retained filter | when the input is exited but the filter is kept: the header reads `WORKTREES  🔍payroll  3/12` with a blue dot before the query | same row | a hidden active filter is the classic "where did my rows go" bug | §5 |

**Keyboard:** printable + `Backspace` + `ctrl-w`/`ctrl-u`; `ctrl-n`/`↓`, `ctrl-p`/`↑` move the list
cursor while typing; `Enter` opens the selection; first `Esc` leaves the input keeping the filter,
second clears it.

---

### 3.11 Toasts

Bottom-right, 320px wide, 12px from both edges, stacked max **2**, 3.2s, slide+fade 140ms.
One line, one icon, no title, no close button.

| Case | Icon | Color | Exact text | Sticky? |
| --- | --- | --- | --- | --- |
| Path copied | `clipboard-check` | green | `Path copied` | no |
| URL copied | `clipboard-check` | green | `PR URL copied` | no |
| Job failed | `circle-x` | red | `clone nixos failed · ⏎ log` | **yes, until Esc/Enter** |
| Session slept with kept windows | `moon` | fg.muted | `Slept · kept cc (claude)` | no |
| Duplicate action suppressed | `info` | fg.muted | `Already running` | no |
| Update available | `arrow-up-circle` | fg.muted | `Fleet 0.2.0 available · U` | yes |

**Never a toast:** job started, job succeeded, worktree created, PR refreshed, context switched,
session opened, settings saved. Each of those is already visible in the surface it changed — a toast
for it is duplicated information, which this lens treats as an error.

---

### 3.12 Daemon-unavailable state

Two distinct cases, deliberately:

**A. Warm (we have a snapshot).** A 28px amber banner slides under the context bar; the lists stay
at 100% opacity and remain navigable (they are true, just frozen), while the session glyph column
fades to 40% because *only* liveness is unknowable.

```
 ⚡ fleetd stopped · reconnecting in 3s        ⏎ start now    l log
```

Text cycles exactly: `reconnecting in 3s` → `reconnecting…` → `reconnecting in 6s` (backoff 1,2,4,8,
capped 8s). On reconnect the banner turns green for 800ms with `reconnected` and leaves.

**B. Cold (no snapshot, e.g. first launch).** Full-window centered block, nothing else drawn:

```
                        ⚡ (32px, fg.faint)

                     fleetd is not running

              Fleet keeps your worktrees, jobs and
              terminals in a background daemon.

                    ⏎  Start fleetd      l  Log
```

**Omitted:** socket path, pid, retry counters, stack traces (all behind `l`), a spinner during
backoff (the countdown *is* the progress).

---

### 3.13 Quit-with-running-work

`ctrl-q` (quit app, daemon lives) **never confirms.** Nothing is lost; a confirmation here would
train the user to dismiss confirmations, which is what makes the real one dangerous. If work is
running, a 3.2s toast on next launch is not needed either — the Jobs panel shows it.

`ctrl-shift-q` (quit app **and** stop daemon) always confirms and always enumerates:

```
┌────────────────────────────────────────────────┐
│ ⚠ Quit Fleet and stop fleetd?                  │
├────────────────────────────────────────────────┤
│ 2 jobs are cancelled:                          │
│   ⟳ clone  nixos                               │
│   ⟳ pool   buk/payroll                         │
│                                                │
│ 3 sessions are killed:                         │
│   ◉ buk/payroll#feat-payroll-fix  claude, :3000│
│   ☾ buk/www#chore-deps                         │
│   ● swarm-agent-claude                         │
│                                                │
│ ⌃q quits Fleet and leaves them running.        │
├────────────────────────────────────────────────┤
│ n cancel                       y  Quit & stop  │
└────────────────────────────────────────────────┘
```

The third line is the important one: the confirmation teaches the safe alternative instead of only
threatening. Clone jobs that survive as detached processes (§6) are listed under a fourth group
`1 job keeps running: clone nixos` when applicable — accuracy over drama.

---

### 3.14 First-run and empty states

Every empty state is **two lines**: the fact, then the key. Copy is swarm's, verbatim where it exists.

| Surface | Line 1 | Line 2 (faint) |
| --- | --- | --- |
| No contexts | `No contexts yet.` | `N  create your first context` |
| No repos | `No repos in <context>.` | `n  clone one` |
| No worktrees | `No worktrees yet.` | `n  create one` |
| No worktrees for repo | `No worktrees for <repo> yet.` | `n  create one` |
| Filter miss | `Nothing matches "<filter>".` | `esc  clear` |
| PR mine | `No open PRs authored by you in <scope>.` | `r  refresh` |
| PR review | `No PRs waiting for your review in <scope>.` | `r  refresh` |
| Jobs | `Nothing running.` | `Fleet keeps jobs alive when you close this.` |
| Terminal exited | `process exited` | `^s x  close    ^s c  new` |

First run (no contexts, no repos) additionally draws a single centered 32px `boxes` glyph above
line 1 and nothing else — **no** onboarding carousel, tour, checklist or sample data.

---

## 4. Keystroke counts (steady state, cursor where the flow leaves it)

| # | Task | Keys | Count | Notes |
| --- | --- | --- | --- | --- |
| 1 | Open a worktree | `Enter` | **1** | MRU sort puts the last-used branch at row 0 |
| 1b | …in another repo | `l` `j`×n `Enter` | 3+n | rail → list |
| 2 | Create worktree from branch | `n` `<type>` `Enter` | **2** + typing | base defaults to `origin/<default>`; dialog closes instantly, work becomes a job |
| 3 | Create worktree from a PR | `p` `j`×n `Enter` | **2**+n | `Enter` on the PR row creates *and* opens |
| 4 | Switch between two sessions | `^s` `o` (proposed) | **2** | today: `^s` `s` `Enter` = 3 |
| 5 | Jump to terminal tab 2 | `^s` `2` | **2** | |
| 6 | Sleep the current session | `^s` `S` (proposed) | **2** | from Hub: `s` = 1 |
| 6b | Kill the current session | `^s` `s` `K` `y` | 4 | destructive, deliberately longer |
| 7 | Delete a worktree | `d` `y` | **2** | facts shown between the two keys |
| 7b | Prune a repo | `x` `y` | **2** | dry-run preview between the two keys |
| 8 | Refresh PRs | `r` | **1** | on the PR screen; refreshes both tabs |
| 9 | Check a background job | `J` … `Esc` | **2** | `J` from Hub or `^s J` (3) from a terminal |
| 10 | Copy the worktree path | `y` | **1** | works in Hub and (proposed) `^s y` in a terminal |

Median for the four highest-frequency tasks (open, switch, tab, copy): **1.5 keystrokes**.

---

## 5. Proposed keymap amendments (KEYMAP.md not edited)

| Key | Context | Action | Rationale |
| --- | --- | --- | --- |
| `^s o` | Workspace prefix | Jump to the previously used session (MRU toggle) | Cuts the single most frequent multi-key flow from 3 to 2 (task 4). Mirrors vim's `^^`. |
| `^s S` | Workspace prefix | Sleep this session and return to Hub | `s` alone already means "go to Hub"; the uppercase pair is consistent with Hub's `s`. |
| `^s y` | Workspace prefix | Copy the worktree path of the current session | `y` means copy everywhere else; there is currently no way to get the path from inside a terminal. |
| `^s ?` | Workspace prefix | Already listed as `?`; keep, and make it context-scroll the Help dialog | — |
| `e` | Hub (any pane) | Edit the selected context / repo hooks | Closes inventory §9 gap: "Context update/edit dialog exists but no normal binding opens edit." |
| `0` | Hub › Repos | Jump to the `All` pseudo-repo | `1`–`9` are contexts; `0` is free and `All` is the most-used scope. |
| `a` / `A` | Hub (Normal) | Open the Claude / OpenCode agent session | Currently only reachable as `^s a` from *inside* a terminal, i.e. unreachable from a cold start. |
| `q` | Hub (Normal) | Close overlay / go back one level; **never quits** | swarm's `q` quit the popup; in a persistent native app an accidental `q` must be harmless. `p`/`q` on the PR screen already behaves this way. |
| `H` | Hub | Collapse / expand the repos rail (240 ↔ 44px icon rail) | Reclaims 196px for the worktrees list on a laptop screen without losing the aggregate glyphs. |
| `c` / `f` / `d` | Jobs panel | Cancel job / follow log / dismiss finished | The panel currently has only `Esc`; cancellation is the one job affordance the architecture explicitly promises ("Only an explicit cancel does"). |
| `Space` | Hub | Peek: open the detail panel while held | Optional; makes the closed-by-default detail panel cheap. Drop if hold-to-peek feels un-nvim. |
| `gp` | Hub | Go to PRs (alias of `p`) | Only if `p` is ever reassigned; listed for completeness, not requested. |

Conflicts noticed and *not* amended: `Tab` means "next pane" in Hub but "next tab" on the PR screen
(KEYMAP.md is explicit; keeping it). `i` is "toggle detail" in Hub and "leave Scroll mode" in the
Workspace — different modes, no real collision.

---

## 6. Open questions

1. **Sleeping vs detached.** `SessionState` has no `sleeping`. To render `moon` distinctly from
   `circle` the daemon needs a `sleptAt: Option<Instant>` (or `kept_windows: Vec<String>`) on
   `Session`. Cheap, but it is a `fleet-proto` change — decide before freezing the contract.
2. **Detail panel default.** I propose closed. That is the minimalist answer but it costs a new user
   discoverability. Alternative: open on first run, and remember the user's choice forever after.
3. **Dirty/ahead/PR facts in the list.** `✎` and the `#n` badge come from `inspect`, which is a job,
   not a poll. Do we run a cheap background `inspect --no-fetch` on the visible rows (glanceable but
   more daemon work), or do those columns only populate after `I`? I lean: auto-inspect visible rows
   on a 30s idle timer, never on scroll.
4. **`All` scope for PRs.** The PR screen scopes to the selected repo *or* the whole context. With
   `All` selected in the rail, "context scope" means every repo — that can be 100+ PRs. Cap and
   sort by `updatedAt`, or refuse and require a repo?
5. **Zoom-mode reminder.** Is the 2px amber top edge in `^s z` enough, or does it need a 3s toast on
   entry? I would ship the bar alone and see if anyone gets lost.
6. **Terminal tab activity dot.** Requires the daemon to report "rows changed since last attach" per
   terminal. Is that already implied by dirty-row diffs, or does it need a per-client watermark?
7. **Toast for slept sessions.** `s` sleeps silently unless windows were kept. Is silence acceptable
   for the common case, or does every sleep need acknowledgement?
8. **Light theme.** Tokens are specified, but the terminal palette resolution
   (`Palette(u8)` → theme) needs a second, tested palette. Ship dark-only for v1?
9. **Window minimum.** 900×560 drops the running-labels and repo columns. Is a narrower "companion"
   layout (rail collapsed, single list) worth building, or do we set a hard 900px minimum?
10. **`^s o` vs muscle memory.** tmux users may expect `^s o` to mean "other pane". Confirm the
    binding does not fight an existing habit before committing.
