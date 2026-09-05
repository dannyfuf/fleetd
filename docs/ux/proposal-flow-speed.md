# Fleet UX proposal — lens: **FLOW SPEED**

Optimized for the daily loop of one power user juggling 10–40 worktrees and 5–15 agent
sessions across 3–6 repos. The measure of every pixel in this document is: *does it remove a
keystroke, a glance, or a wait?* If it does none of the three, it is deleted.

Metrics used throughout: list/terminal type is mono **13px / 18px line-height / 7.8px cell**
(`1ch = 7.8px`); dialog prose is UI sans 13px. Default window **1280×800**, minimum **900×560**.
All character-unit column widths below are exact and are what the layout code should implement.

---

## 1. Design principles derived from the lens

1. **Two keys to anywhere I already am.** Every destination that exists in the user's head
   right now (the other session, the other terminal tab, the jobs list, the PR list) is
   reachable in ≤2 keystrokes with no intermediate focus move. Navigation that requires
   "go back to the hub, find the row, press Enter" is a bug, not a flow.
2. **The cursor is sacred.** Background events (status polls, PR fetches, job completions,
   pool refills) never re-sort, re-scroll, or re-focus a list under the cursor. Sort order is
   recomputed only on explicit user action (`r`, filter change, screen change). A row that
   changes state changes its glyph in place.
3. **Long work leaves the dialog immediately.** Anything that can exceed ~200 ms (create,
   clone, prune, delete, inspect, refresh, hooks) is submitted as a daemon job; the dialog
   closes on `Enter` and the row appears at once in a pending state. No spinner ever owns the
   keyboard. This is the architecture's rule 1 turned into a UI law.
4. **Decide from the row, not from the panel.** The list row carries every fact needed for the
   next decision (session state, running keep-alive labels, PR state, age). The detail panel is
   a passive amplifier for the rare deep question — it is never focusable, so it never costs a
   keystroke to skip past.
5. **Ambient over interrogative.** Anything the user would otherwise stop to *ask* (is the
   clone done? is the daemon alive? how many PRs need review?) lives permanently in peripheral
   chrome at a fixed screen position, so checking it costs 0 keys and one saccade.

---

## 2. Global layout

```
1280 × 800, default Hub

 0        232        560                          952            1280
 ┌─────────────────────────────────────────────────────────────────────┐  28px
 │ ▪ buk  2 payroll  3 infra          ⌥ +2 more            ◆ 4 review  │  context rail
 ├──────────┬────────────────────────────────────┬─────────────────────┤
 │ REPOS  3 │ WORKTREES · payroll         12/12  │ DETAIL              │
 │          │                                    │                     │
 │ ▸ All 27 │ ● feat-payroll-fix  ▸cc ▸:3000     │ branch  feat-payr…  │
 │   payro… │ ☾ fix-tz-bug                       │ base    origin/main │
 │   buk    │ ○ spike-新-ui       #482 ✓approved │ repo    bukhr/payr… │
 │   infra  │ ⟳ hot-copy…                        │ path    ~/.fleet/w… │
 │          │                                    │ ─────────────────── │
 │          │                                    │ 1 nvim              │
 │          │                                    │ 2 cc      claude    │
 │          │                                    │ 3 lg                │
 │          │                                    │ opened  2m ago      │
 │  232px   │            716px  (91ch)           │       328px (42ch)  │
 ├──────────┴────────────────────────────────────┴─────────────────────┤
 │ buk · 3 repos · 12 wt   NORMAL   ⟳ 2 jobs  clone bukhr/buk 64%   ●  │  24px
 └─────────────────────────────────────────────────────────────────────┘  status bar
```

Widths: repos rail **232px fixed** (never resizes — a fixed target is a faster target);
detail **328px**, auto-shown at window width ≥ 1120px, toggled by `i`; worktrees/PRs takes all
remaining space. Below 1120px the detail panel is hidden and worktrees gets its width.

The Workspace replaces the middle+right region entirely (full-bleed terminal) and keeps the
context rail and the status bar. Same chrome position in both screens = same saccade.

### Persistent chrome

| What | Where | Why here |
| --- | --- | --- |
| Context rail (numbered tabs `1..9`) | Top strip, full width, 28px | Contexts are the top of the hierarchy and are switched by number; the rail is a legend for `1`–`9` so the user presses the number without counting. Persisting it across Hub/PR/Workspace means context is never a question. |
| Review counter `◆ 4 review` | Context rail, right edge | The single number that pulls the user out of a session ("someone is waiting for me"). Peripheral, never a toast, never interrupts. Clicking / `p` goes to the Review tab. |
| Status bar left: `buk · 3 repos · 12 wt` | Bottom-left, fixed | Answers "where am I / how big is this" without focusing the rail. |
| Status bar center: mode word (`NORMAL`, `FILTER`, `SCROLL`, `TERMINAL`, `^S`) | Bottom-center | Modal editors fail when the mode is invisible; one fixed word prevents the single most expensive mistake (typing a command into a PTY or vice-versa). |
| Jobs pill `⟳ 2 jobs  clone bukhr/buk 64%` | Status bar, right of center | Ambient answer to "is it done yet" (principle 5). Shows count + the newest running job's kind/target/progress. Zero jobs → the pill disappears entirely (no spam). |
| Daemon dot `●` | Status bar, far right, 8px | Binary health of the thing that owns all the user's processes. Green = connected, amber = reconnecting, red = down. Far right so it is never confused with a list glyph. |
| Toast stack anchor | Above the status bar, right-aligned, max 3 | Out of the reading path of the list; never covers the cursor row. |

Everything else is screen-local. There is deliberately **no menu bar row, no breadcrumb, no
title bar text, no clock, no key-hint footer** — hints live in `?` and in the palette.

---

## 3. Screens

### 3.1 Hub — Worktrees

**Purpose:** *"Which of my many parallel workspaces do I go into right now, and which ones can
I retire?"*

```
┌──────────┬──────────────────────────────────────────────────┬───────────────────┐
│ REPOS  3 │ WORKTREES · payroll                       12/12  │ DETAIL            │
│──────────│──────────────────────────────────────────────────│───────────────────│
│▸ All  27 │ ● feat-payroll-fix     ▸cc ▸:3000  #482 ✓apprvd 2m│ branch  feat-pay… │
│  payro 12│ ☾ fix-tz-bug                       #479 ⧗ci     1h│ base   origin/main│
│  buk   11│ ○ spike-new-ui                                  3d│ repo   bukhr/payr…│
│  infra  4│ ⟳ hot-copy → claiming slot…                     – │ path   ~/.fleet/…│
│  ⟳ acme  │ ⚠ remote-thing        @devbox                   5d│ ───────────────── │
│          │                                                  │ session attached  │
│          │                                                  │ 1 nvim            │
│          │                                                  │ 2 cc     claude   │
│          │                                                  │ 3 lg              │
│          │                                                  │ keep-alive  cc,   │
│          │                                                  │             :3000 │
│          │                                                  │ created 3d ago    │
│          │                                                  │ opened  2m ago    │
└──────────┴──────────────────────────────────────────────────┴───────────────────┘
```

**Worktree row grid (list pane 91ch, 1ch padding each side → 89ch usable):**

| col | width | content |
| --- | --- | --- |
| glyph | 2ch | session/job glyph (see icons) |
| branch | flex, min 24ch | `Worktree.branch`, ellipsized mid-string (keeps prefix + suffix, both carry meaning) |
| repo | 14ch | `owner/name` short form — **only** in the `All` pseudo-repo, or width ≥ 104ch |
| running | 18 / 14 / 10 / 0ch | keep-alive labels `▸cc ▸:3000` from `WorktreeStatus.windows[].keepAlive` |
| pr | 15ch | `#482 ✓apprvd` badge when a PR matches this branch |
| age | 6ch, right | relative `lastOpenedAt ?? createdAt` (`2m`, `1h`, `3d`) |

Responsive ladder by list-pane width: ≥104ch = all columns, running 18ch · 88–103ch = running
14ch · 72–87ch = running 10ch, repo column dropped unless `All` · <72ch = glyph + branch + PR
badge + age only.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Repos rail | `All <n>` pseudo-repo first, then repos of the active context, `owner/name` disambiguated only on collision, worktree count right-aligned 3ch, aggregate session glyph | Left, 232px | Filtering scope is a *prefix* decision, read before the list; left edge = first fixation point in LTR reading | Inventory §5 Repos screen: `REPOS` count, All/repo cursor, disambiguated owner/name, worktree count, aggregate glyph |
| `All` selected by default | – | Top of rail | Power users work across repos; making them pick a repo first would add a keystroke to every session open | Inventory §5: "Worktrees pane and `All` pseudo-repo default" |
| Clone-in-progress repo row | `⟳ acme/thing` + progress ratio when known | In rail, in place | The repo *is* the job's target; a separate list would split attention | `CloneJob.status: starting|cloning|failed` |
| List header | `WORKTREES · <repo|All>` + `shown/total` count + filter chip | Above list, 1 row | Confirms scope and that a filter is hiding rows — the #1 source of "where did my worktree go" | Inventory §5 worktrees header: repo/all/filter/count/scroll |
| Session glyph | see icon table | Row col 1 | Left-most = scannable column; the single most decisive fact (can I resume instantly or does it cold-start) | `SessionState = none|detached|attached|unknown` |
| Branch | `Worktree.branch` | Row col 2 | It is the identity the user thinks in ("the tz bug branch"), not the slug or the id | `Worktree.branch` |
| Running labels | `▸cc ▸:3000` | Row, right of branch | Tells the user an agent is *still working* here — the reason to return, and the reason a delete/prune will be blocked | `WorktreeStatus.windows[].keepAlive`, incl. dynamic `:<port>` |
| PR badge | `#<n> <state>` with state icon | Row, right of running | Turns the worktree list into a shipping list; avoids a trip to the PR screen to know if CI went green | `PrState` priority: draft→ci_fail→changes→ci_pending→approved→review |
| Age | relative time | Row, far right | Right edge is the natural home of a magnitude; drives the prune decision | `lastOpenedAt`, `createdAt` |
| Detail panel | branch, base, repo, host, PR, path, session state, window list with commands, keep-alive labels, created/opened | Right, 328px, non-focusable | Answers only the second-order questions; passive so it costs zero keystrokes to ignore | Inventory §5 worktree detail: branch/host/repo/base/PR, local or `host:path`, windows+keepAlive/offline error, session, created/opened |
| Path line in detail | `~`-collapsed absolute path, or `devbox:/srv/...` for remote | Detail, row 4 | `y` copies exactly this string; showing it makes the copy verifiable at a glance | `swarm path`, `y` copies local path or `host:path` |

**Intentionally omitted**

- **Slug column** — it is derivable from the branch 95% of the time and duplicates it visually.
  Shown in the detail panel and in the Create dialog preview only.
- **`baseRef`, `createdAt` absolute, repo default branch** on the row — never used to pick a row.
- **Ahead/behind counters and dirty flag on the row** — they are `inspect` facts, refreshed by a
  job; putting stale numbers on every row invites wrong decisions. They appear where they are
  actually needed: inside the delete/prune confirmation, freshly inspected.
- **A separate "sessions" list** — a session is an attribute of a worktree, not a peer entity.
- **Per-row action buttons / hover affordances** — the mouse is sugar; buttons would steal 3ch
  from the branch column on every row forever.
- **Host column** — appended as `@devbox` inline after the branch only when `Worktree.host` is
  set; a dedicated column would be empty for the ~95% local case.

**States**

| State | Rendering |
| --- | --- |
| Empty, no context | Centered: `no contexts` / `No contexts yet.` / `N  create your first context` |
| Empty, no repos | `No repos in buk.` / `n  clone one` |
| Empty, no worktrees | `No worktrees for payroll yet.` / `n  create one` |
| Filter matches nothing | `Nothing matches “tz”.` / `Esc  clear filter` — list header keeps `0/12` |
| Loading (first snapshot) | Rail + list skeleton rows at 30% opacity, header shows `…` instead of a count; keys are already live (they queue) |
| Error (daemon returned an error for this list) | One-line red strip under the header: `⚠ state.json quarantined as state.json.broken-1725… · J for log` |
| Job running on a row | `loader-circle` spinning replaces the session glyph, tinted by kind; a right-aligned dim progress phrase replaces the age (`claiming slot…`, `hooks 2/3`) |
| Session attached | `circle-dot`, accent green, branch text at full contrast |
| Session detached (sleeping) | `moon`, muted blue, branch text at 80% |
| Session none | `circle-dashed`, 45% grey |
| Session unknown | `circle-help`, amber + tooltip/detail line `status unavailable` |
| Remote host offline | `cloud-off`, amber; detail shows the host error string |
| PR draft / ci_fail / changes / ci_pending / approved / review | badge `#482 draft` `#482 ci✗` `#482 chgs` `#482 ci⧗` `#482 ✓apprvd` `#482 review` with the icon of that state |

**Icons (Lucide)**

`circle-dot` attached · `moon` detached/sleeping · `circle-dashed` no session ·
`circle-help` unknown · `cloud-off` remote offline · `loader-circle` job running ·
`folder-git-2` repo · `layers` context · `git-branch` worktree · `server` `:<port>` keep-alive ·
`bot` claude keep-alive · `sparkles` opencode · `git-pull-request-draft` draft ·
`circle-x` ci_fail · `message-square-warning` changes_requested · `clock` ci_pending ·
`circle-check` approved · `eye` review · `git-merge` merged · `git-pull-request-closed` closed ·
`trash-2` delete · `scissors` prune · `power` kill · `clipboard-copy` copy · `external-link` browser.

**Keyboard flow.** `j/k` move · `l`/`Tab` repos→worktrees (and **not** into detail; see §5) ·
`Enter`/`o` open (sleeps previous) · `O` open keeping previous · `n` create · `d` delete ·
`x` prune repo · `s` sleep · `K` kill · `I` inspect · `y` copy path · `b` browser ·
`/` filter · `:` palette · `p` PR screen · `J` jobs · `i` detail toggle · `1`–`9`/`gt`/`gT` context.

---

### 3.2 Hub — Pull requests (Mine / Review)

**Purpose:** *"What of mine is blocked, and what is blocking someone else — and can I start
working on one right now without leaving this list?"*

```
┌──────────┬──────────────────────────────────────────────────────────┬────────────┐
│ REPOS  3 │  MINE 7    REVIEW 4                       fetched 32s ago│ DETAIL     │
│──────────│──────────────────────────────────────────────────────────│────────────│
│▸ All  27 │ ● #482 Fix payroll rounding on…  feat-payroll  ✓apprvd 2m│ +142 −18   │
│  payro 12│ ○ #479 Timezone drift in accrual  fix-tz-bug   ci✗     1h│ target main│
│  buk   11│ ○ #468 Bump ruby to 3.4          chore/ruby34  review  3h│ checks ✗ 2 │
│  infra  4│ ○ #455 [draft] New settings UI   spike-new-ui  draft   1d│ review  ✓1 │
│          │                                                          │ worktree — │
│          │                                                          │ → payroll/ │
│          │                                                          │   feat-pay…│
└──────────┴──────────────────────────────────────────────────────────┴────────────┘
```

**PR row grid** (inventory §5 widths, plus a 2ch presence gutter):

| col | width | content |
| --- | --- | --- |
| presence | 2ch | local worktree glyph: `circle-dot`/`moon`/`circle-dashed` = exists+attached / exists+sleeping / not created |
| number | 6ch | `#482` |
| title | flex | `PullRequest.title`, ellipsized at the end |
| author | 12ch @ ≥70ch, 16ch @ ≥130ch | Review tab only (Mine is always me) |
| branch | 12ch @ ≥90ch | `headRefName` |
| repo | 10ch @ ≥110ch | only when scope is multi-repo |
| state | 8ch | derived `PrState` + icon |
| time | 7ch, right | `updatedAt` age |

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Tab strip `MINE 7 / REVIEW 4` | tab name + count, active underlined | Top of list pane | Two tabs, two numbers, one glance — the counts *are* the reason to switch | `PrTab = "mine" \| "review"`; §5 "tab counts/loading/fetch age" |
| Fetch age `fetched 32s ago` | right of tab strip | Top-right of list | PR data is cached (`prTtlSeconds: 90`); the user must know if they are looking at a stale answer before hitting `r` | PR cache `fetchedAt`, `github.prTtlSeconds` |
| Presence glyph | 2ch gutter | Row col 1 | Converts the list into "capture vs. resume": same key `Enter` does the right thing, the glyph says which will happen | PR/worktree match rule: `baseRef === "pull/<n>/head"` or `branch === headRefName` |
| Derived state | one of 6 states, priority-ordered | Row, right | The whole point of the screen; one token instead of 3 raw fields (`isDraft`, `checks`, `reviewDecision`) | PR-state priority draft→ci_fail→changes→ci_pending→approved→review |
| Detail: additions/deletions, target, checks, review, labels, url, proposed destination | | Right panel | The "should I bother" data, and for uncreated PRs the *exact* worktree that `Enter` will create (`payroll/feat-payroll-fix`, pull ref, fork note) | §5 PR detail: target, ±, worktree path/session or proposed destination + pull ref/fork, checks/review/labels/age/url |
| Scope inherited from the repos rail | – | Left rail unchanged | Same rail, same `1..9`, no new mental model; `All` = active-context repos | §5: "Scope selected repo or active-context repos" |

**Intentionally omitted**

- **Labels on the row** — high cardinality, low decision value; detail panel only.
- **CI check-run breakdown** — one aggregate `PrChecks` token on the row; the browser (`b`) is
  one key away and is a far better check inspector than anything we would build.
- **A merged/closed section** — swarm's tabs are open-PR views; showing closed PRs would add
  scrolling to every session-capture.
- **Reviewer avatars** — pixels with no keystroke value in a keyboard app.
- **A third "All PRs" tab** — Mine ∪ Review with Review excluding keys already in Mine is
  already complete coverage for one user.

**States**

| State | Rendering |
| --- | --- |
| Loading a tab | Tab count replaced by `…`; existing rows stay visible and usable (cache-first), dimmed to 70% |
| Empty Mine | `No open PRs authored by you in buk.` |
| Empty Review | `No PRs waiting for your review in buk.` |
| Fetch error | Row-less red strip under the tabs: `⚠ gh: <120-char error> · r retry` and the **stale rows stay listed** with `fetched 14m ago` in amber |
| PR whose worktree is being created | presence glyph → `loader-circle`; row stays in place |
| Cross-repo PR | branch column shows `fork:headRefName`; detail names the `pr/<number>` local branch |

**Keyboard flow.** `Tab`/`S-Tab`, `h`/`l` switch tabs · `j/k` move · `Enter`/`o` open-or-create
the PR worktree (sleeping previous) · `O` keep previous awake · **`c` create without opening**
(proposed, §5) · `b` browser · `y` copy URL · `r` force refresh both tabs · `p`/`q` back to
worktrees · `/` filter · `1`–`9` re-scope by context.

---

### 3.3 Workspace

**Purpose:** *"I am inside this worktree — get out of my way, and let me move between its
terminals and my other sessions instantly."*

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⎇ feat-payroll-fix   bukhr/payroll   #482 ✓apprvd     ~/.fleet/…/feat-pay…  │ 26px
├─────────────────────────────────────────────────────────────────────────────┤
│ 1 nvim   2 cc ●   3 lg        4 test ▸:3000                              +  │ 26px
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ❯ claude                                                                   │
│  ⏺ Reading src/payroll/rounding.rb…                                         │
│                                                          ┌────────────────┐ │
│                                                          │ SCROLL 412/2000│ │ ← scroll pill
│                                                          └────────────────┘ │
│                                                                             │
│  ┌────┐                                                                     │
│  │ ^S │  s hub · 1-9 tab · c new · x close · [ scroll · w last session      │ ← prefix hint
│  └────┘   (appears only after 400 ms of hesitation)                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ payroll/feat-payroll-fix        TERMINAL        ⟳ 1 job  prune payroll   ●  │ 24px
└─────────────────────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session header | `⎇ branch`, `owner/name`, PR badge if any, `~`-collapsed path right-aligned | Top, 26px, one line | Identity of what you are typing into — the single most dangerous thing to be wrong about when 8 agents run in parallel. Path right-aligned so it never pushes the branch off-screen | `Session { kind: Worktree(id) }`, `Worktree.path`, PR match |
| Terminal tab strip | `<index> <name>` per terminal, activity dot, keep-alive icon, `+` at the right end | Under the header, 26px | Indexes are the argument to `ctrl-s 1..9`; the strip is the legend for that keymap, same as the context rail | `Terminal { id, name, command, status, title }`; default layout `nvim | cc | lg` |
| Activity dot `●` | 1 accent dot after the tab name | Inside the tab | Tells you which agent produced output while you were elsewhere — the reason to switch tabs at all | `Terminal.status`, VT dirty rows |
| Keep-alive icon on a tab | `bot` / `server` | Inside the tab, after the name | Marks the tabs that `sleep` will preserve, so "can I close this window" is answered before pressing `x` | sleep policy §4: keep-alive rules and labels |
| Terminal area | painted cell grid | All remaining space | It is the work; everything else is 52px of chrome | Terminal pipeline in ARCHITECTURE |
| Scroll pill | `SCROLL <offset>/<scrollback_len>` amber, + `v` `y` hints on the second line while a selection is active | Top-right corner **inside** the terminal area, 2px margin | Overlays the least information-dense corner of a shell (right of the prompt), never shifts the grid — a mode indicator that reflows the terminal is a flow killer | `viewport { scrollback_len, offset }`; Scroll mode in KEYMAP |
| Prefix hint | `^S` pill + the 6 most-used prefix keys | Bottom-left, inside the terminal area, **delayed 400 ms** | The expert never sees it (they type the second key in <200 ms); the returning user gets it exactly when they hesitate | Prefix mode is one-shot; discoverability without a permanent footer |
| Status bar session name | `payroll/feat-payroll-fix` | Bottom-left, replaces the Hub's context summary | Keeps a second, always-visible identity anchor when `ctrl-s z` has hidden the header | local session name rule `repo/slug` |

**Zoom (`ctrl-s z`)** hides the session header and the tab strip (52px back to the grid); the
status bar stays because it carries the mode word and the daemon dot.

**Intentionally omitted**

- **A permanent key-hint footer** — the status bar's mode word plus the delayed prefix hint
  cover it; a static footer is 24px of decoration for an expert.
- **Window/pane splitting inside a terminal** — tabs + tmux-free sessions are enough; splits
  would demand a whole new navigation keymap and duplicate what nvim already does inside tab 1.
- **Per-terminal scrollback counters / byte stats** — noise.
- **A close (×) button per tab** — `ctrl-s x` is faster and the button would shrink the strip.
- **Session breadcrumb (`context ▸ repo ▸ worktree`)** — the context rail already shows the
  context; the header already shows repo and branch.

**States**

| State | Rendering |
| --- | --- |
| Attaching (first frame not yet received) | Grid area shows a single dim centered line `attaching…`; keys typed here are buffered and flushed on the first frame |
| Terminal process exited | Tab name struck through, dim; grid shows the final screen plus a bottom strip `process exited (code 1) · ctrl-s x close · ctrl-s c new` |
| Session sleeping (opened from Hub while asleep) | Tabs rebuild with `loader-circle` per tab as each PTY spawns; the header shows `waking…` for ≤1.5 s |
| Job running for this worktree | Jobs pill in the status bar tints accent; no overlay in the terminal ever |
| Alt-screen app running | Scroll pill is suppressed (scrollback is meaningless); `ctrl-s [` shows a 1.6 s toast `no scrollback in alt-screen` |
| Daemon dropped | Grid freezes at 55% opacity, amber strip over the tab strip: `fleetd disconnected — reconnecting… your processes keep running`; keys are dropped, not buffered |
| Unknown session state | Not possible in Workspace (we are attached); Hub carries `unknown` |

**Icons.** `square-terminal` generic tab · `bot` claude tab · `sparkles` opencode ·
`git-branch` header · `file-code` nvim tab · `git-compare-arrows` lazygit tab ·
`plus` new tab · `chevrons-up` scroll pill · `command` prefix pill.

**Keyboard flow.** All keys → PTY. `ctrl-s` then: `s` hub · `1`–`9` tab · `h/l`, `p/n` prev/next
tab · `Tab` last tab (proposed) · `w` last session (proposed) · `c` new tab · `x` close tab ·
`,` rename · `[` scroll · `]` paste · `a`/`A` agent session · `z` zoom · `J` jobs · `?` help ·
`ctrl-s` literal · `Esc` cancel.

---

### 3.4 Jobs panel

**Purpose:** *"What is the daemon doing for me right now, is anything stuck, and can I read the
log without stopping my work?"*

```
                       ┌──────────────────────────────────────────┐ 420px sheet
                       │ JOBS            2 running · 3 done       │
                       │──────────────────────────────────────────│
                       │ ⟳ clone   bukhr/buk            0:42   64%│
                       │   Receiving objects: 64% (81k/126k)      │
                       │ ⟳ prune   payroll              0:07      │
                       │   inspecting 11 worktrees…               │
                       │ ✓ hooks   payroll#feat-payroll  0:12     │
                       │ ✓ pool    payroll .hot          0:31     │
                       │ ✗ fetch   infra                 0:03     │
                       │   fatal: could not read Username for …   │
                       │──────────────────────────────────────────│
                       │ Enter log · c cancel · x clear done · J  │
                       └──────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Right sheet, 420px, non-blocking | overlays the detail panel only | Right edge | The list under the cursor stays visible, so the user can correlate a job with its row without closing the panel | Jobs are never attached to a client connection (ARCHITECTURE §Jobs) |
| Job row | status icon, `kind` (8ch), `target` (flex), elapsed `m:ss` (6ch), percent when known (4ch) | one line | `kind + target` is the identity swarm's footer got wrong (`hot-copy:<repo>` didn't match a repo row); here target is the real domain id | `Job { id, kind, target, status, last progress line, log path, timestamps, cancellable }` |
| Progress sub-line | `Job.lastProgressLine`, dim, 1 line, truncated to 40ch | under the row, only while running or failed | The one line that tells you if a clone is stuck; two lines per running job is the whole budget | inventory §6 logging: raw logs per job |
| Log expansion (`Enter`) | last 12 lines of `logs/jobs/<id>.log`, mono 11px, scrollable | in place, pushes rows down | Debugging without leaving the app or hunting the log path | `logs/jobs/<job-id>.log` |
| `c` cancel | only on rows where `cancellable` | – | Explicit cancel is the *only* thing that stops background work (guiding rule 1) | JobManager cancellation tokens |

**Intentionally omitted**

- **Completed jobs older than the current app session** — history belongs in the log files.
- **A job queue/graph view** — per-repo mutexes and concurrency limits are implementation
  detail; the user only needs "running / done / failed".
- **Notifications on job success** — the row and the pill already changed. Only **failures**
  raise a toast, and only once.

**States.** Empty: `Nothing running.` + `Jobs you start keep running when the app is closed.` ·
Running: `loader-circle` accent · Done: `circle-check` dim green, auto-collapses its sub-line ·
Failed: `circle-x` red, sub-line = error, sticky until `x` · Cancelled: `circle-slash` grey ·
Daemon down: `The daemon is unreachable — job state is from <n>s ago` amber strip at the top.

**Icons.** `activity` panel title · `cloud-download` clone · `copy-plus` pool/hot-copy ·
`terminal` hooks · `scissors` prune · `trash-2` delete · `refresh-cw` fetch/refresh ·
`search-check` inspect · `arrow-up-circle` update.

**Keyboard flow.** `J` toggle (from anywhere, including `ctrl-s J` in a terminal) · `j/k` move ·
`Enter` expand log · `c` cancel · `x` clear finished · `Esc`/`J` close and **restore the exact
prior focus** (pane, row, terminal, and mode).

---

### 3.5 Dialog — Create worktree

**Purpose:** *"Make me a fresh copy on a branch, now, and let me get back to work while it
builds."*

```
        ┌────────────────────────────────────────────────────┐ 520px
        │ ⎇ New worktree · bukhr/payroll                     │
        │────────────────────────────────────────────────────│
        │ Branch   feat-payroll-fix▮                         │
        │ Slug     feat-payroll-fix        ~/.fleet/w/payro… │
        │ Base     ┌──────────────────────────────────┐      │
        │          │ ▸ origin/main            default │      │
        │          │   origin/release-2.4             │      │
        │          │   origin/feat-payroll-base       │      │
        │          │   fix-tz-bug            (worktree)│     │
        │          └──────────────────────────────────┘      │
        │ Host     local  ‹ ›  devbox                        │
        │────────────────────────────────────────────────────│
        │ ⚡ prepared copy ready — opens instantly            │
        │ Enter create & open · ⌥Enter create only · Esc     │
        └────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Branch field (focused on open) | free text, validated live by `validateBranch` | Row 1 | The only field the user always types; being pre-focused makes creation `n` + text + `Enter` | `create` requires a valid branch; branch defaults to slug |
| Slug preview + destination path | derived `slugify(branch)`, dim; full destination path right-aligned dim | Row 2, read-only | The slug is a derived fact 99% of the time; showing it read-only removes a field *and* a `Tab` | slug rules: `slugify(slug) === slug`, `${worktreesDir}/<owner>/<name>/<slug>` |
| Base list, 6 rows | `origin/<defaultBranch>` first and preselected, then old `baseRef`s / origin branches, fuzzy-filtered by typing in the field; free text accepted | Row 3 | Base is a *choice from a small set* far more often than a typed ref; a preselected default means `Enter` skips it entirely | Create dialog: fuzzy base (6), initial bases default + old baseRef/origin branch, free text allowed |
| Host selector | `local ‹ › devbox`, hidden entirely when `config.hosts` is empty | Row 4 | Zero-cost when unused; `←`/`→` when used | `defaultHost`, sorted hosts, host `←`/`→` |
| Pool readiness line | `⚡ prepared copy ready — opens instantly` / `⧗ no prepared copy — first open takes ~20 s` / `⟳ preparing…` | Footer, above actions | Sets the expectation that decides whether the user waits or switches away — a pure flow-speed signal | prepared-copy slots `.hot`/`.hot.<n>`, `hotPoolSize` |
| Actions line | `Enter create & open · ⌥Enter create only · Esc cancel` | Footer | `create only` is the batch-capture path (queue three worktrees, open none) | `create` TUI path: "TUI schedules separate operation and can open immediately" |

**Intentionally omitted**

- **Hooks editor** — repo-level config, belongs in Settings/repo detail, not in the hot path.
- **A "fetch base first" checkbox** — the daemon's freshness rules decide; a checkbox here
  would make the user responsible for an invariant they cannot evaluate.
- **A progress bar inside the dialog** — the dialog closes on `Enter`; progress lives on the
  row and in the jobs pill (principle 3).
- **Repo selector** — the repo is the rail selection; opening `n` from `All` uses the repo of
  the highlighted worktree, falling back to a repo picker only when the list is empty.

**States.** Validating: invalid branch → the field underline turns red and the footer becomes
the exact reason (`branch cannot contain "..";`), `Enter` is inert · Base list fetching:
`⟳ fetching branches…` in the list frame, the default row already selectable · Duplicate:
`payroll#feat-payroll-fix already exists — Enter opens it instead` (turning an error into a
shortcut) · Submitted: dialog closes in <16 ms, a pending row with `loader-circle` appears at
the top of the list, focused.

**Icons.** `git-branch` title · `zap` prepared-copy ready · `hourglass` no prepared copy ·
`server` host selector.

**Keyboard flow.** `n` opens · type branch · `Tab`/`S-Tab` fields · `↓`/`ctrl-n`, `↑`/`ctrl-p`
base · `←`/`→` host · `Enter` create & open · `⌥Enter` create only · `Esc` cancel.

---

### 3.6 Dialog — Clone repo

**Purpose:** *"Get a repo I can work in, by typing three letters of its name."*

```
        ┌────────────────────────────────────────────────────┐ 560px
        │ ⌄ Clone repo · into context buk                    │
        │ ⌕ payr▮                                            │
        │────────────────────────────────────────────────────│
        │ ▸ 🔒 bukhr/payroll     Payroll engine        2d    │
        │   🔒 bukhr/payroll-ui  Front-end for payroll 3w    │
        │      acme/payrolls     Toy example           1y    │
        │   …8 results                                       │
        │────────────────────────────────────────────────────│
        │ ssh · clones in the background · Enter · Esc       │
        └────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Query field, focused | 150 ms debounce | Row 1 | Search is the whole dialog | Clone dialog: 150 ms debounce, 8 remote results |
| 8 result rows | privacy icon, `fullName`, description (ellipsized), `updatedAt` age | Middle | Exactly the fields swarm shows; `updatedAt` disambiguates forks and dead mirrors | `RemoteRepo {owner,name,fullName,description,isPrivate,updatedAt}` |
| Target context line | `into context buk` in the title | Title row | Cloning into the wrong context costs a later `m`; naming it costs 0 keys | Repo belongs to a `contextId`; clone assigns it |
| Protocol + background note | `ssh · clones in the background` | Footer | Tells the user `Enter` frees them immediately | `github.cloneProtocol`, detached clone survives popup close |

**Intentionally omitted:** manual URL field (an `owner/name` or a pasted URL typed into the
query is detected and offered as the first row — no second field), branch/depth options,
destination path (it is `reposDir` by contract).

**States.** Idle: `Type to search GitHub repos in buk's owners.` · Searching: `⟳` in the query
row, previous results kept · No results: `Nothing on GitHub matches “payr”.` · `gh` unavailable:
`⚠ gh auth status failed — b open GitHub settings` · Offline: last cache shown with
`cached <age>` · Submitted: dialog closes, a `⟳ acme/thing` row appears in the repos rail.

**Icons.** `cloud-download` title · `search` query · `lock` private · `globe` public.

**Keyboard flow.** `n` in repos pane · type · `↓`/`ctrl-n`, `↑`/`ctrl-p` · `Enter` clone ·
`Esc` cancel (aborts only the search request, never a started clone).

---

### 3.7 Dialog — Confirm (delete / prune / kill / close terminal / delete context)

**Purpose:** *"Tell me exactly what I am about to lose, in facts, in under a second."*

```
        ┌────────────────────────────────────────────────────┐ 520px
        │ ⌫ Delete worktree                                  │
        │   bukhr/payroll#feat-payroll-fix                   │
        │────────────────────────────────────────────────────│
        │   Uncommitted changes    yes · 12 files            │
        │   Commits not on origin  3                         │
        │   Session               attached · claude, :3000   │
        │   Pull request          #482 open                  │
        │────────────────────────────────────────────────────│
        │ The copy moves to ~/.fleet/trash and is removed in │
        │ the background. This cannot be undone.             │
        │                                                    │
        │            y  Delete          n  Cancel            │
        └────────────────────────────────────────────────────┘
```

Fact rows are the freshest `WorktreeInspection`; if the inspection is older than 30 s the
dialog opens instantly with the cached facts, runs `I` in the background, and swaps the values
in place with a 120 ms highlight (never blocks the confirm).

| Variant | Exact title / body / footer |
| --- | --- |
| Delete worktree | as above. If facts are unavailable: `Safety facts unavailable (<warning>). Deleting anyway is unconditional.` |
| Delete repo | `⌫ Delete repo bukhr/payroll` · `11 worktrees, 2 live sessions and the base clone will be deleted.` · `Sessions are killed. Everything moves to ~/.fleet/trash. This cannot be undone.` · `y Delete repo · n Cancel` |
| Delete context | `⌫ Delete context buk` · `3 repos · 27 worktrees · 5 live sessions` · `Everything in this context is deleted. This cannot be undone.` · `y Delete context · n Cancel` |
| Prune | `✂ Prune payroll — 4 of 11 eligible` · eligible list (branch + reason `merged · clean · no session`) · collapsed `7 skipped — s to show` with reasons (`dirty`, `2 unique commits`, `session attached`, `running: claude`) · `y Prune 4 · n Cancel · s Show skipped` |
| Kill session | `⏻ Kill session payroll/feat-payroll-fix` · `Running: claude, server on :3000` · `Processes are terminated immediately; unsaved work in them is lost.` · `y Kill · n Cancel` |
| Close terminal (keep-alive) | `✕ Close terminal 2 · cc` · `claude is running here.` · `y Close · n Cancel` |
| Quit daemon | see §3.14 |

**Intentionally omitted:** a "don't ask again" checkbox (the confirm *is* the safety net; the
escape hatch is that `y` is a single key), a typed-name confirmation (too slow for a tool used
20×/day — the mitigation is trash + background removal, which is recoverable for minutes),
diff previews.

**States.** Facts loading: values render as `…` and the dialog is already confirmable ·
Facts errored: amber line with the exact swarm warning text (`fetch failed`, `gh unavailable`,
`upstream gone`, …) · Nothing eligible (prune): the dialog does not open; a toast says
`Nothing to prune in payroll — 11 skipped · J for reasons`.

**Icons.** `trash-2` delete · `scissors` prune · `power` kill · `x` close terminal ·
`triangle-alert` danger accent on the title row.

**Keyboard flow.** `y`/`Enter` confirm · `n`/`Esc`/`q` cancel · `s` toggle skipped (prune only).
Confirm dialogs never take focus away from the row: on cancel the exact prior cursor is restored.

---

### 3.8 Dialogs — New/Edit context, Assign repo to context

```
 ┌──────────────────────────────────┐   ┌──────────────────────────────────┐
 │ ⧉ New context                    │   │ → Move bukhr/payroll to…         │
 │ Name    Buk HR▮                  │   │ ▸ buk        3 repos · bukhr     │
 │ Id      buk-hr                   │   │   infra      1 repo  · acme-ops  │
 │ Owners  bukhr, acme-hr           │   │   personal   0 repos · dannyfuf  │
 │ Enter save · Esc                 │   │ Enter move · Esc                 │
 └──────────────────────────────────┘   └──────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Name field | free text | Row 1 | The only typed field | `Context.name` |
| Id preview | live `slugify(name)`, read-only, dim | Row 2 | The id is what appears in ids and paths; showing it prevents surprises without a second field | `ContextId` regex + creation slugification |
| Owners field | comma-separated | Row 3 | Owners scope GitHub repo search and PR "mine" | `Context.owners` |
| Assign list | context name, repo count, owners | Middle | Repo counts make the target obvious when contexts have similar names | `m` move repo (assign dialog); context detail: owners + counts |

**Intentionally omitted:** color/icon per context (the number in the rail is the identity),
context ordering UI (rail order = creation order = the `1..9` mapping the user memorized;
reordering would invalidate muscle memory), a description field.

**States.** Duplicate id: `A context with id "buk-hr" already exists.` and `Enter` inert ·
Empty owners: allowed, footer warns `Without owners, GitHub repo search and PR "mine" are empty.`
Edit mode: same dialog, title `⧉ Edit context buk`, id read-only if repos exist.

**Icons.** `layers` context · `arrow-right-left` assign/move · `users` owners.

---

### 3.9 Dialog — Settings

**Purpose:** *"Change the two or three things I actually change, without opening JSON."*

```
        ┌──────────────────────────────────────────────────────┐ 560px
        │ ⚙ Settings                                           │
        │ Agent            ‹ claude ›  opencode                │
        │ Agent command    claude --dangerously-skip-…▮        │
        │──────────────────────────────────────────────────────│
        │ Sleep            [x] enabled          grace 2000 ms  │
        │   [x] claude     process  (^|/)claude( |$)           │
        │   [x] opencode   process  (^|/)opencode( |$)         │
        │   [x] codex      process  (^|/)codex( |$)            │
        │   [x] server     listening port                      │
        │──────────────────────────────────────────────────────│
        │ Windows      nvim `nvim .` · cc `{agent}` · lg `lazy…`│
        │ Pool         1 slot · fresh 60 s · refresh 300 s     │
        │ GitHub       ssh · repos 3600 s · PRs 90 s           │
        │ Hosts        devbox → ssh danny@devbox               │
        │ Home         ~/.fleet   protocol 3   fleet 0.1.0+ab1 │
        │──────────────────────────────────────────────────────│
        │ Enter save · e edit config.json · Esc                │
        └──────────────────────────────────────────────────────┘
```

Editable: agent, per-agent command, sleep enabled, per-rule enabled. Read-only (grey, aligned in
a single block so the eye skips it): grace, windows, pool, GitHub TTLs/protocol, hosts, home,
protocol, version — matching swarm's editable surface, plus one addition: **`e` opens
`config.json` in `$EDITOR` inside a new terminal tab of the current session**, which turns the
"many settings require JSON" pain point into a 1-key path instead of a new dialog.

**Intentionally omitted:** a full form for every config key (the schema is large, the edit
frequency is near zero, and `e` covers it), theme/font pickers in v1, a restart button.

**States.** Dirty: title shows `⚙ Settings ·` in accent and the footer becomes
`Enter save · Esc discard changes` · Invalid regex in a rule: red underline, rule row shows
`invalid pattern — rule is skipped` · Save failed: red footer with the exact write error.

**Icons.** `settings-2` title · `bot` agent · `moon` sleep · `layout-grid` windows ·
`zap` pool · `github` GitHub · `server` hosts.

---

### 3.10 Dialog — Help

Full-screen overlay at 88% width, **three columns** so the whole keymap fits without scrolling
(that is the point: a help you must scroll is a help you will not read).

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ ⌨ Fleet 0.1.0+ab12cd   ·  protocol 3  ·  fleetd running 4h              │
 │ GLOBAL             HUB · WORKTREES        WORKSPACE  (after ^S)          │
 │ j k  move          Enter o  open          s   hub                        │
 │ …                  …                      …                              │
 │ HUB · REPOS        HUB · PULL REQUESTS    SCROLL MODE                    │
 │ …                  …                      …                              │
 │ Esc / ? / q close                                                        │
 └──────────────────────────────────────────────────────────────────────────┘
```

Columns are populated **context-sensitively first**: the column for the screen you pressed `?`
from is rendered first and in accent. Version and daemon uptime live in the title row because
this is the only screen where they are ever wanted.

**Intentionally omitted:** prose explanations, a searchable help (that is what `:` is), links.

---

### 3.11 Command palette

**Purpose:** *"Jump anywhere or do anything by name when I do not remember the key."*

```
        ┌────────────────────────────────────────────────────┐ 600px, top-center,
        │ : pay fix▮                                         │ 120px from top
        │────────────────────────────────────────────────────│
        │ GO                                                 │
        │ ▸ ● payroll#feat-payroll-fix   session attached    │
        │   ☾ payroll#fix-tz-bug         sleeping           │
        │   #482 Fix payroll rounding…   PR · mine     ↵    │
        │ DO                                                 │
        │   Create worktree in payroll                  n   │
        │   Prune payroll                               x   │
        │ CONTEXT                                            │
        │   buk                                         1   │
        │────────────────────────────────────────────────────│
        │ 12 of 63 · Enter run · Esc                         │
        └────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Query row with `:` sigil | fuzzy over **objects and commands** | Top | Top-center is out of the list's way and matches nvim/editor muscle memory | `:` fuzzy palette (inventory §5) |
| `GO` section first | worktrees (with session glyph), open PRs, repos, contexts | First section | This is the flow-speed upgrade: the palette is the universal jump, so a session is reachable from *inside another session* in `ctrl-s s : pay ↵` without navigating a list | extends swarm's "commands + contexts" palette |
| `DO` section | valid commands only, with their keybinding right-aligned | Second | Right-aligned keys teach the shortcut every time it is used — the palette trains itself out of the loop | `label/keys` in swarm's palette |
| Result cap | 10 rows total, sections in fixed order GO → DO → CONTEXT | – | A fixed maximum keeps `Enter` predictable (top match never moves below the fold) | first 10 valid commands (inventory) |

**Intentionally omitted:** command history, `!`-style shell escape, multi-select, fuzzy over
file contents.

**States.** Empty query: `GO` shows the 5 most-recently-opened worktrees (MRU), `DO` shows the 5
most-used commands · No match: `Nothing matches “xyz”.` · Command invalid in the current context
is not listed at all (never greyed — a greyed row costs a `j`).

**Icons.** `chevron-right` sigil · reuses all session/PR glyphs verbatim so the palette rows and
the list rows read identically.

---

### 3.12 Filter bar

**Purpose:** *"Cut 40 rows to 2 without leaving the list."*

The filter is **inline, replacing the list header row** — never an overlay, never a layout
shift (a shifting list forces re-fixation; that is the whole cost of a filter done wrong).

```
 before:  WORKTREES · payroll                                          12/12
 during:  / tz▮                                                         2/12   ×
```

| Element | Content | Position | Why here |
| --- | --- | --- | --- |
| `/` sigil + live query | in the header row | Same y-position as the header it replaces — zero shift |
| `shown/total` | right-aligned in the same row | The count is the feedback that the filter is working |
| Retained-filter chip | after `Esc` once, the header becomes `WORKTREES · payroll  ⌕tz  2/12` with the chip in accent | Makes a retained filter impossible to forget (the "where did my rows go" bug) |

**Keyboard flow.** `/` enter · printable/`Backspace`/`ctrl-w`/`ctrl-u` edit · `ctrl-n`/`↓`,
`ctrl-p`/`↑` move the selection while still typing · `Enter` **opens the selected row** (so
`/tz↵` is a complete open in 4 keystrokes) · first `Esc` exits the input keeping the filter ·
second `Esc` clears it.

**States.** No match: list area shows `Nothing matches “tz”.` and `Enter` is inert ·
Filter survives a refresh; it does **not** survive a repo change or a screen change.

---

### 3.13 Toasts

Bottom-right, above the status bar, 320px wide, max **3** stacked, **3.2 s** each, 1.6 s for
confirmations of instant actions.

**Rule:** a toast is only allowed when there is **no row and no pill** that already shows the
outcome. In practice that leaves exactly four kinds:

| Trigger | Text | Duration | Icon |
| --- | --- | --- | --- |
| Copy | `Copied ~/.fleet/worktrees/bukhr/payroll/feat-payroll-fix` | 1.6 s | `clipboard-check` |
| Job failed | `clone bukhr/buk failed · J for log` | sticky until dismissed or `J` | `circle-x` |
| Action refused with a reason | `Nothing to prune in payroll — 11 skipped · J for reasons` | 3.2 s | `info` |
| Mode/no-op feedback | `no scrollback in alt-screen` | 1.6 s | `chevrons-up` |

**Intentionally omitted:** success toasts for create/delete/open/sleep/refresh (the row changed;
that *is* the feedback), progress toasts (the pill), duplicate toasts (identical text within
1 s coalesces into `×2`).

---

### 3.14 First run, empty states, daemon-unavailable, quit-with-running-work

**First run** (no contexts, no repos): a single centered card, no chrome noise, and a 3-step
path where every step is one key.

```
                ┌────────────────────────────────────┐
                │            ⛵ Fleet                 │
                │                                    │
                │  1  N   create a context           │
                │  2  n   clone a repo               │
                │  3  n   create a worktree          │
                │                                    │
                │  Coming from swarm?                │
                │  i   import ~/.swarm config+state  │
                │                                    │
                │  ?  keys      ,  settings          │
                └────────────────────────────────────┘
```

The `i` import row appears only when `~/.swarm/state.json` exists; it runs
`fleet import --from-swarm` as a job and lands the user in a fully populated Hub. Rationale:
for this user the empty state is a *migration*, not an onboarding.

**Empty states** use swarm's exact copy plus the key that fixes it, always as a two-line
centered block in the affected pane only (never full-screen), so surrounding panes stay usable:
`no contexts` / `No contexts yet.` + `N create your first context` · `No repos in <context>.` +
`n clone one` · `No worktrees [for <repo>] yet.` + `n create one` · `Nothing matches “<filter>”.`
+ `Esc clear` · `No open PRs authored by you in <scope>.` · `No PRs waiting for your review in
<scope>.`

**Daemon unavailable.** Two cases, deliberately different:

| Case | Presentation |
| --- | --- |
| We have a snapshot (daemon died / restarting) | **No modal.** The Hub stays fully rendered at 55% opacity, the daemon dot goes red, and a 20px amber strip appears directly above the status bar: `fleetd disconnected · reconnecting in 2s (attempt 3) · r retry now · l open log`. Read-only actions still work: `j/k`, `y` (copy path), `b` (browser), `/`. Mutating keys flash the strip instead of erroring. Reconnect restores the exact cursor and re-attaches terminals. |
| We never had a snapshot (cold start failure) | Centered card: `plug-zap-off` icon, `fleetd is not responding`, `Fleet keeps your worktrees, jobs and terminals in a background daemon.`, then `Retrying in 2s… (attempt 3)` and `r retry now · l open ~/.fleet/logs/fleetd.log · ctrl-q quit`. If spawn failed with a reason, the reason is shown verbatim in mono. |

Rationale: the user's most common need during an outage is to *copy a path or read state*, not
to be blocked by a dialog. Keeping the last snapshot readable is a pure flow win and costs
nothing but an opacity change.

**Quit with running work.** `ctrl-q` **always quits immediately, no confirm** — sessions and
jobs live in the daemon, so quitting the app is free and must stay free. Only
`ctrl-shift-q` (quit app *and* stop the daemon) confirms:

```
        ┌────────────────────────────────────────────────────┐
        │ ⏻ Stop the Fleet daemon?                           │
        │────────────────────────────────────────────────────│
        │ 2 jobs will be cancelled                           │
        │   ⟳ clone bukhr/buk        64%                     │
        │   ⟳ prune payroll                                  │
        │ 3 sessions will be killed                          │
        │   payroll/feat-payroll-fix   claude, :3000         │
        │   payroll/fix-tz-bug         claude                │
        │   infra/bump-tf                                    │
        │────────────────────────────────────────────────────│
        │ Unsaved work in these processes is lost.           │
        │ ctrl-q quits Fleet and leaves all of this running.  │
        │                                                    │
        │        y  Stop everything      n  Cancel           │
        └────────────────────────────────────────────────────┘
```

The `ctrl-q` line inside the confirm is the important part: it converts the dangerous key into a
lesson about the safe one. With nothing running, `ctrl-shift-q` does not confirm at all.

---

## 4. Keystroke counts — the 10 most frequent tasks

"Keys" counts physical presses; a chord (`ctrl-s`) counts as 1. Text typed by the user is shown
as `+text`. Baseline = swarm today (tmux popup: `prefix s` to even open the TUI).

| # | Task | Fleet (this proposal) | Keys | swarm today | Saved |
| --- | --- | --- | --- | --- | --- |
| 1 | Open a worktree I can see | `j`×n `Enter` | **2** (best), 4 typical | popup `^s s`, `j`×n, `Enter` | 2 |
| 1b | Open a worktree from anywhere by name | `:` `+pay fix` `Enter` | **3** +text | n/a (must reach Hub first) | ≥3 |
| 2 | Create a worktree from a branch | `n` `+branch` `Enter` | **2** +text | `n`, text, `Tab`, base, `Enter` | 1–3 |
| 3 | Create a worktree from a PR | `p` `j`×n `Enter` | **3** | `p`, `j`×n, `Enter` (same) | 0 |
| 3b | Capture 3 PRs without leaving the list | `p` then `c c c` (proposed) | **4** | 3 full open/return round-trips | ~12 |
| 4 | Switch between two sessions (A↔B) | `ctrl-s` `w` (proposed MRU) | **2** | `^s s`, `j`×n, `Enter` (+ re-attach) | ≥3 |
| 5 | Jump to terminal tab 2 | `ctrl-s` `2` | **2** | `^s 2` (tmux) | 0 |
| 5b | Toggle last two terminal tabs | `ctrl-s` `Tab` (proposed) | **2** | `^s l`/`^s 3` (index arithmetic) | 0–2 |
| 6 | Sleep the current session and go to Hub | `ctrl-s` `S` (proposed) | **2** | `^s s`, `s` (and cursor already moved) | 1 |
| 6b | Kill a session from the Hub | `K` `y` | **2** | `K`, `y` | 0 |
| 7 | Delete a worktree with safety facts | `d` `y` | **2** | `d`, `y` (no facts shown) | 0 keys, +certainty |
| 7b | Prune a repo | `x` `y` | **2** | CLI only (`swarm prune`) → leave the TUI | ≫ |
| 8 | Refresh PRs | `r` | **1** | `r` | 0 |
| 9 | Check a background job | glance at the status-bar pill | **0** | glance at footer (last op only) | 0–∞ |
| 9b | Read a job's log | `J` `j`×n `Enter` | **3** | open `~/.swarm/logs/…` in another terminal | ≫ |
| 10 | Copy a worktree path | `y` | **1** | `y` | 0 |
| 10b | Copy a PR URL | `p` `j`×n `y` | **3** | same | 0 |

Aggregate for the loop *"leave session A → start work in a new PR branch → come back to A"*:
`ctrl-s s` `p` `j` `Enter` … `ctrl-s w` = **6 keys**, versus 14+ in swarm today.

---

## 5. Proposed keymap amendments (KEYMAP.md not edited)

| # | Binding | Where | Proposal | Flow rationale |
| --- | --- | --- | --- | --- |
| A1 | `h` / `l` / `Tab` / `S-Tab` | Hub | Cycle **repos ⇄ list only**; remove the detail panel from the focus cycle | The detail panel is read, never operated. Keeping it in the cycle costs one wasted keypress on every second pane switch, forever. `i` toggles its visibility; it always mirrors the cursor row. |
| A2 | `ctrl-s w` | Workspace | Jump to the **last session** (MRU toggle, like `ctrl-^` in vim) | The #1 flow in the daily loop is A↔B between two agents. 2 keys instead of 4–6, and no list re-scan. |
| A3 | `ctrl-s W` | Workspace | Session switcher overlay (the palette pre-filtered to `GO`/sessions) | The 3-or-more-sessions case, without going through the Hub. |
| A4 | `ctrl-s Tab` | Workspace | Last **terminal tab** (MRU toggle within the session) | nvim ↔ agent ping-pong without index arithmetic. |
| A5 | `ctrl-s S` | Workspace | Sleep this session **and** return to the Hub | Today it is `ctrl-s s` then `s` and the Hub cursor may not be on that row. |
| A6 | `ctrl-o` / `ctrl-i` | Global | Jumplist back/forward across Hub panes, PR screen and sessions | nvim-native, and the generic undo for "I navigated somewhere by accident". |
| A7 | `gr` / `gw` / `gp` / `gj` | Hub | Go to Repos pane / Worktrees list / PR screen / Jobs | Absolute destinations beat relative `h`/`l` when more than two panes exist; consistent with the existing `g` prefix (`gg`, `gt`, `gT`). |
| A8 | `c` | PR screen | **Create** the PR worktree without opening it (job runs, cursor stays) | Batch capture of a review queue: 3 PRs in 4 keys instead of 3 round-trips. |
| A9 | `⌥Enter` | Create dialog | Create without opening | Same reason, from the create path. |
| A10 | `a` / `A` | Hub (Normal) | Open the Claude / OpenCode agent session (today only reachable via `ctrl-s a`) | The agent session is a peer of a worktree session; needing to be inside a terminal first is a detour. |
| A11 | `Y` | Hub | Copy the **branch name** (`y` keeps copying the path) | Pasting a branch into a PR description/CI URL is a daily action currently done by re-typing. |
| A12 | `.` | Hub | Repeat the last non-destructive action with the same parameters (create, refresh, inspect) | nvim's dot; makes "create three sibling branches" cheap. Explicitly excludes delete/prune/kill. |
| A13 | `Esc` in Hub | Hub | Never quits the app (KEYMAP already implies this; make it explicit against swarm's "Esc quits when no filter") | A single key that can destroy the current screen is a flow hazard; `ctrl-q` is the only quit. |
| A14 | `q` | Hub | Close the topmost overlay only; unbound when nothing is open | Same reason as A13; swarm's `q`-quits is muscle-memory-dangerous next to `q` in Scroll mode. |
| A15 | `Enter` in Filter mode | Filter | Open the highlighted row directly from inside the input (already swarm behavior — make it authoritative in KEYMAP) | Turns filter into the fastest open path (`/tz↵`). |
| A16 | `J` | Jobs panel | Restores the exact prior focus (pane, row, terminal, mode) on close | A panel that resets focus taxes every peek at a job. |

---

## 6. Open questions

1. **`ctrl-s w` vs. terminal apps.** `ctrl-s` is the prefix, so `w` is free — but should the MRU
   session ring be per-context or global? Global is faster; per-context is more predictable when
   `1`–`9` moves you.
2. **Detail panel default visibility.** Auto-show at ≥1120px is proposed. Should visibility be
   remembered per screen (worktrees vs. PRs) or globally?
3. **Delete-context confirmation strength.** `y` for a cascade that destroys 27 worktrees may be
   too cheap even with trash. Options: require `Y` (uppercase), require holding `y` for 400 ms,
   or accept `y` and rely on trash + an `Undo` toast for 10 s (my preference — undo is faster
   than a stronger confirm).
4. **Prune scope.** `x` prunes the selected repo. Should `X` prune the whole context, and should
   prune ever run with `--kill-sessions` from the UI, or stay CLI-only for that flag?
5. **Sort order of the worktree list.** Proposed: `lastOpenedAt` desc, then `createdAt` desc,
   recomputed only on explicit action (principle 2). Alternative: group by repo in `All` mode —
   faster to scan, slower to find the thing you just touched. Which wins for this user?
6. **PR row presence glyph vs. PR state glyph.** Two glyphs on one row may be one too many at
   narrow widths; the fallback is to drop presence below 90ch. Acceptable?
7. **Terminal tab activity dot.** Should it be per-output (noisy with a running server) or
   per-"agent finished / prompt returned" heuristic (needs foreground-command tracking, which
   the daemon already has via `Terminal.foreground_command`)?
8. **Import banner.** After `fleet import --from-swarm`, should Fleet offer to adopt live tmux
   sessions (attach-and-migrate) or start clean? Adoption is a big win once, and dead code after.
9. **Palette `GO` recency source.** MRU should survive a daemon restart — store it in
   `state.json` (schema change) or in a client-side `ui.json` (not shared with the CLI)?
