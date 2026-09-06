# Fleet — UX specification

**Status: authoritative.** This is the single document UI implementers follow for `fleet-app`
and `fleet-ui-kit`. It is the synthesis of the three lens proposals in `docs/ux/`
(`proposal-glanceability.md` as the base system, with the safety spine of
`proposal-background-safety.md` and the navigation/mode/toast layer of `proposal-flow-speed.md`
merged in). Where the proposals disagreed, this document decides; the decisions are marked
**[D-n]** and collected in §8.

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
   string that explains it. A pane whose data is frozen carries a `stale · <age>` stamp in its
   header. This closes §9's live defect: *"Local status failures can show existing session as
   `none`; only remote failure uses safer `unknown`"*.
4. **Four colors, all semantic, never decorative.** `green` = healthy / done / approved,
   `amber` = needs attention / in flight / unknown, `red` = broken / destructive, `blue` = *only*
   the cursor and the focus ring. Everything else is one of three neutrals. Draft, muted,
   disabled and "not applicable" are rendered by *lowering contrast*, never by adding a hue.
5. **Progressive disclosure with a stable frame.** The detail panel is closed by default (`i`)
   and never focusable; the filter replaces the pane header in place; dialogs are the only layer
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
   slot in the status bar (`!` focuses it). Successes get at most a 3.2 s toast, and only under
   the toast law (§2.7). Nothing blinks, nothing re-announces itself, nothing decays on a timer
   that the user did not set.
9. **The mode is always a visible word.** The app is modal with eight modes; a fixed word in the
   status bar prevents the single most expensive mistake in a modal app — typing a command into a
   PTY, or a PTY key into a list.
10. **Cut anything that does not change the next keystroke.** Every element below had to answer
    *which decision does this change?* The full data still exists behind `i`, `I`, `J`, `,` and `:`.

---

## 2. Global layout

### 2.1 Hub frame (1280 × 800, detail panel closed)

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│ ▍buk 1   personal 2   oss 3              ⟳2  ◉3  ☾5  ?1  ⚑4  ↑0.2.0          ◍  │ 36  context bar
├──────────────┬───────────────────────────────────────────────────────────────────┤
│ REPOS      6 │ WORKTREES · payroll                          8/12      1–8/12     │ 30  pane headers
│──────────────│───────────────────────────────────────────────────────────────────│
│▌◉ All     27 │ ◉ feat/payroll-fix ✎   buk/payroll  ⚡claude,:3000  #412 CI    2h ▏│ 30  rows
│  ◉ payroll 8 │ ☾ fix/rut-validator    buk/payroll                 #408 Appr   1d ▏│
│  · fleetd  3 │ · spike/gpui-vt ☁devbox dannyfuf/fleetd                       3d  │
│  ⟳ nixos     │ ▲ chore/deps           buk/www      ⚠ hooks failed             5d  │
│  ✕ old-api   │ ? api-poc  ☁devbox     buk/api      offline                   3d  │
│              │                                                                   │
│    240 px    │                     flex — 1039 px (138 ch)                       │
├──────────────┴───────────────────────────────────────────────────────────────────┤
│ buk › payroll › feat/payroll-fix   NORMAL   ⟳ clone nixos 40%  +1        ⚠ !     │ 26  status bar
└──────────────────────────────────────────────────────────────────────────────────┘
                                            ┌──────────────────────────────┐
                                            │ ✓ Path copied                │  toasts, bottom-right
                                            └──────────────────────────────┘
```

With the detail panel open (`i`) the worktrees list shrinks to **699 px (93 ch)** and a **340 px**
panel is inserted at the right; the repos rail never moves. Below **1120 px** total width the
detail panel becomes a right-edge overlay (320 px) so the list never drops below **72 ch**.

The Workspace replaces the rail + list + detail region entirely (full-bleed terminal) and keeps
the context bar and the status bar at the same pixel positions — same chrome, same saccade. A
**native tab** (§3.6) replaces the terminal grid with a Fleet-drawn pane in exactly that region
and changes nothing above or below it: there is one context bar, one status bar and one mode
word in the window, and they are the shell's. A pane that draws its own window chrome would put
two status bars on screen, which is why `crates/fleet-lazygit` has an embedded render path with
no `AppFrame`. The pane's own one-row key-hint bar is **not** window chrome — it is the pane's
content, like a list header — and it stays.

### 2.2 Persistent chrome

| What | Where | Size | Why here |
| --- | --- | --- | --- |
| Context tabs, numbered 1–9 | Context bar, left, x = 84 (traffic lights occupy 12–72 of the unified titlebar) | 36 px tall, tab = text + 8 px pad, 16 px gap | Contexts are the outermost coordinate and `1`–`9` / `gt` are the cheapest jump in the app, so the digits must be visible while you press them. |
| Active-tab marker | 2 px `blue` underline, full tab width | — | Blue always answers "where am I", never "how is it going". |
| Overflow chip | `+3`, `fg.faint`, after tab 9 | 22 px | Contexts past 9 are reachable by `gt`/`gT` and the palette only. |
| Status chips (§2.3) | Context bar, right, 12 px from the daemon dot, 8 px gaps | 22 px pills | The "is anything happening without me?" counters. Top-right is the OS status corner, far from the cursor. All zero-suppressed. |
| Daemon dot `◍` | Context bar, far right, 12 px inset | 8 px dot | Liveness of the process that owns every job and PTY. Always present, dot-only when healthy; expands to a labelled amber/red pill when not (§3.12). |
| Repos rail | Left, fixed **240 px** (drag 200–320, remembered) | full height | Second coordinate. Narrow and left because it is a *filter*, not content. |
| Pane header | Top of each pane | 30 px, label type, `fg.faint` | Carries scope, filter state, count and scroll position (§2.5), and hosts the filter bar with zero layout shift. |
| Status bar | Bottom, full width | 26 px | Breadcrumb · mode word · job ticker · sticky error slot. |
| Toast layer | Bottom-right, above the status bar, 320 px wide, 12 px insets | max 3 stacked | Only for events with no other home (§2.7). |
| Focus ring | 2 px `blue` inset on the focused pane; 2 px `blue` left bar on the cursor row | — | The only blue in the app. |

**Status bar slots, left to right:** breadcrumb `context › repo › row` (flex, truncate-middle) ·
**mode word** (fixed 84 px, centered, `fg.muted`, uppercase) · job ticker `⟳ <kind> <target> <pct>`
with `+n` when more run (flex, `fg.muted`) · **sticky error slot** (right, red, `⚠ <text> · !`,
persists until `!` or `Esc`, replaces the ticker when present).

### 2.3 Context-bar status chips (all zero-suppressed, in this fixed order)

| Chip | Icon | Color | Content | Source |
| --- | --- | --- | --- | --- |
| Jobs | `loader-circle` (spin) | amber | count of running jobs | `JobManager` |
| Failed jobs | `triangle-alert` | red | count of failed-and-unseen jobs; replaces the jobs chip's color, never a second chip | §1.8 |
| Live | `circle-dot` | green | count of `attached` sessions | `WorktreeStatus.session` |
| Sleeping | `moon` | `fg.muted` | count of `detached` sessions (awake or slept) | idem |
| Unknown / offline | `circle-help` | **amber** | count of `unknown` sessions **and** unreachable hosts | §1.3 — the count a stale or offline daemon must surface |
| Review | `flag` | `fg.muted` | count of PRs in the `review` tab | §5 header "review count" |
| Update | `arrow-up-circle` | `fg.faint` | `↑<version>` when an update is available | §5 `U` |

**[D-1]** The `unknown/offline` chip is mandatory: collapsing attached + detached into one chip
(as `proposal-glanceability` §3.1 did) drops exactly the count that a stale daemon needs to show.
**[D-2]** "Update available" is a chip and a Settings › About row — **never** a sticky toast. An
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
| agent finished | `circle-check` | green | `Agent finished — waiting for you` |
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
| whole pane frozen (daemon lost) | pane header gains `stale · <age>`; every session glyph forced to `circle-help` |

**[D-4] Auto-inspect cadence (resolves `proposal-glanceability` open question 3).** The daemon
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
| Background success whose row is off-screen | `Cloned buk/ledger · ⏎ opens` | 3.2 s | `circle-check` |
| Dialog-close reassurance | `⟳ base fetch still running · J` | 3.2 s | `loader-circle` |
| Action refused, with the reason | `Nothing to prune in payroll — 11 skipped · J for reasons` | 3.2 s | `info` |
| Duplicate action suppressed | `Already running` | 1.6 s | `info` |
| Mode no-op | `no scrollback in alt-screen` | 1.6 s | `chevrons-up` |

**Never a toast:** job started, job succeeded when its row is on screen, worktree created,
PR refreshed, context switched, session opened, settings saved, update available, **and any
error** — errors are sticky (§1.8), never transient.

Identical toast text within **1 s** coalesces into one toast with a `×2` suffix (this kills the
duplicate spam §6 attributes to overlapping operations with no duplicate suppression). Max 3
stacked; oldest evicted first.

### 2.8 Mode word

| Mode | Word | gpui key context |
| --- | --- | --- |
| Normal | `NORMAL` | `Hub`, `Hub > Repos`, `Hub > Worktrees`, `Hub > Prs` |
| Terminal | `TERMINAL` | `Workspace > Terminal` |
| Prefix (one-shot) | `^S` (amber) | `Workspace > Prefix` |
| Scroll | `SCROLL` | `Workspace > Scroll` |
| Filter | `FILTER` | `Filter` |
| Palette | `PALETTE` | `Palette` |
| Dialog | `DIALOG` | `Dialog > <name>` |
| Jobs overlay | `JOBS` | `Jobs` |

The word is 84 px wide, centered in the status bar, present on **every** screen including the
Workspace and including zoom (`ctrl-s z`).

### 2.9 Column ladders (inventory §5 breakpoints, authoritative in ch)

**Worktrees list**, measured in ch of the list pane (138 ch at default, 93 ch with detail open):

| # | Column | Width | Align | Shown when |
| --- | --- | --- | --- | --- |
| 1 | session / job glyph | 2 ch (16 px) | center | always |
| 2 | branch + `✎` dirty + `☁host` | flex, **min 24 ch** | left | always |
| 3 | repo `owner/name` | 14 ch (105 px), truncate-head | left | scope = `All`, **or** pane ≥ 110 ch |
| 4 | keep-alive labels / degraded chip | **18 / 14 / 10 / 0 ch** (135 / 105 / 75 / 0 px) | left | ≥ 104 / 88 / 72 / < 72 ch |
| 5 | PR badge `#n <state>` | 15 ch (113 px) | left | pane ≥ 60 ch and a PR matches |
| 6 | age | 7 ch (53 px) | right | pane ≥ 52 ch |

Below 72 ch only columns 1, 2, 5, 6 survive. Gaps are 12 px between columns, 16 px pane padding.

**PR list**, measured in ch of the list pane:

| # | Column | Width | Shown when |
| --- | --- | --- | --- |
| 1 | local-presence glyph | 2 ch (16 px) | always |
| 2 | number `#1234` | 6 ch (45 px), right | always |
| 3 | title | flex, min 32 ch | always |
| 4 | author | **12 ch @ ≥ 70 ch · 16 ch @ ≥ 130 ch** | `REVIEW` tab |
| 5 | `headRefName` | 12 ch (90 px), truncate-tail | pane ≥ 90 ch |
| 6 | repo `owner/name` | 10 ch (75 px) | pane ≥ 110 ch **and** scope is multi-repo |
| 7 | state badge | 8 ch (60 px) | always |
| 8 | age | 7 ch (53 px), right | pane ≥ 52 ch |

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
frozen the header appends `· stale · <age>` in amber (§1.3).

---

## 3. Screens

### 3.1 Hub — Context bar

**Purpose:** *Which slice of the world am I in, and is anything moving in it?*

```
 ▍buk 1    personal 2    oss 3           ⟳2  ◉3  ☾5  ?1  ⚑4  ↑0.2.0            ◍
 └ 2px blue underline on active
```

| Element | Content | Position | Why here |
| --- | --- | --- | --- |
| Context tab | `Context.name` + faint index digit | left, 12 px pad | Matches the `1`–`9` binding; first place a reader lands. |
| Active marker | 2 px `blue` underline | under active tab | Blue = "where am I". |
| Overflow | `+3` faint chip | after tab 9 | >9 contexts are `gt`/palette-reachable. |
| Status chips | §2.3, zero-suppressed | right | One saccade answers "is anything happening without me". |
| Daemon dot | §3.12 | far right | The process that owns every job and PTY. |

**Intentionally omitted:** context `owners` (Context dialog, Assign dialog and the detail panel),
`createdAt`, per-context repo counts (the rail counts them), a settings/help gear (`,` / `?`), a
separate window title bar — the GPUI window uses a 36 px unified titlebar that *is* this row.

**States:** *loading* → tabs render from the cached snapshot, chips absent. *no contexts* → the
tab row is replaced by faint `no contexts` and `N create your first context`. *daemon lost* →
§3.12; the chips freeze and the header of every pane gains `stale · <age>`.

**Icons:** `loader-circle`, `triangle-alert`, `circle-dot`, `moon`, `circle-help`, `flag`,
`arrow-up-circle`, `circle` (daemon dot is a filled 8 px dot, not an icon).

**Keyboard:** `1`–`9` jump · `gt` / `gT` cycle · `N` new context · `E` edit active context
(delete lives inside it, KEYMAP A15) · `:` palette for contexts past 9.

---

### 3.2 Hub — Repos rail

**Purpose:** *Scope the worktree list; is any repo unhealthy or still cloning?*

```
┌──────────────┐
│ REPOS      6 │ 30
├──────────────┤
│▌◉ All     27 │ 30   ← pinned pseudo-repo, default cursor
│  ◉ payroll 8 │
│  · fleetd  3 │
│  ☾ www     2 │
│  ⟳ nixos     │      clone in flight, count slot shows 40%
│  ✕ old-api   │      clone failed, stays until dismissed
└──────────────┘
     240 px
```

Row internals: `[12 pad][glyph 16][8][name flex, truncate-middle][8][count 3 ch = 22 right][12 pad]`.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| `All` pseudo-repo | literal `All` + total worktree count | row 0, pinned | swarm's default and the most common scope | §5 "All pseudo-repo default" |
| Aggregate session glyph | worst-of the repo's worktrees, ordered `unknown` > `attached` > `detached` > `none` | left of name | one shape per repo tells you where live work is | §5 "aggregate session glyph" |
| Repo name | `Repo.name`; `owner/name` **only** on collision | flex | you think in repo names | §5 "disambiguated owner/name" |
| Worktree count | integer, right, `fg.muted` | right | sizes the jump you are about to make | §5 "worktree count" |
| Clone row | `loader-circle` + name; the count slot shows `40%` when parseable | sort position | a repo being born must be visible where it will live | §1 `CloneJob`, §6 |
| Clone failed row | `circle-x` red + name + faint `failed` | same | failure must not disappear silently; `Enter` opens the Jobs panel focused on that job, `x` dismisses (KEYMAP arbitrates `d` = delete repo, `x` = dismiss a failed clone) | `CloneJob.status`, `.error` |
| Deleting row | dims to 40 %, `⟳ deleting`, non-selectable | in place | the rename-to-trash happens inside a state transaction | §3 delete |

**Intentionally omitted:** `url`, `path`, `defaultBranch`, `clonedAt`, hook lists, prepared-pool
state, private lock icon, owner avatar, a separate "live" count badge — all in the detail panel;
none of them changes which repo you select.

**States:** *empty* → `No repos in <context>.` + faint `n clone one` (verbatim §5).
*filter-empty* → `Nothing matches "<filter>".` *loading* → the rail renders from `state.json`
instantly, no skeleton; the reconcile shows only as the jobs chip.

**Icons:** `circle-dot`, `circle`, `moon`, `dot`, `circle-help`, `loader-circle`, `circle-x`,
`folder-git-2` (detail header only), `zap` (prepared copies, detail only).

**Keyboard:** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u` · `Enter`/`o`/`l` → focus worktrees · `n` clone
· `d` delete (confirm, cascades) · `x` dismiss a failed clone · `e` edit hooks (KEYMAP A16) · `m` move
to context · `i` detail · `ga` jump to `All` (KEYMAP A7) · `H` collapse/expand the rail.

---

### 3.3 Hub — Worktrees list

**Purpose:** *Which branch is alive, which needs attention, which can I throw away?*

```
 WORKTREES · payroll                                       8/12        1–8/12
 ───────────────────────────────────────────────────────────────────────────────
▌◉ feat/payroll-fix ✎     buk/payroll    ⚡claude, :3000    #412 CI fail    2h
 ☾ fix/rut-validator      buk/payroll                       #408 Approved   1d
 · spike/gpui-vt ☁devbox  dannyfuf/fl…                                      3d
 ▲ chore/deps             buk/www        ⚠ hooks failed                     5d
 ? api-poc ☁devbox        buk/api        offline                            3d
 ⟳ new-slug               buk/payroll    copying files…                      –
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session / job glyph | §2.5 | col 1 | leftmost = first read; it decides `Enter` vs `s` vs `d` | `WorktreeStatus.session` |
| Branch | `Worktree.branch` (not slug, not id), ellipsis-middle | col 2 | the only string the user thinks in | `Worktree.branch` |
| Dirty mark | `✎` `file-pen` 12 px amber, suffixed to the branch | inline | dirty is a property of the branch, so it rides with it instead of buying a column | `WorktreeInspection.dirty` |
| Host chip | `cloud` + host id, `fg.muted`; `cloud-off` amber when unreachable | inline after dirty | absent for local (the 95 % case) | `Worktree.host` |
| Repo prefix | `owner/name` | col 3 | disambiguates in `All` scope | §5 "optional repo prefix" |
| Keep-alive labels | `⚡` + `keepAlive` labels joined `, `, max 3 then `+n` | col 4 | tells you *why* sleep will refuse to close windows, and what `K`/`d` would kill | §4 sleep policy; `WorktreeStatus.windows[].keepAlive` |
| Degraded chip | `⚠ hooks failed` | **same slot, outranks keep-alive chips** | a worktree that looks ready but whose post-create hooks failed is a trap | §9 "Hook failures warn only; no persisted degraded fact despite ready" |
| Job phase | `copying files…`, `checking out…`, `hooks 2/3`, `deleting` | replaces col 4 + age | phases are more honest than percentages | §6 create/delete jobs |
| PR badge | `#412` + state word (§3.5) | col 5 | the single fact that decides "is this branch done?" | `InspectionPullRequest` |
| Age | relative `lastOpenedAt ?? createdAt`, 1 unit | col 6 | recency is the sort you verify visually | §5 |
| Sort | `lastOpenedAt` desc, then `createdAt` desc | — | MRU puts the right answer on row 0, so `Enter` alone is often the whole task | §3 open step 3 |

**Cursor stability.** Background events (status polls, PR fetches, job completions, pool refills)
never re-sort, re-scroll or re-focus the list. Sort order is recomputed only on explicit user
action (`r`, filter change, repo change, screen change). A row that changes state changes its
glyph **in place**.

**Intentionally omitted:** `WorktreeId` (never typed in the GUI), `path` (`y` copies it, detail
shows it), `baseRef`, session name string, window list, `ahead`/`behind`, `uniqueCommits`,
`published`, `mergedIntoTarget`, absolute timestamps, PR title, PR author, additions/deletions,
per-row action buttons. Every one of them appears in the detail panel or in the delete/prune
confirm — i.e. exactly where it changes a decision.

**States**

| State | Rendering |
| --- | --- |
| Empty | `No worktrees yet.` / `No worktrees for <repo> yet.` + faint `n create one` |
| Filter-empty | `Nothing matches "<filter>".` + faint `esc clear` |
| Loading (cold) | rows render from `state.json` immediately; the glyph column shows `circle-help` for at most one poll interval, then real states — **never blank** |
| Job running on a row | glyph → `loader-circle` (amber), col 4 + age → phase text; the row stays selectable and `Enter` opens it as soon as the session exists (§3 create step 6) |
| Deleting | row dims to 40 %, `⟳ deleting`, non-selectable, disappears on state commit |
| Inspect error | glyph unchanged, `triangle-alert` amber prefixed to the age column; the detail panel shows `WorktreeInspection.error` verbatim; the row stays operable |
| Host offline | `cloud-off` amber on the host chip, `offline` in col 4, session forced to `unknown` |
| Degraded | `triangle-alert` amber glyph + `⚠ hooks failed` chip; cleared by a successful re-run of the hooks job or by `d` on the chip in the detail panel |

**Icons:** `circle-dot`, `circle`, `moon`, `dot`, `circle-help`, `triangle-alert`, `file-pen`,
`cloud`, `cloud-off`, `zap`, `bot` (claude/opencode keep-alive), `server` (`:port` keep-alive),
`loader-circle`, `git-pull-request`, `git-pull-request-draft`, `git-merge`, `circle-x`,
`message-square-warning`, `clock`, `circle-check`, `eye`.

**Keyboard:** `j`/`k`/`gg`/`G`/`ctrl-d`/`ctrl-u` · `Enter`/`o` open (sleeps previous) · `O` open
keeping previous · `n` create · `d` delete · `x` prune repo · `s` sleep · `K` kill · `I` inspect ·
`i` detail · `y` copy path · `Y` copy branch · `b` browser · `u` undo last delete (KEYMAP A6) ·
`/` filter · `p` PRs · `J` jobs.

---

### 3.4 Hub — Detail panel (`i`, closed by default, never focusable)

**Purpose:** *Everything deliberately kept out of the row, on demand, in one 340 px column —
with the age of every job-derived fact.*

```
┌──────────────────────────────────────┐
│ ⑂ feat/payroll-fix                   │ 34  title
│   buk/payroll · origin/main · local  │ 20  subtitle
├──────────────────────────────────────┤
│ path   ~/.fleet/worktrees/buk/…/feat…│      y copies
│                                      │
│ SESSION                   ◉ attached │ 20  section header
│  1 nvim   ✎ unsaved changes          │
│  2 cc     ⚡ claude                   │
│  3 lg     —                          │
│                                      │
│ SAFETY              checked 14s ago  │ 20  section header + stamp
│  dirty           12 files            │
│  ahead / behind  ⇡3 ⇣0               │
│  unique commits  —                   │
│  published       no                  │
│  merged          no                  │
│  PR              #412 open · CI fail │
│  ⚠ unique commit count unavailable   │
│  ⚠ gh unavailable                    │
│                                      │
│ opened 2h ago · created 5d ago       │
│ inspected 4m ago · I refresh         │
└──────────────────────────────────────┘
            340 px, 12 px padding
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `git-branch` + branch | row 1 | echoes the cursor row so the eye does not re-search | — |
| Subtitle | `repoId` · `baseRef` · `local` \| `@host` | row 2, `fg.muted` | provenance, read once | `Worktree.repoId/.baseRef/.host` |
| `path` | tilde-collapsed, mono, middle-ellipsis | block 1 | `y` copies exactly this string; showing it makes the copy verifiable | §3 `path` |
| SESSION section | one row per terminal: `index name` + keep-alive label or `—` | block 2 | the only place window names exist; needed before `s` / `K` / `x` | `WorktreeStatus.windows[]`, §4 |
| SAFETY section | `dirty` (file count), `ahead`/`behind`, `uniqueCommits`, `published`, `merged`, `PR` | block 3 | the same facts the delete/prune confirm quotes, so the confirm is never a surprise | `WorktreeInspection`, §3 inspect |
| **Null facts** | `—` in `fg.faint`, **never `0`** | value column | §1.3 | `ahead`/`behind`/`uniqueCommits` are nullable |
| Warnings | amber `triangle-alert` + each `warnings[]` string **verbatim** | under SAFETY | swarm's 11 soft-warning strings are greppable diagnostics and must not be paraphrased | §3 inspect step 6 |
| Freshness stamp | `checked <age> ago` in the SAFETY header (amber > 2 min, red on error) | right of the header | a fact without an age is not a fact | `inspectedAt` |
| Times | `opened <rel> · created <rel>` | last block | low priority by definition | `lastOpenedAt`, `createdAt` |
| Footer | `inspected <age> · I refresh` | last row, only when > 60 s | prevents trusting stale safety facts | §2.6 |

**Variants**

| Cursor on | Panel content |
| --- | --- |
| Repo | name, owner, `defaultBranch`, `path`, worktree count, live count, `hooks.prepare` (count + commands), `hooks.postCreate`, `url`, prepared copies `1/1 ready · refreshed 2m ago` |
| Clone job | status, staging path, log path, `error` in red; `Enter` opens it in the Jobs panel |
| Context (rail header focused) | name, `owners` joined, repo + worktree counts |
| PR row | §3.5 PR detail table |

**Intentionally omitted:** `WorktreeId`, session name string, `host.ssh` / `swarmCommand`,
absolute ISO timestamps, raw hook command lists for worktrees (a repo-level fact), full log tails.

**States:** *never inspected* → SAFETY shows `not checked · I to check` and the delete confirm
escalates to `Y` (§3.8.3). *inspect running* → the section header shows `⟳ checking…`, old values
dim to 60 % but stay readable — **never blanked**. *inspect errored* → red `error: <message>` +
`I retry`.

**Icons:** `git-branch`, `clock` (freshness), `file-diff` (dirty), `git-commit-horizontal`
(unique commits), `cloud-upload` (published), `git-merge` (merged), `triangle-alert`, `circle-x`,
`zap`.

**Keyboard:** `i` toggles. The panel is **never in the focus cycle** (KEYMAP A1); `j`/`k` always move
the list cursor and the panel always mirrors it. `y` copies the path from anywhere in the Hub.

---

### 3.5 Hub — Pull requests screen (`p`)

**Purpose:** *What am I waiting on, what is waiting on me, and can I start on it in one key?*

```
 ▍buk 1   personal 2   oss 3                ⟳2  ◉3  ☾5  ⚑4                    ◍
 ────────────────────────────────────────────────────────────────────────────────
  MINE 7      REVIEW 4                                       fetched 40s ago  ⟳
 ────────────────────────────────────────────────────────────────────────────────
▌◉ #412  Fix RUT validation on payroll import   feat/rut-…   CI fail       2h
  ☾ #408  Bump lazygit to 0.44                  chore/deps   Approved      1d
  · #401  Draft: gpui vt spike                  spike/gpu…   Draft         3d
  ✕  could not load REVIEW: gh: HTTP 502 upstream connect error…    r retry
```

Columns and breakpoints: §2.9.

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Tabs `MINE` / `REVIEW` | label + count; active = 2 px blue underline | row under the context bar | `Tab`/`h`/`l` toggle them; the counts answer "how much is queued" without entering | `PrTab`, §5 |
| Fetch age | `fetched 40s ago` / `⟳ refreshing` / red error | same row, right | trust marker for cached data (`github.prTtlSeconds: 90`) | `PrRepoSlice.fetchedAt/loading/error` |
| Local-presence glyph | §2.5 glyph if a worktree matches, `dot` 30 % if not | col 1 | decides whether `Enter` *opens* or *creates* — the highest-value bit on this screen | §5 "local presence glyph"; match rule §1 |
| Number | `#{number}` | col 2 | the handle you say out loud | `PullRequest.number` |
| Title | single line, tail-truncated | col 3 | — | `PullRequest.title` |
| Author | `login` (null → `ghost`), `fg.muted` | col 4 | only meaningful in `REVIEW` | `PullRequest.author` |
| Branch | `headRefName` mono; `git-fork` prefix when `isCrossRepository` | col 5 | ties the PR to the branch you will get | `PullRequest.headRefName` |
| State badge | one icon + one word, priority draft → ci_fail → changes → ci_pending → approved → review | col 7 | collapses 3 raw fields into 1 glance | `PrState` priority, §1 |
| Age | relative `updatedAt` | col 8 | staleness of the PR, not of the fetch | `PullRequest.updatedAt` |

**PR state badge — exact text (≤ 8 ch), exact icon, exact color**

| `PrState` | Icon | Color | Text |
| --- | --- | --- | --- |
| `draft` | `git-pull-request-draft` | `fg.faint` | `Draft` |
| `ci_fail` | `circle-x` | red | `CI fail` |
| `changes` | `message-square-warning` | amber | `Changes` |
| `ci_pending` | `clock` | amber | `CI ···` |
| `approved` | `circle-check` | green | `Approved` |
| `review` | `eye` | `fg.muted` | `Review` |
| merged (from inspection) | `git-merge` | green | `Merged` |

**PR detail panel** (the §5 field list, enumerated — this was missing from all three proposals):

```
┌──────────────────────────────────────┐
│ ⇱ #412  Fix RUT validation on payro… │ 34
│   bukhr/payroll · dannyfuf           │ 20
├──────────────────────────────────────┤
│ target      main                     │
│ diff        +142 −18                 │
│ checks      fail · 2 of 9            │
│ review      changes requested        │
│ labels      payroll, needs-qa        │
│ updated     2h ago                   │
│ url         github.com/…/pull/412    │   y copies
│                                      │
│ WORKTREE                             │
│  path       ~/.fleet/worktrees/…     │
│  session    ◉ attached · claude      │
└──────────────────────────────────────┘
```

For a PR with **no** local worktree the `WORKTREE` block is replaced by `WILL CREATE`:

```
│ WILL CREATE                          │
│  worktree   payroll/feat-payroll-fix │   ← exact proposed destination
│  branch     feat-payroll-fix         │   same-repo: headRefName
│  base       pull/412/head            │   pull ref persisted as baseRef
│  fork       dannyfuf/payroll → pr/412│   cross-repo only: local branch pr/<n>
```

**[D-6]** The proposed destination is mandatory: it is what `Enter` is about to create, the
highest-stakes bit on this screen, and §5 requires it.

**Intentionally omitted from rows:** additions/deletions (a two-number stat is noise in a list;
detail shows `+142 −18`), labels (detail), `baseRefName` (detail), `isCrossRepository` (folded
into the `git-fork` prefix), `checks` and `reviewDecision` as separate columns (they *are* the
badge), `url` (`y` / `b`), reviewer avatars, a merged/closed section, a third "All" tab.

**States**

| State | Rendering |
| --- | --- |
| Loading with cache | cached rows stay at full opacity; the tab count becomes `…` and the header reads `⟳ refreshing · fetched 4m ago` |
| Loading cold | 6 skeleton rows at 30 % |
| Empty MINE | `No open PRs authored by you in <scope>.` + faint `r refresh` |
| Empty REVIEW | `No PRs waiting for your review in <scope>.` + faint `r refresh` |
| Error | a **sticky row** at the top of the affected tab: `✕ <error, 120 ch> · r retry`; **stale rows stay listed** and the fetch-age stamp turns amber. The same error also occupies the status-bar sticky slot (`!`). Never a toast (§2.7) |
| Creating a worktree from a PR | the presence glyph becomes `loader-circle`; the row does not move; leaving the screen does not stop the job |

**Scope.** The PR screen scopes to the selected repo, or to every repo of the active context when
`All` is selected. **[D-7]** In `All` scope the list is capped at **100** rows per tab, sorted by
`updatedAt` desc, with a final faint row `+n more — select a repo to narrow` (resolves
`proposal-glanceability` open question 4; the cap matches the §9 PR cap of 100 and never refuses).

**Icons:** the badge table above, plus `git-fork`, and the §2.5 session glyphs for presence.

**Keyboard:** `Tab`/`S-Tab`/`h`/`l` tabs · `j`/`k`/`gg`/`G` · `Enter`/`o` open-or-create (sleeps
previous) · `O` keep previous awake · `c` create without opening (KEYMAP A9) · `b` browser · `y` copy
URL · `r` force refresh both tabs · `I` inspect the matching local worktree · `i` detail ·
`/` filter · `p`/`q` back.

---

### 3.6 Workspace

**Purpose:** *Be a terminal. Say only which terminal I am in, whether the session is healthy, and
what is running elsewhere.*

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑂ feat/payroll-fix  buk/payroll  #412 CI fail       ◉  ⚡claude, :3000   ⟳1  │ 30  header
├─────────────────────────────────────────────────────────────────────────────┤
│  1 nvim ✎ │ 2 cc ⚡● │ 3 lg │ 4 test ✕1 │ +                                  │ 30  tab strip
├─────────────────────────────────────────────────────────────────────────────┤
│ ❯ claude                                                                    │
│ ⏺ Reading src/payroll/rounding.rb…                       ┌────────────────┐ │
│                                                          │ SCROLL 412/2000│ │ scroll pill
│                                                          └────────────────┘ │
│  ┌────┐                                                                     │
│  │ ^S │ s hub · 1-9 tab · c new · x close · [ scroll · w last session       │ prefix hint
│  └────┘  (appears only after 400 ms of hesitation)                          │
├─────────────────────────────────────────────────────────────────────────────┤
│ ⚠ process exited (1) · ^s r restart · ^s x close · ^s c new                 │ 22  only on exit
├─────────────────────────────────────────────────────────────────────────────┤
│ payroll/feat-payroll-fix          TERMINAL          ⟳ prune payroll      ◍  │ 26  status bar
└─────────────────────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Session header | `git-branch` + branch (`fg`) + `repoId` (muted) + `cloud host` if remote + PR badge if any | top-left | one line answering "am I in the right worktree?" — the #1 terminal error | `Session.kind = Worktree(id)` |
| Session state + keep-alive | §2.5 glyph + `⚡` labels | header, right | mirrors the Hub row so both screens read identically; these are what `sleep` keeps and `K` kills | `WorktreeStatus`, §4 |
| VT modes | `TerminalModes` glyphs for `alt` / `mouse` / `paste` / `appcur`, zero-suppressed | header, right, before the state glyph | the only explanation for the keymap appearing to lie; it sits in reserved chrome because a badge over the grid permanently hides output | `FrameUpdate.modes` |
| Jobs chip | `⟳n` (`⚠n` red when a job failed) | header, far right | the Hub chrome is not visible here, so background work must still be | §6 |
| Tab strip | `<index> <name>` per `Terminal`, min 84 / max 200 px, auto-sized | under the header | indexes are the argument to `ctrl-s 1`–`9`; the strip is the legend for that binding | `Session.terminals`, `active_terminal` |
| Active tab | `fg` text + 2 px blue bottom border | — | blue = "where am I" | — |
| Activity dot | 6 px amber `●` on an inactive tab with output since last visit | inside the tab | the only background-activity signal; prevents polling tabs by hand | `Terminal.status`, dirty-row events |
| Keep-alive icon on a tab | `bot` / `server` / `file-pen` | inside the tab, after the name | marks the tabs `sleep` will preserve, before you press `^s x` | §4 keep-alive rules |
| Exited tab | label at `fg.faint` + `circle-x` 12 px + exit code | — | a dead command must not look alive | `Terminal.status` |
| `+` tab | `plus` glyph | end of the strip | mouse parity for `ctrl-s c` | — |
| Native tab | `git-branch` glyph between the index and the name | inside the tab | the tab does not type what you press into a shell; the glyph is the only thing that says so before you try | `Terminal.kind = Native` |
| Terminal area | painted cell grid, 8 px padding, no border | fills | maximum rows; chrome is ≤ 86 px total | — |
| Native pane | the Fleet-drawn view for this tab, filling the terminal area exactly | replaces the grid | the tab is a tab: same header, same strip, same bars, same pixel positions | `Terminal.kind = Native`, `Worktree.path` |
| Scroll pill | `SCROLL <offset>/<scrollback_len>`, + a second line `v select · y yank · Esc exit` while selecting; 176 × 22 px, `bg.raised`, amber left bar | overlay, top-right **inside** the terminal area, 12 px inset | during scroll the eyes are on content; top-right never covers the prompt and never shifts the grid | `viewport{scrollback_len, offset}` |
| Prefix hint | `^S` pill + the 6 most-used prefix keys, mono 11 px, `fg.faint`, on `bg.raised` | bottom-left inside the terminal area, **delayed 400 ms** after `ctrl-s` | the expert types the second key in < 200 ms and never sees it; the returning user gets it exactly when they hesitate — 0 px and 0 frames of permanent cost | KEYMAP one-shot Prefix mode |
| Exit strip | `⚠ process exited (<code>) · ^s r restart · ^s x close · ^s c new` | bottom, 22 px, only when the tab's command exited | tmux's `remain-on-exit` made this recoverable; Fleet must not silently swallow a crashed dev server | §4 `remain-on-exit on` |
| Mode word | `TERMINAL` / `^S` / `SCROLL` | status bar, center | §2.8 | KEYMAP modes |

**[D-8]** Every Workspace command drawn over the Workspace states its prefix. In Terminal mode
keys go to the PTY except `ctrl-s` and the standard `cmd-c` / `cmd-v` clipboard actions, so
bare-key hints (`r restart`, `l log`, `⏎ start now`) are forbidden anywhere in this screen; they
are written `^s r`, `^s l`, `^s ⏎`.

**Zoom (`ctrl-s z`)** hides the session header and the tab strip; a 2 px amber bar on the window's
top edge remains as the only reminder that chrome is hidden. The status bar always stays, because
it carries the mode word and the daemon dot.

**Intentionally omitted:** the PTY window title (`FrameUpdate.title` names the *tab* only when the
terminal was never renamed), shell PID, `foreground_command`, cwd, cols × rows readout, latency
readout, a permanent key cheat sheet, a scrollbar, per-tab byte counters, pane splitting, per-tab
close buttons, a breadcrumb (the session name in the status bar is the breadcrumb).

**States**

| State | Rendering |
| --- | --- |
| Attaching | one dim centered line `attaching…`; typed keys are buffered and flushed on the first frame |
| Attached | normal |
| Waking a slept session | tabs rebuild with `loader-circle` per tab as each PTY spawns; the header reads `waking…` for ≤ 1.5 s |
| Recognized agent working / finished | the agent terminal shows an amber spinning `loader-circle` / green `circle-check`; the header uses the same aggregate state |
| Terminal exited | grid frozen at the last frame + the exit strip |
| Alt-screen app running | the scroll pill is **suppressed**; `ctrl-s [` shows the 1.6 s toast `no scrollback in alt-screen` |
| Native tab selected | the grid, the scroll pill and the exit strip are all absent — there is no PTY. `ctrl-s [` toasts `no scrollback in this tab`; every key except `ctrl-s` belongs to the pane. The mode word stays `TERMINAL`: §2.8 has eight words and this is still "the Workspace has the keyboard" |
| Job running for this worktree | `⟳n` in the header and the status-bar ticker; **never** an overlay on the grid |
| Daemon lost | grid dims to 55 %, keys are dropped (not buffered), and a 26 px amber banner replaces the header — §3.12 |

**Icons:** `git-branch`, `cloud`, `cloud-off`, `circle-dot`, `circle`, `moon`, `circle-help`, `circle-check`, `loader-circle`,
`zap`, `bot`, `sparkles` (opencode), `server`, `file-pen`, `plus`, `circle-x`, `square-terminal`,
`chevrons-up` (scroll pill), `command` (prefix pill), `maximize-2` (zoom hint), `unplug`.

The default third tab (`lg`) is a native tab: Fleet's own git UI (`crates/fleet-lazygit`) drawn
in the terminal area, created the first time the tab is selected and kept alive per worktree
until the daemon stops listing that worktree. Its own keys are documented in
`crates/fleet-lazygit/README.md`; `q` inside it selects the previous tab rather than quitting
Fleet. **[D-8] still holds**: the pane's key-hint bar lives *inside* the pane, which is its own
key context, so its bare keys are not "drawn over Terminal mode".

**Keyboard:** all keys → PTY except `cmd-c` copy selection and `cmd-v` paste — or, on a native
tab, every key except `ctrl-s` → the pane; `ctrl-s` then
`ctrl-s` (literal) · `s` hub · `1`–`9` tab ·
`h`/`l`, `p`/`n` prev/next tab · `Tab` last terminal tab (KEYMAP A2) · `w` last session (KEYMAP A3) ·
`W` session switcher (KEYMAP A4) · `S` sleep this session and return to Hub (KEYMAP A5) · `c` new tab ·
`x` close tab (confirm if a keep-alive process runs) · `,` rename · `[` scroll · `]` paste ·
`a`/`A` agent session · `r` restart the exited command (KEYMAP A10) · `y` copy worktree path (KEYMAP A11)
· `z` zoom · `!` sticky error slot (prefixed: `^s !`, KEYMAP A18) · `J` jobs · `?` help · `Esc` cancel
prefix.

---

### 3.7 Jobs panel (`J`)

**Purpose:** *What is the daemon doing for me, is it stuck, what failed, and what can I do about it?*

Right-docked sheet, **440 px** wide (**640 px** when a log is expanded), full height between the
context bar and the status bar, `bg.raised`, 1 px left border, 160 ms slide. The list behind stays
fully visible and readable — a centered modal would hide exactly the rows the jobs are about.

```
                              ┌──────────────────────────────────────────────┐
                              │ JOBS      ⟳2 running · ✕1 failed · ✓5 done   │ 30
                              │ ~/.fleet/logs/jobs/j-8f3c.log            y   │ 22
                              ├──────────────────────────────────────────────┤
                              │▌⟳ clone   nixos                 0:42     40% │ 44
                              │   Receiving objects: 40% (81/202)            │
                              │ ⟳ hooks   buk/payroll#feat-rut  0:08         │
                              │   pnpm install (2/3)                         │
                              ├──────────────────────────────────────────────┤
                              │ ✕ prs     review                1m       R   │ 44
                              │   gh: HTTP 502 upstream connect error        │
                              ├──────────────────────────────────────────────┤
                              │ ✓ prune   buk/www          deleted 2    12s  │ 30
                              │ ⊘ fetch   dannyfuf/fleetd  cancelled    —    │ 30
                              ├──────────────────────────────────────────────┤
                              │ ⏎ log · c cancel · R retry · y copy log path │ 24
                              │ f filter · D dismiss · X cancel all · Esc    │ 24
                              └──────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Header counts | `⟳n running · ✕n failed · ✓n done` | top | the count is why you opened it | `JobManager` |
| Log path | `~/.fleet/logs/jobs/<id>.log` of the selected job, verbatim, mono 11 px | header row 2 | makes the failure survivable **outside** the app; `y` copies it | ARCHITECTURE `FLEET_HOME` |
| Status glyph | `clock` queued · `loader-circle` running · `circle-stop` cancelling · `circle-slash` cancelled · `circle-check` done · `circle-x` failed | col 0 | — | `Job.status` |
| Kind | fixed 7-ch slug: `clone`, `pool`, `hooks`, `prune`, `create`, `delete`, `fetch`, `prs`, `inspect`, `update`, `import` | col 1, 56 px | fixed width makes the column scannable as a shape | §6 job table |
| Target | `RepoId` or `WorktreeId` — the real domain id | col 2, flex | swarm's footer got this wrong (`hot-copy:<repo>` matched no row) | `Job.target` |
| Elapsed | `m:ss`, shown after 30 s | col 3, 6 ch right | — | `Job` timestamps |
| Progress | percent when parseable | col 4, 4 ch right | — | last progress line |
| Progress sub-line | last stdout line, 1 line, `fg.muted` 11 px, tail-truncated | row 2 of the item | the single most reassuring artifact for a long job, and the only way to see a stuck clone | §6 "last progress line" |

**Log view.** `Enter` expands the sheet to 640 px and shows the tail of `logs/jobs/<id>.log`
(last 200 lines, mono 11.5 px, 16 ms batched — the existing budget maps 1:1). `f` inside the log
toggles follow; `j`/`k` scroll; `G` re-enables follow; `Esc` collapses back to 440 px.

**Retention. [D-9]** Failed jobs are **never** auto-dismissed: they stay until `D`, and hold the
red jobs chip and the status-bar sticky error slot until the panel has been opened. Succeeded and
cancelled jobs collapse to a single 30 px line after 10 s and are kept for
`jobs.keepFinishedFor` (Settings, default **10 min**, `0` = forever). Evidence of what ran never
decays on a timer the user did not set.

**Intentionally omitted:** job ids in the row (they are in the log path and on `y`), queue
position, concurrency limits, per-job PID, absolute start timestamps, progress bars for
non-percent jobs, pool "skip-if-fresh" no-ops (logged, not listed).

**States:** *empty* → centered faint `Nothing running.` + `Jobs and sessions live in fleetd, so
they survive closing this window.` (the product promise, stated once, in the one place it is
being demonstrated). *daemon down* → amber strip at the top: `The daemon is unreachable — job
state is from <age> ago`, rows still readable.

**Icons:** `activity` (panel title), `loader-circle`, `circle-check`, `circle-x`, `circle-slash`,
`circle-stop`, `clock`, `cloud-download` (clone), `copy-plus` (pool), `terminal` (hooks),
`scissors` (prune), `trash-2` (delete), `refresh-cw` (fetch/prs), `search-check` (inspect),
`arrow-up-circle` (update), `import` (import).

**Keyboard:** `J` toggle (also `ctrl-s J` from a terminal) · `j`/`k` · `gg`/`G` · `Enter` expand
log · `c` cancel (only when `cancellable`) · `X` cancel every cancellable job (confirm) ·
`R` retry a failed job with identical parameters · `y` copy the log path · `D` dismiss finished
and failed · `f` cycle filter all → running → failed · `Esc`/`J` close and **restore the exact
prior focus** (pane, row, terminal and mode).

---

### 3.8 Dialogs — shared frame

Every dialog: centered, `bg.raised`, 12 px radius, 1 px `border`, shadow `0 16px 48px rgba(0,0,0,.45)`,
backdrop = the base screen at 45 % opacity with an 8 px blur ("ghosts base", §5). **Header 44 px**:
icon + title, no close button (`Esc`). **Footer 44 px**: left = contextual key hints in `fg.faint`
mono 11 px, right = the primary action label only (`⏎ Create`). **No OK/Cancel button pair
anywhere** — the hint row states the keys, and this is a keyboard app.

| Dialog | Width × height | Icon |
| --- | --- | --- |
| Create worktree | 560 × auto (≈ 380) | `git-branch-plus` |
| Clone repo | 560 × 420 | `cloud-download` |
| Confirm — compact | 480 × auto (≈ 160) | `trash-2` / `scissors` / `power` / `x` |
| Confirm — expanded | 560 × auto (≈ 300) | `triangle-alert` |
| Confirm — prune | 720 × auto (≈ 420) | `scissors` |
| New / Edit context | 460 × 260 | `boxes` |
| Assign repo to context | 460 × 340 | `arrow-right-left` |
| Settings | 720 × 560 | `settings-2` |
| Help | 880 × 620 | `circle-question` |
| Quit (`ctrl-q`) | 520 × auto | `circle-question` |
| Quit + stop daemon | 560 × auto | `power` |

Dialog text inputs follow KEYMAP exactly: printable keys, `Backspace`, `ctrl-w`, `ctrl-u`,
`ctrl-a`/`ctrl-e`, `←`/`→`; lists **under a text field** use `ctrl-n`/`ctrl-p` or `↓`/`↑` and
never `j`/`k`. **A dialog with no text field (Assign, Settings list, Confirm) does bind `j`/`k`.**

---

#### 3.8.1 Create worktree (`n` in the worktrees pane)

**Purpose:** *Name a branch, pick a base, go — with the latency and the hooks visible before I commit.*

```
┌──────────────────────────────────────────────────────────┐
│ ⑂+ New worktree · buk/payroll                            │ 44
├──────────────────────────────────────────────────────────┤
│ Branch                                                   │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ feat/rut-validator                                  ▏│ │ 36
│ └──────────────────────────────────────────────────────┘ │
│ → buk/payroll#feat-rut-validator                         │ 18  faint slug + id preview
│                                                          │
│ Base                                      ⟳ fetching     │
│ ┌──────────────────────────────────────────────────────┐ │
│ │▌origin/main                                  default │ │ 28 × 6
│ │ origin/release-2026                                  │ │
│ │ origin/feat/payroll-import                           │ │
│ │ pull/412/head                       (previous base)  │ │
│ └──────────────────────────────────────────────────────┘ │
│                                                          │
│ Host   ◂ local ▸                                         │ 28  only if hosts configured
│                                                          │
│ ⚡ prepared copy ready — create takes ~2 s                │ 18
│ hooks: pnpm install · pnpm build     (run in background) │ 18
├──────────────────────────────────────────────────────────┤
│ ⇥ field · ⌃n/⌃p base · esc cancel          ⏎ Create      │ 44
└──────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here | Why needed |
| --- | --- | --- | --- | --- |
| Title | `New worktree · <repoId>` | header | the repo is already chosen; restating it prevents the #1 mistake | §3 create |
| Branch input | free text, `validateBranch` live, auto-focused | field 1 | the only always-typed input | §1 branch rules |
| Slug + id preview | `→ <repoId>#<slugify(branch)>` faint | under the input | shows the id **and** the directory you will get, without a second field | §1 slug rules |
| Validation | red line replacing the preview with the exact failing rule, e.g. `branch cannot contain ".."` | same slot | zero layout shift; fail before a job starts | §1 `validateBranch` reject list |
| Base list | up to **6** rows: `origin/<defaultBranch>` first and preselected, then the previous `baseRef`, then `origin/*` fuzzy-filtered by the typed text; free text accepted | field 2 | 6 is swarm's number and fits without scrolling | §5 Create dialog |
| `default` tag | faint, right-aligned on the default row | — | one word instead of a "use default" control | — |
| Fetch indicator | `loader-circle` + `fetching` at the section's right | — | the list may grow under you; say so, and never block `Enter` | §5 "fetching indicator" |
| Host cycler | `◂ local ▸`, hidden when `config.hosts` is empty | field 3 | zero-suppressed for the local-only majority | `defaultHost` |
| Expectation line | `⚡ prepared copy ready — create takes ~2 s` **or** `⧗ no prepared copy — the first create copies the repo (~40 s) in the background` | above the footer | the pool's only user-visible consequence is latency; saying it decides whether the user waits or switches away | §1 prepared-copy slots; §6 |
| **Hooks preview** | `hooks: <prepare · joined> · <postCreate · joined>` + `(run in background)`; `hooks: none` when both are empty | above the footer | post-create hooks run detached and can fail *after* the worktree looks ready; naming them here is what makes the later `⚠ hooks failed` chip intelligible | §3 create step 6; §9 |

**Intentionally omitted:** slug as an editable field (derived; the palette command
`create with custom slug` covers the rare case), `--url` / `--default-branch` / `--hooks`
(CLI-only), a "run post-create hooks" checkbox (always on), a "wait for hooks" checkbox, a
"fetch base first" checkbox (the daemon's freshness rules decide), a progress bar (the dialog
closes on `Enter`), a repo selector (the rail selection is the repo; from `All` it is the repo of
the highlighted worktree, falling back to a repo picker only when the list is empty).

**States:** *submitting* → the dialog closes in < 16 ms, a pending `⟳` row appears in the list and
a `create` job appears in the ticker and the Jobs panel. *duplicate id* →
`buk/payroll#feat-rut-validator already exists — ⏎ opens it` (turning §3's idempotency rule into
a shortcut). *conflict* (existing id with a different explicit `--branch`/`--host`) → the exact
conflict message in red in the footer, dialog stays open. *closed while a base fetch is running*
→ the fetch **keeps running** and a 3.2 s toast says `⟳ base fetch still running · J`
(retires §9 "create-dialog close does not cancel forced fetch").

**Icons:** `git-branch-plus`, `loader-circle`, `zap`, `hourglass`, `server` (host), `terminal` (hooks).

**Keyboard:** `Tab`/`S-Tab` fields · `ctrl-n`/`ctrl-p` or `↓`/`↑` base list · `←`/`→` host ·
`Enter` create & open · `⌥Enter` create without opening (KEYMAP A8) · `Esc` cancel (jobs keep running).

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
│ ⌃n/⌃p · esc cancel · ssh · clones in the background  ⏎ Clone │
└──────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| Target context | `into context "<name>"` in the header | header | the destination is otherwise invisible and cloning into the wrong context costs a later `m` | `Repo.contextId` |
| Search input | 150 ms debounce, auto-focused | top | search is the whole dialog | §5 Clone |
| Results | **8** rows max, 2 lines: `lock`/`globe` + `fullName` + relative `updatedAt`; description `fg.muted` 11 px | list | 8 is swarm's cap; `updatedAt` disambiguates forks and dead mirrors | §5, `RemoteRepo` |
| Empty description | row collapses to a 22 px single line | — | zero-suppression (`description` null → `""`) | §1 |
| Protocol + background note | `ssh · clones in the background` | footer | tells the user `Enter` frees them immediately | `github.cloneProtocol`; §6 "survives popup" |

**Intentionally omitted:** stars / forks / language, a clone-protocol picker (Settings), the full
SSH URL, avatars, a manual URL field (an `owner/name` or a pasted URL typed into the query is
detected and offered as the first row), a destination-path field (`reposDir` by contract).

**States:** *idle* → faint `Type to search GitHub repos in buk's owners.` *searching* →
`loader-circle` replaces the magnifier in place, previous results kept. *no results* →
`Nothing matches "<query>".` *gh failure* → a red one-line row with the gh error + `r retry`;
the input stays live. *offline* → the last cache is shown with `cached <age>` in amber.
*submitted* → the dialog closes instantly and a `⟳` row appears in the rail from the moment the
`CloneJob` is persisted, before the child process starts (§6).

**Keyboard:** type · `ctrl-n`/`ctrl-p` or `↓`/`↑` · `Enter` clone · `Esc` cancel (aborts only the
search request, never a started clone).

---

#### 3.8.3 Confirm — delete / prune / kill / close terminal

**Purpose:** *Show me exactly what I will lose, in facts, with their age.*

Two sizes, chosen by the facts. This is §1.7 made literal.

**Compact** — every decisive fact is known **and** benign:

```
┌────────────────────────────────────────────────────────┐
│ Delete buk/payroll#fix-rut-validator?                  │
│ ✓ clean   ✓ merged into origin/main   ✓ no session     │
│ checked 8s ago · I re-check · trash, then removed      │
│                                     y delete · n cancel│
└────────────────────────────────────────────────────────┘
```

**Expanded** — any risk fact is true, **or** any decisive fact is unknown:

```
┌ ⚠ Delete worktree ─────────────────────────── 560 px ─┐
│ buk/payroll#feat-payroll-fix                          │
│                                                       │
│ ⚠ 12 uncommitted files                                │
│ ⚠ 3 commits not on origin/main                        │
│ ⚠ session attached · claude, :3000 running            │
│ ⚠ unique commit count unavailable (gh unavailable)    │
│ ✓ PR #412 open (not merged)                           │
│                                                       │
│ checked 3m ago · I re-check                           │
│ Deleting kills the session and moves the copy to      │
│ trash; commits that exist only here are lost.         │
├───────────────────────────────────────────────────────┤
│ I re-check · n cancel                       Y  Delete │
└───────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why here |
| --- | --- | --- | --- |
| Target | full `WorktreeId` (compact: in the title; expanded: row 1) | top | deleting the wrong copy is the top failure mode |
| Fact list | one line each — dirty (with file count), unique commits, published, merged/PR, session + running labels | body, `⚠` risks first, `✓` safe after | ordered so the eye lands on the reason to stop |
| Unknown facts | `⚠ <exact inspect warning string>` | with the facts | §1.3, and it explains why the key is `Y` |
| **Freshness** | `checked <age> ago · I re-check` | above the consequence line | **mandatory on every facts confirm** — the confirm is the only place where freshness decides an outcome |
| Consequence | plain future tense, irreversibility and background-ness | above the key row | users confirm the *sentence*, not the title |
| Key row | `y` or `Y` confirm · `n` cancel · `I` re-check | footer | KEYMAP confirm convention |

**Escalation rule. [D-10]** `y` confirms when every decisive fact (`dirty`, `uniqueCommits`,
`published`, `session`) is **known**. `Y` (shift) is required when any is unknown or the
inspection errored — KEYMAP already states "lowercase = safe action, uppercase = stronger
variant". `Enter` is accepted wherever `y` is; `Enter` is **not** accepted where `Y` is required.

**Exact wording per action**

| Action | Title | Consequence line | Key |
| --- | --- | --- | --- |
| Delete worktree, clean | `Delete buk/payroll#fix-rut-validator?` | `Moves the copy to trash, then removes it in the background.` | `y` |
| Delete worktree, risky | `Delete worktree` | `Deleting kills the session and moves the copy to trash; commits that exist only here are lost.` | `y` / `Y` |
| Delete repo | `Delete repository buk/payroll?` | `Also deletes 8 worktrees and their sessions. The base clone and every copy go to trash.` | `Y` |
| Delete context | `Delete context "buk"?` | `Also deletes 4 repositories, 23 worktrees and every session in them.` | `Y` |
| Prune | `Prune buk/payroll — 3 of 8 worktrees` | `Deletes the 3 listed below. The 5 skipped ones are kept, with the reason shown.` | `y` |
| Kill session | `Kill session payroll/feat-payroll-fix?` | `Kills 3 terminals at once. nvim has unsaved changes; claude and the server on :3000 are killed too. Nothing is saved.` | `Y` when unsaved or keep-alive present, else `y` |
| Close terminal | `Close terminal 2 "cc"?` | `claude is running in it and will be killed.` | `y` |
| Sleep | *no confirm* | toast `Slept · kept cc (claude)` | — |

**Prune dialog** — a multi-target confirm, so it gets its own body:

```
┌ ✂ Prune buk/payroll — 3 of 8 ─────────────────────────── 720 px ─┐
│ DELETE                                                           │
│  ✓ fix-rut-validator    merged · clean · no session              │
│  ✓ chore-deps           merged · clean · no session              │
│  ✓ spike-cache          merged · clean · slept                   │
│ KEEP                                                             │
│  ⚠ feat-payroll-fix     2 unique commits, not merged             │
│  ⚠ api-poc              session attached                         │
│  ⚠ hotfix-vat           12 uncommitted files                     │
│  ⚠ old-spike            session has running commands: claude, :3000│
│  ? devbox/ledger-sync   status unknown (host offline)            │
│  ⚠ legacy-import        unique commit count unavailable          │
├──────────────────────────────────────────────────────────────────┤
│ dry run · fetched 6s ago                     y prune 3 · n cancel│
└──────────────────────────────────────────────────────────────────┘
```

The KEEP reasons are the **verbatim** skip reasons of §3 prune (`error`, `dirty`,
`attached/unknown session`, `unknown required unique count`, `unmerged`, running labels —
swarm's `tmux session has running commands: …` becomes `session has running commands: …`, the
only rewording, because there is no tmux). The `? host offline` row is mandatory: it is exactly
the case where an automated prune must be **seen** to have refused. The footer stamp
`dry run · fetched <age>` is mandatory.

**Intentionally omitted from every confirm:** a "don't ask again" checkbox (the *compact* form is
the real answer to confirm fatigue), a second "are you sure" step, a countdown or a disabled
button delay, a typed-name confirmation (typing trains people to type), diff previews, the
worktree path.

**States:** *facts loading* → values render as `…` and the dialog is confirmable with `Y` only;
a background `inspect --no-fetch` swaps values in place with a 120 ms highlight. *facts errored* →
amber line with the exact warning text and `Y`. *prune dry-run running* → the body shows
`⟳ checking 8 worktrees…` and `y` is inert (not styled disabled — it simply does nothing until
facts exist). *nothing eligible to prune* → the dialog does **not** open; a 3.2 s toast says
`Nothing to prune in payroll — 11 skipped · J for reasons`.

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
│ ⇥ field · esc cancel               ⏎ Create │
└────────────────────────────────────────────┘
```

`Name` → live `ContextId` preview under the field (§1 slugify rules). `Owners` is comma-separated;
its one-line explainer is the **only** teaching copy in the app, because `owners` is the single
field whose purpose is not guessable.

**Edit variant** (`E`, KEYMAP A15): title `⬚ Edit context "buk"`, the id shown read-only and faint
(read-only outright once repos exist), and **context delete lives here** as `ctrl-d` → the
expanded confirm. **[D-11]** `D` therefore keeps its KEYMAP-defined meaning as *delete active
context* but is **routed through the same expanded confirm with `Y`**; `E` + `ctrl-d` is the
discoverable path. Rationale for not unbinding `D`: KEYMAP is authoritative and a spec must not
silently retire a documented binding; the risk is handled by `Y` escalation, the fact list and
the trash undo (`u`), not by hiding the key.

**States:** duplicate id → `A context with id "buk-hr" already exists.` and `Enter` inert. Empty
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
│ j/k · ⌃n/⌃p · esc cancel           ⏎ Move  │
└────────────────────────────────────────────┘
```

Rows: `Context.name` (`fg`) + `owners` joined (`fg.muted`, truncate) + faint `current` tag.
Owners are shown here specifically because they are the reason a repo belongs to a context.

**[D-12]** This dialog has **no text field**, so KEYMAP's "never `j`/`k` under a text field" rule
does not apply: `j`/`k`, `↓`/`↑` and `ctrl-n`/`ctrl-p` all move the selection, exactly as
inventory §5 records for swarm's Assign dialog.

The footer sentence is mandatory: the action *sounds* destructive and is not.
**Omitted:** repo counts, ids, created dates.

---

#### 3.8.6 Settings (`,`)

720 × 560, two columns: a 180 px section rail and a 540 px pane. Editable fields carry a
normal-contrast value in an input box; read-only facts are `fg.muted` **with no input chrome** —
the absence of a box is how "you cannot edit this here" is said, instead of a disabled style.
Each section shows a faint trailing `edit in config.json` **once**, not per row.

| Section | Rows |
| --- | --- |
| **General** | `Agent ◂ claude ▸` · `Claude command [claude]` · `OpenCode command [opencode]` |
| **Sleep** | `Sleep on switch [x]` · `Grace ms [2000]` (editable, clamped ≥ 0) · rule list, each `[x] <label>  <kind>  <pattern>` **plus a live match count** `claude — matching 2 processes now` · invalid regex → red `invalid pattern — rule is skipped` |
| **Jobs & warnings** | `Warn before quitting with running jobs [x]` · `Keep finished jobs for ◂ 10 min ▸` · `Trash retention ◂ 10 min ▸` |
| **Pool** | `Hot pool size ◂ 1 ▸` · `Freshness ms [60000]` · `Refresh interval ms [300000]` · read-only `prepared copies: 1/1 ready` |
| **GitHub** | `Clone protocol ◂ ssh ▸` · `Repo cache s [3600]` · `PR cache s [90]` |
| **Status** | `Local status refresh ms [2000]` (min 500) · `Remote status refresh ms [10000]` (min 500) |
| **Windows** | read-only ordered list `1 nvim — nvim .` / `2 cc — {agent}` / `3 lg — lazygit` |
| **Hosts** | read-only per host `devbox — ssh danny@devbox — fleet` |
| **About** | `Fleet 0.1.0+<sha>` · update row `Fleet 0.2.0 available · U` (§2.3) · `fleetd running · pid 4211 · up 3h` · `FLEET_HOME ~/.fleet` · `protocol 4` · `E open config.json in a new terminal tab` · `Run doctor · D` |

**[D-13]** The editable set closes §9's *"Settings cannot edit grace/rule definitions/windows/
hosts/protocol/pool/timers/status intervals; many require JSON"* for everything a user changes
more than once a year. `windows` and `hosts` stay read-only in v1 because both are ordered/keyed
structures whose editor is a whole screen; `E` (open `config.json` in a terminal tab) is the
escape hatch and is one key. The **`Warn before quitting with running jobs`** toggle is the
mechanism that makes KEYMAP's `ctrl-q` clause implementable at all (§3.8.9).

**States:** saving is synchronous and silent (never a toast, §2.7); a failed write shows a red
footer line with the exact error and keeps the dialog open. Dirty state marks the title
`⚙ Settings ·` in accent and the footer becomes `⏎ save · esc discard changes`.

**Keyboard:** `j`/`k`, `↓`/`↑`, `ctrl-n`/`ctrl-p` move (`j`/`k` are surrendered while a text input
has focus) · `Space` toggles · `←`/`→` cycles a choice · `Enter` saves · `Esc` cancels.

---

#### 3.8.7 Help (`?`)

880 × 620, **three columns × ~14 rows**, grouped by *mode* because the app is modal:
`Hub` · `Worktrees & PRs` · `Terminal (^s)` · `Scroll` · `Dialogs & filter`. Keys in a 68 px mono
`fg` column, action in `fg.muted`. Context-sensitive: opening `?` from a terminal renders the
`Terminal (^s)` column **first and in accent** and dims the others to 55 %.

The dialog opens with one block above the columns, which is the single most valuable paragraph in
the app:

> **What keeps running.** Jobs and sessions live in fleetd. Closing a dialog, leaving a screen or
> quitting Fleet (`ctrl-q`) never stops them. Only `c` in the Jobs panel, `K`, and `ctrl-shift-q`
> stop things. Terminals do not survive a **daemon** restart.

Footer: `Fleet <version> · protocol 4 · fleetd up 3h`.
**Omitted:** prose explanations, links, a search field (the palette *is* the searchable surface).

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
│ J jobs · W never warn again          y quit · n cancel│
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
│   ⟳ hooks payroll#feat-rut     (not restartable)      │
│ 1 job keeps running: post-create hooks (detached)     │
│                                                       │
│ Worktrees, repos and state on disk are untouched.     │
│ ctrl-q quits Fleet and leaves all of this running.    │
├───────────────────────────────────────────────────────┤
│ n cancel                            Y  Stop and quit  │
└───────────────────────────────────────────────────────┘
```

Cancellable vs. not comes from the job's cancel token; a detached post-create runner (§6 "no
cancel") is listed under a fourth group `n job(s) keep running` and labelled `not restartable`
rather than pretending. The `ctrl-q` line is the important one: the confirm teaches the safe
alternative instead of only threatening. With nothing running, `ctrl-shift-q` does not confirm.

---

### 3.9 Command palette (`:`)

**Purpose:** *Jump to anything by name, or do the thing whose key I do not remember.*

**640 px** wide, top-anchored at **y = 120** (thinking position, not screen center), 44 px input,
up to **10** rows × 34 px, sections in the fixed order `GO` → `DO` → `CONTEXT`.

```
┌──────────────────────────────────────────────────────────────┐
│ : pay fix                                                   ▏│ 44
├──────────────────────────────────────────────────────────────┤
│ GO                                                           │ 20
│▌◉ payroll#feat-payroll-fix          session attached         │ 34
│  ☾ payroll#fix-rut-validator        sleeping                 │
│  ⇱ #412 Fix RUT validation…         PR · mine                │
│  🗀 buk/payroll                      repo                     │
│ DO                                                           │ 20
│  ✂ Prune worktrees · buk/payroll                          x  │ 34
│  ⌫ Cancel job: clone nixos                                J  │
│  ⤓ Clone repo                                             n  │
│ CONTEXT                                                      │ 20
│  ⬚ personal                                               2  │
├──────────────────────────────────────────────────────────────┤
│ 9 of 63 · ⏎ run · esc cancel                                 │
└──────────────────────────────────────────────────────────────┘
```

| Element | Content | Position | Why | Cite |
| --- | --- | --- | --- | --- |
| `GO` section, **first** | worktrees (with their §2.5 session glyph), open PRs, repos — objects, ranked above commands | section 1 | makes a session reachable from *inside another session*: `ctrl-s s` `:` `pay fix` `⏎` with no list scan | extends §5's "commands + contexts" palette |
| `DO` section | only **valid** commands, with the bound key **right-aligned** on every row | section 2 | right-aligned keys teach the shortcut every time, so the palette trains itself out of the loop | §5 "label/keys" |
| Job entries | running/failed jobs as `Cancel job: <kind> <target>` | in `DO` | a background action is reachable without learning the panel | §6 |
| `CONTEXT` section | `Switch to context: <name>` + its digit | section 3 | swarm mixes commands and contexts | §5 |
| Icon | the same Lucide glyph the action or object uses elsewhere | col 1 | teaches the icon language; palette rows and list rows read identically | — |
| Cap | 10 rows total, fixed section order | — | a fixed maximum keeps `Enter` predictable — the top match never moves below the fold | §5 "first 10" |
| Destructive commands | prefixed with `triangle-alert`, and still routed through their confirm dialog | — | the palette never bypasses a confirm | §1.7 |

**States:** empty query → `GO` shows the 5 most-recently-opened worktrees, `DO` the 5 most-used
commands. No match → `Nothing matches "<query>".` A command invalid in the current context is
**not listed at all** — never greyed, because a greyed row costs a `j`.

**Omitted:** section descriptions, command history, a shell escape, multi-select, fuzzy-match
highlighting beyond a subtle weight bump.

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
| `search` icon | 14 px `fg.muted` | replaces the pane label | signals the mode without a mode word in the pane (the status bar carries `FILTER`) |
| Query | live text, blue caret | inline | — |
| Match count | `<shown>/<total>` | right | tells you whether to keep typing |
| `esc` hint | faint, right of the count | right | the two-stage `Esc` is non-obvious |
| Retained chip | `⌕rut` in accent inside the restored header, with a blue dot | same row | a hidden active filter is the classic "where did my rows go" bug |

**Keyboard:** printable · `Backspace` · `ctrl-w` · `ctrl-u` · `ctrl-n`/`↓` and `ctrl-p`/`↑` move
the list cursor **while still typing** · `Enter` opens the selected row (so `/rut⏎` is a complete
open in 5 keys) · first `Esc` leaves the input keeping the filter · second `Esc` clears it.
**[D-15]** `Esc` in the Hub **never quits the app** — swarm's "clear filter, else quit" is
retired (KEYMAP A13).

**States:** no match → the list area shows `Nothing matches "<filter>".` + faint `esc clear` and
`Enter` is inert; the header keeps `0/12`. A filter survives a refresh; it does **not** survive a
repo change or a screen change.

---

### 3.11 Toasts

Rendering: bottom-right, above the status bar, **320 px** wide, 12 px insets, max **3** stacked,
**3.2 s** (1.6 s for instant-action acknowledgements), slide + fade 140 ms. One line, one icon,
no title, no close button. Contents and the governing law: **§2.7**.

---

### 3.12 Daemon states, degraded states and Doctor

**[D-16]** Three *distinct* daemon situations. Conflating them is the failure mode, and only one
of them is about reconnecting.

| Situation | Surface | Exact text | Keys |
| --- | --- | --- | --- |
| **A. Cold start, daemon not yet up** | full window, centered, no chrome | `Starting fleetd…` + spinner; after 3 s it appends `~/.fleet/fleetd.sock` | none (auto-spawn) |
| **B. Daemon will not start** | full window, centered | `fleetd could not start.` · the last 3 lines of `~/.fleet/logs/fleetd.log` in mono · `The socket ~/.fleet/fleetd.sock is stale.` when that is the cause | `r` retry · `L` open log · `D` run doctor · `ctrl-q` quit |
| **C. Daemon died while attached** | 28 px amber banner under the context bar; the daemon dot turns red; terminal grids get a 55 % veil | `◍ fleetd stopped · reconnecting in 3s` — the countdown cycles `3s → reconnecting… → 6s` (backoff 1, 2, 4, 8 s, capped 8 s) | `r` reconnect now · `l` open log · `Esc` dismiss the banner (the dot stays red) |

**On reconnect after C**, the banner turns amber for 6 s (not green, not 800 ms) and reads,
verbatim:

> `fleetd restarted. Terminal sessions did not survive; worktrees, jobs and state are intact.`

**[D-17]** This sentence is mandatory. `ARCHITECTURE.md` §Sessions is explicit — *"PTYs do not
survive a daemon restart (like a tmux server)"* — and a warm banner that implies the agents came
back is the single most damaging false reassurance in the app. If the daemon merely dropped the
*connection* without dying (the PTYs are still alive), the banner instead reads `reconnected` and
leaves after 800 ms.

**While disconnected (case C):** the lists stay at 100 % opacity and remain navigable — they are
true, just frozen — every pane header gains `· stale · <age>`, **every** session glyph is forced
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
| No worktrees | `No worktrees yet.` | `n  create one` |
| No worktrees for a repo | `No worktrees for <repo> yet.` | `n  create one` |
| Filter miss | `Nothing matches "<filter>".` | `esc  clear` |
| PR mine | `No open PRs authored by you in <scope>.` | `r  refresh` |
| PR review | `No PRs waiting for your review in <scope>.` | `r  refresh` |
| Jobs | `Nothing running.` | `Jobs and sessions live in fleetd, so they survive closing this window.` |
| Terminal exited | `process exited (<code>)` | `^s x  close    ^s c  new    ^s r  restart` |

**First run** (no contexts, no repos) is a single centered card. **[D-18]** For this user the
empty state is a **migration**, not an onboarding, so the import row is the primary path and is
shown only when `~/.swarm/state.json` exists:

```
                ┌──────────────────────────────────────────┐
                │                 ⛵ Fleet                  │
                │   Copies, sessions and PRs — all owned    │
                │   by fleetd, so they survive this window. │
                │                                          │
                │   Found ~/.swarm.                         │
                │   i   import contexts, repos and worktrees│
                │       (nothing in ~/.swarm is modified)   │
                │                                          │
                │   N   create your first context           │
                │   n   clone a repository                  │
                │   ?   keymap        ,   settings          │
                │                                          │
                │   ◍ fleetd running · 0.1.0 · ~/.fleet     │
                └──────────────────────────────────────────┘
```

`i` runs `fleet import --from-swarm` as a **job** and lands the user in a populated Hub. Without
`~/.swarm`, the import block is omitted entirely (zero-suppression) and the card shows the `N` /
`n` / `?` rows under a single centered 32 px `boxes` glyph. **No** onboarding carousel, tour,
checklist or sample data.

---

## 4. Keystroke counts (steady state, cursor where the previous flow left it)

| # | Task | Keys | Count | Note |
| --- | --- | --- | --- | --- |
| 1 | Open a worktree | `Enter` | **1** | MRU sort puts the last-used branch on row 0 |
| 1b | Open one in another repo | `l` `j`×n `Enter` | 3+n | rail → list |
| 1c | Open one by name from anywhere | `:` `<text>` `Enter` | **3** + text | palette `GO` section |
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
2. Blue is used **only** for cursor and focus. Nothing else, ever.
3. `unknown` never renders like `none`; `none` never renders like an empty cell.
4. A nullable inspection fact renders `—` and its verbatim warning; it never renders `0`.
5. Every job-derived fact on screen has a stamp available (row → detail panel → confirm), and
   every confirm quotes its stamp inline.
6. No surface auto-hides a failure.
7. No bare-key affordance is drawn over a terminal (§3.6, D-8).
8. The mode word is visible on every screen, in every mode, including zoom.
9. Every toast passes the toast law (§2.7).
10. The detail panel is never in the focus cycle; the Jobs panel restores the exact prior focus.
11. Background events never move the cursor, re-sort a list, or steal focus.
12. Every destructive confirm states facts and their age; every unknown decisive fact escalates
    the key to `Y`.

---

## 6. Contract changes this spec requires (before `fleet-proto` is frozen)

ARCHITECTURE rule 4 freezes `fleet-core`, `fleet-proto` and the `fleet-ui-kit` API before parallel
implementation, so these are **blocking decisions**, not UI details.

| # | Change | Why the UI needs it |
| --- | --- | --- |
| C1 | `Session { slept_at: Option<Timestamp>, kept_terminals: Vec<KeptTerminal { name: String, reason: String }> }` | §2.5 renders `moon` for *slept* distinctly from `circle` for *detached and awake*. `SessionState` stays `none \| detached \| attached \| unknown` (§1) — **sleeping is derived**, not a fifth variant, so the wire enum is unchanged. `kept_terminals[].reason` carries §4 step 6 strings verbatim (`unsaved changes`, `claude`, `:3000`, `sleep disabled`) for the sleep toast and the detail panel. |
| C2 | `Worktree { degraded: Option<Degraded { kind: HooksFailed, step: String, exit_code: i32, at: Timestamp, log_path: PathBuf }> }`, persisted in `state.json` | The `⚠ hooks failed` chip (§3.3). Closes §9 "Hook failures warn only; no persisted degraded fact despite ready". Requires a `state.json` schema bump or an additive optional field. |
| C3 | `Job { retryable: bool }` and a `RetryJob { id }` request | `R` in the Jobs panel (§3.7). Closes §9 "no retry path". |
| C4 | `config.trash.retentionMs` (default **600000**) and a `RestoreTrash { entry }` request; the daemon delays the detached `rm -rf` by that long | `u` undo-last-delete (KEYMAP A6). The delete algorithm already renames to `trash/<epochms>-<slug>` first, so the safety net is nearly free. |
| C5 | `config.jobs.warnBeforeQuit` (default **true**) and `config.jobs.keepFinishedFor` (default **600000**) | KEYMAP's `ctrl-q` clause is unimplementable without the first (§3.8.8); §3.7 retention needs the second. |
| C6 | `Terminal { has_unseen_output: bool }`, cleared on attach/activate per client | The tab activity dot (§3.6). If the daemon cannot hold a per-client watermark, the client derives it from `FrameUpdate.seq` per terminal and this field is dropped. |
| C7 | `Snapshot { generated_at: Timestamp }` | The `stale · <age>` header stamp (§1.3, §3.12). |
| C8 | `WorktreeStatus.session` must be set to `unknown` — **never `none`** — whenever the local status observation fails, matching the remote path | Directly retires the §9 defect. This is a daemon behavior requirement, not a type change. |

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
| D-11 | `D` (context delete) adjacency risk | §3.8.4: `D` kept, routed through the expanded `Y` confirm; `E` + `ctrl-d` is the discoverable path |
| D-12 | Assign dialog bound only `⌃n`/`⌃p` | §3.8.5: `j`/`k` restored — the dialog has no text field |
| D-13 | Settings could not edit grace / pool / TTLs / intervals | §3.8.6: all editable; `windows`/`hosts` read-only with a 1-key `E` escape to `config.json` |
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
| `AppFrame` | Context bar + body region + status bar; fixed heights 36 / flex / 26 | Hub, PR screen, Workspace |
| `ContextBar` | Numbered context tabs, overflow chip, chip tray, daemon dot | all screens (§3.1) |
| `StatusBar` | Breadcrumb · `ModeWord` · job ticker · sticky error slot | all screens (§2.2) |
| `Pane` | Bordered region with a header slot, a body slot and a scroll thumb | repos rail, lists, detail panel |
| `PaneHeader` | Label · scope · `shown/total` · visible range · `stale` stamp; swaps in `FilterBar` in place | §2.10, every list |
| `Sheet` | Right-docked panel, 440 / 640 px, 160 ms slide, non-blocking, focus-restoring on close | Jobs panel (§3.7) |
| `Dialog` | The shared frame: scrim + card + 44 px header + 44 px footer, `Esc` close, no button pair | all of §3.8 |
| `Overlay` | Centered top-anchored layer at y = 120 | Palette (§3.9) |
| `ToastStack` | Bottom-right stack, max 3, 3.2 / 1.6 s, 1 s identical-text coalescing into `×n` | §2.7 |
| `SplitLayout` | Two panes with a fixed side and a flex side, on either axis | Hub body, Workspace body |
| `Divider` | 1 px rule, horizontal or vertical | dialogs, panels |

### 9.3 Data display

| Component | Responsibility | Used by |
| --- | --- | --- |
| `ListView` | Virtualized 30 px rows, cursor, scrolloff 2, `gg`/`G`/`ctrl-d`/`ctrl-u`, cursor stability under background updates | repos rail, worktrees, PRs, palette, assign, base list, clone results |
| `Row` | One row: leading glyph slot, flex content, trailing columns, selected/dimmed/disabled states | every list |
| `ColumnLadder` | Resolves a ch-based responsive column set for the current pane width (§2.9) | worktrees list, PR list |
| `StatusGlyph` | The §2.5 vocabulary — the single source of truth for session/job/clone state rendering | worktree rows, repo rows, PR presence, Workspace header, palette `GO`, confirms, quit dialogs |
| `Chip` | 22 px pill: icon + text + count, tinted, zero-suppressible | context-bar chips, host chip, degraded chip |
| `KeepAliveChips` | `⚡` labels with a max-3 + `+n` overflow and the width ladder 18/14/10/0 ch; outranked by `DegradedChip` in the same slot | worktree rows, Workspace header, detail panel, confirms |
| `DegradedChip` | `⚠ hooks failed` with a link into the Jobs panel | worktree rows, detail panel |
| `PrBadge` | `#n` + state icon + ≤8 ch word, from the `PrState` priority | worktree rows, PR rows, Workspace header, detail panels |
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
| `TextField` | Printable + `Backspace` + `ctrl-w`/`ctrl-u`/`ctrl-a`/`ctrl-e`, blue caret, inline validation line that replaces the preview slot with zero layout shift | Create, Clone, Context, Settings, Filter, Palette |
| `FuzzyList` | Debounced query → ranked rows, capped, `ctrl-n`/`ctrl-p` + arrows (and `j`/`k` **only** when no text field is present) | Clone results, Create base list, Palette, Assign |
| `FilterBar` | In-place pane-header replacement with live `shown/total`, two-stage `Esc`, retained chip | every list (§3.10) |
| `Cycler` | `◂ value ▸`, `←`/`→` | host selector, Settings choices |
| `Toggle` | `[x]` / `[ ]`, `Space` | Settings |
| `NumberField` | Integer with a unit suffix and a clamp | Settings (grace, TTLs, intervals, pool) |
| `SegmentedTabs` | Underlined tabs with counts, `Tab`/`S-Tab`/`h`/`l` | PR Mine/Review, Help columns |
| `ConfirmDialog` | Compact/expanded switch driven by `FactList`; binds only `y`/`Y`/`Enter`/`n`/`Esc`/`q` (+ `I`, + `s` for prune) | §3.8.3, §3.8.8, §3.8.9 |
| `Palette` | Sectioned `GO`/`DO`/`CONTEXT` result list with right-aligned key hints, cap 10 | §3.9 |
| `Select` | A closed choice rendered as a row with its current value, for a set too long for `Cycler` | Settings, Create dialog |

### 9.5 Jobs and terminal

| Component | Responsibility | Used by |
| --- | --- | --- |
| `JobRow` | Two-line job item: glyph · kind (7 ch) · target · elapsed · percent · optional `(restartable)` / `(not restartable)`, plus the progress sub-line | Jobs panel, quit dialogs |
| `JobTicker` | Newest running job as one status-bar line with a `+n` suffix | status bar |
| `StickyErrorSlot` | Red, addressable (`!`), persists until dismissed; owns the last failed job | status bar |
| `LogView` | Tail of `logs/jobs/<id>.log`, last 200 lines, 16 ms batching, follow toggle, `G` re-follow | Jobs panel |
| `TerminalGrid` | Paints the mirror cell grid from `FrameUpdate`: the full VT attribute set (bold, dim, italic, single/double/curly underline with its own color, strikethrough, inverse, blink, invisible), narrow/wide/spacer cells, the four cursor shapes, the selection overlay and the scrollback badge | Workspace |
| `TerminalModes` | Zero-suppressed badges for `alt` / `mouse` / `paste` / `appcur` — why the keymap appears to lie | Workspace header |
| `ScrollbackBadge` | `↥ <offset>/<len>` in the grid corner whenever the viewport is scrolled back, in or out of Scroll mode | Workspace |
| `TerminalTabStrip` | Numbered tabs 84–200 px with an activity dot, a per-tab waking spinner, keep-alive icon, exited mark (code or `—`) and a `+` tab | Workspace |
| `ScrollPill` | `SCROLL <offset>/<len>` overlay with a selection hint line; **suppressed in alt-screen** | Workspace Scroll mode |
| `PrefixHint` | `^S` pill + 6 keys, 400 ms delayed, bottom-left inside the terminal area | Workspace Prefix mode |
| `ExitStrip` | `⚠ process exited (<code>)` + prefixed recovery keys | Workspace |
| `ModeWord` | The §2.8 mode word, fixed 84 px | status bar |
| `Banner` | 28 px full-width amber/red strip with a countdown and prefixed keys | daemon state C (§3.12) |
| `DaemonSplash` | The full-window cold-start and will-not-start surfaces: title, spinner, socket path, `fleetd.log` tail, bare recovery keys | daemon states A and B (§3.12) |
| `DaemonDot` | 8 px liveness dot that expands into a labelled pill when degraded | context bar |
| `Veil` | 55 % scrim over terminal grids only, with key-dropping | daemon disconnect |

## Subagent watch pane

A new subagent watch for the current Workspace session opens a read-only split
on the right and selects that watch. The terminal is the flexible leading region
of `SplitLayout::horizontal()`; the trailing region is 40% of the current window
width, clamped to 360–640 px and recomputed on resize. PTY dimensions follow the
terminal's actual reduced painted bounds. Zoom (`^s z`) hides the session header
and terminal tabs while leaving the watch pane, including its header, visible.

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
