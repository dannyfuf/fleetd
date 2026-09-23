# Fleet — UX specification

**Status: authoritative.** This is the single document UI implementers follow for `fleet-app`
and `fleet-ui-kit`. It is the synthesis of three lens proposals — glanceability as the base
system, with a background-safety spine and a flow-speed navigation/mode/toast layer merged in
(`docs/decisions/0006-ux-lens-synthesis.md`). Where those proposals disagreed, this document
decides; the decisions are marked **[D-n]** and collected in §8.

It obeys `docs/ARCHITECTURE.md` and `docs/KEYMAP.md`, and preserves every behavior in
`docs/SWARM-INVENTORY.md` (cited as §1 domain model, §3 operations, §4 sessions/sleep, §5 TUI,
§6 jobs, §9 gaps). Every key it uses is defined in `docs/KEYMAP.md`, which is authoritative (§7).
Contract changes this spec requires before `fleet-proto` is frozen are in §6.

## 0. Measurement conventions

| Thing | Value |
| --- | --- |
| Base unit | 4 px |
| Default window | **1280 × 800** logical px · minimum **900 × 560** |
| UI type | SF Pro Text **13 / 18** |
| Data type (branch, path, sha, log, terminal) | SF Mono **12.5 / 18** → cell **7.5 × 18 px**; **1 ch = 7.5 px** |
| Label type | **11 / 14**, uppercase, tracking .06em |
| Row height | **30 px** in every list; palette rows **34 px** (they carry a key hint); job rows **44 px** (two lines) |
| Icon set | Lucide, 16 px stroke 1.5 · 14 px inside chips · 12 px inside the status bar |

Column widths are stated in **ch first** (they are the inventory §5 breakpoints and are what
makes this port verifiable) with the px equivalent in parentheses, rounded to the nearest px.
Responsive ladders are expressed in **ch of the pane that owns the columns**, not of the window.

---

## 1. Design principles

1. **One glyph beats one word; one word beats one sentence.** Any fact with ≤5 possible values
   becomes a 16 px Lucide glyph in a fixed column, so the eye reads a *shape pattern* down the
   list instead of parsing text. Text is spent only on the two unbounded things: branch names
   and PR titles.
2. **Zero-suppression.** A count of 0, an empty label, a "no error", a "not remote" — none of
   them render. Chrome appears only when it carries information, so a calm screen *means*
   "nothing needs you". The one exception is §1.3: absence of *knowledge* always renders.
3. **Absence of information is its own state and never renders as good news.**
   `SessionState::unknown` renders an amber `circle-help` and **never** the rendering used for
   `none`; `none` renders a dim `dot` at 30 % opacity, and a truly blank cell means "this row has
   no such column". Every nullable inspection fact (`ahead`, `behind`, `uniqueCommits`, §1
   `WorktreeInspection`) renders `—` **never `0`**, accompanied by the verbatim `warnings[]`
   string that explains it. A screen whose data is frozen carries one amber `Stale · <age>` chip
   in its list header. This closes §9's live defect: *"Local status failures can show existing session as
   `none`; only remote failure uses safer `unknown`"*.
4. **Four colors, all semantic, never decorative.** `green` = healthy / done / approved,
   `amber` = needs attention / in flight / unknown, `red` = broken / destructive, `blue` = *only*
   the cursor, the focus ring and the fill of a surface's one primary button. Everything else is
   one of three neutrals. Draft, muted, disabled and "not applicable" are rendered by *lowering
   contrast*, never by adding a hue.
5. **Progressive disclosure with a stable frame.** The detail panel is open by default on a
   window at least 1120 px wide, `i` toggles it, and it is never focusable; the filter replaces
   the pane header in place; dialogs are the only layer
   that ghosts the base; the Jobs panel docks to the right instead of covering the list. Nothing
   the user opens ever reflows what they were already looking at — position is memory.
6. **Nothing the user started is owned by a surface.** Anything that can exceed ~200 ms is a
   daemon `Job` with an id, a log path and a cancel token. The UI only *observes* it. Closing a
   dialog, leaving a screen or quitting the app never cancels it — and the UI says so at the
   exact moment the user would fear otherwise (dialog close, quit, Help). Only `c` in the Jobs
   panel, `K` and `ctrl-shift-q` stop things.
7. **Confirmation cost is proportional to what is at risk, and every fact is stamped.** The same
   `d` shows a one-line confirm when the inspection says clean + merged + no session, and a facts
   panel when it does not. Facts are stated, never adjectives, and always carry their age —
   the confirm is the only place where freshness actually decides an outcome. When any decisive
   fact is unknown, the confirm key escalates from `y` to `Y` (KEYMAP: "uppercase = stronger
   variant").
8. **Errors are sticky and actionable; successes are transient.** A failed job stays in the Jobs
   panel until dismissed, with `R retry` and `y copy log path`, and holds an addressable sticky
   slot in the status bar (`!` or a click on it opens the failure in the Jobs panel; its ✕ or `X`
   clears the slot and leaves the failed jobs in the panel). Successes get at most a 3.2 s toast, and only under
   the toast law (§2.7). Nothing blinks, nothing re-announces itself, nothing decays on a timer
   that the user did not set.
9. **Every action has a key and a visible control, and the state is shown where it matters.**
   Every key keeps working, and every action also has a control that shows its key, so the app
   teaches its keys where they are used (ADR 0023). There is no mode word: the surface that owns
   the keyboard shows it — the terminal grid, the focused pane's ring, the open overlay, the filter
   bar, the scroll pill, the ⌃S command menu — which is what prevents typing a command into a PTY,
   or a PTY key into a list.
10. **Cut anything that does not change the next keystroke.** Every element below had to answer
    *which decision does this change?* The full data still exists behind `i`, `I`, `J`, `,` and `:`.

---

## 2. Global layout

### 2.1 Hub frame (1280 × 800, detail panel closed)

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│ ●●● [A] Acme ⌄ [Worktrees 4│Pull requests 3│Board 5] [⌕ Search or run a command :] ● 1 needs you ⟳ 2 jobs ? ⚙ │ 44  title bar
├──────────────┬───────────────────────────────────────────────────────────────────┤
│ Repositories+│ WORKTREES · payroll                          8/12      1–8/12     │ 30  pane headers
│──────────────│───────────────────────────────────────────────────────────────────│
│▌▦ All     27 │ ◉ feat/payroll-fix ✎   buk/payroll  ⚡claude,:3000  #412 CI    2h ▏│ 30  rows
│  • payroll 8 │ ☾ fix/rut-validator    buk/payroll                 #408 Appr   1d ▏│
│  • fleetd  3 │ · spike/gpui-vt ☁devbox dannyfuf/fleetd                       3d  │
│  ⟳ nixos     │ ▲ chore/deps           buk/www      ⚠ hooks failed             5d  │
│  ✕ old-api   │ ? api-poc  ☁devbox     buk/api      offline                   3d  │
│              │                                                                   │
│    232 px    │                     flex — 1048 px (139 ch)                       │
├──────────────┴───────────────────────────────────────────────────────────────────┤
│ ● fleetd  buk › payroll › feat/payroll-fix     ⟳ clone nixos 40%  +1   Shortcuts ? │ 28  status bar
└──────────────────────────────────────────────────────────────────────────────────┘
                                            ┌──────────────────────────────┐
                                            │ ✓ Path copied                │  toasts, bottom-right
                                            └──────────────────────────────┘
```

The detail panel is **open by default** at 1120 px and wider (§3.4): the worktrees list shrinks by
**344 px** and the panel is inserted at the right; the sidebar never moves. The Worktrees list
itself is a page — H1, subtitle, toolbar, column heads, 44 px rows (§3.3); the frame above shows
the dense pane anatomy the other Hub lists still use. Below **1120 px** total width the panel is
closed by default and, opened with `i`, becomes a right-edge overlay (320 px) so the list never
drops below **72 ch**.

The Workspace replaces the rail + list + detail region entirely (full-bleed terminal) and keeps
the title bar and the status bar at the same pixel positions — same chrome, same saccade. Only the
title bar's leading region changes: the context switcher and the section nav give way to the
breadcrumb `← Worktrees / repo / worktree` (§3.6). A **native tab** (§3.6) replaces the terminal
grid with a Fleet-drawn pane in exactly that region and changes nothing above or below it: there is
one title bar and one status bar in the window, and they are the shell's. A pane that draws its own
window chrome would put two status bars on screen, which is why `crates/fleet-lazygit` has an
embedded render path with no `AppFrame`. The pane's own one-row key-hint bar is **not** window
chrome — it is the pane's content, like a list header — and it stays.

### 2.2 Persistent chrome

| What | Where | Size | Why here |
| --- | --- | --- | --- |
| Title bar | Top, full width; the unified macOS titlebar, so it starts at x = 84 after the traffic lights (12 px on other platforms) | 44 px, `chrome` ground, hairline below | One row does what three did: where you are, a way to search or run anything, and what needs you. |
| Context switcher | Title bar, left: a monogram tile, the context name and a chevron | compact ghost button | Contexts are the outermost coordinate. It opens a menu of every context with its `1`–`9` key (a context past nine has none), then *New context* `N`, *Edit context* `E`, *Delete context* `D` (red). The keys keep working without it, and `gt` / `gT` still cycle. |
| Section nav | Title bar, after the switcher | a segmented control | *Worktrees · Pull requests · Board*, each with its count (worktrees in the context, PRs waiting for your review, open board cards; zero-suppressed). The same actions as `g w` / `p` / `g b`; the segments are the harness's `hub.tab[N]`. Hub only. |
| Command field | Title bar, centred in the window | 340 × 30 | Looks like a search input — magnifier, *Search or run a command*, the palette key — and is a button: a click opens the palette. Nobody has to know a key to find a command. |
| Status cluster (§2.3) | Title bar, right | compact ghost buttons | What needs you and what is running, each shown only while non-zero, each opening what it counts. Top-right is the OS status corner, far from the cursor. |
| Help, Settings | Title bar, far right | 26 px icon buttons | `?` and `,` as controls; the tooltip names the key. |
| Sidebar | Left, **232 px** on the `chrome` ground; its edge drags between 200 and 320 px, and the dragged width is kept while Fleet runs (not across restarts); `H` or its foot button collapses it to 44 px of icons | full height | Second coordinate. Narrow and left because it is a *filter*, not content — and the agents one click away from anywhere in the Hub (§3.2). |
| Pane header | Top of each pane | 30 px, label type, `fg.faint` | Carries scope, filter state, count and scroll position (§2.5), and hosts the filter bar with zero layout shift. |
| Status bar | Bottom, full width | 28 px, `chrome` ground | Where you are and what is running: daemon · breadcrumb · job ticker or sticky error · the buttons that teach the two keys everything else hangs off. |
| Toast layer | Bottom-right, above the status bar, 320 px wide, 12 px insets | max 3 stacked | Only for events with no other home (§2.7). |
| Focus ring | 2 px `blue` inset on the focused pane; 2 px `blue` left bar on the cursor row | — | The only blue in the app. |

**Status bar slots, left to right:** the daemon — a small dot and `fleetd`, green and silent when
healthy, and `fleetd <word>` in the daemon's tone otherwise (`fleetd connection lost`, `fleetd
starting`) · the breadcrumb `context › repo › row` (flex, truncates) · the job ticker `⟳ <kind>
<target> <pct>` with `+n` when more run (`fg.muted`) · the **sticky error slot** (red, `⚠ <text>`
and its `!` chip, then a ✕; a click on it does what `!` does, the ✕ or `X` clears it; it replaces
the ticker when present) · the buttons: `Shortcuts ?` in
the Hub; `Fleet commands ⌃S` and `Shortcuts ⌃S ?` in the Workspace. Over a native agent thread only
`Shortcuts ⌃S ?` shows: the thread takes `⌃S` as a chord, so there is no prefix mode for a button to
enter, and holding the chord shows the ⌃S command menu anyway.

There is no mode word (ADR 0023, §2.8). Each state it used to name is on the surface that has it:
Terminal is the focused grid; Agent is the thread's composer; Prefix is the ⌃S command menu; Scroll
is the scroll pill; Filter is the filter bar in the pane header; Palette, Dialog and Jobs are the
overlay itself.

### 2.3 Title-bar status cluster (all zero-suppressed, in this fixed order)

Each one is a button that opens what it counts, and its tooltip carries the key that does the same.

| Button | Mark | Color | Content | Opens |
| --- | --- | --- | --- | --- |
| Needs you | dot | amber | `<n> needs you` — top-level threads blocked on a permission, a question, a plan, or a finished turn nobody has read | with one waiting, that thread (as the palette's Agents row does); with more, the agents picker (`⌃S d` in the Workspace) |
| Jobs | `loader-circle` (spin) | `fg.muted` | `<n> job(s)` running | the Jobs sheet (`J`) |
| Failed jobs | `triangle-alert` | red | `<n> failed`, failed and unseen; replaces the jobs button, never a second one (§1.8) | the Jobs sheet (`J`) |
| Sleeping | — | `fg.muted` | `<n> sleeping` (detached sessions); inert text — it counts nothing a click could open. Hub only, until the Worktrees subtitle takes the session counts over | — |
| Update | `arrow-up-circle` | `fg.muted` | `Update <version>` when an update is available. Hub only, where `U` is bound | runs the update (`U`) |
| Daemon | dot | amber / red | nothing while healthy; `Reconnecting…` (amber) while the link retries on its own, `fleetd down` (red) once it stopped or could not start | the fleetd log (`l` on the banner) |

**[D-1]** The live, unknown/offline and review counts left the chrome with the context bar. The
review count is the *Pull requests* segment's count; the session counts move to the Worktrees
subtitle, and until they do only `sleeping` stays, muted. An unreachable host is still shown on its
rows and in the rail.
**[D-2]** "Update available" is a button and a Settings › About row — **never** a sticky toast. An
update is never urgent and `U` is already bound.

### 2.4 Tokens

| Token | Dark | Light | Use |
| --- | --- | --- | --- |
| `bg` | `#0E1013` | `#FBFBFC` | app ground |
| `bg.raised` | `#16181D` | `#FFFFFF` | rails, panels, sheets, dialogs |
| `bg.row.sel` | `#1E2430` | `#EDF2FB` | cursor row |
| `bg.scrim` | `rgba(0,0,0,.45)` + 8 px blur | `rgba(0,0,0,.25)` + 8 px blur | dialog backdrop |
| `border` | `#22262E` | `#E3E5E9` | 1 px hairlines only |
| `fg` | `#E6E8EB` | `#16181D` | branch, title, values |
| `fg.muted` | `#8A9099` | `#6B7280` | repo, host, age, counts, labels |
| `fg.faint` | `#5A6069` | `#9CA3AF` | draft, disabled, key hints, `—` |
| `green` | `#3FB950` | `#1A7F37` | attached, pass, approved, done |
| `amber` | `#D29922` | `#9A6700` | running, pending, dirty, **unknown**, degraded |
| `red` | `#F85149` | `#CF222E` | failed, changes requested, danger |
| `blue` | `#58A6FF` | `#0969DA` | cursor and focus **only** |

Terminal `Palette(u8)` colors resolve through a theme palette table shipped with each theme;
`Default` fg/bg resolve to `fg` / `bg`.

### 2.5 Status glyph vocabulary — identical on every screen

| State | Lucide icon | Color | Detail wording |
| --- | --- | --- | --- |
| `attached` | `circle-dot` | green | `attached` |
| `detached`, awake | `circle` | `fg` | `running, detached` |
| `detached`, slept (`slept_at` set, §6) | `moon` | `fg.muted` | `sleeping — kept cc (claude)` |
| `none` | **`dot` at 30 % opacity** | `fg.faint` | `no session` |
| `unknown` | `circle-help` | **amber** | `unknown — <reason>` |
| agent working | `loader-circle` (spin) | amber | `Agent working` |
| terminal agent idle | `circle-check` | green | `Agent idle` (status only; no notification) |
| terminal agent needs you | existing amber tab dot plus the activity glyph | amber | hook-reported permission, question, plan, or finished attention |
| degraded (hooks failed, §6) | `triangle-alert` | amber | `post-create hooks failed — J for log` |
| job running on this row | `loader-circle` (spin) | amber | job kind + phase |
| clone in flight (`CloneJob.status`) | `loader-circle` (spin) | amber | `cloning…` |
| clone failed | `circle-x` | red | `clone failed` |
| host unreachable | `cloud-off` | amber | forces session to `unknown`, never `none` |

**[D-3]** `none` is a dim `dot`, not a blank cell. A blank cell means "this column does not apply
to this row". This is the rendering from which a false `none` (§9) must remain recoverable.

### 2.6 Freshness law

Every fact that comes from a **job** (`inspect`, `prune --dry-run`, PR fetch) rather than a
**poll** (status) is stamped and aged:

| Age of `inspectedAt` / `fetchedAt` | Rendering |
| --- | --- |
| ≤ 60 s | normal contrast, no stamp on rows; stamp shown in the detail panel and in every confirm |
| 60 s – 10 min | normal contrast; stamp in `fg.muted` |
| > 10 min | the derived marks (`✎` dirty, `#n` PR badge) drop to **55 % opacity**; stamp turns amber |
| errored | mark is not drawn at all; the detail panel shows `error: <message>` and `I retry` |
| whole pane frozen (daemon lost) | the list header gains one amber `Stale · <age>` chip and the Worktrees rows dim to 55 %; every session glyph forced to `circle-help` |

**[D-4] Auto-inspect cadence.** The daemon
re-inspects (a) the **selected** worktree with `--no-fetch`, debounced **400 ms** after the cursor
settles; (b) all **visible** rows with `--no-fetch` on a **30 s idle** timer (never on scroll,
never while a modal is open); (c) the affected worktree after any `create` / `delete` / `open` /
`sleep`. `I` runs a full inspect **with** fetch as an explicit job. Nothing safety-adjacent is
ever rendered from a fact older than the ladder above allows without its stamp.

### 2.7 Toast law

> **A toast is allowed only when there is no row and no pill that already shows the outcome.**

That rule is the decidable test every new surface must pass; it replaces an enumerated allow-list
that would drift. Consequences today:

| Allowed | Text | Duration | Icon |
| --- | --- | --- | --- |
| Clipboard | `Path copied` / `PR URL copied` / `Branch copied` / `Log path copied` | 1.6 s | `clipboard-check` |
| Sleep report with kept windows | `Slept · kept cc (claude)` | 3.2 s | `moon` |
| Background success whose row is off-screen | `Cloned buk/ledger` + `View J` (opens Jobs) | 3.2 s | `circle-check` |
| Dialog-close reassurance | `Base fetch still running` + `View J` (opens Jobs) | 1.6 s | `loader-circle` |
| Action refused, with the reason | `Nothing to prune in payroll — 11 skipped` + `View J` (the reasons, in Jobs) | 1.6 s | `scissors` |
| Duplicate action suppressed | `Already running` | 1.6 s | `info` |
| Mode no-op | `no scrollback in alt-screen` | 1.6 s | `chevrons-up` |
| Agent attention | `<session>: needs permission` / `asks a question` / `proposed a plan` / `agent finished`; a native agent thread's adds `View`, which opens that thread | 3.2 s | `lock` / `circle-question-mark` / `clipboard-check` / `circle-check` |

**Never a toast:** job started, job succeeded when its row is on screen, worktree created,
PR refreshed, context switched, session opened, settings saved, update available, **and any
error** — errors are sticky (§1.8), never transient.

Identical toast text within **1 s** coalesces into one toast with a `×2` suffix (this kills the
duplicate spam §6 attributes to overlapping operations with no duplicate suppression). Max 3
stacked; oldest evicted first.

### 2.8 Input modes (no mode word)

ADR 0023 retired the mode word: nothing in Fleet's chrome names the mode. The modes themselves are
unchanged — they decide the key context, and the harness snapshot reports them as `mode`
(`TESTING-HARNESS.md` §3) — and each one is visible on the surface that owns the keyboard.

| Mode (snapshot `mode`) | gpui key context | Where it shows |
| --- | --- | --- |
| `Normal` | `Hub`, `Hub > Repos`, `Hub > Worktrees`, `Hub > Prs` | the focused pane's ring and the cursor row |
| `Terminal` | `Workspace > Terminal` | the focused terminal grid and its cursor |
| `Native` | `Workspace > Native` | the Fleet-drawn pane in the selected tab |
| `Agent` | `Agent > AgentIdle` / `AgentWorking` / `AgentDecision > …` | the thread's composer, or its open decision |
| `Prefix` (one-shot) | `Workspace > Prefix` | the ⌃S command menu, once the prefix is held |
| `Scroll` | `Workspace > Scroll` | the scroll pill over the grid |
| `Filter` | `Filter` | the filter bar in place of the pane header |
| `Palette` | `Palette` | the palette |
| `Dialog` | browsing `Dialog > <name>`; text ownership `Dialog > <name>Editing > FleetTextInput` | the dialog |
| `Jobs` | `Jobs` | the Jobs sheet |

### 2.9 Column ladders (inventory §5 breakpoints, authoritative in ch)

**Worktrees list**, measured in ch of the list pane (114 ch at 1440 px with the detail panel
open, 160 ch without):

| # | Column | Head | Width | Align | Shown when |
| --- | --- | --- | --- | --- | --- |
| 1 | name: icon + branch + `↑n` / `uncommitted changes` / `☁host` / hooks chip | Name | flex, **min 24 ch** | left | always |
| 2 | repo `owner/name` | Repository | 12 ch, truncate-head | left | scope = `All`, **or** pane ≥ 110 ch |
| 3 | session in words, or a job's phase | Session | **18 / 14 / 0 ch** | left | ≥ 100 / 72 / < 72 ch |
| 4 | PR chip `#n <state>` | Pull request | 15 ch | left | pane ≥ 60 ch |
| 5 | age | Age | 5 ch | right | pane ≥ 52 ch |
| 6 | hover actions `Open ⏎` `⋯` | — | 13 ch | right | always (reserved; drawn on hover or selection) |

Below 72 ch only columns 1, 4, 5, 6 survive. Gaps are 12 px between columns, 12 px row padding.

**PR list**, measured in ch of the list between the page's 24 px gutters (≈ 108 ch at 1440 px
with the detail panel open):

| # | Column | Head | Width | Shown when |
| --- | --- | --- | --- | --- |
| 1 | number `#1234` | `#` | 6 ch | always |
| 2 | title + `has worktree` tag | Title | flex, min 24 ch | always |
| 3 | author | Author | **12 ch @ ≥ 70 ch · 16 ch @ ≥ 130 ch** | review tab |
| 4 | `headRefName` | Branch | 12 ch, truncate-tail | ≥ 100 ch (≥ 120 ch on the review tab) |
| 5 | repo `owner/name` | Repository | 12 ch | ≥ 125 ch **and** scope is multi-repo |
| 6 | PR chip (review state) | Status | 16 ch (fits `Needs changes`) | always |
| 7 | checks | Checks | 10 ch | ≥ 70 ch |
| 8 | age | Age | 5 ch, right | ≥ 52 ch |
| 9 | `Open ⏎` · `⋯` | — | 14 ch, right, shown on hover or selection | ≥ 60 ch |

**[D-5]** The two-step author breakpoint (12 ch / 16 ch) is preserved exactly; it is the one
inventory ladder the px-only port lost.

### 2.10 Pane header anatomy (closes the §5 "scroll" gap)

```
 WORKTREES · payroll                              8/12          1–8/12
 └ label ─────┘ └ scope ┘                       └ shown/total ┘ └ visible range ┘
```

Left: `WORKTREES` (label type) + `· <repo | All>` scope. Right, in order: `shown/total` when a
filter is active (else just `total`), then the **visible range** `<first>–<last>/<total>` — the
scroll-position indicator inventory §5 requires and no proposal supplied. A 3 px `border` scroll
thumb is drawn on the pane's right edge whenever the content overflows. When the pane data is
frozen the header appends an amber `Stale · <age>` chip (§1.3); the repos rail beside it carries
none, so the screen says it once.

---

## 3. Screens

### 3.1 Hub — Title bar

**Purpose:** *Which slice of the world am I in, where can I go, and is anything waiting for me?*

```
 ●●●  [A] Acme ⌄  [Worktrees 4 │ Pull requests 3 │ Board 5]   [⌕ Search or run a command  :]   ● 1 needs you  ⟳ 2 jobs  3 sleeping  ?  ⚙
```

| Element | Content | Position | Why here |
| --- | --- | --- | --- |
| Context switcher | monogram tile, `Context.name`, chevron; opens a menu of every context (`✓` on the active one, `1`–`9` chips), then *New*, *Edit* and *Delete context* (`N`, `E`, `D`; Delete in red) | left, after the traffic lights | The first place a reader lands. The digits are on the menu rows, so the keys are taught where the choice is made. |
| Section nav | *Worktrees · Pull requests · Board* with counts; the raised segment is the one shown | after the switcher | The Hub's three sections as one control, the same actions as their keys. |
| Command field | *Search or run a command* and the palette key | centred | §2.2. |
| Status cluster | §2.3 | right | One saccade answers "is anything happening without me". |
| Help, Settings | icon buttons | far right | `?` and `,`, with their keys in the tooltip. |

**Intentionally omitted:** context `owners` (Context dialog, Assign dialog and the detail panel),
`createdAt`, per-context repo counts (the rail counts them), numbered context tabs (the switcher's
menu carries the digits), and a separate window title — the GPUI window uses a 44 px unified
titlebar that *is* this row.

**States:** *loading* → the bar renders from the cached snapshot, the counts absent. *no contexts*
→ the switcher reads `No context` and its menu offers *New context*. *first run* → the bar is empty
(§3.13). *daemon lost* → §3.12; the red `fleetd down` or amber `Reconnecting…` button joins the
status cluster and the list header gains one `Stale · <age>` chip.

**Icons:** `chevron-down`, `search`, `loader-circle`, `triangle-alert`, `arrow-up-circle`,
`circle-question-mark`, `settings-2`; the needs-you and daemon marks are dots, not icons.

**Keyboard:** unchanged, and every control has one: `1`–`9` jump · `gt` / `gT` cycle · `N` new
context · `E` edit · `D` delete · `g w` / `p` / `g b` sections · `:` palette · `J` jobs · `U` update
· `?` help · `,` settings. The controls are not focusable (ADR 0023): the keyboard path to each is
its key.

**Workspace:** the switcher and the section nav give way to the breadcrumb `← Worktrees / repo /
⎇ worktree ⌄` and its chips (§3.6). *Worktrees* is a button for `⌃S s` (back to the Hub; the
session keeps running) and shows that chip; the repository is text; the worktree is the **worktree
switcher**. The command field, the status cluster (without *sleeping* and *Update*, whose keys are
Hub keys) and Help / Settings stay; Help and Jobs show their `⌃S` chords.

**Harness:** `titlebar.context`, `titlebar.command`, `titlebar.needs_you`, `titlebar.jobs`,
`titlebar.update`, `titlebar.daemon`, `titlebar.help`, `titlebar.settings`, `titlebar.back` (also
`workspace.back`), `workspace.switcher`, `workspace.pr`, the segments `hub.tab[N]`, and the status bar's `statusbar.shortcuts` and `statusbar.commands`
(`TESTING-HARNESS.md` §3).

---

### 3.2 Hub — Sidebar: repositories and agents

**Purpose:** *Scope the worktree list; is any repo unhealthy or still cloning; which agent needs
me?*

```
┌────────────────────────────┐
│ Repositories             + │ 26  section title · Clone repo (n)
│▌▦ All repositories       4 │ 30  pinned scope row, default cursor
│  • acme/api    [1 issue] 3 │     a worktree degraded or on an unreachable host
│  • acme/web              1 │
│  ⟳ nixos       cloning 40% │     clone in flight, thin bar along its foot
│  ✕ old-api          failed │     clone failed, stays until dismissed
│                            │
│ Agents                     │     hidden while there are none
│  • codex · REA… [needs you]│
│  • claude · spike          │
│  • claude · agent window   │
│                            │
│ ◧                          │     collapse (H)
└────────────────────────────┘
      232 px, drag 200–320
```

The sidebar sits on the `chrome` ground with a hairline on its right edge. Items are `row_h` tall,
rounded, secondary text; the item under the cursor is `row_selected` with the 2 px cursor bar while
the sidebar owns the keyboard, and the sidebar draws the pane focus ring then.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Section title | `Repositories` (sentence case), then a `+` icon button that opens Clone repo (`n`); a retained filter shows as a search chip before it, and the live filter bar replaces the title row in place (§3.10) | top | the add action lives where the list it adds to is | §5 clone |
| `All repositories` | grid icon + literal label + total worktree count | row 0, pinned | swarm's default and the most common scope | §5 "All pseudo-repo default" |
| Repository dot | small dot from the worst-of session state of its worktrees: amber while a finished agent waits for you, green while one works, lighter while any session is alive, dim otherwise | left of name | where live work is, at a glance | §5 "aggregate session glyph" |
| Repo name | `Repo.name`; `owner/name` **only** on collision | flex | you think in repo names | §5 "disambiguated owner/name" |
| Issue chip | amber `1 issue` / `n issues`: worktrees whose post-create hooks failed or whose host is unreachable; zero-suppressed | before the count | a repo that needs attention says so in words, not only in a glyph | `Worktree.degraded`, host reachability |
| Worktree count | integer, right, muted | right | sizes the jump you are about to make | §5 "worktree count" |
| Clone row | spinning `loader-circle` + name + `cloning 40%` (`cloning…` until a percent is parseable) + a thin progress bar along the item's foot | sort position | a repo being born must be visible where it will live | §1 `CloneJob`, §6 |
| Clone failed row | `circle-x` red + name + faint `failed` | same | failure must not disappear silently; `Enter` opens the Jobs panel focused on that job, `x` dismisses (KEYMAP arbitrates `d` = delete repo, `x` = dismiss a failed clone) | `CloneJob.status`, `.error` |
| Agents section | every top-level native agent thread whose worktree is in the active context, in the daemon's order, then every agent window (`a` / `A`) the daemon holds: a dot (amber needs you, green working, red failed, grey otherwise), `provider · title` (`provider · worktree` before the thread has a title, `provider · agent window` for a window) and an amber `needs you` chip | under the repositories | a running agent one click from anywhere in the Hub | the palette's `AGENTS` data, `Attention` |
| Collapse | an icon button at the foot, the same as `H`; collapsed, the sidebar is 44 px of icons whose labels are tooltips | foot | the pointer's way to the room `H` makes | KEYMAP A22 |
| Edge | drag the right edge between 200 and 320 px; the width is kept while Fleet runs and comes back when the sidebar is expanded again | right border | a long repo name can be given room | — |

**Pointer (§5.1).** A click on a repository selects it: the cursor goes there, the list is scoped to
it, and the sidebar keeps the keyboard, so the row's menu shows and runs the sidebar's own keys. A
double-click opens it like `Enter`, handing the scoped list the keyboard. A failed clone has nothing
to scope to: a click only puts the cursor on it, and a double-click opens its job like `Enter`. A repository's `⋯` (drawn while it is hovered) and its
right-click menu are the same list, each item with its key from the live keymap: Clone repo `n`,
Edit setup commands `e`, Move to another context `m` — Delete the repository `d` (red, confirms);
a failed clone lists Dismiss the failed clone `x` and Clone repo `n`, a clone in flight Clone repo
`n`. A click on an agent goes to it: a thread opens the way the palette's `go` does (its tab, its
worktree's session first when needed), and an agent window is shown the way `a` / `A` shows it,
never hidden by the click. Agent rows carry no menu: going to one is their only action.

**Intentionally omitted:** `url`, `path`, `defaultBranch`, `clonedAt`, hook lists, prepared-pool
state, private lock icon, owner avatar, a separate "live" count badge — all in the detail panel;
none of them changes which repo you select. Delegated child threads — their caller's transcript
reaches them, and the caller's dot already carries a child that needs you. The stale stamp, which
the Worktrees page header carries.

**States:** *empty* → `No repos in <context>.` + faint `n clone one` (verbatim §5).
*filter-empty* → `Nothing matches "<filter>".` *loading* → the rail renders from `state.json`
instantly, no skeleton; the reconcile shows only as the jobs chip. *no agents* → the Agents
section is not drawn. *collapsed* → icons and dots only; no empty sentence.

**Icons:** `layout-grid`, `plus`, `ellipsis`, `loader-circle`, `circle-x`, `panel-left-close`,
`panel-left-open`, `folder-git-2` (detail header only), `zap` (prepared copies, detail only).

**Keyboard:** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u` · `Enter`/`o`/`l` → focus worktrees · `n` clone
· `d` delete (confirm, cascades) · `x` dismiss a failed clone · `e` edit hooks (KEYMAP A16) · `m` move
to context · `i` detail · `ga` jump to `All` (KEYMAP A7) · `H` collapse/expand the sidebar. The
Agents section has no keys of its own: the palette's `AGENTS` rows and `^s <n>` reach the same
threads.

---

### 3.3 Hub — Worktrees list

**Purpose:** *Which branch is alive, which needs attention, which can I throw away?*

```
 Worktrees                                   [⌕ Filter  /] [Clone repo] [+ New worktree n]
 4 across 2 repositories · 1 needs attention
 Name                          Repository  Session                   Pull request   Age
 ─────────────────────────────────────────────────────────────────────────────────────────
▌⑂ spike ↑2                     acme/web    ● claude working · 2 tabs  #4 In review    2d  [Open ⏎] [⋯]
 ⚠ broken [Setup hook failed] View log  acme/api    No session           —              2d
 ⑂ hotfix uncommitted changes   acme/api    ● 1 terminal               #13 Draft       5d
 ⑂ feature                      acme/api    No session                 #12 Approved    1w
```

**Page header.** An H1 `Worktrees` (`page_title`) over one subtitle the view model builds:
`<n> across <r> repositories`, or `<n> in <owner/name>` when one repository is selected (or the
scope holds only one), then `· <k> needs attention` when a row has failed hooks, an offline host
or an inspection error (zero-suppressed). A frozen snapshot appends an amber `Stale · <age>` chip
and draws the rows at 55 % (`stale_opacity`); they stay navigable.
The toolbar on the right: the **filter field** (`/`; a click opens the same filter mode, §3.10),
`Clone repo` (`repos::Clone`) and the primary `New worktree  n` (`worktrees::Create`).

**Column heads** (`ListHeader`, sentence case): Name, Repository, Session, Pull request, Age; the
§2.9 ladder drops them with the pane exactly as it drops the cells.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Name icon | `git-branch`, blue while a session is alive, muted otherwise; a failure or a job takes it over (`triangle-alert`, `cloud-off`, spinning `loader-circle`) | col 1 | health first, as the glyph it replaces | `WorktreeStatus`, `Worktree.degraded`, jobs |
| Branch | `Worktree.branch` at weight 500, ellipsized | col 1 | the only string the user thinks in | `Worktree.branch` |
| Ahead | mono `↑n`, only when ahead > 0 | after the branch | the one git fact that says "unpushed work" at a glance | `WorktreeInspection.ahead` |
| Dirty | muted `uncommitted changes` | after the branch | dirty is a property of the branch, so it rides with it | `WorktreeInspection.dirty` |
| Host chip | `cloud` + host id; `cloud-off` amber when unreachable | after the branch | absent for local (the 95 % case) | `Worktree.host` |
| Hooks failed | amber `Setup hook failed` chip + `View log` ghost button, which opens Jobs on that hooks job | after the branch | a worktree that looks ready but whose post-create hooks failed is a trap | `Worktree.degraded` + the failed `PostCreateHooks` job |
| Repository | `owner/name` | col 2 | disambiguates in `All` scope | — |
| Session | in words behind a dot: `claude working · 2 tabs`, `claude waiting · 1 tab`, `1 terminal`, `Sleeping`, `No session`, `Host offline` | col 3 | says what `s`/`K`/`d` would stop without decoding a glyph | `WorktreeStatus` (session, windows, agent activity) |
| Job phase | `copying files…`, `running hooks…`, `deleting` | replaces the session words and the age | phases are more honest than percentages | create/delete jobs |
| PR chip | tinted `#n` + state words: `In review`, `Draft`, `Approved`, `CI failing`, `Needs changes`, `CI running`, `Merged`; `—` without a PR | col 4 | the single fact that decides "is this branch done?" | inspection PR, refined by the PR cache |
| Age | relative `lastOpenedAt ?? createdAt`, 1 unit | col 5 | recency is the sort you verify visually | — |
| Hover actions | `Open ⏎` and a `⋯` menu; drawn while the row is hovered or selected, width always reserved | col 6 | the pointer's way to the row's verbs | ADR 0023 |
| Sort | `lastOpenedAt` desc, then `createdAt` desc | — | MRU puts the right answer on row 0, so `Enter` alone is often the whole task | — |

**Row menu.** The `⋯` menu and the right-click menu are the same list, each item with its key
from the live keymap: Open `⏎`, Open, keep awake `O`, Sleep `s`, Inspect `I` — Copy path `y`,
Copy branch `Y` — Kill session `K` and Delete `d` (both red, both confirm), and Undo delete `u`
only while there is a delete to undo.

**Pointer (§5.1).** A click selects the row (and gives the list the keyboard), a double-click
opens it like `⏎`, a right-click selects it and opens the row menu at the pointer. Every button on
a row first selects that row, so the action it dispatches acts on the row under the pointer.

**Cursor stability.** Background events (status polls, PR fetches, job completions, pool refills)
never re-sort, re-scroll or re-focus the list. Sort order is recomputed only on explicit user
action (`r`, filter change, repo change, screen change). A row that changes state changes its
words **in place**.

**Intentionally omitted:** `WorktreeId` (never typed in the GUI), `path` (`y` copies it, detail
shows it), `baseRef`, session name string, window names, `behind`, `uniqueCommits`,
`published`, `mergedIntoTarget`, absolute timestamps, PR title, PR author, additions/deletions.
Every one of them appears in the detail panel or in the delete/prune confirm — i.e. exactly where
it changes a decision.

**States**

| State | Rendering |
| --- | --- |
| Empty | `No worktrees yet` / `No worktrees for <repo> yet` over a primary `New worktree  n` button |
| Filter-empty | `Nothing matches "<filter>".` + faint `esc clear` |
| Loading (cold) | `Loading…` until the first snapshot; rows then render from `state.json` immediately — **never blank** |
| Job running on a row | name icon → spinning `loader-circle` (amber), session + age → phase text; the row stays selectable and `Enter` opens it as soon as the session exists |
| Deleting | row dims to 40 %, `deleting`, non-selectable, disappears on state commit |
| Inspect error | `triangle-alert` amber prefixed to the age column; the detail panel shows `WorktreeInspection.error` verbatim; the row stays operable |
| Host offline | `cloud-off` amber on the host chip and the name, `Host offline` in the session column |
| Degraded | `triangle-alert` amber name icon + `Setup hook failed` chip + `View log` |

**Icons:** `git-branch`, `triangle-alert`, `cloud`, `cloud-off`, `server`, `unplug`,
`loader-circle`, `plus`, `search`, `ellipsis`, and the PR chip's `eye`, `git-pull-request-draft`,
`circle-check`, `circle-x`, `message-square-warning`, `clock`, `git-merge`.

**Keyboard:** `j`/`k`/`gg`/`G`/`ctrl-d`/`ctrl-u` · `Enter`/`o` open (sleeps previous) · `O` open
keeping previous · `n` create · `d` delete · `x` prune repo · `s` sleep · `K` kill · `I` inspect ·
`i` detail · `y` copy path · `Y` copy branch · `b` browser · `u` undo last delete (KEYMAP A6) ·
`/` filter · `p` PRs · `J` jobs.

---

### 3.4 Hub — Detail panel (`i`, on by default ≥ 1120 px, never focusable)

**Purpose:** *What I can do with the selected worktree first, then everything deliberately kept
out of the row, in one 344 px column — with the age of every job-derived fact.*

The panel is **on by default** when the window is at least 1120 px wide (inset beside the list)
and off below that; `i` toggles it either way, and once toggled the user's choice sticks. Below
1120 px an open panel docks over the list as a sheet.

```
┌──────────────────────────────────────┐
│ ⑂ spike                              │  section_title
│ acme/web · from origin/main · on this Mac
│                                      │
│ [ Open workspace ⏎ ] [Sleep s] [⋯]   │  primary, secondary, row menu
│                                      │
│ ╭ Session ───────────────────────╮   │  InfoCard
│ │ ◌ claude is working         4m │   │
│ │ 2 tabs: zsh, claude — kept by fleetd
│ ╰────────────────────────────────╯   │
│ Git                                  │
│  Changes          Clean              │
│  vs origin/main   2 ahead · 0 behind │
│  Published        Yes, origin/spike  │
│  Pull request     #4 Ship the…  [In review]
│                                      │
│ Location                             │
│ [ ~/worktrees/acme/web/spike    ⧉ ]  │  CopyField, y
│ Created 2d ago · safety checked 1s ago
└──────────────────────────────────────┘
```

| Element | Content | Why needed |
| --- | --- | --- |
| Title | the row's name icon + branch | echoes the cursor row so the eye does not re-search |
| Subtitle | `repoId · from baseRef · on this Mac` \| `@host` | provenance, read once |
| Action row | primary `Open workspace ⏎` (`worktrees::Open`), `Sleep s`, and `⋯` — the row menu of §3.3 | the panel leads with what you can do |
| Session card | the session state in words (`claude is working`, `Open in a window`, `Running in the background`, `Sleeping`, `No session`) with its glyph and the age of the last change; then `n tabs: <names> — kept by fleetd` | the only place window names exist; needed before `s` / `K` |
| Git | `Changes` (`Clean` / `n files`), `vs <base>` (`a ahead · b behind`), `Published` (`Yes, <upstream>` / `No`), `Pull request` (a link that opens it in the browser, `b`, plus its chip) and each `warnings[]` string **verbatim** | the same facts the delete/prune confirm quotes, so the confirm is never a surprise |
| **Null facts** | `—` in `fg.faint`, **never `0`** | `ahead`/`behind` are nullable (§1.3) |
| Location | the tilde-collapsed path in a `CopyField` whose button copies it (`y`), then `Created <age> ago · safety checked <age> ago` | `y` copies exactly this string; the stamp keeps stale facts from being trusted |

Every string is built with the row in the Hub's projection; the panel only lays it out.

**Variants**

| Cursor on | Panel content |
| --- | --- |
| Repo | name, owner, `defaultBranch`, `path`, worktree count, live count, `hooks.prepare` (count + commands), `hooks.postCreate`, `url`, prepared copies `1/1 ready · refreshed 2m ago` |
| Clone job | status, staging path, log path, `error` in red; `Enter` opens it in the Jobs panel |
| Context (rail header focused) | name, `owners` joined, repo + worktree counts |
| PR row | §3.5 PR detail |

**Intentionally omitted:** `WorktreeId`, session name string, `host.ssh` / `swarmCommand`,
absolute ISO timestamps, raw hook command lists for worktrees (a repo-level fact), full log tails.

**States:** *never inspected* → Git shows `Not checked` and an `Inspect I` button (absence of
knowledge still renders, §1.3), and the delete confirm escalates to `Y` (§3.8.3). *inspect running*
→ `Checking…` the first time; on a refresh the old values dim to 60 % but stay readable — **never
blanked**. *inspect errored* → the error in red + `Inspect I`.

**Keyboard:** `i` toggles. The panel is **never in the focus cycle** (KEYMAP A1); `j`/`k` always move
the list cursor and the panel always mirrors it. `y` copies the path from anywhere in the Hub. Its
buttons are pointer twins of those keys and act on the cursor row.

---

### 3.5 Hub — Pull requests screen (`p`)

**Purpose:** *What am I waiting on, what is waiting on me, and can I start on it in one key?*

```
 Pull requests                                              [⌕ Filter /]  [⟳ Refresh r]
 Open on GitHub for Acme · fetched 12s ago
 Mine (3)   Waiting for my review (1)
 ─────────────────────────────────────────────────────────────────────────────────────────
 #     Title                                Branch     Status               Checks   Age
▌#12   Add the worktree ticker [has worktree] feature  ✓ Approved           ✓ 6 / 6   3d  Open ⏎  ⋯
 #13   Rework the hub header                hotfix     ⎇ Draft              ✕ 1 failing 5d
 #4    Ship the settings dialog [has worktree] spike   ◉ In review          ⟳ running  1w
```

Columns and breakpoints: §2.9. The detail panel (§3.4) is on by default and shows the PR under
the cursor.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `Pull requests` (`page_title`) | page header | names the page | — |
| Subtitle | `Open on GitHub for <repo \| context> · fetched 12s ago` / `⟳ refreshing…` / `never fetched` | under the title (`PageHeader`) | trust marker for cached data (`github.prTtlSeconds: 90`) | `PrRepoSlice.fetchedAt/loading/error` |
| Filter | the page's `FilterField` with `/` (`prs.filter`); while the filter owns the keys it holds the live editor and `shown/total` | header, right | the pointer's way into §3.10 | — |
| Refresh | secondary button with `r` (`prs.refresh`) | header, right | a refresh is the one thing this page does to itself | `prs::Refresh` |
| Tabs `Mine` / `Waiting for my review` | sentence-case label + count pill; active = 2 px blue underline; the review count is amber when not zero (someone is waiting on you) | under the header | `Tab`/`h`/`l` or a click toggle them; the counts answer "how much is queued" without entering | `PrTab`, §5 |
| Number | `#{number}`, mono, muted | col 1 | the handle you say out loud | `PullRequest.number` |
| Title | single line, weight 500, tail-ellipsised, then a `has worktree` tag when a local worktree matches (a spinning `creating worktree` while `⏎`/`c` makes one) | col 2 | the tag decides whether opening is instant or creates — the highest-value bit on this screen | match rule §1 |
| Author | `login` (null → `ghost`), muted | col 3 | only meaningful on the review tab | `PullRequest.author` |
| Branch | `headRefName` mono; `git-fork` prefix when `isCrossRepository` | col 4 | ties the PR to the branch you will get | `PullRequest.headRefName` |
| Repository | `owner/name`, muted | col 5 | disambiguates in `All` scope | `PullRequest.repoId` |
| Status | the kit's PR chip (as on the worktrees page), reading the review state alone | col 6 | a failing check no longer hides an approval | `isDraft`, `reviewDecision` |
| Checks | `✓ 6 / 6` green · `✕ 1 failing` red · spinner `running` · faint `—` | col 7 | CI state at a glance, separate from review | `checks`, `checksPassed/Total` |
| Age | relative `updatedAt` | col 8 | staleness of the PR, not of the fetch | `PullRequest.updatedAt` |
| Row actions | `Open ⏎` and `⋯`, on hover or on the selected row | col 9 | §5.1: hover reveals, the menu holds everything | — |

**Status chip — exact text, icon and tone** (draft first, then the review decision; the words are `PrBadgeState::chip_word`)

| State | Icon | Tone | Text |
| --- | --- | --- | --- |
| draft | `git-pull-request-draft` | faint | `Draft` |
| changes requested | `message-square-warning` | amber | `Needs changes` |
| approved | `circle-check` | green | `Approved` |
| anything else | `eye` | accent | `In review` |

**Row pointer (§5.1).** A click selects the row (and puts the keyboard on the list); a double-click
is `⏎`. The `⋯` button and a right click open the same menu: `Open ⏎`, `Open, keep last awake O`,
`Create worktree only c` (only when no worktree exists yet), `Check worktree I` (only when one
does), then `Open in the browser b` and `Copy link y`. A verb that cannot work on the row is
left out, not greyed.

**PR detail panel:**

```
┌──────────────────────────────────────┐
│ acme/api #12 · by dannyfuf           │  mono, muted
│ Add the worktree ticker              │  section title
│ [✓ Approved] [feature → main]        │
│ [ Open worktree ⏎ ] [GitHub b] [⋯]   │  one primary
│ Changes   +184 −37                   │
│ Checks    All 6 passing              │
│ Review    Approved                   │
│ Labels    [ui] [hub]                 │
│ Updated   3d ago                     │
│ ┌ Worktree ────────────────────────┐ │
│ │ ⑂ api / feature      on this Mac │ │
│ │ ○ no session                     │ │
│ │ ~/.fleet/worktrees/acme/api/feat…│ │
│ └──────────────────────────────────┘ │
└──────────────────────────────────────┘
```

With a local worktree the primary button is `Open worktree ⏎` and the card names the worktree,
where it lives (`on this Mac` or `on <host> · <link>`), its session and its path. Without one the
primary is `Create worktree c` (`Creating worktree…`, disabled, while it runs) and the card says
what opening will create:

```
│ │ Opening creates payroll/feat-rut from pull/412/head │
│ │ Branch feat-rut                                     │   same-repo: headRefName
│ │ Fork dannyfuf/payroll → pr/412                      │   cross-repo only: local branch pr/<n>
```

**[D-6]** The proposed destination is mandatory: it is what `Enter` is about to create, the
highest-stakes bit on this screen, and §5 requires it. `GitHub b` opens the PR; the `⋯` menu is
the row's menu.

**Intentionally omitted from rows:** additions/deletions and labels (detail), `baseRefName`
(detail, `head → base`), `url` (`y` / `b`), reviewer avatars, a merged/closed section, a third
"All" tab.

**States**

| State | Rendering |
| --- | --- |
| Loading with cache | cached rows stay at full opacity; the tab count becomes `…` and the subtitle reads `⟳ refreshing · fetched 4m ago` |
| Loading cold | 6 skeleton rows at 30 % |
| Empty Mine | `No open PRs authored by you in <scope>.` + a `Refresh r` button |
| Empty review tab | `No PRs waiting for your review in <scope>.` + a `Refresh r` button |
| Error | a red **callout** under the tabs: `Could not load pull requests` over the error, with a `Retry r` button (`prs.retry`); **stale rows stay listed**. The same error also occupies the status-bar sticky slot (`!`). Never a toast (§2.7) |
| Creating a worktree from a PR | the title's tag becomes a spinning `creating worktree`; the row does not move; leaving the screen does not stop the job |

**Scope.** The PR screen scopes to the selected repo, or to every repo of the active context when
`All` is selected. **[D-7]** In `All` scope the list is capped at **100** rows per tab, sorted by
`updatedAt` desc, with a final ghost button `+n more — select a repo to narrow` (`prs.more`) that
runs `g r` and puts the keyboard on the repositories (the cap matches the §9 PR cap of 100 and
never refuses).

**Icons:** the chip table above, `circle-check` / `circle-x` / `loader-circle` for checks,
`git-fork`, `git-branch`, `refresh-cw`, `ellipsis`.

**Keyboard:** `Tab`/`S-Tab`/`h`/`l` tabs · `j`/`k`/`gg`/`G` · `Enter`/`o` open-or-create (sleeps
previous) · `O` keep previous awake · `c` create without opening (KEYMAP A9) · `b` browser · `y` copy
URL · `r` force refresh both tabs · `I` inspect the matching local worktree · `i` detail ·
`/` filter · `p`/`q` back. Every one of them is also a button or a menu item above.

---

### 3.5.1 Hub — Board (`g b`)

The board is the Hub's third tab, beside Worktrees and Pull requests, and its full contract —
columns, card tiles, the card detail, the four board dialogs, and every key — lives in
`docs/BOARD.md` §8. Three things it does differently from the rest of the Hub are stated here
because they are cross-screen rules:

- **The board owns the body and its own keys.** While it is up, the Hub's rail and list are not
  composed at all, and the board's bindings shadow the inherited `Hub` ones (`docs/KEYMAP.md`
  "Board and card detail").
- **`/` is not the Hub's filter.** The board's rows are cards in columns, not worktrees, so
  it keeps its own query in `BoardState.filter` and publishes the `Filter` key context while the
  input has the keyboard — which is what makes bare letters type instead of fire (§3.10's two-stage
  `Esc` still applies: leave the input, then clear the filter).
- **The breadcrumb names the card, live.** §2.2's third segment is derived when the status bar is
  built rather than cached by the body's render, because the status bar is composed first and a
  cached row would name the previously selected card.

---

### 3.6 Workspace

**Purpose:** *Be a terminal. Say only which terminal I am in, whether the session is healthy, and
what is running elsewhere.*

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ← Worktrees / buk/payroll / ⎇ feat-payroll-fix ⌄  ↑2 ↓0  3 files changed  ◷ #412 CI fail   [⌕ Search…]  ? ⚙ │ 44  title bar
├─────────────────────────────────────────────────────────────────────────────┤
│ ╭──────────╮                                                                │
│ │>_ nvim  ✕│ >_ cc ● │ ⎇ lg │ >_ test exited 1 │ ✦ fix README needs you │ + Watch Zoom │ 40  tab strip
├─╯          ╰────────────────────────────────────────────────────────────────┤
│ ❯ claude                                                                    │
│ ⏺ Reading src/payroll/rounding.rb…                       ┌────────────────┐ │
│                                                          │ SCROLL 412/2000│ │ scroll pill
│                                                          └────────────────┘ │
│    ┌─────────────────────────────────────────────────────────────────┐      │
│    │ ⌃S Fleet commands  Press a key or click. ⌃S again…  Close esc   │      │ ⌃S command
│    │ Tabs          Session         Terminal     Agents        Panels │      │ menu (only
│    │ 1–9 Go to tab s Back to hub   [ Scroll back a New Claude… ? All…│      │ after 400 ms
│    │ c New terminal …              …            …             …      │      │ of hesitation)
│    └─────────────────────────────────────────────────────────────────┘      │
├─────────────────────────────────────────────────────────────────────────────┤
│ ⚠ process exited (1) · ^s r restart · ^s x close · ^s c new                 │ 22  only on exit
├─────────────────────────────────────────────────────────────────────────────┤
│ ● fleetd  nvim · 164×42 · kept alive by fleetd   Fleet commands ⌃S  Shortcuts ⌃S ? │ 28  status bar
└─────────────────────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Breadcrumb | `← Worktrees` (`⌃S s`) / `repoId` / the worktree switcher `⎇ <worktree> ⌄` | title bar, left (§3.1) | one line answering "am I in the right worktree?" — the #1 terminal error — in the row that is always there | `Session.kind = Worktree(id)` |
| Worktree switcher | a menu of every session, most recently used first, each with the palette's GO state word (`session attached`, `sleeping`, …) and a `✓` on this one; then *Last session* `⌃S w` and *All sessions…* `⌃S W` | the breadcrumb's last segment | the same rows as `⌃S W`, one click away; choosing one opens it exactly as the palette row does | `session_mru`, the palette's GO rows |
| Git chips | `↑a ↓b` while either is non-zero · `n files changed` while dirty | after the switcher | whether the work here is committed and pushed, without opening Lazygit. From an inspection (no fetch) when the Workspace shows the worktree and every 30 s while it stays; a failed inspection keeps the last chips | `InspectWorktrees` |
| Host chip | `cloud <host>`, `cloud-off` amber while the link is down | after the git chips, remote worktrees only | a remote shell must not pass for a local one | `Worktree.host`, `HostStatus` |
| PR button | `#412 CI fail` in the §3.5 badge's glyph and tone | after the chips | a click opens the pull request on GitHub | the shared PR badge cache |
| Tab strip | 40 px, `chrome` ground: per tab a kind glyph and the name, min 84 / max 200 px; the index is in the tooltip (`Tab 2 ⌃S 2`) | under the title bar | the strip says what each tab is; its keys are in its tooltips and menus | `Session.terminals`, the worktree's agent threads |
| Kind glyph | `terminal` (PTY) · `git-branch` (a Fleet-drawn tab: `lg`) · `square-kanban` (the board tab) · `bot` / `sparkles` (a Claude / Codex thread) | before the name | a tab that is not a shell says so before you type into it | `Terminal.kind`, `command`, `AgentThreadSummary.provider` |
| Active tab | the content ground, a hairline on three sides and none underneath, `ui_strong` name | — | the active tab joins the content it shows | — |
| State mark | at most one: a spinner while the tab starts or its agent works · an amber `needs you` chip (survives selection) · a 6 px blue dot for unseen output on another tab · `exited 1` in red (`killed` for a signal) | after the name | the only background-activity signal; prevents polling tabs by hand | `Terminal.status`, dirty-row events, `Attention` |
| Keep-alive icon on a tab | `bot` / `server` / `file-pen` | after the name | marks the tabs `sleep` will preserve, before you press `^s x` | §4 keep-alive rules |
| Close `✕` | on the active tab and on a hovered one; a middle-click does the same | tab, right end | the pointer twin of `^s x`, with its confirm when a keep-alive process runs | — |
| Tab menu | right-click: *Rename* `⌃S ,` · *Restart command* `⌃S r` (an exited PTY) · *Close* `⌃S x` · *Close others* (a tab running a keep-alive process stays open, and a toast says how many); the right-click selects the tab first | at the pointer | every tab verb in one place, each showing its key | — |
| `+` | a menu: *Terminal* `⌃S c` · *Claude thread* `⌃S a` · *Codex thread* `⌃S A` · *Board* `⌃S b`; then *Lazygit* (selects the git tab, or opens one) and, on an agent tab, *Terminal fallback* `⌃S F` | after the last tab | what can be opened here, with its key | — |
| Strip toggles | *Open as terminal* `⌃S F` on an agent tab · *Watch* `⌃S v`, pressed while the watch split shows, only while the session has a subagent watch · *Zoom* `⌃S z` | strip, right | the pointer twins of the Workspace's panel keys | — |
| Native child tab | `↳ <provider> — <title>` | in the same numbered strip, immediately after its caller and older attached siblings | the arrow is the sole child-specific tab chrome; no provider or `child` badge is added | `AgentThreadSummary.parent`, the window-local attached set |
| Terminal area | painted cell grid, 8 px padding, no border | fills | maximum rows; chrome is the 40 px strip | — |
| Native pane | the Fleet-drawn view for this tab, filling the terminal area exactly | replaces the grid | the tab is a tab: same strip, same bars, same pixel positions | `Terminal.kind = Native`, `Worktree.path` |
| Scroll pill | `SCROLL <offset>/<scrollback_len>`, + a second line `v select · y yank · Esc exit` while selecting; 176 × 22 px, `bg.raised`, amber left bar | overlay, top-right **inside** the terminal area, 12 px inset | during scroll the eyes are on content; top-right never covers the prompt and never shifts the grid | `viewport{scrollback_len, offset}` |
| ⌃S command menu | *Fleet commands*: the held prefix as an amber chip, "Press a key or click", "⌃S again sends it to the terminal" (only where a program is behind it), a Close `esc` button; then every command the prefix reaches here, in the catalogue's groups (Tabs, Session, Terminal, Agents, Panels), each a clickable row of second-key chip + short label | floating, bottom-centre just above the status bar, at most 900 px wide; **delayed 400 ms** after `ctrl-s` | the expert types the second key in < 200 ms and never sees it; the returning user gets the whole table exactly when they hesitate, and can click instead of reading — 0 px and 0 frames of permanent cost. A click runs the row as its key would; the menu never takes focus. The agent popup and a native agent tab show it too, with their own rows | KEYMAP one-shot Prefix mode; the action catalogue |
| Exit strip | `⚠ process exited (<code>) · ^s r restart · ^s x close · ^s c new` | bottom, 22 px, only when the tab's command exited | tmux's `remain-on-exit` made this recoverable; Fleet must not silently swallow a crashed dev server | §4 `remain-on-exit on` |
| Status bar | over a PTY: `<tab> · <cols>×<rows> · kept alive by fleetd` in the breadcrumb slot; then `Fleet commands ⌃S` (enters the prefix, as `ctrl-s` does) · `Shortcuts ⌃S ?` (Help) | status bar | which process the keys reach and that closing the window does not end it; the two keys every other Workspace key hangs off, taught where the eye rests; there is no mode word (§2.8) | `MirrorGrid`, KEYMAP one-shot Prefix mode |

**Terminal modes are not drawn.** The frame's VT modes (`alt`, `mouse`, `paste`, `appcur`) used to
be four unlabelled glyphs in the session header. Only one of them changes what a documented Fleet
key does — the alternate screen has no scrollback — and that one already says so where it bites:
`ctrl-s [` toasts `no scrollback in alt-screen`. The other three change no Fleet key, so nothing
replaces them. The kit's `TerminalModes` stays for the gallery.

**[D-8]** Every Workspace command drawn over the Workspace states its prefix. In Terminal mode
keys go to the PTY except `ctrl-s` and the standard `cmd-c` / `cmd-v` clipboard actions, so
bare-key hints (`r restart`, `l log`, `⏎ start now`) are forbidden anywhere in this screen; they
are written `^s r`, `^s l`, `^s ⏎`.

**Zoom (`ctrl-s z`)** hides the tab strip; a 2 px amber bar on the window's top edge remains as
the only reminder that chrome is hidden. The title bar, with its breadcrumb, and the status bar
always stay, because they carry the way back, the daemon and the ⌃S buttons.

**Intentionally omitted:** the PTY window title (`FrameUpdate.title` names the *tab* only when the
terminal was never renamed), shell PID, `foreground_command`, cwd, latency readout, a permanent key
cheat sheet, a scrollbar, per-tab byte counters, pane splitting, the tab index on the tab (it is in
the tooltip), the session's status glyph and a Workspace jobs chip (the title bar's jobs button
counts every job), and a *Changes* toggle until the Changes panel exists.

**States**

| State | Rendering |
| --- | --- |
| Attaching | one dim centered line `attaching…`; key/paste input retains an ordered prefix capped at 1,024 events and 1 MiB, rejects newer overflow, and flushes only after the first valid frame. Attach has one absolute 5 s deadline; failure reports `could not attach terminal <id>: …` and stays sticky until that same terminal attaches, which retires it; already-expired work cannot resize, and no post-deadline result can claim the terminal. |
| Attached | normal |
| Waking a slept session | tabs rebuild with `loader-circle` per tab as each PTY spawns; the breadcrumb adds `waking…` for ≤ 1.5 s |
| Recognized terminal agent working / idle | the agent terminal shows an amber spinning `loader-circle` / green `circle-check`; this heuristic status is glyph-only and never toasts or plays a sound |
| Terminal agent hook attention | the PTY tab keeps the same amber `needs you` chip native tabs use, including while selected; each session edge into permission, question, plan, or finished uses the configured toast/sound channels once |
| Terminal exited | grid frozen at the last frame + the exit strip |
| Alt-screen app running | the scroll pill is **suppressed**; `ctrl-s [` shows the 1.6 s toast `no scrollback in alt-screen` |
| Non-agent native pane selected (`fleet://`: `lg`, or the `board` tab of `ctrl-s b`) | the grid, the scroll pill and the exit strip are all absent — there is no PTY. `ctrl-s [` toasts `no scrollback in this tab`; every key except `ctrl-s` belongs to the pane. The snapshot's `mode` is `Native` (§2.8): the Workspace still has the keyboard, and the pane says so itself. A native **agent thread** is the separate §3.6.0 surface and `ctrl-s [` enters its transcript scroll mode. |
| Job running for this worktree | the title bar's jobs button and the status-bar ticker; **never** an overlay on the grid |
| Daemon lost | grid dims to 55 %, keys are dropped (not buffered), and the §3.12 C banner appears |

Caller threads stay in daemon order. Each caller is followed by only its **attached** children,
in child creation order; an unattached child remains daemon-owned and reachable through its
delegation row or `^s d`, but consumes no strip slot. `^s x` closes a caller tab as usual and
detaches a child tab without stopping or deleting that child.

**Icons:** `git-branch`, `cloud`, `cloud-off`, `chevron-left`, `chevron-down`, `terminal`,
`square-kanban`, `circle-check`, `loader-circle`, `zap`, `bot` (Claude), `sparkles` (Codex),
`server`, `file-pen`, `plus`, `x`, `square-pen`, `refresh-cw`, `square-terminal`, `chevrons-up`
(scroll pill), `unplug`.

The default third tab (`lg`) is a native tab: Fleet's own git UI (`crates/fleet-lazygit`) drawn
in the terminal area, created the first time the tab is selected and kept alive per worktree
until the daemon stops listing that worktree. Its own keys are documented in
`crates/fleet-lazygit/README.md`; `q` inside it selects the previous tab rather than quitting
Fleet. **[D-8] still holds**: the pane's key-hint bar lives *inside* the pane, which is its own
key context, so its bare keys are not "drawn over Terminal mode".

The `board` tab `ctrl-s b` opens is the other native tab, and it is Fleet's own board rather
than an embedded app: it draws **this worktree's** board — `EnsureWorktreeBoard(worktree)`, not
the active context's — with the same columns, cards, dialogs and keys the Hub's board tab has,
under the key context `Fleet > Workspace > Native > Board` (`docs/KEYMAP.md`). It builds
nothing per worktree: the Hub tab and this one share one board mirror behind a scope, and
selecting the tab is what points that mirror at the worktree. Selecting any other tab, closing
the tab or leaving the Workspace points it back at the active context. `o` on a card linked to
the worktree you are already standing in answers `Already in this worktree` rather than
re-opening the session.

**The board tab is not in `windows[]`.** `ctrl-s b` asks fleetd for it the first time and
selects it every time after, so it is a tab the session acquired rather than one it was born
with: sleeping the session closes it with every other non-kept tab, and waking does not bring
it back. That is one keystroke to undo — `ctrl-s b` again — and it is the reason the key is
worth memorising rather than the tab position. A user who wants it permanent adds
`{"name":"board","command":"fleet://board"}` to `windows[]` in `config.json` (§3.8.6); it is
then created with every new session and rebuilt on wake like `lg`, and `ctrl-s b` still selects
it instead of adding a second one, because the tab is recognised by its reserved command and
never by its name — rename it with `ctrl-s ,` and the key still finds it.

**Keyboard:** all keys → PTY except `cmd-c` copy selection and `cmd-v` paste — or, on a native
tab, every key except `ctrl-s` → the pane; `ctrl-s` then
`ctrl-s` (literal) · `s` hub · `1`–`9` tab ·
`h`/`l`, `p`/`n` prev/next tab · `Tab` last terminal tab (KEYMAP A2) · `w` last session (KEYMAP A3) ·
`W` session switcher (KEYMAP A4) · `u` select this child's caller · `d` `AGENTS` picker ·
`S` sleep this session and return to Hub (KEYMAP A5) · `c` new tab ·
`b` this worktree's board tab — created the first time, selected every time; on a session with
no worktree it toasts `boards belong to worktrees` ·
`x` close tab (confirm if a keep-alive process runs) · `,` rename · `[` scroll · `]` paste ·
`a`/`A` new native Claude/Codex agent thread · `F` the agent PTY popup (the terminal fallback) · `r` restart the exited command (KEYMAP A10) · `y` copy worktree path (KEYMAP A11)
· `z` zoom · `!` sticky error slot (prefixed: `^s !`, KEYMAP A18) · `J` jobs · `?` help · `Esc` cancel
prefix.

---

### 3.6.0 Native agent tab

**Purpose:** *Show me what the agent is doing, and let me answer it, without reading a terminal.*

A Claude Code or Codex session is a numbered tab in the **same strip** as the terminals —
`[2] claude — rounding fix`, `[6] codex — tz shifts` — drawn by Fleet rather than by a PTY.
`^s a` starts Claude, `^s A` starts Codex, `^s x` closes the tab and keeps the transcript, the
close survives a relaunch and a daemon restart, and `^s F` opens the PTY popup below as the
explicit fallback. There is no thread-list sidebar, no inspector and no detached diff pane.
`docs/NATIVE-AGENTS.md` is the authority for the event
model and the state machine, `docs/KEYMAP.md` for exactly which `^s` keys an agent tab binds;
this section is what the screen shows.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑂ feat/payroll-fix  buk/payroll  #412 CI fail      needs you          ⟳1    │  30  header
├─────────────────────────────────────────────────────────────────────────────┤
│  1 nvim ✎ │ 2 claude — rounding fix ● │ 3 lg │ +                            │  30  tab strip
├─────────────────────────────────────────────────────────────────────────────┤
│        ┌─────────────────────────── 760 ───────────────────────────┐        │
│        │ ┌───────────────────────────────────────────────────────┐ │        │
│        │ │ fix the rounding in the payroll totals                │ │        │  user block
│        │ └───────────────────────────────────────────────────────┘ │        │
│        │ thinking · 6s  [⏎] show                                   │        │  thinking line
│        │ I'll start from the totals helper.                        │        │  assistant prose
│        │ ◉ Read   src/payroll/rounding.rb             0.2s   ›     │        │  30  tool row
│        │ ◉ Edit   src/payroll/rounding.rb   (+14 −3)  ⧉ ◫ ↗ ›     │        │  30  hovered row
│        │ worked 12s · 3 tool calls  ›                              │        │  fold
│        │   48s · 12.4k tokens · 2 files +36 −3  [Diff] [Revert turn]│        │  turn footer
│        │ for FLT-5 · In review · Fleet                             │        │  link line
│        │ ╭───────────────────────────────────────────────────────╮ │        │
│        │ │ 🔒 claude wants to run a command               1 of 2 │ │        │  decision dock
│        │ │ ┌ rm -rf tmp/cache ─────────────────────────────────┐ │ │        │
│        │ │ [Allow once y] [Allow for this session a] [Deny n]    │ │        │
│        │ │ [Edit e]                          Deny and stop esc   │ │        │
│        │ ├───────────────────────────────────────────────────────┤ │        │
│        │ │ Message claude… @ files · $ skills · / commands       │ │        │  composer
│        │ │ opus-5 · high ⌄  asks before edits ⌄  [Build|Plan]    │ │        │
│        │ │                       ▰▱ 34%  $0.42 · 48m  [Send ⏎]   │ │        │
│        │ ╰───────────────────────────────────────────────────────╯ │        │
│        └───────────────────────────────────────────────────────────┘        │
├─────────────────────────────────────────────────────────────────────────────┤
│ ● fleetd  buk › feat-payroll-fix                         Shortcuts ⌃S ?     │  28  status bar
└─────────────────────────────────────────────────────────────────────────────┘
```

The content measure is **760 px** with a 16 px inset, centered; a wide window leaves the right
side empty on purpose. The transcript is bottom-anchored and the composer is docked under it.

The composer's `/` picker lists Fleet's own built-ins before the harness's commands: `model`,
`plan`, `default`, `compact`, and — on a **Codex** thread only — `login` and `logout`, which sign
that harness in and out of its provider account. They are absent on a Claude thread because
Claude publishes no account surface, and `DESIGN-SYSTEM.md` §4 does not list a command that would
answer "unsupported". `/login` opens Codex's ChatGPT page in the user's browser and says so in a
toast; the URL is also a `Notice` row, so it stays reachable when the browser does not open.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Tab | `<index> <provider> — <title>`, title from the first message or a later provider metadata update; bare provider name until there is one | the terminal strip | one strip, one numbering: an index addresses exactly one surface | `AgentThreadSummary.title` |
| Tab mark | spinner · amber dot · neutral dot · `exited <code>` — **at most one** | inside the tab | §3.3 of `NATIVE-AGENTS.md`; amber wins over neutral because amber means *waiting on you* | `AgentThreadSummary.attention` |
| Session header word | `working` · `needs you` · `failed` · `idle` | header, right | the same vocabulary as the tab and the chips, so three surfaces cannot disagree | idem |
| Title-bar `needs you` | `<n> needs you`, including the current tab; a click opens the waiting thread | §2.3 | a blocked thread on another worktree is invisible otherwise | `AgentCounts` |
| User turn | the message on `bg.panel`, radius 6, attachments as pills below | transcript | the **only** block with a background: it is the one thing the user wrote | `ItemKind::UserMessage` |
| Assistant prose | Markdown on the ground — no bubble, no avatar, no header | transcript | the answer is the content; chrome around it is noise | `AssistantText` |
| Thinking | one collapsed muted line, `thought 6s` + `[⏎] show`; while it streams it **is** the live row | transcript | reasoning is available, never dominant | `ItemKind::Reasoning` |
| Tool row | 30 px pointer-first row: state glyph · 60 px verb column (`Read`, `Edit`, `Run`) · one-line summary · result chip (`+1 −1`, `exit 1` in danger, `waiting for you` in amber) · duration · chevron; a click toggles it as `⏎` does; under the pointer **Copy** (`y`), **Diff** (`d`) and **Open in editor** (`o`) icon buttons, and the same verbs on a right-click menu; `agents.tool[N]` | transcript | one shape for every tool means the eye scans a column, not sentences; the verbs are reachable without scroll mode | `ItemKind::Tool` |
| Nested rows | children indented 16 px behind a 1 px divider | under an `Agent` row | a subagent's work belongs to the row that started it | `Item.children` |
| Inline diff | `DiffView` under an `edit` / `write` row, red/green 14 % washes | expanded row | the review happens where the edit is announced | `ToolDiff` |
| `worked …` fold | `worked 12s · 3 tool calls` + `[⏎] show` | end of a settled turn | finished successful work is history; a **failed** row stays exposed | `TurnRecord.ended` |
| Turn footer | `48s · 12.4k tokens · 2 files +36 −3` + **Diff** (opens every file change of the turn) and **Revert turn** (only where a checkpoint exists) as compact ghost buttons reporting `d` and `u`, right-aligned | after the fold | one line of accounting per turn, withheld until the turn completes so it never moves under the reader | `TurnEnd` |
| Checkpoint line | `context compacted · 84k → 12k tokens` · `session resumed · 2h ago` | transcript | the two moments that silently change what the agent remembers | `Checkpoint` |
| Error card | the failure, or `rate limited · retrying in 12s` with a spinner while a backoff counts down | transcript | a backoff is progress, a failure is not; they must not look alike | `RuntimeError`, `Retrying` |
| Notice | one muted line with an amber glyph — a config warning, a deprecation, `Stop hook error occurred`, or `Claude Code started this turn on its own` | transcript; a turn-scoped one leads the turn it explains and never folds | the provider is talking to the user, not failing; an unrecognised frame is a tracing diagnostic and never a notice | `Notice`, `ThreadProjection.notices`, `ItemKind::Notice` |
| Decision drawer | 760 px, `bg.panel`, top corners only, docked to the composer's top edge with the shared border masked; an amber glyph-led title naming the subject (`codex wants to edit README.md`) and `1 of N`; the payload well and the diff under a file header; a row of buttons with live key chips — **Allow once** (primary) · **Allow for this session** · **Deny** · **Edit** (Claude only) · **Deny and stop** (ghost danger, far right); a question's options are clickable rows with digit chips (checkboxes when multi-select) over **Answer** / **Next** and **Previous**; a plan's are **Implement** (primary) and **Refine** | above the composer | a card in the transcript can be scrolled out of the viewport while it still owns the keyboard, which is a modal with the chrome removed; the drawer is always on screen by construction; each button reports the action its key does | `OpenGate` |
| Plan card | the plan's promoted title, its body faded out past 900 chars or 20 lines, and no actions of its own | transcript | a plan is a durable artifact the user scrolls back to and quotes; its *verbs* live in the decision dock, beside the composer, because whether you implement or refine is decided by whether you typed anything | `ItemKind::Plan` |
| Settled gate row | one line — `allowed once · bash: git push --force`, `answered · which package manager? → pnpm`, `withdrawn · the agent stopped waiting` | transcript, where it was asked | docking the live drawer must not lose the narrative | resolved `OpenGate` |
| Live activity row | one row, one id, present tense: `working 1m 12s` → `thought 6s` → `running cargo` | pinned in the running turn | thinking → tool A running → tool A done → tool B running is one row changing its label, not four mounts | `RowId::LiveActivity` |
| Steered message | an ordinary user bubble with a leading `↳` | transcript, inside the running turn | a message sent while a turn runs is a steer, dispatched immediately — there is no queue and no queued row | `UserRow.steered` |
| Delegation row | `↳` + provider glyph + title + `starting` / `working` / `blocked` / `done` / `incomplete` / `failed` / `cancelled`; one gray spinner, amber dot, `circle-check` or `circle-x`; optional headline on line two; elapsed time and `⏎ attach` trail; the line is a click target that attaches the child, as `⏎` does | at the caller item that launched the child | the durable row keeps a hidden child reachable and changes in place as `DelegationChanged` arrives, without rewriting the caller projection | `ItemKind::Delegation` joined to `Delegation` |
| Delegation result card | `↳ <provider> finished · <word> · <elapsed> · <n> files` above the delivered Markdown; collapsed to eight lines with the ordinary `⏎ show` / `⏎ hide` fold affordance | the caller's delivered user-message origin | the answer reads as a result of the child rather than as text typed by the user; `Enter` expands the body without attaching the still-hidden child | `UserMessage.origin = Delegation { id }` |
| Composer | one framed panel: the editor (placeholder `Message claude… @ files · $ skills · / commands`, grows one line at a time to eight; `⏎` sends, `⇧⏎` inserts a newline; `agents.composer`) over a settings strip | docked, bottom | what the next send carries is visible and clickable, not hidden behind `^s m` and `^s t` | `MultilineInput` |
| Settings strip | left: the model chip `claude-opus-5 · high ⌄` (its menu lists the harness's models, then its efforts — `^s m`, `^s e`), the access chip `asks before edits ⌄` (the declared ladder — `^s t`), a **Build \| Plan** control (`⇧⇥`); right: the context meter `34%`, `$0.42 · 48m · dev@example.com`, and **Send** (`⏎`; **Steer** with a draft while the agent works, **Stop** `esc` without one; `agents.send`); **nothing the harness does not report is shown** | the composer's last line | each control calls what its key calls; a chip's tooltip names its key | `ThreadProjection`, `ComposerChip`, `SegmentedControl`, `ContextMeter` |
| Child caller segment | pinned first segment of the link line `for [<n>] <provider> — <title>`; `[<n>]` is `·` while the caller is hidden; link tone and focus ring, target = caller thread | the muted link line above the composer, unmounted under an approval | activating it or `^s u` attaches the caller when needed, selects it and focuses its composer | `AgentThreadSummary.parent`, `MetadataSegment.target` |
| Child composer | `Steering a subagent of [<n>]. It reports to its caller when it finishes.` | composer placeholder on a child only | makes the reporting boundary explicit before a human steers the child | caller strip index |
| Card caller segment | pinned **first** link-line segment `for <KEY> · <column> · <board>`, ahead of a child caller segment when a thread somehow has both; link tone and focus ring, target = the card | first and non-collapsible on a card run's link line | a column-started run is a caller like a thread is, and this is the only chrome its tab has that no other tab does | `Delegation.caller = Card { .. }`, the shown board |
| Card composer | `Steering a card run. Its report moves the card when it finishes.` | composer placeholder on a card run only | the reply steers the run; the card's column, not the reader, moves the card | card caller |
| Account segment | the **last** fact: `signed out` when the harness reports no account, else its email — or its plan when there is no email — and **nothing at all** when the harness reports no account signal (Claude always, Codex until its first `account/read`) | end of the settings strip's facts | the first thing a revoked token costs is a turn, and the composer is where the user finds out why before the refusal | `ThreadProjection.account` |
| Empty state | `new claude thread · feat-x` over `ask anything · @ files · $ skills · / commands` | centered in an empty transcript | a new thread must say what to type | — |

**Copy is fixed.** `Allow once` · `Allow for this session` — the effective scope is always spelled
out and the word "always" is never used on a command or a file change, on either harness. The
permission mode reads `asks before edits` · `accepts edits` · `plans before editing` ·
`auto-approves safe actions` · `denies unlisted tools` · `full access`; each picker shows only
the modes in that session's `capabilities.modes`, in declared order. Build ⇄ Plan uses the same
Plan value as the picker rather than creating a second planning concept.

**Color is semantic** (§1.4 unchanged): green = alive, amber = needs you or cannot verify, red =
broken, gray = everything else *including progress*, blue = where you are and never a state.
Only gray spinners and the text caret animate; attention is a static amber dot or bar.

**States**

| State | Rendering |
| --- | --- |
| Empty thread | the empty-state line only; the composer is focused |
| Working | tab spinner, header `working`; the composer stays live and `⏎` **steers** the running turn, dispatched immediately; `esc` interrupts the *active* turn and holds `stopping…` until the daemon reports liveness cleared |
| Streaming | text grows in place with a caret after the last paragraph; tool rows keep their 30 px geometry so nothing jitters |
| Decision open | the drawer is docked above the composer, its title led by an amber glyph, the composer dims to 60 % and Send is disabled; bare keys and the drawer's buttons route to the same actions. **`⏎` is not bound on a permission** |
| Turn settled | successful tool rows fold; a failed row stays; the footer appears once, complete |
| Interrupted | footer reads `stopped · 12s · 3.1k tokens`; there is no error card — stopping is not failing |
| Failed / exited | red `exited <code>` on the tab, header `failed`, an error card at the end of the transcript |
| Scroll mode (`^s [`) | the tail is frozen so new output cannot pull the viewport away, and the row nearest the bottom takes the focus ring — which is what makes `⏎`/`u`/`o`/`y`/`d` fire. `G` reaches the newest row **without** leaving the mode; only `q`/`i`/`esc` re-arms the follow |
| Unread | a neutral dot on the tab only; the header stays `idle`, because nothing is waiting on the user |
| Provider unavailable | the create fails with the typed reason and names `^s F`, the terminal fallback — never a silent no-op. The reason is the daemon's, verbatim, and names the harness and the `agentBinaries` command that failed (`` `cc` is not Claude Code: `cc (GCC) 16.2.1`. ``) rather than a default executable name Fleet never ran |

**Intentionally omitted:** a thread sidebar, an inspector, a detached diff pane,
per-message timestamps, an avatar or role header on assistant text, a token counter that moves
during a turn, and any modal for a permission.

**Icons:** `loader-circle` (running, retrying), `circle-check` (done), `circle-x` (error,
denied, exited), `triangle-alert` (error card), `circle-arrow-down` (jump to latest), `bot` /
`sparkles` on the tab and the `needs you` chip; `lock` / `message-square-warning` / `list-checks`
lead the decision drawer; `copy`, `file-diff`, `external-link` and `undo-2` are the row verbs;
`shield` leads the access chip, and `square` / `square-check` are a multi-select question's boxes.

**Keyboard:** `docs/KEYMAP.md` § *Native agent thread* is authoritative. In short: `⏎` send ·
`⇧⏎` newline · `⇧⇥` mode · `/` commands · `@` files · `$` skills · `↑` history · `^s m` model ·
`^s e` reasoning / traits · `^s t` access mode · `esc`
interrupt while working · `^s [` scroll · `^s x` close a caller or detach a child · `^s u`
select the caller · `^s d` open the `AGENTS` picker · `^s a`/`^s A` new thread · `^s F` the
PTY fallback. With a delegation row focused, `Enter` attaches and selects its child and `x`
cancels the delegation; with a result card focused, `Enter` expands or collapses its body without
attaching the child. `y` copies the delegation id from either row. A decision drawer takes `y` /
`a` / `n` / `e` / `esc`
(permission), `1`–`5` / `space` / `⏎` / `p` (question), and `y` / `n` / `⏎` on the plan's
composer context (implement / refine / send the current draft).

---

### 3.6.1 Agent popup (terminal fallback)

The Claude/Codex agent is a window-wide `AppFrame.overlay` surface above the current Hub or
Workspace, centered at **90 % of window width × 85 % of window height** over the dialog scrim,
in a rounded frame at the **popover** elevation (DESIGN-SYSTEM §2.6) so it reads as a window of
its own. The base screen remains mounted and rendering: its list selection, active tab, native
pane, and terminal attachment do not change.

A `title_bar_h` (44 px) window header reads, left to right:

```
[✦ Claude] [▣ Codex ⌃S A]   ● agent-claude · idle · runs in fleetd, hiding keeps it alive   [Restart ⌃S r] [Hide ⌃Q]
```

- **Provider switch.** `Claude | Codex` as two segments, marked `bot` and `sparkles` as the tab strip marks them (§3.6); the showing provider is the selected
  one and has no action (its key would hide the popup). The other segment switches to it and
  carries `^s a` / `^s A` as its chip. A config still naming OpenCode labels the second segment
  `OpenCode`.
- **Session line.** A status dot driven by the shared agent-activity source (amber while
  working, green while idle or otherwise live, red when exited or unreachable), the fixed session
  id in muted data type, then `· <idle|working|starting|exited|fleetd unreachable> · runs in
  fleetd, hiding keeps it alive`, which answers "will hiding kill it?" before it is asked.
- **Restart** (`^s r`) — ghost, drawn disabled until the agent's process has exited, because the
  daemon restarts only an exited command. **Hide** (`ctrl-q`) — secondary.
- **While attaching** (the daemon is still ensuring the session) the header keeps the provider
  switch and Hide, without the session line or Restart, above the `attaching…` body — so a
  pointer can switch away from, or put away, a session that is slow to start.

There is no "Open as tab": no action moves the popup's PTY session into a Workspace tab, and a
native thread (`^s a` in a terminal) is a different session. The header states no key hints of
its own and the status bar shows no agent keys while the popup is up — every key is on the control that
does the same thing. The body is the same `TerminalGrid` used by Workspace, sized from its
measured popup area and resized with the window.

**Pointer.** A left press on the scrim — anywhere outside the card — hides the popup exactly as
`ctrl-q` does (it dispatches the same action). Hiding never kills the session, so a stray click
costs nothing: `a` or `^s a` brings back the same PTY. A press while Help or a quit confirmation
is open above the popup belongs to that dialog, not to the scrim.

Hiding releases only the popup's attachment claim (leaving an identical underlying Workspace
claim intact); it never kills or sleeps the daemon session. Reopening therefore restores the same
PTY and scrollback. Popup state is independent of the base screen and focus returns to the Hub
pane, Workspace terminal, or native pane underneath. Help and the quit/stop confirmation dialogs
may open above the popup; palette and Settings are unavailable while the popup owns focus.
`ctrl-q` hides here (swarm parity), while `ctrl-shift-q` retains the global quit-and-stop-daemon
confirmation.

**Keyboard:** Hub `a`/`A` and Workspace `ctrl-s a`/`ctrl-s A` open the floating Claude/Codex
popup. Inside it all keys go to the PTY except `ctrl-s`, `cmd-c`, `cmd-v`, `ctrl-q`, and the
unchanged global `ctrl-shift-q`. Popup Prefix binds `q` hide · `a`/`A` hide-current-or-switch ·
`[` scroll · `]` paste · `r` restart exited command · `?` Help · `ctrl-s` literal · `Esc` cancel.
Agent Scroll mode matches Workspace Scroll mode.

---

### 3.7 Jobs panel (`J`)

**Purpose:** *What is the daemon doing for me, is it stuck, what failed, and what can I do about it?*

Right-docked sheet, **440 px** wide (**640 px** when a log is expanded), full height between the
title bar and the status bar, `bg.raised`, 1 px left border. It appears and disappears in
place — nothing slides (DESIGN-SYSTEM §2.7). The list behind stays fully visible and readable —
a centered modal would hide exactly the rows the jobs are about. A close ✕ sits at the top right of
the header, and a click anywhere in the band outside the sheet closes it; both dispatch `jobs::Close`,
the action `Esc` runs when no log is expanded (ADR 0023).

The mockup dims the rows behind the sheet; Fleet deliberately does not (no scrim), because those
rows are what the jobs are about and must stay readable.

```
                              ┌──────────────────────────────────────────────┐
                              │ Jobs  1 running · 1 failed  Clear finished D ⋯│ ✕
                              │ [All 7] Running 1  Failed 1  Done 5           │
                              ├──────────────────────────────────────────────┤
                              │▌⟳ Clone acme/infra               0:42 Cancel c│
                              │   ██████████████░░░░░░░░░░░░░░░░░░░░░    64% │
                              │   Receiving objects: 64% (5121/8002)        │
                              │ ✕ Run hooks for acme/api#broken          0:07 │
                              │   ┌ npm install exited with code 1 ────────┐ │
                              │   └ ERR! peer dep react@18 conflicts … ────┘ │
                              │   [Retry R] [Show log ⏎] Copy log path y     │
                              │ Finished                                     │
                              │ ✓ Create acme/api#injected-1     2s · 1m ago │
                              │ ✓ Inspect worktrees              1s · 3m ago │
                              ├──────────────────────────────────────────────┤
                              │ Jobs run in fleetd and survive closing this  │
                              │ window.                                      │
                              └──────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title and summary | `Jobs` + `n running · n failed`, zero-suppressed, the failed count in `danger` | header row 1 | the count is why you opened it | `JobManager` |
| Clear finished · ⋯ | `Clear finished D` (hidden with nothing to clear); ⋯ holds `Cancel all X`, danger, behind the existing confirm (hidden with nothing cancellable) | header row 1, right | the panel-wide verbs sit on the panel, the row verbs on the row | [A19] |
| Filter | a `SegmentedControl`: `All / Running / Failed / Done`, each with its count (a segment says `0` rather than vanish); `f` still cycles it | header row 2 | a filter you can see and click, not a hidden cycle | `Job.status` |
| Status glyph | `clock` queued · `loader-circle` running · `circle-stop` cancelling · `circle-slash` cancelled · `circle-check` done · `circle-x` failed | col 0 | — | `Job.status` |
| Sentence | the job as a sentence: verb phrase + the real domain id in mono (`Clone acme/infra`, `Create acme/api#injected-0`, `Run hooks for acme/api#broken`). The daemon's title is used when it ends in the job's target; otherwise the kind's verb phrase (`Clone`, `Prepare copies for`, `Run hooks for`, `Inspect worktrees`, …) | col 1, flex | reads as what is happening; the mono id still matches the row it is about (swarm's `hot-copy:<repo>` matched nothing) | `Job.kind`, `.target`, `.title` |
| Time | running: `m:ss` after 30 s; finished: `2s · 1m ago` (duration · age); cancelled: the age | right | — | `Job` timestamps |
| Progress | running only: a bar with its percent when the output states one (no bar otherwise), then the last stdout line, muted mono, tail-truncated | under the sentence | the single most reassuring artifact for a long job, and the only way to see a stuck clone | last progress line |
| Cancel | `Cancel c`, on a cancellable live row, shown while the row is hovered or selected | right | the button sits on the row it acts on | `Job.cancellable` |
| Inline error | failed only: the error's first line in the danger wash, the last output line under it | under the sentence | the failure is read without opening the log | `JobStatus::Failed.error` |
| Failure actions | `Retry R` (primary, when retryable) · `Show log ⏎` · `Copy log path y` | under the error | every key lives on the button it triggers | `Job.retryable`, `log_path` |
| Finished group | succeeded and cancelled jobs, one quiet line each (sentence in `text_secondary`), under a `Finished` label when live or failed rows sit above them | bottom of the list | finished work stops competing for the eye | `Job.status` |
| Footer | `Jobs run in fleetd and survive closing this window.`, muted, one line | bottom | the product promise, stated once, where it is being demonstrated | — |

The list is ordered live and failed jobs first, then the finished group, each in the daemon's
order; the cursor, `j`/`k` and `jobs.row[N]` all index that one order. A press on a row selects
it, a double-click opens its log (`Enter`), a right click opens its menu — Show log, Retry,
Cancel, Copy log path, each only when it can work. A row's button first puts the cursor on its
row, then runs the same action as the key.

**Log view.** `Enter` (or Show log, or a double-click) expands the sheet to 640 px and shows the
tail of `logs/jobs/<id>.log` (last 200 lines, mono 11.5 px, 16 ms batched — the existing budget
maps 1:1). The header becomes the log's toolbar: `← Back esc`, the job's sentence, a `Following f`
toggle and `Jump to end G`; under it the log path, verbatim, with `Copy log path y` — the path is
what makes a failure survivable **outside** the app. `j`/`k` scroll; `G` re-enables follow; `Esc`
collapses back to 440 px.

**Retention. [D-9]** Failed jobs are **never** auto-dismissed: they stay until `D`, and hold the
red jobs chip and the status-bar sticky error slot until the panel has been opened. Succeeded and
cancelled jobs fold into the quiet one-line "Finished" group as soon as they end and are kept for
`jobs.keepFinishedFor` (Settings, default **10 min**, `0` = forever). Evidence of what ran never
decays on a timer the user did not set.

**Intentionally omitted:** job ids in the row (they are in the log path and on `y`), queue
position, concurrency limits, per-job PID, absolute start timestamps, progress bars for
non-percent jobs, pool "skip-if-fresh" no-ops (logged, not listed), a key legend (every key is on
the button it triggers).

**States:** *empty* → centered faint `Nothing running.` (the footer already states the promise);
a filtered segment with nothing in it says `No <filter> jobs.` *daemon down* → amber strip at
the top: `The daemon is unreachable — job state is from <age> ago`, rows still readable.
*cancel all armed* → amber strip `Cancel n cancellable jobs?` with `Keep` and `Cancel all X`.

**Icons:** `activity` (panel title), `loader-circle`, `circle-check`, `circle-x`, `circle-slash`,
`circle-stop`, `clock`, `cloud-download` (clone), `copy-plus` (pool), `terminal` (hooks),
`scissors` (prune), `trash-2` (delete), `refresh-cw` (fetch/prs), `search-check` (inspect),
`arrow-up-circle` (update), `import` (import).

**Keyboard:** `J` toggle (also `ctrl-s J` from a terminal) · `j`/`k` · `gg`/`G` · `Enter` expand
log · `c` cancel (only when `cancellable`) · `X` cancel every cancellable job (confirm) ·
`R` retry a failed job with identical parameters · `y` copy the log path · `D` dismiss finished
and failed · `f` cycle filter all → running → failed → done · `Esc`/`J` close and **restore the
exact prior focus** (pane, row, terminal and mode).

The title-bar jobs chip is to show as pressed while the sheet is open once the title bar lands
(CHO-9); today's context-bar chip is unchanged.

---

### 3.8 Dialogs — shared frame

Every dialog: centered, `bg.raised`, 12 px radius, 1 px `border`, shadow `0 16px 48px rgba(0,0,0,.45)`,
backdrop = the base screen at 45 % opacity with an 8 px blur ("ghosts base", §5). **Header 44 px**:
icon + title, then a close ✕ at the right. **Footer 44 px**: left = contextual key hints in
`fg.faint` mono 11 px, right = the primary action label (`⏎ Create`). The ✕ and a click on the
scrim outside the card both dispatch the action `Esc` runs in that dialog (`dialog::Cancel`, or the
dialog's own `Reject` / `Close`), so a dialog whose `Esc` steps back in stages steps the same way
under the pointer (ADR 0023). The kit frame also draws a right-aligned button footer — `Cancel`,
then the one primary, each with its key chip — and each dialog moves its footer onto it when that
dialog is rebuilt; until then its footer is the hint row and label described here. Create worktree
and every Confirm are on the button footer. A Confirm draws the kit's **alert** header instead of
the 44 px bar: its icon in a tinted tile, the title question beside it and the target under the
title, with no ✕ — `Cancel` and a click outside close it. Clone repo, New / Edit context, Assign
repo, both Quit dialogs, Rename terminal, Repository hooks, and the board's New card, Card property
and Board settings are on the button footer too: no legend, `Cancel` running what `Esc` runs, the
primary running what its key runs, and every other verb a button of its own with its key chip.
Their lists and controls answer a click as the keys would.

| Dialog | Width × height | Icon |
| --- | --- | --- |
| Create worktree | 560 × auto (≈ 380) | `git-branch-plus` |
| Clone repo | 560 × 420 | `cloud-download` |
| Confirm — compact | 480 × auto (≈ 160) | `trash-2` / `scissors` / `power` / `x` |
| Confirm — expanded | 560 × auto (≈ 300) | `triangle-alert` |
| Confirm — prune | 720 × auto (≈ 420) | `scissors` |
| New / Edit context | 460 × 260 | `boxes` |
| Assign repo to context | 460 × 340 | `arrow-right-left` |
| Settings | 760 × 600 | `settings-2` |
| Help | 1040 × 720, the whole window less the scrim margin when narrower | — (the search field takes the title's place) |
| Quit (`ctrl-q`) | 520 × auto | `circle-question` |
| Quit + stop daemon | 560 × auto | `power` |

Every dialog field is a live `TextInput`, and editing is the whole `FleetTextInput` table of
KEYMAP — printable keys, `Backspace`, `ctrl-w`, `ctrl-u`, `ctrl-a`/`ctrl-e`, motion, selection
and undo. Lists **under a text field** use `ctrl-n`/`ctrl-p` or `↓`/`↑` and never `j`/`k`.
**A dialog row with no editor open (Assign, the Settings list while browsing, Confirm) does bind
`j`/`k`.**

---

#### 3.8.1 Create worktree (`n` in the worktrees pane)

**Purpose:** *Name a branch, pick a base, go — with the latency and the hooks visible before I commit.*

```
┌──────────────────────────────────────────────────────────┐
│ ⑂+ New worktree  buk/payroll                          ✕  │ 44
├──────────────────────────────────────────────────────────┤
│ BRANCH                                                   │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ feat/rut-validator                                  ▏│ │ 36
│ └──────────────────────────────────────────────────────┘ │
│ Creates buk/payroll#feat-rut-validator                   │ 18  faint sentence
│                                                          │
│ START FROM                                               │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ ⌕ Matching "feat/rut-validator"      ⟳ fetching      │ │ 30
│ ├──────────────────────────────────────────────────────┤ │
│ │▌origin/main                          [default]    ✓  │ │ 30 × 6, then scrolls
│ │ origin/release-2026                                  │ │
│ │ pull/412/head                   [previous base]      │ │
│ └──────────────────────────────────────────────────────┘ │
│                                                          │
│ Run on                              [ local │ devbox ]   │ 30  only if hosts configured
│ devbox unavailable — ssh: connect timed out              │     only if a host is blocked
│                                                          │
│ ┌ ⚡ Prepared copy ready — about 2 s ──────────────────┐ │
│ │    Hooks: pnpm install · pnpm build (run in background)│ │
│ └──────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────┤
│ ☑ Open after creating           [Cancel esc] [Create ⏎]  │ 44
└──────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `New worktree` + the repo id as the muted subtitle | header | the repo is already chosen; restating it prevents the #1 mistake | §3 create |
| Branch input | free text, `validateBranch` live, auto-focused | field 1 | the only always-typed input | §1 branch rules |
| Status sentence | `Creates <repoId>#<slugify(branch)>` faint | under the input | shows the id **and** the directory you will get, without a second field | §1 slug rules |
| Validation | red line replacing the sentence with the exact failing rule, e.g. `branch cannot contain ".."` | same slot | zero layout shift; fail before a job starts | §1 `validateBranch` reject list |
| Start from | one box: a top row saying what narrows the list (`Matching "…"`, `Type a branch name to narrow the list`, or `Nothing matches "…" — every ref is listed`) with the fetch state at its right, then the refs: `origin/<defaultBranch>` first and preselected, then the previous `baseRef`, then `origin/*` fuzzy-filtered by the typed branch, at most 50, 6 visible and the rest scrolling; free text accepted | field 2 | the branch is the filter, so the tab cycle stays branch, base, host | §5 Create dialog |
| `default` / `previous base` | neutral badges on those rows; an accent `✓` ends the chosen row | — | one word instead of a "use default" control | — |
| Row click | chooses that base and gives the list the keyboard, as `↓`/`↑` walking there would | — | the mouse is added, the keys stay | ADR 0023 |
| Fetch state | `loader-circle` + `fetching`; the exact failure in red with a `Retry` button (whose chip is `⏎` while `⏎` retries); else `N refs` | the box's top row | the list may grow under you; say so, and never block `Enter` | §5 "fetching indicator" |
| Run on | the hosts side by side with the chosen one raised (a dropdown past four hosts, or names too long to sit side by side), `←`/`→` cycle it, a click chooses; hidden when `config.hosts` is empty. A host that cannot take a create is dimmed and takes no click (the keys still reach it) | field 3 | zero-suppressed for the local-only majority | `defaultHost` |
| Host note | `<host> unavailable — <daemon's reason>` for each blocked host, the chosen one first, in amber with `cloud-off`; else the chosen host's provider (`this machine` for local) | under the host row | says why `Enter` is refused on a blocked host, in the daemon's words | — |
| Expectation callout | green `zap` `Prepared copy ready — about 2 s` **or** amber `hourglass` `No prepared copy — the first create copies the repo (~40 s) in the background` | above the footer | the pool's only user-visible consequence is latency; saying it decides whether the user waits or switches away | §1 prepared-copy slots; §6 |
| **Hooks preview** | the callout's second line: `Hooks: <prepare · joined> · <postCreate · joined> (run in background)`; `Hooks: none` when both are empty | in the callout | post-create hooks run detached and can fail *after* the worktree looks ready; naming them here is what makes the later `⚠ hooks failed` chip intelligible | §3 create step 6; §9 |
| Open after creating | checkbox, checked each time the dialog opens; unchecked, `Enter` and Create create **without** opening, as `⌥Enter` always does; its tooltip names `⌥⏎` | footer left | the hidden `⌥Enter` becomes visible where the choice is made | KEYMAP A8 |
| Buttons | `Cancel esc`, then the primary `Create ⏎` (`Open ⏎` on a duplicate id), disabled while nothing could be created | footer right | ADR 0023 | — |

**Intentionally omitted:** slug as an editable field (derived; the palette command
`create with custom slug` covers the rare case), `--url` / `--default-branch` / `--hooks`
(CLI-only), a "run post-create hooks" checkbox (always on), a "wait for hooks" checkbox, a
"fetch base first" checkbox (the daemon's freshness rules decide), a progress bar (the dialog
closes on `Enter`), a repo selector (the rail selection is the repo; from `All` it is the repo of
the highlighted worktree, falling back to a repo picker only when the list is empty), a separate
filter field for the base list (the branch already filters it, and a second editor would change
the tab cycle), and a split Create button (the checkbox carries "create without opening").

**States:** *submitting* → the dialog closes in < 16 ms, a pending `⟳` row appears in the list and
a `create` job appears in the ticker and the Jobs panel. *duplicate id* →
`buk/payroll#feat-rut-validator already exists — Open it ⏎` and the primary reads `Open`
(turning §3's idempotency rule into a shortcut). *conflict* (existing id with a different
explicit `--branch`/`--host`) → the exact conflict message in red in the footer, dialog stays
open. *closed while a base fetch is running* → the fetch **keeps running** and a 3.2 s toast says
`Base fetch still running` with a `View J` button (retires §9 "create-dialog close does not cancel forced fetch").

**Icons:** `git-branch-plus`, `search`, `loader-circle`, `check`, `zap`, `hourglass`, `cloud-off`.

**Keyboard:** `Tab`/`S-Tab` fields · `ctrl-n`/`ctrl-p` or `↓`/`↑` base list · `←`/`→` host ·
`Enter` create (and open, while the box is checked) · `⌥Enter` create without opening (KEYMAP A8)
· `Esc` cancel (jobs keep running).

---

#### 3.8.2 Clone repo (`n` in the repos pane)

**Purpose:** *Find a GitHub repo by typing a few letters and get it cloning.*

```
┌──────────────────────────────────────────────────────────┐
│ ⤓ Clone repo · into context "buk"                        │ 44
├──────────────────────────────────────────────────────────┤
│ ┌──────────────────────────────────────────────────────┐ │
│ │ ⌕ payroll                                           ▏│ │ 36
│ └──────────────────────────────────────────────────────┘ │
│▌🔒 bukhr/payroll                                     2d  │ 34 × 8
│    Nómina y remuneraciones                               │
│ 🔒 bukhr/payroll-legacy                              1y  │
│    Archived import pipeline                              │
│ 🌐 acme/payrolls                                     3w  │
├──────────────────────────────────────────────────────────┤
│ Clones over ssh in the background    [Cancel esc] [Clone ⏎] │
└──────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| Target context | `into context "<name>"` in the header | header | the destination is otherwise invisible and cloning into the wrong context costs a later `m` | `Repo.contextId` |
| Search input | 150 ms debounce, auto-focused | top | search is the whole dialog | §5 Clone |
| Results | **8** rows max, 2 lines: `lock`/`globe` + `fullName` + relative `updatedAt`; description `fg.muted` 11 px | list | 8 is swarm's cap; `updatedAt` disambiguates forks and dead mirrors | §5, `RemoteRepo` |
| Empty description | row collapses to a 22 px single line | — | zero-suppression (`description` null → `""`) | §1 |
| Protocol + background note | `Clones over ssh in the background` | footer, left of the buttons | tells the user `Enter` frees them immediately | `github.cloneProtocol`; §6 "survives popup" |
| Buttons | `Cancel` · `Clone` (primary, `⏎`) | footer | a click on a result row is `⏎` on that row, so the list is the whole choice | ADR 0023 |

**Intentionally omitted:** stars / forks / language, a clone-protocol picker (Settings), the full
SSH URL, avatars, a manual URL field (an `owner/name` or a pasted URL typed into the query is
detected and offered as the first row), a destination-path field (`reposDir` by contract).

**States:** *idle* → faint `Type to search GitHub repos in buk's owners.` *searching* →
`loader-circle` replaces the magnifier in place, previous results kept. *no results* →
`Nothing matches "<query>".` *gh failure* → a red one-line row with the gh error and a `Retry ⏎`
button (`⏎` retries a failed search);
the input stays live. *offline* → the last cache is shown with `cached <age>` in amber.
*submitted* → the dialog closes instantly and a `⟳` row appears in the rail from the moment the
`CloneJob` is persisted, before the child process starts (§6).

**Keyboard:** type · `ctrl-n`/`ctrl-p` or `↓`/`↑` · `Enter` clone · `Esc` cancel (aborts only the
search request, never a started clone).

---

#### 3.8.3 Confirm — delete / prune / kill / close terminal

**Purpose:** *Show me exactly what I will lose, in facts, with their age.*

Two sizes, chosen by the facts. This is §1.7 made literal. Both are alerts: the icon in a tinted
tile, the title as a question, the target under it, and a footer of two buttons.

**Compact** — every decisive fact is known **and** benign:

```
┌────────────────────────────────────────────────────────┐
│ [🗑] Delete worktree fix-rut-validator?                │
│      buk/payroll · ~/worktrees/buk/payroll/fix-rut-…   │
│      ✓ clean   ✓ merged into origin/main   ✓ no session│
│      Checked 8s ago · Re-check I                       │
│      Moves the copy to trash, then removes it in the   │
│      background.                                       │
├────────────────────────────────────────────────────────┤
│                          [Cancel esc]  [Delete y]      │  primary
└────────────────────────────────────────────────────────┘
```

**Expanded** — any risk fact is true, **or** any decisive fact is unknown:

```
┌───────────────────────────────────────────────── 560 px ─┐
│ [⚠] Delete worktree feat-payroll-fix?                     │  amber tile
│     buk/payroll · ~/worktrees/buk/payroll/feat-payroll-fix│
│     ⚠ **12 uncommitted files**                            │
│     ⚠ **3 commits** not on origin/main                    │
│     ⚠ session attached · claude, :3000 running            │
│     ⚠ unique commit count unavailable (gh unavailable)    │
│     ✓ PR #412 open (not merged)                           │
│     Checked 3m ago · Re-check I                           │
│     The session is killed and the copy moves to the trash.│
│     The 3 unpushed commits and 12 uncommitted files exist │
│     only here and will be lost.                           │
├───────────────────────────────────────────────────────────┤
│                     [Cancel esc]  [Delete anyway ⇧Y]      │  red: `Y` required
└───────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here |
| --- | --- | --- | --- |
| Header | icon tile (neutral compact, amber expanded) · the title question · the target as the subtitle: a worktree's repo and its path on disk, otherwise the full id when the title does not already carry it | top | deleting the wrong copy is the top failure mode |
| Fact list | one line each — dirty (with file count), unique commits, published, merged/PR, session + running labels | body, amber `⚠` risks first with their numbers in **bold**, green `✓` safe after | ordered so the eye lands on the reason to stop |
| Unknown facts | `⚠ <exact inspect warning string>` | with the facts | §1.3, and it explains why the key is `Y` |
| **Freshness** | `Checked <age> ago · Re-check I` — `Re-check` is a link button showing its key | above the consequence line | **mandatory on every facts confirm** — the confirm is the only place where freshness decides an outcome |
| Consequence | plain future tense, naming exactly what is lost (`The 2 unpushed commits exist only here and will be lost.`) | above the buttons | users confirm the *sentence*, not the title |
| Buttons | `Cancel esc`, then the action: a primary `Delete y` where `y` confirms (and `Enter` does too), a **red** `Delete anyway ⇧Y` where `Y` is required (a cascade names itself: `Delete repository`); disabled while the facts are still loading | footer | how dangerous the action is shows in the button's colour, not in letter case (KEYMAP confirm convention) |

**Escalation rule. [D-10]** `y` confirms when every decisive fact (`dirty`, `uniqueCommits`,
`published`, `session`) is **known**. `Y` (shift) is required when any is unknown or the
inspection errored — KEYMAP already states "lowercase = safe action, uppercase = stronger
variant". `Enter` is accepted wherever `y` is; `Enter` is **not** accepted where `Y` is required.

**Exact wording per action**

| Action | Title | Consequence line | Key |
| --- | --- | --- | --- |
| Delete worktree, clean | `Delete worktree fix-rut-validator?` (both forms; the id and path are the subtitle) | `Moves the copy to trash, then removes it in the background.` | `y` |
| Delete worktree, risky | `Delete worktree feat-payroll-fix?` | `The session is killed and the copy moves to the trash.` (`The copy moves to the trash.` with no session), then `The <n> unpushed commits and <m> uncommitted files exist only here and will be lost.` naming what applies, or `Commits that exist only here would be lost.` when the count is unknown | `y` / `Y` |
| Delete repo | `Delete repository buk/payroll?` | `Also deletes 8 worktrees and their sessions. The base clone and every copy go to trash.` | `Y` (`Delete repository`) |
| Delete context | `Delete context "buk"?` | `Also deletes 4 repositories, 23 worktrees and every session in them.` | `Y` (`Delete context`) |
| Prune | `Prune buk/payroll — 3 of 8 worktrees` | `Deletes the 3 listed below. The 5 skipped ones are kept, with the reason shown.` | `y` |
| Kill session | `Kill session payroll/feat-payroll-fix?` | `Kills 3 terminals at once. nvim has unsaved changes; claude and the server on :3000 are killed too. Nothing is saved.` | `Y` when unsaved or keep-alive present, else `y` |
| Close terminal | `Close terminal 2 "cc"?` | `claude is running in it and will be killed.` | `y` |
| Sleep | *no confirm* | toast `Slept · kept cc (claude)` | — |

**Prune dialog** — a multi-target confirm, so it gets its own body on the same alert frame:

```
┌──────────────────────────────────────────────────────────── 720 px ─┐
│ [✂] Prune buk/payroll — 3 of 8                                       │  amber tile
│     DELETE                                             3 worktrees   │
│     ✓ fix-rut-validator                                              │
│     ✓ chore-deps                                                     │
│     ✓ spike-cache                                                    │
│     KEEP                                               5 worktrees   │
│     ⚠ feat-payroll-fix     2 unique commits, not merged              │
│     ⚠ api-poc              session attached                          │
│     ⚠ old-spike            session has running commands: claude, :3000│
│     ⚠ devbox/ledger-sync   status unknown (host offline)             │
│     ⚠ legacy-import        unique commit count unavailable           │
│     dry run · fetched 6s ago · Re-check I                            │
│     Deletes the ones listed below. The skipped ones are kept, …      │
├──────────────────────────────────────────────────────────────────────┤
│ [Hide kept s]                           [Cancel esc]  [Prune 3 y]    │
└──────────────────────────────────────────────────────────────────────┘
```

`Show kept s` / `Hide kept s` is a toggle button for the KEEP section (the `s` key), shown when
anything was skipped. The action button is disabled while the dry run is still running or a
re-check is required, and red (`Prune 3 anyway ⇧Y`) when `Y` is.

The KEEP reasons are the **verbatim** skip reasons of §3 prune (`error`, `dirty`,
`attached/unknown session`, `unknown required unique count`, `unmerged`, running labels —
swarm's `tmux session has running commands: …` becomes `session has running commands: …`, the
only rewording, because there is no tmux). The `? host offline` row is mandatory: it is exactly
the case where an automated prune must be **seen** to have refused. The footer stamp
`dry run · fetched <age>` is mandatory.

The DELETE list is also the commit authority. The dry run uses ordinary repo-scoped discovery;
confirm sends the exact displayed DELETE IDs as the additive IPC-v4 `PruneWorktrees.ids`
allowlist. The daemon locks and re-inspects only those worktrees immediately before deletion. It
may move newly unsafe entries to KEEP, but a worktree that was not reviewed can never enter the
commit set.

**Intentionally omitted from every confirm:** a "don't ask again" checkbox (the *compact* form is
the real answer to confirm fatigue), a second "are you sure" step, a countdown or a disabled
button delay, a typed-name confirmation (typing trains people to type), diff previews. The
worktree path is shown once, as the delete confirm's subtitle, because it is what tells two
same-named copies apart.

**States:** *facts loading* → `checking…` above the facts already known, and the action button is
disabled (red, `Y`) until the inspection answers — no key confirms before then;
a background `inspect --no-fetch` swaps values in place, with no highlight or flash
(DESIGN-SYSTEM §2.7). *facts errored* → amber line with the exact warning text and `Y`.
*prune dry-run running* → the body shows
`⟳ checking 8 worktrees…` and the action button is disabled until facts exist (it is a button
now, and a button that does nothing when clicked must say so). *nothing eligible to prune* → the dialog does **not** open; a 3.2 s toast says
`Nothing to prune in payroll — 11 skipped`, whose `View J` button opens the Jobs panel with the reasons.

**Keyboard:** `y`/`Y`/`Enter` confirm · `n`/`Esc`/`q` cancel · `I` re-check (delete) · `s` toggle
the KEEP list (prune). Nothing else is bound, so muscle memory cannot misfire. On cancel the exact
prior cursor is restored.

---

#### 3.8.4 New / Edit context

```
┌────────────────────────────────────────────┐
│ ⬚ New context                              │
├────────────────────────────────────────────┤
│ Name    ┌────────────────────────────────┐ │
│         │ Buk HR                        ▏│ │
│         └────────────────────────────────┘ │
│         → buk-hr                            │ faint id preview
│ Owners  ┌────────────────────────────────┐ │
│         │ bukhr, dannyfuf                 │ │
│         └────────────────────────────────┘ │
│         GitHub orgs/users used to scope PRs │ faint, one line
├────────────────────────────────────────────┤
│                   [Cancel esc] [Create ⏎]  │
└────────────────────────────────────────────┘
```

`Name` → live `ContextId` preview under the field (§1 slugify rules). `Owners` is comma-separated;
its one-line explainer is the **only** teaching copy in the app, because `owners` is the single
field whose purpose is not guessable.

**Edit variant** (`E`, KEYMAP A15): title `⬚ Edit context "buk"`, the id shown read-only and faint
(read-only outright once repos exist), and **context delete lives here** as `ctrl-shift-d` → the
expanded confirm; the footer shows it as a red ghost `Delete context ⌃⇧D` button on the left, and
the primary reads `Save`. **[D-11]** `D` therefore keeps its KEYMAP-defined meaning as *delete active
context* but is **routed through the same expanded confirm with `Y`**; `E` + `ctrl-shift-d` is the
discoverable path. Rationale for not unbinding `D`: KEYMAP is authoritative and a spec must not
silently retire a documented binding; the risk is handled by `Y` escalation, the fact list and
the trash undo (`u`), not by hiding the key.

**States:** duplicate id → `A context with id "buk-hr" already exists.` inline under the name, and
`Enter` inert (the primary is drawn disabled until the name can be saved). Empty
owners → allowed; the footer warns `Without owners, GitHub repo search and PR "mine" are empty.`

**Omitted:** `createdAt`, the repo list, a color/emoji picker, a description, context reordering
(rail order = creation order = the `1`–`9` mapping the user memorised).

---

#### 3.8.5 Assign repo to context (`m`)

```
┌────────────────────────────────────────────┐
│ ⇄ Move buk/payroll                         │
├────────────────────────────────────────────┤
│▌buk         bukhr                  current │
│ personal    dannyfuf                       │
│ oss         zed-industries, ghostty-org    │
├────────────────────────────────────────────┤
│ Moves the repo record only — nothing on    │
│ disk changes, sessions keep running.       │
│                     [Cancel esc] [Move ⏎]  │
└────────────────────────────────────────────┘
```

Rows: `Context.name` (`fg`) + `owners` joined (`fg.muted`, truncate) + a `current` badge. A click
selects a row and a double-click moves the repo there (`⏎`); `Move` is disabled on the current
context, where it would change nothing.
Owners are shown here specifically because they are the reason a repo belongs to a context.

**[D-12]** This dialog has **no text field**, so KEYMAP's "never `j`/`k` under a text field" rule
does not apply: `j`/`k`, `↓`/`↑` and `ctrl-n`/`ctrl-p` all move the selection, exactly as
inventory §5 records for swarm's Assign dialog.

The footer sentence is mandatory: the action *sounds* destructive and is not.
**Omitted:** repo counts, ids, created dates.

---

#### 3.8.6 Settings (`,`)

760 × 600: a header, a 196 px section rail beside a pane of controls, and a button footer.
**Board settings** (§Board, "The other three dialogs") is the older shape, 720 × 560 with a 180 px
rail, and borrows this section's rules for browsing, editing and cycling a row; where the two
differ is stated there — its three sections are the board's own, it remembers the section it was
left on for the app session, and `^s` rather than `⏎` is its save, because `⏎` inside its Columns
pane has a level to drill into.

**Header.** `⚙ Settings`, then a **Search settings** field with its `/` chip, then the close ✕
(`Esc`'s action, ADR 0023).

**Rail.** One row per section, its glyph and its name; the shown section is selected. A click
opens it, as `Tab` / `S-Tab` step through them.

**Pane.** One row per setting, drawn with the real control its kind calls for, each clickable and
each keeping its key:

| Kind | Control | Key | Pointer |
| --- | --- | --- | --- |
| On / off | a switch at the row's end | `Space` | a click on the switch |
| Closed choice | a segmented control when its options are short (≤ 4, ≤ 32 characters), otherwise a dropdown | `h`/`l`, `←`/`→` | a segment, or an option of the dropdown's list |
| Number | a box at the row's end, the unit after the number | `Enter` opens it | a click on the row |
| Text | a box filling the row after its label; an empty value reads as what it means (`Harness default`) | `Enter` opens it | a click on the box or the row |
| Read-only | a plain `label  value` line, with a copy button where a value is worth pasting (`FLEET_HOME`, the fleetd pid) | — | the copy button |

A click on a row puts the cursor on it first, so the keyboard carries on from where the pointer
left it. A value is in a live editor **only while it is being edited**: `Enter` (or a click on a
text or number box) opens the editor inside the same box, so the row never moves. A helper
sentence sits under the row in the caption face — plain words, e.g. *Typed into a terminal tab.
Aliases work.* Rows that belong together sit in a titled card (`Claude`, `Codex`, the keep-alive
rules, the terminal tabs). A section that cannot be edited here ends with a faint *Change these in
config.json.* once, not per row.

| Section | Rows |
| --- | --- |
| **General** | read-only, in a *Terminal tabs a new worktree opens* card: `1 nvim — nvim .` / `2 cc — {agent}` / `3 lg — fleet://lazygit (built in)` |
| **Agents** | `Default agent [ Claude │ Codex ]` (*Used by the Agent buttons and by a new thread.*), then one card per harness: `Terminal command [claude]` (*Typed into a terminal tab. Aliases work.*) · `Binary for threads [claude]` (*Run directly for an agent thread, without a shell.*) · `Default access [full access ⌄]` (that harness's supported modes only; *New threads start with it. Each thread can change it.*) · `Default model [Harness default]` · `Effort [ Default │ Low │ Medium │ High ]` (*Used with the default model.*). Effort offers `low`/`medium`/`high` plus whatever that harness has declared on a thread this app opened; a configured value outside them is shown as it is until the row is moved. Both access defaults start at `full access`; new-thread requests omit mode/model and the daemon resolves them. No health chip: doctor does not check the agent binaries, and the dialog does not invent a check. |
| **Sleep** | `Sleep on switch` switch · `Grace [2000 ms]` (clamped ≥ 0) · a *Keep awake while running* card with one switch per rule, `<label>  <kind>  <pattern>`, **plus a live match count** `claude — matching 2 processes now` · invalid regex → red `invalid pattern — rule is skipped` |
| **Jobs & warnings** | `Warn before quitting with running jobs` switch (*They keep running in fleetd either way.*) · `Keep finished jobs for [10 min ⌄]` · `Trash retention [10 min ⌄]` |
| **Pool** | `Hot pool size [ 0 │ 1 │ 2 │ 3 ]` · `Freshness [60000 ms]` · `Refresh interval [300000 ms]` · read-only `prepared copies: 1/1 ready` |
| **GitHub** | `Clone protocol [ ssh │ https ]` · `Repo cache [3600 s]` · `PR cache [90 s]` |
| **Status** | `Local status refresh [2000 ms]` (min 500) · `Remote status refresh [10000 ms]` (min 500) |
| **Hosts** | read-only per host `devbox — tailscale · node devbox — ready · fleetd 0.2.0` |
| **About** | `Fleet 0.1.0+<sha>` · `fleetd running · pid 4211 · up 3h` (copy: the pid) · `FLEET_HOME ~/.fleet` (copy: the path) · update row `Fleet 0.2.0 available · U` (§2.3) · `protocol 4` |

**Search.** `/` (or a click) puts the keyboard in the header's field. Typing replaces the pane with
every row, across all sections, whose section, card, label or helper sentence holds each typed
word — its label (led by its card, `Claude › Effort`), its helper sentence and its section's name.
`↓`/`↑` move over them, `Enter` or a click opens that row's section with the cursor on it and
clears the search, and `Esc` clears the search and gives the keys back to the rows. Nothing
matching says so.

**Footer.** Left: *Open config.json* (`E`) and *Run doctor* (`D`), each a button showing its key.
Right: *Unsaved changes* in amber while the draft differs from what was loaded, then *Cancel*
(`Esc`) and the primary *Save* (`⏎`), disabled until there is something to save; while the
configuration failed to load, the primary reads *Retry* and is enabled, because `⏎` retries.

**[D-13]** The editable set closes §9's *"Settings cannot edit grace/rule definitions/windows/
hosts/protocol/pool/timers/status intervals; many require JSON"* for everything a user changes
more than once a year. `windows` and `hosts` stay read-only in v1 because both are ordered/keyed
structures whose editor is a whole screen; `E` (open `config.json` in a terminal tab) is the
escape hatch and is one key. The **`Warn before quitting with running jobs`** switch is the
mechanism that makes KEYMAP's `ctrl-q` clause implementable at all (§3.8.9).

**States:** saving is synchronous and silent (never a toast, §2.7); a failed write shows a red
footer line with the exact error and keeps the dialog open. While the draft is dirty the footer
says *Unsaved changes*, Save is enabled, and the amber strip above the buttons says *Esc or Cancel
discards the unsaved changes* — `Esc` discards, as it always has, and the strip says so before it
happens. An error takes that strip's place.

**Keyboard:** while browsing, `j`/`k`, `↓`/`↑`, `ctrl-n`/`ctrl-p` move · `Tab`/`S-Tab` change
section · `Space` toggles · `h`/`l` and `←`/`→` cycle a choice · `Enter` **opens** the focused text
or number row for editing, and saves on every other row · `/` searches · `E` config.json · `D`
doctor · `Esc` discards.

Landing on a row deliberately does not open it: a row that grabbed the keyboard on arrival would
make the next `j` type into the value instead of moving on. `Enter` is the gesture that opens it,
and the row then materializes a live `TextInput` inside its own box — a number row filters to ASCII
digits, so `j` can never become part of a number. While that editor owns the keyboard every
printable key types, `Enter` saves, and `↓`/`↑` or `ctrl-n`/`ctrl-p` move to the next row and
close it.

---

#### 3.8.7 Help (`?`)

**Purpose:** *What can I do here, and how do I get this task done?* Help is a guide, not a key
dump, and everything in it does what it describes when clicked.

1040 × 720; on a window too small for that it takes the whole window less the scrim's margin.
The header is a focused **search field** ("What do you want to do?  Try “agent”, “delete”,
“copy”") in place of the title, then the **Guides | All shortcuts** switch and the close ✕.
The footer reads `Fleet <version> · fleetd up 3h · protocol 8` and ends in a **Run doctor**
button, which closes Help and raises the doctor report.

**Guides tab.**

* Left, **Here in <surface>**: the six most useful actions of the surface Help was opened over
  (the Worktrees list, the board, a terminal, an agent thread…), each with its key. They come
  from the action catalogue's `rank` and from the key table resolved against that surface's
  context chain, deeper bindings shadowing shallower ones, so a key the surface does not reach
  is never offered. Under it, **Guides**: the seven task guides.
* Right, the selected guide: a title, a sentence of context, then one card per step — a number,
  what to do, what happens — with a button per action the step needs, each showing its key.
* The guides, in `dialogs/help/guides.rs`: *Start work on a task* (card → worktree → open →
  agent → back to the hub), *Work with an agent* (start a thread, `@` files, `$` skills, `/`
  commands, requests, plan mode, steering, stopping, reviewing changes), *Review a pull request*,
  *Plan on the board*, *Automate a board column* (board settings → Columns → on enter →
  provider, instructions, expect → on success; worktree boards only), *Terminals: copy, paste,
  scroll* and *What keeps running*. Every sentence describes what the code does; a guide that
  drifts from it is a bug in the guide.
* *Start work*, *Work with an agent*, *Review a pull request* and *Terminals* end with the one
  callout that confuses everyone: **inside a terminal, Fleet's keys start with `ctrl-s`**;
  everything else goes to the program; holding `ctrl-s` a moment shows every Fleet command
  (the ⌃S menu, KEYMAP), and `ctrl-s` twice sends it to the program.

**What keeps running** is the old paragraph, rewritten as a guide: closing Fleet never stops
your work — terminals, agents and jobs run in fleetd and terminals even survive a fleetd restart
— and three things do stop it: cancelling a job in the Jobs panel, ending a session (`K`, asks
first), and quitting with fleetd (`ctrl-shift-q`, lists what dies and asks first). Sleep is
named as the gentler fourth: it closes idle terminals and keeps agents, servers and unsaved
editors, and opening another worktree sleeps the one left unless it is opened keeping it awake.

**Terminal clipboard**, as the *Terminals* guide states it: drag selects, double-click selects a
word, triple-click a line, and the text is copied **when the button is released**; the copy key
copies a selection and, with none, goes to the program; the paste key pastes, bracketed when the
program asks. `ctrl-c` and `ctrl-v` stay the program's.

**All shortcuts tab.**

* Left, **Where**: *All places*, then every catalogue place (Everywhere, Hub, Worktrees, … ,
  Editing text, fleetd), each with how many shortcuts the search matches there, and **you are
  here** on the surface Help was opened over. *All places* leaves out *Editing text*: the forty
  text-field editing keys are there when asked for and do not bury the rest. Under the list, a
  **Reading the keys** legend: `ctrl-s` then `a`, `g` then `b`, and a capital letter as Shift.
* Right, a table of **Action** (the catalogue label), **Where** (its place) and **Keys** (its key
  in that place, the prefix spelled out; a numbered range reads `1`–`9`). The row under the
  cursor, or the pointer, shows **Run ⏎** when its key works where Help was opened, and a click
  on such a row runs it. A range never runs from Help.
* Under the table, **Related guide** when the search matches a guide; clicking it opens the
  guide.

**Search.** Words match an action's label, its description, its place and its keys, the label
first. On the Guides tab the search lists matching guides first, then matching actions; the
right side shows the selected guide, or the selected action with its description, place, keys
and **Run** when it works here. The cursor starts on the best matching action that works here,
so `?`, a word and `⏎` does the thing: `ctrl-s ?`, "zoom", `⏎` zooms the terminal.

**Running.** A row or a step button closes Help and runs its action on the surface Help was
opened over — the same action its key dispatches, landing where the key would
(`APP-CONTRACTS.md`, the pending action). A step whose action does not work there keeps its
button, disabled, with its key and a tooltip saying where it works; a step may offer
alternatives (the board's `w` or the card's `w`) and runs the first one that works.

**Keys.** `?` opens (`ctrl-s ?` inside a terminal or an agent tab); `Esc` or `?` closes; typing
goes to the search field; `↓` / `↑` (and `ctrl-n` / `ctrl-p`) move in the list; `⏎` runs the
row, or opens a guide found by search; `ctrl-tab` / `ctrl-shift-tab` or a click switch the tab.

---

#### 3.8.8 Quit — `ctrl-q` (app only, daemon keeps running)

**[D-14] This resolves the direct contradiction between the two source proposals and KEYMAP.**
KEYMAP is authoritative: quitting is "`ctrl-q` (with a confirm only if a job is running **and**
the user asked to be warned)". Neither "never confirms" nor "always confirms" is implementable
against that sentence, so:

- `Settings › Jobs & warnings › Warn before quitting with running jobs` is the opt-in, **default
  on** for a fresh install.
- With the toggle **off**, or with **nothing running**, `ctrl-q` quits immediately, no dialog.
- With the toggle **on** and at least one running job, this dialog appears:

```
┌ Quit Fleet? ───────────────────────────────── 520 px ─┐
│ These keep running in fleetd:                         │
│   ⟳ clone  nixos                          40%         │
│   ⟳ hooks  buk/payroll#feat-rut   pnpm install        │
│   ◉ 3 sessions · 7 terminals                          │
│ They will be here when you come back.                 │
├───────────────────────────────────────────────────────┤
│ [Show jobs J] [Never warn W]  [Cancel esc] [Quit y]   │
└───────────────────────────────────────────────────────┘
```

`W` writes `warnBeforeQuit=false` and quits — a legitimate "don't ask again" precisely because
**nothing is lost** by quitting (the ban on that control in §3.8.3 applies to destructive
confirms only).

#### 3.8.9 Quit and stop the daemon — `ctrl-shift-q`

Always confirms when anything is running, and always enumerates. Tone inverts, because everything
listed dies:

```
┌ ⏻ Stop fleetd and quit? ───────────────────── 560 px ─┐
│ ⚠ Killed now:                                         │
│   ◉ payroll/feat-payroll-fix   nvim ✎ unsaved,        │
│                                claude, server :3000   │
│   ○ www/chore-deps             2 terminals            │
│   ● swarm-agent-claude                                │
│ ⚠ Cancelled now:                                      │
│   ⟳ clone nixos          40%   (restartable)          │
│   ⟳ hooks payroll#feat-rut     (retryable → restartable)│
│ 1 job keeps running: post-create hooks (detached)     │
│                                                       │
│ Worktrees, repos and state on disk are untouched.     │
│ [⌃Q] quits Fleet and leaves all of this running.      │
├───────────────────────────────────────────────────────┤
│                 [Cancel esc] [Stop and quit ⇧Y] (red) │
└───────────────────────────────────────────────────────┘
```

Cancellable vs. not comes from the job's cancel token; a detached post-create runner (§6 "no
cancel") is listed under a fourth group `n job(s) keep running`. Its restartability label is
derived from `JobRecord.retryable`, just like every other job, rather than inferred from its
detached execution. The `ctrl-q` line is the important one: the confirm teaches the safe
alternative instead of only threatening, with the key drawn as a chip from the live keymap. With nothing running, `ctrl-shift-q` does not confirm.

#### 3.8.10 Rename terminal and Repository hooks

**Rename terminal** (`ctrl-s ,`, 460 px, `file-pen`) is one `Name` field over `Cancel` and
`Rename ⏎`; while the request is in flight the primary reads `Renaming…` and is disabled, and a
refusal is the dialog's error line.

**Repository hooks** (`e`, 560 px, `file-pen`) is two lists, *Prepare* and *After a worktree is
created*, each a column of command fields ending in a blank one where the next command is typed
(typing into it grows the next). Every filled row has a remove ✕ at its end; the trailing blank has
none to offer and draws it disabled. *Add command* under each list puts the keyboard in that
blank row. The footer is `Cancel` and `Save ⏎`; `Tab` / `S-Tab` still walk every row of both lists
in order.

---

### 3.9 Command palette (`:`, ⌘K)

**Purpose:** *Jump to anything by name, or do the thing whose key I do not remember.*

Opened by `:`, by ⌘K on macOS or `ctrl-k` elsewhere, by `^s k` in the Workspace (KEYMAP.md §
Palette mode), and by clicking the title bar's command field. **640 px** wide, top-anchored at
**y = 120** (thinking position, not screen center). A click on the scrim outside the card closes
it through `palette::Close`, the action `Esc` runs.

```
┌──────────────────────────────────────────────────────────────┐
│ ⌕ del▏                                      [All]            │ 44
├──────────────────────────────────────────────────────────────┤
│ Commands                                                     │ 20
│▌[🗑] Delete worktree                    asks first      [d]  │ 34
│ [🗑] Delete card                        Board           [d]  │
│ [⬚] Delete context                      asks first    [⇧D]  │
│ Go to                                                        │
│ [◉] acme/api#model-registry             session attached     │
│ Cards                                                        │
│ [☑] FLT-7 Model delivery windows  Todo                       │
├──────────────────────────────────────────────────────────────┤
│ Delete worktree                                   Run [⏎]    │ 30
└──────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why |
| --- | --- | --- | --- |
| Query row | `search` icon, the query at 15 px, placeholder `Search or run a command`, the scope chip | top | the palette reads as a search field, the same one the title bar draws |
| Scope chip | `All`, or what a prefix narrowed to: `Commands` (`>`), `Worktrees` (`@`), `Cards` (`#`), `Agents` (`!`) | right of the query | a prefix is invisible once typed; the chip says what the list is |
| Prefix legend | `type > commands · @ worktrees · # cards`, only while the query is empty | right of the chip | teaches the prefixes to whoever looks, gone once typing starts |
| Results | **one ranked list** with section headings `Commands`, `Go to`, `Cards`, `Pull requests`, `Agents` | body | the best match is always the first row, whatever kind of thing it is, so `Enter` is predictable |
| Ranking | a prefix match beats a word-start match beats a run anywhere beats a scattered one; rows sort by score inside a section and sections by their best row | — | `del` puts "Delete worktree" above "Undo delete" above `model-registry` |
| Scrolling | `10` rows tall, the rest scrolls; no per-section cap while a query is typed (one overall bound of 200 rows) | — | every match is reachable; the cursor keeps its row in view |
| Row | icon tile · label with its matched characters in a heavier weight · status word · muted detail · the row's own key chip | — | the key chip teaches the direct key, so the palette trains itself out of the loop |
| Command detail | the catalogue place it acts in (`Board`, `Card`, `Worktrees`…); `Everywhere` commands carry none | `Commands` rows | one label per action across Help, the palette and every button; the place tells apart a board row and an open card's row that do the same thing |
| Destructive commands | a red icon tile, and `asks first` in place of the place; still routed through their confirm | — | the palette never bypasses a confirm, and says so before you press |
| `Go to` rows | sessions (most recent first) and worktrees with their §2.5 glyph and state (`session attached`, `sleeping`), repositories (`repo`), contexts (`context`, with their digit) | — | a session is reachable from inside another session: `^s k` `pay fix` `⏎` |
| Card rows | `FLT-7 <title>` and the card's column as a status word; `Enter` selects the card on its board and opens its detail | `Cards` | a card is an object you jump to, like a worktree |
| Pull request rows | `#412 <title>`, `<repo> · mine` / `review`; `Enter` opens its worktree when one is checked out, otherwise shows it selected on the PR screen | `Pull requests` | only the PRs the PR screen has already loaded — opening the palette fetches nothing |
| Job rows | `Cancel job: <kind> <target>`, `Show failed job: <title>` | `Commands` | a background action is reachable without learning the panel |
| Footer | the selected row's label, then `Run ⏎` | bottom | names what `Enter` will do. There is no `⌘⏎` "other targets" yet: no row has a second target |

**Prefixes.** `>` commands only, `@` worktrees and sessions (with repositories and contexts),
`#` cards, `!` agent threads. The prefix is read off the first character; the rest is the query.

**Pointer.** Rows hover; a click runs the row, exactly as `Enter` does with the cursor on it.

`^s d` opens the same palette with the query seeded to `!` and shows the single `Agents`
section (typing `agents <filter>` still means the same). Its rows have five fixed slots:

| Slot | Content |
| --- | --- |
| Key | `^s <n>` for the thread's strip index when attached — the key that selects its tab — otherwise none |
| Mark | the thread's attention glyph; a blocked gate reads `needs you` through the same amber vocabulary as its tab |
| Primary | `↳ <provider> — <title>` for a child, `<provider> — <title>` for a caller |
| Detail | `<word> · <age>`, except an open gate names `blocked · permission`, `blocked · question` or `blocked · plan` |
| Trailing verb | `go` for an attached thread or caller, `attach` for a hidden child |

The order is callers for the current worktree, their children in creation order, then those
callers' children on other worktrees. An other-worktree child appends ` · <worktree>` to its
primary label. `Enter` selects an attached thread, attaches and selects a hidden child, reopens
and selects a closed caller, or switches to the named worktree before attaching and selecting its
child; all four paths finish with the composer focused, and merely being blocked never attaches a
thread. Closing a selected caller removes it from this window's attached set, selects the
remaining terminal and restores `TERMINAL`; its picker row remains available with key `·`, verb
`go` and hidden state until `Enter` reopens it.

The strip still has the hard nine-tab ceiling. Attach what you look at, detach when done, and
reach the rest through `^s d`; delegation rows and the picker retain every hidden child.

**States:** empty query → `Recent`: the 5 most recently opened sessions, then `Suggested`:
the 6 highest-ranked commands valid where the palette was opened. No match →
`Nothing matches "<query>".` A command invalid in the current context is **not listed at all**
— never greyed, because a greyed row costs a `j`.

**Omitted:** section descriptions, command history, a shell escape, multi-select, a second
"other targets" run (`⌘⏎`) until a row has one.

---

### 3.10 Filter bar (`/`)

**Purpose:** *Narrow this list without moving it.*

The filter **replaces the pane header in place** — 30 px, same row, no overlay, no reflow:

```
 WORKTREES · payroll                          8/12      1–8/12    ← normal
 ⌕ rut                                        2/12         esc    ← filtering
 WORKTREES · payroll  ⌕rut                    2/12      1–2/2     ← filter retained (input exited)
```

| Element | Content | Position | Why |
| --- | --- | --- | --- |
| `search` icon | 14 px `fg.muted` | replaces the pane label | the filter bar *is* how Filter mode shows; there is no mode word (§2.8) |
| Query | live text, blue caret | inline | — |
| Match count | `<shown>/<total>` | right | tells you whether to keep typing |
| `esc` hint | faint, right of the count | right | the two-stage `Esc` is non-obvious |
| Clear ✕ | compact icon button at the end of the query, only while it holds text | inline | empties the query as `ctrl-u` would; the input keeps the keyboard, so `Esc` still leaves it first |
| Retained chip | `⌕rut` in accent inside the restored header, with a blue dot | same row | a hidden active filter is the classic "where did my rows go" bug |

**Keyboard:** the query is a live `TextInput`, so editing is the whole `FleetTextInput` table
(KEYMAP) — printable, `Backspace`, `ctrl-w`, `ctrl-u`, motion, selection, undo. `ctrl-n`/`↓` and
`ctrl-p`/`↑` move the list cursor **while still typing** · `Enter` opens the selected row (so
`/rut⏎` is a complete open in 5 keys) · first `Esc` leaves the input keeping the filter · second
`Esc` clears it. Typing narrows the list and returns the cursor to its top row.
**[D-15]** `Esc` in the Hub **never quits the app** — swarm's "clear filter, else quit" is
retired (KEYMAP A13).

**States:** no match → the list area shows `Nothing matches "<filter>".` + faint `esc clear` and
`Enter` is inert; the header keeps `0/12`. A filter survives a refresh; it does **not** survive a
repo change or a screen change.

---

### 3.11 Toasts

Rendering: bottom-right, above the status bar, **320 px** wide, 12 px insets, max **3** stacked,
**3.2 s** (1.6 s for instant-action acknowledgements). A toast appears and vanishes in place —
it does not slide or fade (DESIGN-SYSTEM §2.7). One line, one icon, no title, and a ✕ that takes
it down at once.

**Pointer (ADR 0023).** A toast that points somewhere — a background success the Jobs panel
holds, an agent thread that needs you — carries a small ghost `View` button with the key that goes
to the same place (`J` for Jobs; none for a thread, which `1 needs you` in the title bar also
opens); a click on the button or on the line goes there and retires the toast. The key is never
spelled into the text (`· J` is gone). **Hovering a toast holds its dwell**: it does not decay
while the pointer rests on it, and resumes with the time it had left when the pointer leaves.
Contents and the governing law: **§2.7**.

---

### 3.12 Daemon states, degraded states and Doctor

**[D-16]** Three *distinct* daemon situations. Conflating them is the failure mode, and only one
of them is about reconnecting.

| Situation | Surface | Exact text | Keys |
| --- | --- | --- | --- |
| **A. Cold start, daemon not yet up** | full window, centered, no chrome | `Starting fleetd…` + spinner; after 3 s it appends `~/.fleet/fleetd.sock` | none (auto-spawn) |
| **B. Daemon will not start** | full window, centered | `fleetd could not start.` · the last 3 lines of `~/.fleet/logs/fleetd.log` in mono · `The socket ~/.fleet/fleetd.sock is stale.` when that is the cause | buttons `Retry r` (primary) · `Open log L` · `Run doctor D` · `Quit ctrl-q`; a protocol mismatch drops `Retry` and leads with `Run doctor` |
| **C. Daemon died while attached** | 40 px amber banner under the title bar; the title bar's `Reconnecting…` pill is amber (red `fleetd down` only once it stopped); the daemon dot turns red; terminal grids get a 55 % veil | `Lost connection to fleetd — reconnecting in 3s. Your terminals and agents keep running.` (`fleetd stopped — …` after a shutdown) — the countdown cycles `3s → reconnecting… → 6s` (backoff 1, 2, 4, 8 s, capped 8 s) in its own slot | buttons `Reconnect now r` · `Open log l` · ✕ (`Esc`) dismisses the banner (the dot stays red) |

**On reconnect after C**, the banner turns amber for 6 s (not green, not 800 ms) and reads,
verbatim:

> `fleetd restarted. <n> terminals were reattached; worktrees, jobs and state are intact.`

or, when none did:

> `fleetd restarted. No terminals survived; worktrees, jobs and state are intact.`

**[D-17]** This sentence is mandatory, it is the one place the app states what a restart did to
the user's terminals, and the count is **read from the first snapshot, never assumed**. Terminals
normally survive: every PTY lives in a detached holder process (`ARCHITECTURE.md`, "Detached PTY
holders"). They do not always — `pkill fleetd` matches `fleetd pty-hold` too, and nothing survives
a reboot — and a banner that promised a reattach over an empty session list would be the false
reassurance this rule exists to prevent. The daemon's emulator state never survives either way, so
each grid is repainted from its holder's replay buffer and the first screen after a restart can be
shorter than the scrollback that preceded it. The banner still dwells the full six seconds: a
restart means the `fleetd` binary changed, which is worth noticing. If the daemon merely dropped
the *connection* without dying, the banner instead reads `reconnected` and leaves after 800 ms.

**While disconnected (case C):** the lists remain navigable — they are true, just frozen — the
Worktrees rows are drawn at 55 % (`stale_opacity`) so the frozen state reads at a glance, the list header gains one amber `Stale · <age>` chip (the rail carries none), **every** session glyph is forced
to `circle-help` (`unknown`, never `none`), and read-only actions keep working (`j`/`k`, `y`, `b`,
`/`, `i`, `:`). Mutating keys flash the banner instead of erroring. Terminal grids are veiled at
55 % and keys typed into them are **dropped, not buffered**.

**Doctor** (`D` from case B, `Run doctor` in Settings › About, or the palette) renders §3's
`doctor` output as a compact table inside the same surface:

```
 CHECK          STATUS   DETAIL
 git            ok       git version 2.49.0
 gh auth        fail     gh: not logged in to github.com
 copy-on-write  ok       cp -c (APFS clonefile)
 fleetd         ok       pid 4211 · protocol 4 · up 3h
 host devbox    fail     ssh: connect timed out after 5s
```

Failures render `red`, `ok` renders `fg.muted` (zero-suppression of good news at the color level).
The report's footer is three buttons with their keys: `Run again D`, `Open log L`, `Close Esc`.

**Degraded worktree.** A worktree whose post-create hooks failed keeps a persisted degraded fact
(§6) and shows `triangle-alert` + `⚠ hooks failed` on its row until the hooks job is re-run
successfully (`R` on that job in the Jobs panel) or the fact is dismissed from the detail panel.

---

### 3.13 First run and empty states

Every empty state is **two lines** — the fact, then the key — rendered centered **in the affected
pane only**, never full-screen, so surrounding panes stay usable. Copy is swarm's, verbatim.

| Surface | Line 1 | Line 2 (faint) |
| --- | --- | --- |
| No contexts | `No contexts yet.` | `N  create your first context` |
| No repos | `No repos in <context>.` | `n  clone one` |
| No worktrees | `No worktrees yet` | primary `New worktree  n` button |
| No worktrees for a repo | `No worktrees for <repo> yet` | primary `New worktree  n` button |
| Filter miss | `Nothing matches "<filter>".` | `esc  clear` |
| PR mine | `No open PRs authored by you in <scope>.` | `r  refresh` |
| PR review | `No PRs waiting for your review in <scope>.` | `r  refresh` |
| Jobs | `Nothing running.` | `Jobs and sessions live in fleetd, so they survive closing this window.` |
| Terminal exited | `process exited (<code>)` | `^s x  close    ^s c  new    ^s r  restart` |

**First run** (no contexts, no repos) is a single centered page, 560 px wide, under a bare title
bar (no context switcher, no section nav, no mode word). It says what Fleet is for and gets the
user started with controls, each showing its key (ADR 0023):

```
   ⛵
   Run many branches and agents at once — they keep running when you close Fleet.
   Three steps to your first workspace.

   ┌ (1) Create a context                                             N ┐   ← current: accent
   │     Group repositories by GitHub org or client, e.g. “Acme”.        │
   ├ (2) Clone a repository                                           n ┤
   │     Search your orgs on GitHub; it clones in the background.        │
   ├ (3) Start a worktree and an agent                       after step 2 ┤   ← dimmed, no click
   │     A branch copy with its own terminals and Claude or Codex threads.│
   └┄┄ ⛵ Import from ~/.swarm                                         i ┄┘   ← dashed; only with ~/.swarm
         Brings over contexts, repos and worktrees. Nothing in ~/.swarm changes.
   ─────────────────────────────────────────────────────────────────────
   ● fleetd running · 0.1.0 · ~/.fleet          Keyboard shortcuts ?   Settings ,
```

Clicking a step card does the step — the same action as its key (`N` opens New context, `n` Clone
a repository). Step 3 needs a repository, so it stays dimmed with `after step 2` in place of a key.
**[D-18]** For this user the empty state is also a **migration**, so the dashed import card is
shown only when `~/.swarm/state.json` exists (zero-suppression); `i` or a click runs `fleet import
--from-swarm` as a **job** and lands the user in a populated Hub. **No** onboarding carousel, tour
or sample data.

---

## 4. Keystroke counts (steady state, cursor where the previous flow left it)

| # | Task | Keys | Count | Note |
| --- | --- | --- | --- | --- |
| 1 | Open a worktree | `Enter` | **1** | MRU sort puts the last-used branch on row 0 |
| 1b | Open one in another repo | `l` `j`×n `Enter` | 3+n | rail → list |
| 1c | Open one by name from anywhere | `:` (or ⌘K / `ctrl-k`) `<text>` `Enter` | **3** + text | palette `Go to` rows |
| 2 | Create a worktree from a branch | `n` `<text>` `Enter` | **2** + text | base preselected; the dialog closes instantly |
| 2b | Create without opening | `n` `<text>` `⌥Enter` | 2 + text | batch capture |
| 3 | Create a worktree from a PR | `p` `j`×n `Enter` | **2**+n | `Enter` creates *and* opens |
| 3b | Capture 3 PRs without leaving the list | `p` then `c c c` | 4 | vs. 3 round trips |
| 4 | Switch between the two sessions I am juggling | `ctrl-s` `w` | **2** | MRU alternate |
| 4b | Switch to a third session | `ctrl-s` `W` `<text>` `Enter` | 3 + text | pre-filtered palette |
| 5 | Jump to terminal tab 2 | `ctrl-s` `2` | **2** | |
| 5b | Toggle the last two terminal tabs | `ctrl-s` `Tab` | **2** | no index arithmetic |
| 6 | Sleep this session and return to Hub | `ctrl-s` `S` | **2** | from the Hub: `s` = 1 |
| 6b | Kill the session | `ctrl-s` `s` `K` `Y` | 4 | destructive, deliberately longer |
| 7 | Delete a worktree | `d` `y` | **2** | facts, with their age, shown between the two keys |
| 7b | Prune a repo | `x` `y` | **2** | dry-run preview between the two keys |
| 7c | Undo the last delete | `u` | **1** | while the trash entry survives (§6) |
| 8 | Refresh PRs | `r` | **1** | both tabs |
| 9 | Notice a background job | *glance at the chip* | **0** | ambient |
| 9b | Read a job's log | `J` `j`×n `Enter` | 3 | |
| 10 | Copy the worktree path | `y` (Hub) / `ctrl-s` `y` (Workspace) | **1 / 2** | |

Median for the four highest-frequency tasks (open, switch session, switch tab, copy): **1.5 keystrokes**.

---

## 5. Cross-screen invariants (implementation checklist)

1. One row height (30 px), one glyph vocabulary (§2.5), one color law (§1.4), one dialog frame
   (§3.8) across every screen.
2. Blue is used **only** for cursor, focus and the one primary button's fill. Nothing else, ever.
3. `unknown` never renders like `none`; `none` never renders like an empty cell.
4. A nullable inspection fact renders `—` and its verbatim warning; it never renders `0`.
5. Every job-derived fact on screen has a stamp available (row → detail panel → confirm), and
   every confirm quotes its stamp inline.
6. No surface auto-hides a failure.
7. No bare-key affordance is drawn over a terminal (§3.6, D-8): every Workspace key chip carries
   its `⌃S` prefix, except inside the ⌃S command menu, where the prefix is already held.
8. Every action valid on a surface has a visible control there, and every control shows its key
   as a chip from the live keymap. No surface has a footer key legend; the terminal exit strip and
   the scroll pill, which have no controls, are the only `KeyHintRow`s. No mode word is drawn;
   the surface that owns the keyboard says so itself.
9. Every toast passes the toast law (§2.7).
10. The detail panel is never in the focus cycle; the Jobs panel restores the exact prior focus.
11. Background events never move the cursor, re-sort a list, or steal focus.
12. Every destructive confirm states facts and their age; every unknown decisive fact escalates
    the key to `Y`, drawn as a red button that does not accept `⏎`.

### 5.1 Pointer

1. Hovering a row shows its actions; every one of them is also in the row's ⋯ / right-click menu,
   so nothing lives only behind hover.
2. A click selects a row; a double-click or `⏎` opens it.
3. Clicking outside a dialog or sheet closes it, through the same cancel action as `Esc`.
4. Buttons and menu triggers are not focusable. The keyboard reaches them by their key, and
   `Tab`, `j`/`k` and pane focus keep their KEYMAP meaning.
5. A control runs the same action as its key, so pointer and keyboard can never diverge.
6. A Workspace tab is selected by a click and closed by its `✕` or a middle-click; a right-click
   selects it, then opens its menu, so the menu's verbs act on the tab that was clicked (§3.6).

---

## 6. Wire and state contracts used by this spec

These are the implemented data seams behind the UI. Additive fields retain their serde defaults so
version-1 config/state and older IPC payloads remain readable.

| # | Change | Why the UI needs it |
| --- | --- | --- |
| C1 | `Session { slept_at: Option<Timestamp>, kept_terminals: Vec<KeptTerminal { name: String, reason: String }> }` | §2.5 renders `moon` for *slept* distinctly from `circle` for *detached and awake*. `SessionState` stays `none \| detached \| attached \| unknown` (§1) — **sleeping is derived**, not a fifth variant, so the wire enum is unchanged. `kept_terminals[].reason` carries §4 step 6 strings verbatim (`unsaved changes`, `claude`, `:3000`, `sleep disabled`) for the sleep toast and the detail panel. |
| C2 | `Worktree { degraded: Option<Degraded { kind: HooksFailed, step: String, exit_code: i32, at: Timestamp, log_path: PathBuf }> }`, persisted in `state.json` as an additive optional field | The `⚠ hooks failed` chip (§3.3). |
| C3 | `Job { retryable: bool }` and a `RetryJob { id }` request | `R` in the Jobs panel (§3.7). Closes §9 "no retry path". |
| C4 | `config.trash.retentionMs` (default **600000**) and a `RestoreTrash { entry }` request; the daemon delays the detached `rm -rf` by that long | `u` undo-last-delete (KEYMAP A6). The delete algorithm already renames to `trash/<epochms>-<slug>` first, so the safety net is nearly free. |
| C5 | `config.jobs.warnBeforeQuit` (default **true**) and `config.jobs.keepFinishedFor` (default **600000**) | KEYMAP's `ctrl-q` clause is unimplementable without the first (§3.8.8); §3.7 retention needs the second. |
| C6 | `Terminal { has_unseen_output: bool }`, cleared when the terminal becomes active | The tab activity dot (§3.6). |
| C7 | `Snapshot { generated_at: Timestamp }` | The `Stale · <age>` header chip (§1.3, §3.12). |
| C8 | `WorktreeStatus.session` must be set to `unknown` — **never `none`** — whenever the local status observation fails, matching the remote path | Directly retires the §9 defect. This is a daemon behavior requirement, not a type change. |
| C9 | `PruneWorktrees { …, ids: Option<Vec<WorktreeId>> }`, defaulted and omitted when absent | `None` preserves legacy repo-scoped discovery; confirm commits `Some(exact displayed DELETE ids)`, and daemon reinspection may shrink but never expand that authority. |
| C10 | `Snapshot { agent_threads: Vec<AgentThreadSummary> }` (`#[serde(default)]`), the capability-gated native-agent request family with its thread, window, body, account, checkpoint and acknowledgement answers, and the `Agent` / `AgentSummary` synchronization events | §3.6.0's tab marks, §2.3's agent chips and the session-header word are all one derived `Attention` carried in the summary, so the strip, the header and the chips cannot disagree. `AgentMarkSeen` is what clears a finished turn's amber dot. IPC is version 8; the defaulted snapshot field keeps version-4 payloads readable. |
| C11 | Optional/defaulted `AttentionKind` on terminal `SetAgentActivity`, `AgentActivityChanged`, `Terminal.agent_attention`, and `WorktreeWindowStatus.agent_attention` | PTY hooks and native threads share permission/question/plan/finished vocabulary. Silence-driven idle remains status-only; hook attention alone can notify. |

---

## 7. Keymap

**`docs/KEYMAP.md` is authoritative for every key in this document.** The 24 amendments this
spec proposed (A1–A24), the arbitrations they needed and the bindings the screens above use are
all applied there; cite `docs/KEYMAP.md` rather than restating a binding here.

---

## 8. Decision log (issues raised against the base proposal, and how they are resolved here)

| # | Issue | Resolution |
| --- | --- | --- |
| D-1 | Header dropped `unknown/offline` | §2.3: five zero-suppressed chips including an amber unknown/offline count |
| D-2 | "Update available" as a sticky toast | §2.3 chip + Settings › About row; never a toast |
| D-3 | `none` rendered as a blank cell | §2.5: dim `dot` at 30 %; blank means "column not applicable" |
| D-4 | No rule for null inspection facts, no auto-inspect cadence | §1.3 + §2.6: `—` plus the verbatim warning; 400 ms debounced selected-row re-inspect, 30 s idle visible-row re-inspect |
| D-5 | px-only port lost the two-step author breakpoint | §2.9: ch-first ladders, px derived |
| D-6 | PR detail panel never enumerated | §3.5: full detail table incl. `WILL CREATE` destination, pull ref and fork note |
| D-7 | `All`-scope PR explosion undecided | §3.5: cap 100/tab, sort `updatedAt` desc, `+n more` row |
| D-8 | Bare-key affordances drawn over Terminal mode | §3.6: every Workspace affordance is prefixed |
| D-9 | Completed/failed jobs decayed on a timer | §3.7: failures never auto-dismiss; successes obey a user-set retention |
| D-10 | Uniform `y` for everything up to a context cascade | §3.8.3: `Y` escalation on unknown facts, and for repo/context delete |
| D-11 | `D` (context delete) adjacency risk | §3.8.4: `D` kept, routed through the expanded `Y` confirm; `E` + `ctrl-shift-d` is the discoverable path |
| D-12 | Assign dialog bound only `⌃n`/`⌃p` | §3.8.5: `j`/`k` restored — the dialog has no text field |
| D-13 | Settings could not edit grace / pool / TTLs / intervals | §3.8.6: all editable; `windows` (General) / `hosts` read-only with a 1-key `E` escape to `config.json` |
| D-14 | `ctrl-q` rule contradicted KEYMAP in **both** rival proposals | §3.8.8: opt-in warning implemented exactly as KEYMAP words it |
| D-15 | swarm's "Esc quits" | §3.10 / A13: `Esc` never quits |
| D-16 | One daemon banner for three situations | §3.12: cold start / will-not-start / died-while-attached, each with its own surface and keys |
| D-17 | Warm banner implied terminals survived | §3.12: mandatory reconnect sentence |
| D-18 | First run drew only a glyph | §3.13: migration-first card gated on `~/.swarm/state.json` |
| D-19 | Worktrees header lacked filter/scroll state | §2.10: scope · shown/total · visible range · stale stamp |
| D-20 | `sleeping` was an open contract question | §6 C1: derived from `slept_at`, wire enum unchanged |
| D-21 | Row-level `✎` / `#n` cadence undecided | §2.6 D-4: pinned, with an opacity ladder for aging facts |

---

## 9. Component inventory (feeds `fleet-ui-kit`)

Views in `fleet-app` compose **only** these; no ad-hoc styling. Names are the Rust type names in
`fleet-ui-kit`, and this table is kept in sync with §6 of `docs/DESIGN-SYSTEM.md`, which carries
each component's full API. `Modal` is an alias of `Dialog` and `TabBar` an alias of
`SegmentedTabs`.

### 9.1 Foundation

| Component | Responsibility | Used by |
| --- | --- | --- |
| `Theme` / `tokens` | The §2.4 token set in light and dark, plus the terminal `Palette(u8)` resolution table | everything |
| `Icon` | Lucide SVG via `AssetSource`; sizes 12 / 14 / 16 px, stroke 1.5, tinted by token | everywhere |
| `Text` | The four type roles (ui, data/mono, label, hint) with fixed line heights | everywhere |
| `Truncate` | Head / tail / middle ellipsis at a fixed ch or px budget | branch, path, title, repo, target columns |
| `FocusRing` | 2 px inset blue on a focused pane; 2 px left bar on a cursor row | panes, lists |
| `Tone` | The semantic color roles (`Default`/`Secondary`/`Muted`/`Accent`/`Success`/`Warning`/`Danger`/`Info`/`Inverse`) and their fills | every component that takes a color |

### 9.2 Structure

| Component | Responsibility | Used by |
| --- | --- | --- |
| `AppFrame` | Title bar + body region + status bar; fixed heights 44 / flex / 28 | Hub, PR screen, Workspace |
| `ContextBar` | Numbered context tabs, overflow chip, chip tray, daemon dot | all screens (§3.1) |
| `StatusBar` | Breadcrumb · `ModeWord` · job ticker · sticky error slot | all screens (§2.2) |
| `Pane` | Bordered region with a header slot, a body slot and a scroll thumb | lists, detail panel |
| `PaneHeader` | Label · scope · `shown/total` · visible range · `stale` stamp; swaps in `FilterBar` in place | §2.10, every list |
| `Sheet` | Right-docked panel, 440 / 640 px, no slide, focus-restoring on close; close ✕ and click-outside close | Jobs panel (§3.7) |
| `Dialog` | The shared frame: scrim + card + 44 px header + 44 px footer; close ✕, scrim click and `Esc` close through one action; button footer | all of §3.8 |
| `Overlay` | Centered floating layer with optional scrim and explicit paint layer | Palette (§3.9), Agent popup (§3.6.1) |
| `ToastStack` | Bottom-right stack, max 3, 3.2 / 1.6 s, 1 s identical-text coalescing into `×n` | §2.7 |
| `SplitLayout` | Two panes with a fixed side and a flex side, on either axis | Hub body, Workspace body |
| `Divider` | 1 px rule, horizontal or vertical | dialogs, panels |

### 9.3 Data display

| Component | Responsibility | Used by |
| --- | --- | --- |
| `ListView` | Virtualized 30 px rows, cursor, scrolloff 2, `gg`/`G`/`ctrl-d`/`ctrl-u`, cursor stability under background updates | sidebar repositories, worktrees, PRs, palette, assign, base list, clone results |
| `Row` | One row: leading glyph slot, flex content, trailing columns, selected/dimmed/disabled states | every list |
| `ColumnLadder` | Resolves a ch-based responsive column set for the current pane width (§2.9) | worktrees list, PR list |
| `StatusGlyph` | The §2.5 vocabulary — the single source of truth for session/job/clone state rendering | worktree rows, repo rows, PR presence, palette `GO`, confirms, quit dialogs |
| `Chip` | 22 px pill: icon + text + count, tinted, zero-suppressible | host chip, degraded chip |
| `KeepAliveChips` | `⚡` labels with a max-3 + `+n` overflow and the width ladder 18/14/10/0 ch; outranked by `DegradedChip` in the same slot | worktree rows, detail panel, confirms |
| `DegradedChip` | `⚠ hooks failed` with a link into the Jobs panel | worktree rows, detail panel |
| `PrBadge` | `#n` + state icon + ≤8 ch word, from the `PrState` priority | worktree rows, PR rows, detail panels |
| `AgeLabel` | Single-unit relative time (`2m`, `3h`, `5d`, `2w`) | rows, stamps, jobs |
| `FreshnessStamp` | `checked/fetched/inspected <age> · <key> re-check`, with the §2.6 amber/red ladder | detail panel, confirms, prune dialog, PR tabs, pane headers |
| `FactRow` | `label  value` with the null rule (`—` + `fg.faint`) and a `⚠ warning` variant | detail panel SAFETY, confirms |
| `FactList` | Ordered `⚠` risks then `✓` safe facts; decides compact vs. expanded confirm and the `y`/`Y` key | all confirms (§3.8.3) |
| `SectionHeader` | 20 px label row with an optional right-aligned stamp or action | detail panel, Settings, palette |
| `EmptyState` | Two centered lines (fact + key), pane-scoped | every list, Jobs, PR tabs |
| `SkeletonRows` | 30 % opacity placeholder rows, cold-load only | PR list cold fetch |
| `KeyHint` | Mono 11 px `fg.faint` key + label; right-aligns in palette rows | dialog footers, empty states, hint rows |
| `DoctorTable` | Fixed `CHECK STATUS DETAIL` table, red on fail | §3.12, Settings › About |
| `Badge` | Small count / label pill, distinct from the 22 px tinted `Chip` | PR tabs, headers |
| `StatusDot` | The bare 6–8 px dot behind `Chip`, `DaemonDot` and the tab activity mark | tab strip, chips |
| `KeyValueList` | A stack of `FactRow`s under one label column width | detail panel, Settings › About |
| `Spinner` / `SpinnerWithLabel` | `loader-circle` turning once per second — the only looping animation in Fleet | cold start, per-tab waking, jobs |

### 9.4 Input

| Component | Responsibility | Used by |
| --- | --- | --- |
| `TextInput` | The one editor (ADR 0020): the whole editing vocabulary, selection, undo, IME and clipboard, in single-line and multi-line modes | every text surface — Create, Clone, Context, Rename, Hooks, Settings, board dialogs, Filter, Palette, agent composer, lazygit prompt |
| `FuzzyList` | Debounced query → ranked rows, capped, `ctrl-n`/`ctrl-p` + arrows (and `j`/`k` **only** when no text input is present) | Clone results, Create base list, Palette, Assign |
| `FilterBar` | In-place pane-header replacement with live `shown/total`, two-stage `Esc`, retained chip | every list (§3.10) |
| `Cycler` | A closed choice, `←`/`→`; drawn as a `SegmentedControl` up to four options, a `Dropdown` field past that | host selector, Settings choices |
| `Toggle` | A labelled row ending in a `Switch`, `Space` | Settings |
| `SegmentedControl` | Two to four options side by side, the chosen one raised; a click runs the surface's own action | Hub screens, agent popup provider, Help's Guides / All shortcuts switch, Settings choices |
| `NumberField` | Integer with a unit suffix and a clamp | Settings (grace, TTLs, intervals, pool) |
| `SegmentedTabs` | Underlined tabs with counts, `Tab`/`S-Tab`/`h`/`l` | PR Mine/Review |
| `ConfirmDialog` | Compact/expanded switch driven by `FactList`; binds only `y`/`Y`/`Enter`/`n`/`Esc`/`q` (+ `I`, + `s` for prune) | §3.8.3, §3.8.8, §3.8.9 |
| `Palette` | One ranked, scrolling result list with section headings, icon tiles, match highlight and each row's key chip; a scope chip and prefix legend in the query row, the selected row and `Run ⏎` in the footer; seeded agent mode is one `Agents` section whose rows carry attention, provider/title, worktree, child/caller status, and the strip key | §3.9 |
| `Select` | A closed choice rendered as a row with its current value, for a set too long for `Cycler` | Settings, Create dialog |

### 9.5 Jobs and terminal

| Component | Responsibility | Used by |
| --- | --- | --- |
| `JobRow` | A job as a sentence: glyph · verb phrase + mono target · time, then its state's details (progress bar and last line, inline error and buttons) · optional `(restartable)` / `(not restartable)` | Jobs panel, quit dialogs |
| `JobTicker` | Newest running job as one status-bar line with a `+n` suffix | status bar |
| `StickyErrorSlot` | Red, addressable (`!`), persists until dismissed; owns the last failed job | status bar |
| `LogView` | Tail of `logs/jobs/<id>.log`, last 200 lines, 16 ms batching, follow toggle, `G` re-follow | Jobs panel |
| `TerminalGrid` | Paints the mirror cell grid from `FrameUpdate`: the full VT attribute set (bold, dim, italic, single/double/curly underline with its own color, strikethrough, inverse, blink, invisible), narrow/wide/spacer cells, the four cursor shapes, the selection overlay and the scrollback badge | Workspace, Agent popup |
| `TerminalModes` | Zero-suppressed badges for `alt` / `mouse` / `paste` / `appcur` | the gallery only: the Workspace draws no modes (§3.6) |
| `ScrollbackBadge` | `↥ <offset>/<len>` in the grid corner whenever the viewport is scrolled back, in or out of Scroll mode | Workspace |
| `TerminalTabStrip` | Numbered tabs 84–200 px with an activity dot, a per-tab waking spinner, keep-alive icon, exited mark (code or `—`) and a `+` tab | Workspace |
| `ScrollPill` | `SCROLL <offset>/<len>` overlay with a selection hint line; **suppressed in alt-screen** | Workspace and Agent Scroll modes |
| `PrefixMenu` | The ⌃S command menu: every command the held prefix reaches, grouped, clickable, 400 ms delayed, bottom-centre | Workspace, agent popup and agent thread prefixes |
| `ExitStrip` | `⚠ process exited (<code>)` + prefixed recovery keys | Workspace, Agent popup |
| `ModeWord` | The embedded Git UI's status word, fixed 84 px (Fleet's chrome has none, §2.8) | Git UI status bar |
| `Banner` | 28 px full-width amber/red strip with a countdown and prefixed keys | daemon state C (§3.12) |
| `DaemonSplash` | The full-window cold-start and will-not-start surfaces: title, spinner, socket path, `fleetd.log` tail, bare recovery keys | daemon states A and B (§3.12) |
| `DaemonDot` | 8 px liveness dot that expands into a labelled pill when degraded | kit only today: the status bar's daemon slot and the title bar's daemon button carry liveness |
| `Veil` | 55 % scrim over terminal grids only, with key-dropping | daemon disconnect |

### 9.6 Native agent transcript

| Component | Responsibility | Used by |
| --- | --- | --- |
| `TranscriptList` | Bottom-anchored variable-height rows over gpui `list`, keyed and spliced, jump-to-latest, scroll mode | §3.6.0 |
| `ToolRow` | The 30 px `glyph · 60 px verb · summary · result chip` pointer-first row, with hover verbs, a right-click menu, nested children and a bounded expanded body | §3.6.0 |
| `DelegationRow` | Two-line live/terminal child summary with provider, status mark, headline, elapsed time and attach hint; never fold-grouped | §3.6.0 |
| `DelegationResultCard` | Delivered child result in Markdown, collapsed to eight lines and expandable with the shared fold affordance | §3.6.0 |
| `DecisionDock` | The docked permission / question / plan drawer: a glyph-led title, buttons with live key chips, clickable option rows; owns the decision and key vocabulary, while plans remain transcript rows whose verbs are the dock's buttons | §3.6.0 |
| `MultilineInput` | The docked composer: wrapping, IME, paste, `↑` history, `⏎`/`⇧⏎`, `/`, `@` and `$` triggers | §3.6.0 |
| `MetadataRow` / targeted `MetadataSegment` | Width-aware metadata whose opaque optional target renders in link tone, takes a focus ring and activates through click or `Enter`; the composer's link line above it (a child's caller, a card run's card) | §3.6.0 |
| `ComposerChip` / `ContextMeter` | The composer's settings strip: a value-and-chevron menu trigger for the model and the access mode, and the context-window meter | §3.6.0 |
| `Markdown` | Assistant prose parsed from a stream, stable under growth | §3.6.0 |
| `DiffView` (`fleet-lazygit`) | Inline unified diff under an edit row, ADR 0005 rows with the semantic diff washes | §3.6.0 |

## Subagent watch pane

A new subagent watch for the current Workspace session opens a read-only split
on the right and selects that watch. The terminal is the flexible leading region
of `SplitLayout::horizontal()`; the trailing region is 40% of the current window
width, clamped to 360–640 px and recomputed on resize. PTY dimensions follow the
terminal's actual reduced painted bounds. Zoom (`^s z`) hides the tab strip while
leaving the watch pane, including its header, visible.

The pane header shows the child label, prefixed with the quiet `◦ ` marker when the
daemon discovered the process, a status dot and `running`, `exited <code>`,
or `interrupted` for signal-only completion; elapsed time as `mm:ss`, frozen after
exit; the `^s v` hint; and a right-aligned clickable `×`. Multiple watches add a
compact tab strip with each label and status dot; discovered tab labels use the same
marker. Clicking a tab selects it.

The body uses `LogView` in following mode. Newline-delimited lines are assembled
independently for stdout and stderr, with live partial trailing lines. Stdout uses
normal text contrast and stderr uses secondary text. Empty output says
`waiting for output…`; an unavailable earlier sequence range or local retention
trimming adds `older output trimmed` above the log. Each watch retains at most
1 MiB of text and 20,000 displayed lines, including partial lines.

| Workspace prefix | Action |
| --- | --- |
| `^s N` | Select the next watch in this session's start order, wrapping to the first; show the pane if hidden. |
| `^s P` | Select the previous watch in this session's start order, wrapping to the last; show the pane if hidden. |
| `^s v` | Toggle the pane locally. With no watches, toast `no subagent watches`. Repeated toggles never cycle tabs. |
| `^s V` / mouse `×` | Dismiss the selected completed watch and select the next tab, wrapping at the end. If it was the last, close the pane. For a running watch, hide locally and toast `watch still running; pane hidden`. |

Both cooperative and discovered watches participate in the same session-local list.
`^s N`/`^s P` toast `no subagent watches` when the session has none. With one watch,
selection is unchanged without a toast; the pane is shown if hidden. Lowercase
`n`/`p` remain terminal-tab navigation. The `?` help overlay lists both watch keys.

Hiding persists per session across navigation and reconnect until explicitly shown or a **new**
WatchStarted event arrives. Every new start reopens that session's pane and selects
the new watch. Duplicate start events, output, completion, and ordinary catch-up
responses do not undo a user's hide or selection. First discovery selects the
newest retained watch. Mouse tab selection and `^s N`/`^s P` share the same session-local selection.

The pane never sends input and never takes keyboard focus from the terminal.
Closing/hiding a pane never kills a process. Subscribe before listing/tailing;
reconcile on Workspace entry, session changes, reconnect, and event gaps as
specified in `APP-CONTRACTS.md`. Daemon dismissal, TTL, and terminal/session cleanup
remove the corresponding local watch. See that contract for duration recovery
limits when an already-completed watch is first discovered.

## Board

*One unscoped board per context and, on demand, one per worktree; one column per status, one key
per edit* (BOARD §8).

### Placement

The board has **two surfaces and one pane**. The Hub's third screen tab (`g b`, tab label
`Board`) shows the active context's board; a worktree Workspace's `fleet://board` tab (`ctrl-s b`,
§3.6) shows that worktree's. Both are drawn by the same `screens::board::BoardScreen` — the two
are never on screen at once, so there is one of it, one filter editor and one mirror behind them.

On the Hub the board replaces the worktrees list and the sidebar in place, and the title bar's
context switcher above it is what scopes it: the board shown is always `EnsureBoard(active_context)`. Switching
context clears the board and re-ensures the new one.

Inside a worktree session the `fleet://board` tab shows `EnsureWorktreeBoard(worktree)` instead —
the board of the worktree whose Workspace you are standing in, whatever the active context's
board holds. Selecting the tab is what asks for it; selecting another tab, leaving the Workspace
or closing the tab hands the pane back to the context board. **The Hub never shows a worktree
board's cards**, and the worktree tab never shows the context board's: a board belongs to exactly
one scope, so `g b` and `ctrl-s b` answer different questions and neither inherits the other's
answer. A daemon too old to serve worktree boards refuses the tab and says so
(`this daemon does not support worktree boards; run fleet daemon restart`, §2.7) rather than
opening a tab it could never fill.

The `Board` tab carries the active context's **unscoped** `Snapshot.boards` summary: `open_count`
as the tab count, and a `•` appended to the label when `conflict_count > 0`. Worktree-scoped
summaries from that context are ignored — the Hub's count is the context board's count, not the
sum of everything in the context. The tab spins while a load is in flight. The Workspace's board
tab carries no open count and no conflict dot: its only chrome is the native-tab glyph every
`fleet://` tab has, because the tab strip is a strip of terminals and a count there would be the
one number in it that is not about a terminal.

### The pane

```text
 Fleet board FLT                          ☁ Jira · synced 2m  S  ⋯  [⌕ Filter cards  /]  Board settings  ,  ⋯  [+ New card  c]
 8 cards · 1 of 2 runs working · ● 1 needs you
 ┌ ● Backlog 2          + ┐ ┌ ● In progress 2          + ┐ ┌ ● In review 1           + ┐
 │ ┌───────────────────┐  │ │ ⚡ On enter: codex implements│ │ ⚡ On enter: claude reviews │
 │ │ ▂▅ FLT-1        ⋯ │  │ │ ┌────────────────────────┐ │ │ ┌───────────────────────┐ │
 │ │ Photograph the    │  │ │ │ ▂▅█ FLT-5  ◌ working 4m │ │ │ │ ▂▅ FLT-7  ✓ review passed│ │
 │ │ harness  2 pt  DF │  │ │ │ Build real git origins │ │ │ │ Make hub tabs clickable │ │
 │ └───────────────────┘  │ │ │ git ⑂ flt-5-git-origins DF│ │ │ ui  #14 Review       JS │ │
 │ + Add card             │ │ └────────────────────────┘ │ │ └───────────────────────┘ │
```

**Header.** A `PageHeader`, not a pane header. On the left the board's name as the page title and
its prefix badge, and under them one muted line: `8 cards` (`3 of 8 cards` while the filter hides
some), then `1 of 2 runs working` — the cards holding a run slot, live **or** owed, over
`settings.max_live_runs` — and then, in amber, `1 needs you`, which is clickable and selects the
first card waiting on a person. Both counts are zero-suppressed; the dirty (`⬆2`) and conflict
(`⚠1`) counters and the `refreshing` spinner follow them on the same line. On the right, in order:
the **sync button**, which reads the backend's registry label and its age (`Jira · synced 2m`,
`… · never synced` before the first) and runs `S`, with a `⋯` beside it holding **Full sync** (`F`)
and **Reload** (`r`) — a local board mirrors nothing, so it has only the `⋯`, holding Reload; the
**filter field** (`Filter cards  /`, always on screen: a press on it is `/`; while typing it
shows `shown/total`, it keeps the retained query after the first `Esc`, and its `✕` clears it); **Board settings** (`,`) with a `⋯` holding **Columns**
(`C`); and the one primary button, **New card** (`c`). Every control dispatches its key's action
and shows that key from the live keymap. The chip reads the backend's **label** from the
daemon's registry (`Jira (acli)`), not its registry key; until the registry answers the raw kind
stands in, because an empty label reads as a broken header. A `board.sync` job in
`Snapshot.jobs` puts a `syncing` spinner before the sync button.

**Error callouts.** Under the header, never over the columns: the load error verbatim with a
**Reload** button, the board's own `sync.last_error` as `Sync failed: …` with **Board settings**,
and the orphan sentence — cards whose status the board no longer has — with **Board settings** and
(on a remote board) **Sync**, the two things that can place them.

**Columns** are `KanbanColumn`s in `board.statuses` order. The header is a coloured **dot**, the
name as the board writes it, the count, and a `+` that opens New card **in that column** — the
only way to file a card somewhere other than the board's default column in one step. The dot is
the status **category**, not its name: muted for `Backlog`, secondary for `Unstarted`, amber for
`Started`, green for `Completed`, a strong border grey for `Canceled`; a status that carries an
explicit token name in `color` overrides it. A column whose entry runs an action wears a pill
under the header naming it in words — `On enter: codex implements`, `On enter: claude reviews`,
built from the action's provider (`agent` when the column leaves it to the card) and a verb read
from the skill's name or the first word of the prompt's instructions — and clicking the pill opens
Board settings drilled into that column. Only `on_enter` earns a pill; `on success` and `when
unblocked` move a card the column has already finished with. Every column ends in an `Add card`
row, the `+`'s twin. Cards are `CardTile`s from `ops::column_cards`, so the app never invents an
order the daemon does not agree with.

**Tiles.** The first line is the priority bars, the key, and at its right end the one **state
pill** the card has: its run, in words — `working 4m · codex` (the age from the run's start),
`waiting`, `needs you`, `review passed` — or, when it is not running, what blocks it: `blocked by
FLT-5`, or `blocked by 2 cards`. The run wins over the blockers, because a card that is running
has nothing left to wait for. The blocked pill is neutral while a blocker can still finish and
amber when one of them is canceled, archived or gone from the board, because nothing will release
the card on its own. Then the title, two lines at most. Then the meta row: label chips, `n pt`,
the due date, the linked worktree's **branch** and its PR badge (`#14 Review`), the dirty and
conflict dots, `show_on_card` extras — and at its right end an **Answer** button on a card whose
run needs you (the worktree board only; it runs `A`), and the assignee's avatar. Everything but
the first line and the title is zero-suppressed. Hover lifts a tile (a stronger hairline and a
short shadow) and reveals its `⋯`, which a selected tile keeps.

**The card menu.** The `⋯` and a right-click open the same menu, which holds every card action
with its key: **Status** `s`, **Priority** `p`, **Assignee** `a`, **Labels** `t`, **Estimate** `e`,
**Blocked by** `b`, **Agent** `m`; then **New worktree** `w`, **Open worktree** `o`, **Open remote
issue** `x`, **Move to previous / next column** `[` / `]`; on the worktree board **Attach run** `A`,
**Run now** `>` and **Cancel run** `X`; and last **Delete** `d`, in red. An entry that could only
refuse is left out rather than shown and refused: a picker for a field the backend owns, Open
worktree on a card with none, Open remote issue without an address, `[` in the first column and
`]` in the last, the run entries where the card has nothing for them, and Delete on a mirrored
card (it is deleted in its backend). The key stays bound either way and still says why.

Both counts, every mark and every pill string are folded once per change —
`AppState::refresh_card_marks` for the marks, the board projection for the words, rebuilt once a
minute while a run is live so its age moves — and nothing about a run is derived in a `render`
(§5, `APP-CONTRACTS.md`).

### States

* **cold** — skeleton columns while the first load is in flight, whichever request the scope
  named (`EnsureBoard` on the Hub, `EnsureWorktreeBoard` in the worktree tab);
* **failed** — the message verbatim in a sticky row plus `The board could not be loaded. · r reload`;
* **no context, no daemon, no worktree boards** — the three states where a load can never go out
  take the failed shape rather than cold columns, because skeletons promise a request that was
  never sent: `fleetd is not reachable`, `No active context — pick one with 1–9 or gt / gT`, and
  — in a worktree's board tab on a daemon that serves no worktree boards — the same sentence the
  refusal toasts. Activating a context clears the board, which drops the message and asks again;
* **empty board** — `No cards yet. · c new card`;
* **empty column** — `No cards` in muted text, then its `Add card` row;
* **no match** — `Nothing matches "<query>". · esc clear`.

**A card's run states**, one glyph each, identical on the tile and in the card detail. Each names
what the reader does next; a board no column automates shows none of them and is pixel-for-pixel
the board it was before automation existed.

| Mark | Glyph | What it means · what you do |
| --- | --- | --- |
| `Pending` | gray `Spinner`, secondary | The card is owed a run and the board is at its limit. Nothing to do — or `X` to drop the slot it waits for. |
| `Stalled` | amber `StatusDot` | The same wait, now older than `PENDING_AMBER_AFTER_SECS` (60 s). Something ahead of it is not finishing: look at what is working. |
| `Working` | gray `Spinner`, secondary | A child is running for this card. `A` attaches its thread as an ordinary agent tab. |
| `NeedsYou` | amber `StatusDot` | The run ended `needs you`, `failed` or `incomplete`, or its child is blocked on a question. Open the card: the run row and the report say which. |
| `Succeeded` | faint `Icon::Check` | The run succeeded and the column did **not** carry the card on. A column with `on success` says it by moving the card instead, so the check never marks a card its workflow already advanced. |

A canceled run draws nothing: a deliberate stop is a decision, not a state to keep reporting. A
run whose start never reached a thread raises the sticky error **once** — the run's own `detail`,
or `<KEY> could not start a run` when it carries none — rather than a mark: there is nothing to
attach to, nothing still moving, and no second view of the same board says it again.

### Keyboard

`h`/`l` and `←`/`→` move between columns, `j`/`k` and `↓`/`↑` between cards; both clamp and never
wrap (§5.11), and the focused column and card are always scrolled into view. Every §8 key is in
`docs/KEYMAP.md`. Two rules are worth stating here:

1. **The selection follows the card, not the index.** After any mutation the reducer applies the
   `Card` the daemon returned and the focus moves to wherever that card now is — including the
   column it was just moved to by `[` / `]`.
2. **`/` publishes the `Filter` key context.** The board's filter is a live single-line
   `TextInput`, not the Hub's `FilterState`
   (its rows are cards in columns, and `Overlay::Filter`'s `Enter` opens a worktree), so the board
   mirrors into `BoardState.filter` on `Changed`. While the input has the keyboard,
   `AppState::context_chain` returns
   `["Filter", "BoardFilter"]` instead of `["Hub", "Board"]` — or instead of
   `["Workspace", "Native", "Board"]` in the worktree tab, where the same editor and the same
   two-stage `Esc` serve: that is the only thing that makes `c`, `d`, `s` and
   `w` type instead of fire. `Esc` is the two-stage §3.10 one — leave the input keeping the filter,
   then clear it — and never quits. `Tab` / `Shift-Tab` move columns while left/right and
   ctrl-b/ctrl-f move the filter caret.

**The workflow keys.** The first five act on the focused card — its *run*, or the links and agent
that decide one — and are bound on the worktree board and inside the card detail, so the card
being read is the card they act on; `b` and `m` are on the Hub's board too, and `C`, which acts on
the board rather than on a card, is on both boards and not in the detail:

| Key | Does | Refuses with |
| --- | --- | --- |
| `A` | Attaches the card's run as an ordinary agent tab — the live run's thread, else the newest one that reached one. | `{KEY} has no run` on a card that never ran; `{KEY}'s runs never reached a thread` when it ran but no run of it did |
| `X` | Cancels the live run, or drops the slot an owed one waits for — a child the mirror holds live counts, whether or not the card's own `runs` row carries it yet. Asks first (§3.8.3). | `{KEY} has no live run` |
| `>` | Asks the column to run its action on this card now, whatever the last run ended as. | `{column name} has no action` |
| `b` | Opens the multi-select picker over the cards this one is **blocked by**. | — |
| `m` | Opens the agent pickers — provider, model, effort. | — |
| `C` | Opens Board settings on its **Columns** section. | — |

`A`, `X` and `>` are **not** bound on the Hub's context board: a context board has no worktree to
run in. `b` shadows the Hub's own `b` (open in browser) while the board owns the keys
(`KEYMAP.md`, `APP-CONTRACTS.md` §3). Every refusal is the daemon's own sentence where the daemon
has one, so the key, the palette row and `fleet board` cannot disagree.

Two moves ask before they destroy a run. `[` / `]` onto a card the app's delegation mirror knows
is working — which it does from the child's own caller, before the card's `runs` row catches up,
exactly as the tile mark is painted (`BOARD.md` §11.8) — raises `Move {KEY}?` — `{KEY} is working ({elapsed}). Move to {target} and cancel the
run?` — and confirming sends **one** `MoveCard { cancel_run: true }`, never a cancel and a move.
`X` raises `Cancel {KEY}'s run?` with `Stops the run. What it already changed in the worktree is
kept.` and one risk line naming the child and its age, or the owed slot being dropped. A card the
mirror holds no live child for moves with no question; if the daemon disagrees, its `Conflict` is
what says so, in the sticky slot.

The filter is a case-insensitive substring over the six things a card is looked up by: title,
display key, local key, label names, label ids and assignee. It is deliberately wider than
`resolve_card`: a local key is not a *selector* on a mirrored card, but it is still something a
reader can see on the tile and type here.

### Mouse

The board follows §5.1. A press on a tile selects it; a double-click, like `⏎`, opens its detail;
a right-click selects it and opens the card menu, whose visible twin is the tile's `⋯`. Clicking a
column focuses that column without moving the card selection. A column's `+` and `Add card` focus
that column and open New card with its status chosen — the dialog's subtitle names it, `· Fleet ·
In progress` — and its automation pill opens Board settings on that column. Every header control
is the pointer twin of its key (above).

### Card detail

A **sheet** docked to the right of the board, `sheet_w_detail` (736 px) wide and full height
between the title bar and the status bar, over the board dimmed by its scrim — so the board the
card belongs to stays in view. `Enter` or a double-click on a tile opens it. `Esc`, the sheet's ✕
and a click on the dimmed board beside it all close it through `card_detail::Close`, so a first
`Esc` (or click) during an edit cancels the edit and a second one closes the card. It publishes
browsing `Dialog > CardDetail`, then switches to `Dialog > CardDetailEditing` while the title,
description or comment owns the keyboard; the migrated live editor adds `FleetTextInput` beneath
that word. There is no footer legend: every key is on the control it runs (ADR 0023).

**Header** — the card's key in the data face; a **status button** (the column's dot and name, `s`)
that opens the Status picker; on a linked card the backend line `Jira FLT-5 · synced 2m`; then
`Open in <backend> x` on a card whose issue has an address, a `⋯` holding *New worktree* (`w`),
*Keep your version* / *Take the tracker's version* (`K` / `R`, only while the card is
conflicted) and *Delete* (a card Fleet owns; the confirm follows), and the sheet's ✕.

*Left column* — the card as prose, scrolling:

- The **title** in the page-title face; a click edits it, exactly as `i` does.
- The **conflict callout** when the card has one: amber, `Conflict — Status, Due differ from
  FLT-5`, with `Keep local K` and `Take remote R`.
- **The run card** (`BOARD.md` §11.9), only when the card has a run or is owed one: the board's
  own mark — the identical glyph, read from the same fold, so tile and card can never disagree —
  then `Working 4m` and `codex · gpt-5 · high`, and once the run is over and the numbers are known
  `12.4k tok · $0.31` at the line's right end. A missing model or effort drops with its separator
  rather than printing a dash. The state word is the delegation's (`starting`, `working`,
  `blocked`, `settling`) while the run is live and the outcome's (`succeeded`, `needs you`,
  `failed`, `incomplete`, `cancelled`) once it is not; a card waiting for a slot reads `Pending 2m
  · waiting for a slot`, which is a different fact from a slow run and the only one the reader can
  act on. An owed run wins over a finished one. Beneath the line, the buttons that act on it —
  `Attach A` (primary, when the run has a thread), `Re-run >` (when the column runs an action) and
  `Cancel run X` (red, while the run is live or owed) — each drawn only when its key would work.
- The **description** as `MarkdownText` under a `Description` label with `Edit d`, or `No
  description` and `Write one d`; while `d` edits it, the shared multi-line `TextInput` with
  `Cancel esc` and `Save ⌃⏎` under it.
- The **comments** under `Comments · n`: each an avatar, the author (`You` for a local comment),
  its age and the Markdown body. Then the composer: `Add a comment… c`, which `c` or a click turns
  into the same input with `Cancel esc` and `Comment ⌃⏎`; `⌃⏎` and `⌃S` both post.

**Report comments.** A comment a run wrote is authored by its provider in a secondary avatar with
a `run {n}` badge, and folds at eight lines — `REPORT_COLLAPSE_LINES`, the same fold the transcript
gives a delivered child result — ending in a `Show more ⏎` button. While any report is folded,
`⏎` (and that button) opens **all** of them before it goes back to meaning "edit the selected
property"; the button is drawn only while one is closed, so nothing on screen ever names a key
that would do nothing. Expansion lasts as long as the sheet.

*Right column* — the card as facts, under `Properties`: `Status`, then the workflow rows below,
then `Priority, Assignee, Labels, Estimate, Due, Parent`, a hairline, `Repo, Worktree`, then
`Remote` / `URL` / `Synced` when the card is linked, then the board's custom properties in schema
order. Under them, **Activity**: the last three entries, newest first, `message · age`, with
`Show all n` opening the last ten.

Every row is **clickable**: a click selects it and runs what `⏎` runs on it — the same code path —
so it opens that field's Card property picker, opens the worktree's session on the Worktree row
(drawn as a link), or the issue on the Remote row. While the pointer is over a row (or the
keyboard's `j` / `k` has selected it) its key shows at the right end, from the live keymap: `s`,
`p`, `a`, `t`, `e`, `b` (the first *Blocked by* row), `m` (*Provider*), `o` (*Worktree*), `x`
(*Remote*), and `⏎` for a row with no letter of its own. Those letters work on the sheet as they
do on the board, aimed at the card on show: each selects its row and opens it. A row that states
a fact (`URL`, `Synced`, `Parent`) takes no click.

The **workflow rows** sit directly under Status, because they answer the question Status raises on
an automated board — what runs this card next — and each is zero-suppressed, so a board nobody
automated prints exactly the property list it printed before automation existed. `Provider`,
`Model` and `Effort` appear only while the card's **column runs an action**, and state what this
run would actually use once the card has been read over the column: a value the column supplied
carries a muted `column default` beside it, a value the card carries does not. A skill column
shows `claude` however the card was set, because that is what the run will use. `Blocked by` and
`Blocks` appear as soon as the board uses links at all — an empty row is how the reader learns the
card can have them — with one row per link, the label on the first only. A blocker already
satisfied reads `✓ FLT-3 · Done` rather than vanishing; one that was canceled or archived is amber,
because nothing will release the card on its own. `Blocks` is derived from everyone else's
`blocked_by`, never stored. Unset values read as an en dash in the muted tone, never as an empty
cell. `x` opens the card's remote issue in the browser and leaves the sheet open, because the
browser is another window and closing the card the user is reading loses their place.

A field the board's backend declared it cannot write back (`board.sync.readonly_fields`) reads in
the secondary tone with a trailing lock glyph and no hover, and takes no click; it keeps its
picker target for the keyboard: `⏎` or its letter still answers, with the sentence `<field> is
read-only on <backend label> boards` on the sheet's error line. A row that silently did nothing
would be indistinguishable from a broken key.

The three text surfaces — title, description, comment — share **one live `TextInput` entity**,
created when `i`, `d` or `c` starts an edit and dropped when `ctrl-s` / `ctrl-enter` saves or
`Esc` cancels it. Selection, paste, word/line deletion and undo/redo are available uniformly; Tab
inserts a hard tab in the multi-line description/comment editor. While an edit is open
`CardDetail` is absent from the context chain, so its bare browsing letters cannot steal input;
`CardDetailEditing` carries only the container commands, and the property rows take no click.

Nothing on this surface is optimistic. Every save sends its request and waits; the reducer applies
the `Card` that comes back, and a refusal becomes a red callout at the top of the left column
rather than a change the user believes happened. Opening a picker from the sheet swaps the sheet
for the centred Card property dialog, which returns to the sheet when it closes.

### The other three dialogs

* **New card** (560 px) — a single-line title `TextInput` and an optional multi-line one for
  the description. `Enter` creates and closes; `ctrl-Enter` creates and opens the card it made. Nothing else is asked for,
  because every other field has a one-letter picker on the board. The footer is `Cancel`,
  `Create & open ⌃⏎` and the primary `Create ⏎`; in the description, where `⏎` is a newline,
  `Create` still creates but shows no key.
* **Card property** (560 px) — one surface for every field: a query input over a `FuzzyList` of
  the values that field can take. The open-ended kinds (assignee, estimate, due date, and `Text` /
  `Number` / `Url` / `User` properties) offer the typed query itself as the first row. Estimates
  must parse as numbers and dates as real `YYYY-MM-DD` days, and the field says which rule failed.
  `Labels` and custom `MultiSelect` properties are the multi-selects: `space` toggles, `Enter`
  applies the whole set. The list shows eight rows and scrolls past them, keeping the cursor in
  view. A click on a row is that row's key: it toggles a multi-select row (the check marks the
  chosen set) and applies a single-select one, so the clear rows and the typed-value row are
  clicked like any other. The footer is `Cancel` and `Apply ⏎`.

  The workflow kinds follow those two rules and add one. `Blocked by` and `Blocks` are
  multi-selects over every non-archived card but this one, each opening with a clear row
  (`No blockers` / `Blocks nothing`) as `Labels` opens with `No labels`; a candidate that would
  close a dependency loop is **listed and disabled** with the detail `would cycle`, because a
  hidden row teaches nothing and the daemon would refuse it anyway — the disabled set is
  `ops::validate_links`' own answer, asked once per candidate. `space` on a disabled row keeps the
  key and changes nothing; `Enter` on one, in a single-select kind, puts `{label} — {detail}` on
  the error line. Applying `Blocked by` sends one `UpdateCard` for this card; applying `Blocks`
  sends one per dependant whose membership changed, in key order, stopping at the first refusal
  with its sentence on the error line — the cards already patched stay patched. `Provider` is the
  closed set `column default · claude · codex`. `Model` and `Effort` offer `column default`, the
  vocabulary this client has seen the resolved provider's harness declare, and **the typed query
  as a row of its own**: the catalogue is only what the app has met, and a model it has not met yet
  still has to be settable. Each of the three replaces its one field and sends the card's whole
  `agent` block, last writer wins, exactly as labels do.
* **Board settings** (720 × 560) — the same two-column shape as the global Settings dialog
  (§3.8.6): a section rail and a pane. Three sections, `Tab` between them — **General**,
  **Backend**, **Columns**. `,` opens it on the section last used in this app session (General on
  the first open of a session) and `C` opens it on Columns. While it is open the status bar's
  breadcrumb drops its row: the dialog edits the board, and the focused card is the one thing it
  cannot change. The rail's sections and every row take a click that puts the cursor there, a
  closed choice draws its options (side by side, or a dropdown when they are many or long) and
  a click on one lands where `h` / `l` would, and a flag's switch flips as `space` does.

  **General** — name, prefix, default repository, start-on-worktree, push-new-cards, conflict
  policy, then `Max live runs` with the hint `runs share one checkout`, last because it is the
  only row about *runs* rather than about the board's identity. **Push new cards** is drawn
  disabled on a local board, where there is no backend to file anything with; it defaults to
  **off**, so on a linked board it is the row that says why a card made here has not become a
  remote issue. **Backend** is the kind cycler and that backend's own schema rows, generically
  (below).

  **Columns** is the board's `statuses` vector as a draft. The list shows one row per column with
  a muted `⚡` when entering it runs something, and its keys are `n new · d delete · J/K reorder ·
  P preset · ⏎ open`. The footer carries `New column n`, `Delete d` and `Apply preset P` while
  the list is showing, and each row shows ↑ / ↓ buttons on hover (`K` / `J`); a click selects a
  column and a double-click opens it. Inside a column the footer offers `Columns` to go back. `⏎` drills into a column and the pane becomes that column's form, in this
  order: Name, Category, On enter, then — only while On enter is not `none` — Provider, Model,
  Effort, Mode, Instructions, Expect, Env, then On success and When unblocked. `On enter` is
  spelled exactly as `fleet board columns edit --on-enter` spells it (`none`, `prompt`,
  `skill:<name>[:<args>]`); it is drawn as a choice of the three (a click picks one), `⏎` opens
  it for typing a skill name, and it refuses
  anything else with `on enter must be none, prompt, or skill:<name>[:<args>]`. `P` adds the
  workflow preset's **missing** columns by id and never rewrites one the board already has,
  reporting which of the two happened on the notice line. `d` on a column holding cards does not
  delete it: it arms, naming the count (`In review holds 3 cards — choose the column they move
  to`), marks the column `deleting`, and the next `⏎` on — or click on — another column is where
  those cards go — one `MoveCard` each in column order, and a refusal stops the sequence there, keeps the
  column and leaves the cards already moved where they are. A board needs at least one column, and
  says so.

  Nothing is sent until `^s`, which is the primary button (`Save ⌃S`, disabled until something
  changed) beside `Cancel`, and saves from any section;
  `⏎` still saves from General and Backend, as §3.8.6's dialog does, and inside a column it opens
  the focused row's editor and commits it. A save is
  one `UpdateBoard` carrying the whole vector, so reordering three columns, renaming one and
  routing another is one request rather than five. The setting the dialog does not show
  (`branchTemplate`) is carried through unchanged. `esc` goes back a level — out of an editor, out
  of a column, out of an armed delete — and at the top level with unsaved edits it asks **once**,
  on the dialog's own error line (`unsaved changes — esc again to discard them`), rather than
  opening a second dialog over the one it is asking about. On a context or Jira board every
  automation row is disabled and the pane says why, once, under the rows:
  `Automation is available on worktree boards`.

  The backend rows are **generic**: the dialog knows no backend by name. `Backend` is a cycler
  over the kinds the daemon registers, drawn by their labels, and every row under it is one entry
  of that backend's `settings_schema` — the schema's `name` is the row label, its `key` is the
  JSON key written into `BackendRef.settings`, and its `PropertyKind` decides the control: a text
  field, a toggle, a number field that takes digits only, a cycler over a closed set, or a
  comma-separated list for a multi-select. A schema name ending in `(required)` marks the row `∗`
  and refuses an empty save before the request goes out; every other empty row **removes** its
  key rather than writing `""`, because an absent optional key is `None` to a backend and an
  empty string is a setting it has to honour. Keys the schema never names — the ones only
  `fleet board set` writes — survive a pass through this dialog untouched.

  Changing the kind starts from empty settings, exactly as the daemon does, and cycling back to
  the board's own kind restores the settings it was opened with. A refusal keeps the dialog open
  and shows the daemon's message verbatim: only the backend can say why a project key is wrong,
  or why a linked board will not change kind.

### Three facts the board states rather than hides

`S` on a `local` board is refused by the daemon (`backend does not support pull`). That is a fact
about the board, not a failure of the user's keystroke, so it is a toast rather than the sticky
error slot. And `w` on a card with no repository, on a board with no default one, opens the
repository picker first and creates the worktree from the same pick — one request, so the daemon
can never see the create before the repo it needs.

The third is **who owns a field**. On a board whose backend cannot write `priority` back, `p`
does not open a picker whose `Enter` could only fail: it says
`Priority is read-only on Jira (acli) boards` — a toast on the board, the sheet's own error line
in the card detail, beside the row that refused. The wording is assembled from the
backend's own two answers, the field list it declared and the label the registry gave it, so
`fleet-app` neither knows nor spells any backend's name.
