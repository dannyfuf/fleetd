# lazygit reference — layout, keybindings, visual language, git plumbing

Reference notes for the native lazygit clone (`crates/fleet-lazygit`) being built in this
workspace. Everything here is grounded in a real lazygit checkout; every claim carries a
`path:line` reference into that tree.

- **Upstream source read for this document:** `https://github.com/jesseduffield/lazygit`,
  commit `e0b2a5081dbe7511e7752884e571e6eae97948d1` (2026-09-03, `master`), shallow clone at
  `/private/tmp/.../scratchpad/lazygit-src`.
- **All `path:line` references below are relative to that checkout root** (e.g.
  `pkg/gui/layout.go:120` means `<lazygit-src>/pkg/gui/layout.go` line 120).
- Companion document: `docs/research/lazygit-native-brief.md` — how to build this UI natively
  in *this* repo (gpui boot, fleet-ui-kit inventory, recommended architecture).

Contents:

- [a. Window and panel layout](#a-window-and-panel-layout)
- [b. Complete default keybindings, per context](#b-complete-default-keybindings-per-context)
- [c. Visual language worth mirroring](#c-visual-language-worth-mirroring)
- [d. The git command lines lazygit runs](#d-the-git-command-lines-lazygit-runs)

## a. Window and panel layout

### a.1 The side column is user-configurable groups, not a fixed list

The five side panels are a config value, not hardcoded
(`pkg/config/user_config.go:870`-`:876`):

```go
SidePanels: []SidePanel{
    {"status"},
    {"files", "worktrees", "submodules"},
    {"branches", "remotes", "tags"},
    {"commits", "reflog"},
    {"stash"},
},
```

The field doc (`pkg/config/user_config.go:116`-`:120`) reads: *"The side panels, in the order
they appear from top to bottom. Each entry is a list of one or more names that share a single
panel as tabs… 'files', 'branches', and 'commits' must always be included."*

**Config name → gocui view name → window name** (`pkg/gui/side_panels.go:13`-`:24`):

| config name | view-name string | window name (= panel[0]) |
|---|---|---|
| `status` | `"status"` | `status` |
| `files` | `"files"` | `files` |
| `worktrees` | `"worktrees"` | `files` |
| `submodules` | `"submodules"` | `files` |
| `branches` | `"localBranches"` | `branches` |
| `remotes` | `"remotes"` | `branches` |
| `tags` | `"tags"` | `branches` |
| `commits` | `"commits"` | `commits` |
| `reflog` | `"reflogCommits"` | `commits` |
| `stash` | `"stash"` | `stash` |

A panel's **window name is the name of its first tab**
(`pkg/gui/controllers/helpers/window_helper.go:144`-`:148`):

```go
func (self *WindowHelper) sideWindowNames() []string {
	return lo.Map(self.c.UserConfig().Gui.SidePanels, func(panel config.SidePanel, _ int) string {
		return panel[0]
	})
}
```

Window assignment is `assignSidePanelWindows` (`pkg/gui/side_panels.go:111`-`:137`); a panel not
listed in the config gets its own window name, therefore no dimensions, therefore is hidden
(`pkg/gui/side_panels.go:108`-`:110`, `pkg/gui/layout.go:70`-`:78`). Transient drill-down
contexts borrow a window: `RemoteBranches` and `SubCommits` → the branches window,
`CommitFiles` → the commits window (`pkg/gui/side_panels.go:134`-`:136`).

The full z-ordered view list is `orderedViewNameMappings` (`pkg/gui/views.go:26`-`:83`); the
`types.Views` struct with every view field is `pkg/gui/types/views.go:5`-`:49`.

**Tab strips exist only for panels with two or more entries** — `viewTabMap()`
(`pkg/gui/gui.go:899`-`:916`), whose guard is `if len(panel) < 2 { continue }` (`:903`-`:906`)
with the comment *"A single-tab panel shows its view's own title, not a tab strip."* Titles come
from `sidePanelTabTitles()` (`pkg/gui/side_panels.go:27`-`:41`) and are pushed onto views as
`view.Tabs` / `view.TabIndex` (`pkg/gui/views.go:286`-`:304`). Context mapping is
`sidePanelContexts()` (`pkg/gui/side_panels.go:44`-`:57`).

Exact English titles (`pkg/i18n/english.go`): `"Status"` (`:1208`), `"Files"` (`:1173`),
`"Worktrees"` (`:2045`), `"Submodules"` (`:1924`), `"Local branches"` (`:1502`),
`"Remotes"` (`:1507`), `"Tags"` (`:1504`), `"Commits"` (`:1175`), `"Reflog"` (`:1512`),
`"Stash"` (`:1176`), `"Command log"` (`:1947`), `"Diff"` (`:1172`),
`"Not enough space to render panels"` (`:1171`).

The `[1]`…`[5]` jump labels are `view.TitlePrefix` per panel group
(`pkg/gui/views.go:245`-`:274`), keys `JumpToBlock: []Keybinding{{"1"},{"2"},{"3"},{"4"},{"5"}}`
(`pkg/config/user_config.go:1024`), gated on `gui.showPanelJumps` (default `true`,
`pkg/config/user_config.go:903`). Tab cycling is `NextTab: "]"` / `PrevTab: "["`
(`pkg/config/user_config.go:1059`-`:1060`).

### a.2 Main panel and secondary panel

The root box tree (`pkg/gui/controllers/helpers/window_arrangement_helper.go:153`-`:178`):

```go
root := &boxlayout.Box{
    Direction: boxlayout.ROW,
    Children: []*boxlayout.Box{
        {Direction: sidePanelsDirection, Weight: 1, Children: []*boxlayout.Box{
            {Direction: boxlayout.ROW, Weight: sideSectionWeight, ConditionalChildren: sidePanelChildren(args)},
            {Direction: boxlayout.ROW, Weight: mainSectionWeight, Children:  mainPanelChildren(args)},
        }},
        {Direction: boxlayout.COLUMN, Size: infoSectionSize, Children: infoSectionChildren(args)},
    },
}
```

`mainSectionChildren` (`:217`-`:248`) decides the split: if `!SplitMainPanel` → a single `main`
box; `SCREEN_FULL` with the current window `main` → only `main`; `SCREEN_FULL` with the current
window `secondary` → only `secondary`; otherwise `main` (Weight 1) + `secondary` (Weight 1).

**Split direction** — `splitMainPanelSideBySide` (`:384`-`:401`):

```go
case "vertical":   return false
case "horizontal": return true
default:
    if args.Width < 200 && args.Height > 30 { return false }
    return true
```

The comment on the 200 threshold (`:396`) is *"2 80 character width panels + 40 width for side
panel"*. `MainPanelSplitMode` is the enum `horizontal|flexible|vertical`
(`pkg/config/user_config.go:121`-`:126`), default `"flexible"` (`:877`). Side-by-side squeezes
the side column: `mainSectionWeight = sideSectionWeight * 5` (`:257`-`:259`).

**`SplitMainPanel` is purely derived** from whether the last render supplied a secondary view —
`gui.splitMainPanel(opts.Secondary != nil)` (`pkg/gui/main_panels.go:136`, setter `:139`-`:141`).

**Main context pairs** (`pkg/gui/main_panels.go:72`-`:107`): `Normal` = (`Contexts.Normal` → view
`main`, `Contexts.NormalSecondary` → view `secondary`) `:72`-`:77`; `Staging` = (`staging`,
`stagingSecondary`) `:79`-`:84`; `PatchBuilding` = (`patchBuilding`, `patchBuildingSecondary`)
`:86`-`:91`; `MergeConflicts` = (`mergeConflicts`, `nil`) `:93`-`:98`. Note that
`staging`, `stagingSecondary` and `patchBuilding` all live in window `"main"`, while
`patchBuildingSecondary` lives in window `"secondary"`
(`pkg/gui/context/setup.go:46`, `:53`, `:60`, `:81`).

**The secondary panel is used for exactly two things**: the staged half of a file diff
(`renderWorkingTreeDiff`, `pkg/gui/controllers/files_controller.go:366`-`:403`; split when
`gui.splitDiff == "always"` or the file has both staged and unstaged changes, `:369`), and the
aggregated custom-patch preview beside the commits diff
(`secondaryPatchPanelUpdateOpts`, `pkg/gui/controllers/local_commits_controller.go:725`-`:731`).

### a.3 The command log ("extras") panel

- View name `"extras"`, window `"extras"` (`pkg/gui/views.go:52`,
  `pkg/gui/context/setup.go:115`).
- Title `"Command log"` (`pkg/gui/views.go:232`, `pkg/i18n/english.go:1947`), with
  `Autoscroll = true`, `Wrap = true`, `AutoRenderHyperLinks = true`
  (`pkg/gui/views.go:165`-`:167`).
- **It sits inside the main (right) column, below the main panel** — not full width
  (`pkg/gui/controllers/helpers/window_arrangement_helper.go:199`-`:204`).
- Height — `getExtrasWindowSize` (`:403`-`:416`):
  ```go
  if args.CurrentWindow == "extras" { baseSize = 1000 }  // "my way of saying 'fill the available space'"
  else if args.Height < 40         { baseSize = 1 }
  else                             { baseSize = args.UserConfig.Gui.CommandLogSize }
  frameSize := 2
  return baseSize + frameSize
  ```
  `CommandLogSize` defaults to `8` (`pkg/config/user_config.go:918`), so the **default box
  height is 10** — 8 content rows plus 2 frame rows.
- Shown when `ShowExtrasWindow`, initialised
  `gui.ShowExtrasWindow = userConfig.Gui.ShowCommandLog && !gui.c.GetAppState().HideCommandLog`
  (`pkg/gui/gui.go:518`); `ShowCommandLog` defaults `true`
  (`pkg/config/user_config.go:901`); the persisted flag is `HideCommandLog`
  (`pkg/config/app_config.go:858`).
- **Toggle**: the global `ExtrasMenu` key, default `"@"`
  (`pkg/config/user_config.go:1072`), bound at `pkg/gui/keybindings.go:169`-`:176`, opening a
  menu with `t` = toggle show/hide and `f` = focus
  (`pkg/gui/extras_panel.go:12`-`:46`).
- Content styling: action lines `style.FgYellow` (`pkg/gui/command_log_panel.go:41`); command
  lines `theme.DefaultTextColor`, or `style.FgMagenta` when not a runnable command line,
  indented two spaces (`:51`-`:57`, `:65`); header `style.FgCyan` with
  `CommandLogHeader: "You can hide/focus this panel by pressing '%s'\n"` (`:70`-`:75`,
  `pkg/i18n/english.go:1952`); git-output prefix
  `style.FgMagenta.Sprintf("\n\n%s\n", Tr.GitOutput)` (`pkg/gui/extras_panel.go:97`).

### a.4 The bottom bar

The bottom bar is **one row tall** (`infoSectionSize = 1`,
`pkg/gui/controllers/helpers/window_arrangement_helper.go:144`-`:151`, `:172`-`:176`), shown
when `gui.showBottomLine || InSearchPrompt || IsAnyModeActive || AppStatus != ""` (`:144`-`:147`).
`ShowBottomLine` defaults `true` (`pkg/config/user_config.go:902`).

Children — `infoSectionChildren` (`:280`-`:382`): in search mode, `searchPrefix` (sized to the
prefix width) + `search` (weight 1) (`:281`-`:292`). Otherwise, in order: `appStatus` (sized to
its width, only when non-empty) `:334`-`:340`; **`options` (weight 1)** `:342`-`:344`;
**`information`** (sized `StringWidth(Decolorise(InformationStr))`) `:346`-`:353`. One-character
spacer boxes sit between them (`:294`-`:328`, `:376`-`:379`) and a flexible spacer
right-aligns `information` (`:355`-`:374`).

**`options` is the keybinding-hint view.** Frame off (`pkg/gui/views.go:94`), rendered by
`OptionsMapMgr.renderContextOptionsMap` (`pkg/gui/options_map.go:37`-`:106`). Entries are
`fmt.Sprintf("%s: %s", info.description, info.key)` joined by `" | "` and truncated with `"…"`
(`:108`-`:131`; `separator := " | "` at `:112`, `ellipsis := "…"` at `:111`). Default style
`theme.OptionsFgColor` (`:57`; default `optionsTextColor: ["blue"]`,
`pkg/config/user_config.go:888`), with mode overrides: paste-commits `style.FgCyan` (`:75`),
bisect `style.FgGreen` (`:83`), rebase/merge options `style.FgYellow` (`:93`), patch options
`style.FgYellow` (`:101`).

**`information` is the bottom-right mode/version view.** Frame off,
`FgColor = gocui.ColorGreen` (`pkg/gui/views.go:161`-`:163`). Content — `informationStr()`
(`pkg/gui/information_panel.go:11`-`:23`):

```go
if activeMode, ok := gui.helpers.Mode.GetActiveMode(); ok { return activeMode.InfoLabel() }   // :12-14
if gui.g.Mouse {
    donate      := style.FgMagenta.Sprint(style.PrintHyperlink(gui.c.Tr.Donate, constants.Links.Donate))          // :17
    askQuestion := style.FgYellow.Sprint(style.PrintHyperlink(gui.c.Tr.AskQuestion, constants.Links.Discussions)) // :18
    return fmt.Sprintf("%s %s %s", donate, askQuestion, gui.Config.GetVersion())                                   // :19
}
return gui.Config.GetVersion()   // :22
```

**The repo name and branch are NOT in the bottom bar** — they are the *content of the `status`
side panel*. `FormatStatus` (`pkg/gui/presentation/status.go:15`-`:49`):

```go
status += BranchStatus(currentBranch, itemOperation, tr, time.Now(), userConfig)              // :27
status += style.FgYellow.Sprintf("(%s) ", workingTreeState.LowerCaseTitle(tr))                // :34 (only if a mode is active)
name := GetBranchTextStyle(currentBranch.Name).Sprint(currentBranch.Name)                     // :37
repoName = fmt.Sprintf("%s(%s%s)", repoName, icon, style.FgCyan.Sprint(linkedWorktreeName))   // :44 (linked worktree only)
status += fmt.Sprintf("%s → %s", repoName, name)                                              // :46
```

The separator is `→` (U+2192) with a space each side (`:46`). Written by `refreshStatus`
(`pkg/gui/controllers/helpers/refresh_helper.go:1625`-`:1643`). The repo name itself is
unstyled; only the linked-worktree name is cyan.

**The list footer** (the `1 of 10` on each list's bottom border) is
`fmt.Sprintf("%d of %d", selectedLineIdx+1, length)`
(`pkg/gui/context/list_context_trait.go:102`-`:104`), drawn at
`v.x1 - 1 - width(message)` (`pkg/gocui/gui.go:1495`-`:1521`), gated on `gui.showListFooter`
(default `true`, `pkg/config/user_config.go:900`).

### a.5 Screen modes

```go
// pkg/gui/types/common.go:476-481
type ScreenMode int
const (
    SCREEN_NORMAL ScreenMode = iota
    SCREEN_HALF
    SCREEN_FULL
)
```

Cycle keys: `NextScreenMode: Keybinding{"+"}`, `PrevScreenMode: Keybinding{"_"}`
(`pkg/config/user_config.go:1061`-`:1062`), cycling over
`[]types.ScreenMode{SCREEN_NORMAL, SCREEN_HALF, SCREEN_FULL}` **with wrap-around**
(`pkg/gui/controllers/screen_mode_actions.go:12`-`:34`, helpers `nextIntInCycle` /
`prevIntInCycle` at `:62`-`:84`). Screen-mode-dependent views are re-rendered afterwards
(`:38`-`:50`). The initial mode is `gui.screenMode`, default `"normal"`
(`pkg/config/user_config.go:922`), parsed by `parseScreenModeArg`
(`pkg/gui/gui.go:732`-`:741`), and forced to `SCREEN_HALF` when launched with a filter path or a
git argument (`pkg/gui/gui.go:722`-`:730`).

**The arrangement math** — `getMidSectionWeights`
(`pkg/gui/controllers/helpers/window_arrangement_helper.go:250`-`:278`):

```go
sidePanelWidthRatio := args.UserConfig.Gui.SidePanelWidth
const maxColumnCount = 120                                                        // :253
mainSectionWeight := int(math.Round(maxColumnCount * (1 - sidePanelWidthRatio)))   // :254
sideSectionWeight := int(math.Round(maxColumnCount * sidePanelWidthRatio))         // :255
if splitMainPanelSideBySide(args) { mainSectionWeight = sideSectionWeight * 5 }    // :257-259
if args.CurrentWindow == "main" || args.CurrentWindow == "secondary" {
    if HALF || FULL { sideSectionWeight = 0 }                                      // :261-264
} else {
    if HALF { mainSectionWeight = lo.Ternary(enlargedSideViewLocation=="top",
                                             sideSectionWeight*2, sideSectionWeight) }  // :266-271
    else if FULL { mainSectionWeight = 0 }                                         // :272-274
}
```

**Side-panel heights per screen mode** — `sidePanelChildren` (`:435`-`:531`):

- `SCREEN_FULL` / `SCREEN_HALF`: the focused side window gets `Weight: 1`, **every other one
  `Size: 0`** — fully collapsed (`:456`-`:471`).
- `SCREEN_NORMAL` at height ≥ `minHeightForNormalLayout` (28, scaled down by panel count,
  `:444`-`:446`): the window showing `status` gets `Size: 3` (fixed, never expanded by accordion
  mode); the window showing `stash` gets `Size: 3` unfocused / `Weight: 1` focused
  (`getDefaultStashWindowBox`, `:423`-`:433`); everything else `Weight: 1` (`:491`-`:505`).
  Accordion mode (`gui.expandFocusedSidePanel`, default `false`) gives the focused window
  `gui.expandedSidePanelWeight` (default `2`) — `:479`-`:489`,
  `pkg/config/user_config.go:867`-`:868`.
- Below 28 rows: the focused window gets `Weight: 1`, the others `Size: squashedHeight` — 3 if
  height ≥ 21 else 1 (`:510`-`:529`).

**Portrait mode** (side panels stacked above main) — `shouldUsePortraitMode` (`:120`-`:134`): in
`SCREEN_HALF` it follows `gui.enlargedSideViewLocation == "top"`; otherwise `gui.portraitMode`
is `never|always|auto`, with auto meaning
`Width <= portraitModeAutoMaxWidth (84) && Height >= portraitModeAutoMinHeight (46)`. Defaults
`"auto"`, `84`, `46` (`pkg/config/user_config.go:925`-`:927`); `enlargedSideViewLocation`
defaults `"left"` (`:878`).

### a.6 Side-column width

- `SidePanelWidth float64` (doc at `pkg/config/user_config.go:107`-`:109`), default **`0.3333`**
  (`:866`).
- Weights: `sideSectionWeight = round(120 * 0.3333) = 40`,
  `mainSectionWeight = round(120 * 0.6667) = 80`
  (`pkg/gui/controllers/helpers/window_arrangement_helper.go:253`-`:255`). The comment at `:252`
  explains the constant: *"Using 120 so that the default of 0.3333 will remain consistent with
  previous behavior."*
- boxlayout normalises by the common factor, so `[40, 80]` becomes `[1, 2]`
  (`vendor/github.com/jesseduffield/lazycore/pkg/boxlayout/boxlayout.go:148`-`:173`).
- Sizing — `calcSizes` (`boxlayout.go:96`-`:145`): static boxes (`Size > 0`, `isStatic()` at
  `:185`-`:187`) reserve their space first; the remainder is `unitSize * weight`, with the
  integer remainder dealt out one row/column at a time to the weighted boxes (`:129`-`:142`).
  Leaf dimensions are `{X0: x0, Y0: y0, X1: x0+width-1, Y1: y0+height-1}` (`:60`).
- **There are no per-box minimums.** The entire UI is replaced by the `limit` view when
  `height < minimumScreenHeight(...) || width < 10` (`pkg/gui/layout.go:147`-`:150`);
  `minimumScreenHeight` is `max(9, sideWindowCount+4)`, bumped to ≥ 11 with a filterable menu
  open (`pkg/gui/layout.go:244`-`:260`; tests at `pkg/gui/layout_test.go:9`-`:14` confirm
  5 panels → 9 and 8 panels → 12).

Executing the real algorithm gives: 200 columns → `[67, 133]`; 120 columns → `[40, 80]`; side
rows at height 49 → `[3, 15, 14, 14, 3]`; main rows at height 49 → `[39, 10]`.

### a.7 How focus is shown

Focus is a **frame-colour and title-colour swap**, plus bold on the selected row. There is no
focus glyph.

```go
// pkg/gui/gui.go:1248-1256
theme.UpdateTheme(userConfig.Gui.Theme)          // :1250
gui.g.FgColor       = theme.InactiveBorderColor  // :1252  -> unfocused TITLE color
gui.g.SelFgColor    = theme.ActiveBorderColor    // :1253  -> focused TITLE color
gui.g.FrameColor    = theme.InactiveBorderColor  // :1254  -> unfocused BORDER color
gui.g.SelFrameColor = theme.ActiveBorderColor    // :1255  -> focused BORDER color
```

The per-draw decision (`pkg/gocui/gui.go:1654`-`:1672`):

```go
if g.Highlight && g.hasFocus(v) && g.IsFocused() {
    fgColor = g.SelFgColor; bgColor = g.SelBgColor; frameColor = g.SelFrameColor   // :1657-1659
} else {
    bgColor = g.BgColor                                                            // :1661
    fgColor    = lo.Ternary(v.TitleColor != ColorDefault, v.TitleColor, g.FgColor)      // :1662-1666
    frameColor = lo.Ternary(v.FrameColor != ColorDefault, v.FrameColor, g.FrameColor)   // :1667-1671
}
```

`fgColor` drives the title (`:1681`), subtitle (`:1686`) and list footer (`:1691`);
`frameColor` drives the edges (`:1674`) and corners (`:1677`). **Border runes and title text
change colour together — there is no separate title style.**

With stock config that means **focused frame + title = green + bold; unfocused = terminal
default, no bold** (`pkg/config/user_config.go:885`, `:887`). `g.IsFocused()` is *terminal
window* focus (`pkg/gocui/gui.go:2031`-`:2035`), so when the terminal loses focus **every** panel
draws unfocused. While searching, the active border switches to
`theme.SearchingActiveBorderColor` (`pkg/gui/controllers/helpers/search_helper.go:326`-`:334`;
default `["cyan","bold"]`, `pkg/config/user_config.go:886`).

Border runes (`gui.border`, default `"rounded"`, `pkg/config/user_config.go:923`) —
`pkg/gui/views.go:181`-`:198`, rune order (horizontal, vertical, TL, TR, BL, BR) documented at
`:174`-`:176`:

```go
frameRunes := []rune{'─','│','┌','┐','└','┘'};  teeLeft, teeRight := '├','┤'   // single/fallback :182,185
case "double":  {'═','║','╔','╗','╚','╝'};  '╠','╣'                            // :188-189
case "rounded": {'─','│','╭','╮','╰','╯'}                                      // :190-191
case "hidden":  {' ',' ',' ',' ',' ',' '};  ' ',' '                            // :192-194
case "bold":    {'━','┃','┏','┓','┗','┛'};  '┣','┫'                            // :195-197
```

**The selected-line highlight** is not reverse video (`pkg/gocui/view.go:668`-`:689`):

```go
} else if v.Highlight {
    if y >= rangeSelectStart && y <= rangeSelectEnd {
        // this ensures we use the bright variant of a colour upon highlight
        fgColorComponent := fgColor & ^AttrAll                                     // :679
        if fgColorComponent >= AttrIsValidColor && fgColorComponent < AttrIsValidColor+8 {
            fgColor += 8                                                            // :681
        }
        fgColor = fgColor | AttrBold                                                // :683
        if v.HighlightInactive || !isWindowFocused {
            bgColor = (bgColor & AttrStyleBits) | v.InactiveViewSelBgColor           // :685
        } else {
            bgColor = (bgColor & AttrStyleBits) | v.SelBgColor                       // :687
        }
    }
}
```

**Bright foreground (+8) + bold + a background swap**, and the selection **dims when the panel
is unfocused**. `Highlight` / `HighlightInactive` are recomputed on every context change
(`pkg/gui/context.go:221`-`:227`):

```go
view.Highlight         = onStack.Includes(c.GetKey()) && c.HasSelectableContent()   // :224
view.HighlightInactive = c.GetKey() != currentKey                                   // :225
```

Defaults are `selectedLineBgColor: ["blue"]` (focused) and
`inactiveViewSelectedLineBgColor: ["bold"]` (unfocused = bold only, no background) —
`pkg/config/user_config.go:889`-`:890`.

> **Native mapping.** `Pane::focused(bool)` gives the 2 px accent ring and `Row::cursor(bool)`
> the 2 px accent bar (brief §c.2). Together they reproduce "focused frame + brighter selected
> row" without reverse video. Crucially, keep `Row::selected(true)` on the unfocused panel's
> cursor row and set `.cursor(false)` — that is lazygit's `HighlightInactive` behaviour exactly.

### a.8 What each side panel puts in the main panel

Everything goes through `c.RenderToMainViews(types.RefreshMainOpts{...})` → `refreshMainViews`
(`pkg/gui/main_panels.go:109`-`:137`), registered via `controller.GetOnRenderToMain()` →
`context.AddOnRenderToMainFn` (`pkg/gui/controllers/attach.go:12`,
`pkg/gui/context/base_context.go:199`).

| Panel | Main content | Where |
|---|---|---|
| **Status** | `gui.statusPanelView` = `"dashboard"` (default) → the ASCII lazygit logo, copyright and doc links, titled `Tr.StatusTitle`; `"allBranchesLog"` → `git log --graph --all --color=always …` in a PTY, titled `Tr.LogTitle` | `pkg/gui/controllers/status_controller.go:78`-`:89`; `showDashboard` `:188`-`:217` (logo `:131`-`:141`, lines joined by `"\n\n"` `:198`-`:208`, only the sponsor line coloured `style.FgMagenta` `:207`); `showAllBranchLogs` `:147`-`:162`; default `StatusPanelView: "dashboard"` at `pkg/config/user_config.go:933` |
| **Files** | The working-tree diff of the selected node as a PTY task, titled `Tr.UnstagedChanges` / `Tr.StagedChanges`; the **secondary** panel carries the other half when the file has both, or when `gui.splitDiff == "always"`. No file → `Tr.NoChangedFiles`. An inline merge conflict switches to the merge-conflict main context | `pkg/gui/controllers/files_controller.go:248`-`:277`; `renderWorkingTreeDiff` `:366`-`:403`; `renderToMainWithTask` `:281`-`:290` |
| **Local branches** | `git log --graph --color=always … <branch>` in a PTY (`GetGraphCmdObj(branch.FullRefName())`), titled `Tr.LogTitle`, with an optional PR header and a full-width `─` rule prefix. Empty → `Tr.NoBranchesThisRepo` | `pkg/gui/controllers/branches_controller.go:199`-`:228` (prefix `:212`-`:216`) |
| **Commits** | The diff for the selected commit or range, titled literally `"Patch"`, subtitled with the ignoring-whitespace hint; the **secondary** panel shows the aggregated custom patch when the patch builder is active. An `update-ref` todo → `Tr.UpdateRefHere`; an `exec` todo → `Tr.ExecCommandHere`; empty → `Tr.NoCommitsThisBranch` | `pkg/gui/controllers/local_commits_controller.go:690`-`:723`; secondary `:725`-`:731` |
| **Stash** | `git stash show …` in a PTY, prefixed with `style.FgYellow.Sprintf("%s\n\n", stashEntry.Description())`, titled literally `"Stash"`. Empty → `Tr.NoStashEntries` | `pkg/gui/controllers/stash_controller.go:87`-`:112` |
| Reflog | `git show <hash>` in a PTY, titled `"Reflog Entry"`; empty → `"No reflog history"` | `pkg/gui/controllers/reflog_commits_controller.go:40`-`:61` |
| Remotes | `fmt.Sprintf("%s\nUrls:\n%s", style.FgGreen.Sprint(remote.Name), …)` plus `"\nPush Urls:\n…"`, titled `"Remote"` | `pkg/gui/controllers/remotes_controller.go:101`-`:125` |
| Tags | Tag info + `"\n\n---\n\n"` + the branch graph command, titled `"Tag"` | `pkg/gui/controllers/tags_controller.go:101`-`:123` |
| Worktrees | A tabwriter table: `style.FgGreen` name, `style.FgYellow` branch, `style.FgRed` missing marker | `pkg/gui/controllers/worktrees_controller.go:80`-`:100` |
| Submodules | `"Name: %s\nPath: %s\nUrl:  %s\n\n"` in FgGreen/FgYellow/FgCyan, then the submodule diff | `pkg/gui/controllers/submodules_controller.go:107`-`:129` |

`Diff.WithDiffModeCheck(...)` wraps nearly all of these so diffing mode can hijack the main view
(e.g. `pkg/gui/controllers/files_controller.go:250`).

### a.9 The default layout, drawn to scale

Derived by executing the real algorithm (`getMidSectionWeights` + `boxlayout.calcSizes`) for a
**200 × 50** terminal with stock config (`sidePanelWidth 0.3333`, `commandLogSize 8`,
`showBottomLine true`; portrait off since 200 > 84):

- The top box is 49 rows, the info row 1
  (`pkg/gui/controllers/helpers/window_arrangement_helper.go:148`-`:151`).
- Columns: weights `[40, 80]` → normalised `[1, 2]` → **side 67 cols, main 133 cols**
  (≈ 33 % / 67 %).
- Side rows (`status` Size 3, `files`/`branches`/`commits` Weight 1, `stash` Size 3 unfocused):
  **3, 15, 14, 14, 3**.
- Main rows (`main` Weight 1, `extras` Size 10): **39, 10**.

```
┌── 67 cols ─────────────────────┬── 133 cols ─────────────────────────────────────────┐
│ ╭─[1] Status────────────────╮  │ ╭─[0] Diff / Log / Patch / Stash ─────────────────╮  │  rows
│ │ ↑2 (rebasing) myrepo → mn │3 │ │                                                 │  │  1..39
│ ╰───────────────────────────╯  │ │   main   (secondary appears here beside or      │  │
│ ╭─[2] Files - Worktrees -   ╮  │ │           below main when the diff is split)    │  │
│ │      Submodules           │15│ │                                                 │  │
│ │ ?? new.go                 │  │ │                                                 │  │
│ │ MM edited.go       1 of 7 │  │ ╰─────────────────────────────────────────────────╯  │
│ ╰───────────────────────────╯  │ ╭─ Command log ───────────────────────────────────╮  │  10
│ ╭─[3] Local branches -      ╮  │ │ Stage file                                      │  │
│ │      Remotes - Tags       │14│ │   git add -- 'new.go'                           │  │
│ ╰───────────────────────────╯  │ ╰─────────────────────────────────────────────────╯  │
│ ╭─[4] Commits - Reflog      ╮14│                                                      │
│ ╰───────────────────────────╯  │                                                      │
│ ╭─[5] Stash                 ╮3 │                                                      │
│ ╰───────────────────────────╯  │                                                      │
├────────────────────────────────┴──────────────────────────────────────────────────────┤
│ options: keybinding hints … | … …                       information (mode / version)  │  1
└───────────────────────────────────────────────────────────────────────────────────────┘
```

A `Size: 3` box is **1 content row + 2 frame rows** — frames are drawn inside the box bounds
(`pkg/gui/layout.go:80`-`:84`, `:123`-`:130`) — which is why the comment at
`window_arrangement_helper.go:418`-`:422` says the stash view "by default only contains one
line". `extras` Size 10 is `commandLogSize 8` plus 2 frame rows. Jump prefixes `[1]`…`[5]` and
`[0]` for main come from `pkg/gui/views.go:261`-`:280`.

In `SCREEN_HALF` the main section weight equals the side weight (50/50) and only the focused
side panel is visible; in `SCREEN_FULL` the other side gets weight 0 entirely
(`window_arrangement_helper.go:261`-`:275`, `:456`-`:471`).

> **Native mapping.** The frame is
> `AppFrame { context_bar, body, status_bar }` where `body` is
> `SplitLayout::horizontal().leading_size(side_w).leading(side_column).trailing(main_area)`,
> `side_column` is a `flex_col` of five `Pane`s with the Status and Stash panes at a fixed
> height and the other three `flex_1`, and `main_area` is
> `SplitLayout::vertical().trailing_size(command_log_h)` over an inner
> `SplitLayout` for main + secondary. `ScreenMode::Half`/`Full` are implemented by
> setting the non-focused side panes to zero height and, for `Full`, the side column to zero
> width — matching `:456`-`:471` exactly.
## b. Complete default keybindings, per context

Two sources are combined below:

1. `docs/keybindings/Keybindings_en.md` (auto-generated from `pkg/i18n` by `go generate ./...`,
   see `docs/keybindings/Keybindings_en.md:1`). Everything under
   "Generated keybinding tables" is reproduced from that file **verbatim**.
2. The raw defaults in `pkg/config/user_config.go`, because the generated tables deliberately
   omit the universal navigation keys (arrows, `hjkl`, `1`-`5`, `<tab>`) — the implementer needs
   those too.

### b.1 Universal / navigation defaults not present in the generated tables

Source: `pkg/config/user_config.go:995` (`Keybinding: KeybindingConfig{`), universal block
`pkg/config/user_config.go:996`-`pkg/config/user_config.go:1080`.

| Key(s) | Config field | file:line |
|---|---|---|
| `q` | `Quit` | `pkg/config/user_config.go:997` |
| `<ctrl+c>` | `QuitAlt1` | `pkg/config/user_config.go:998` |
| `<ctrl+z>` | `SuspendApp` | `pkg/config/user_config.go:999` |
| `<esc>` | `Return` | `pkg/config/user_config.go:1000` |
| `Q` | `QuitWithoutChangingDirectory` | `pkg/config/user_config.go:1001` |
| `<tab>` | `TogglePanel` | `pkg/config/user_config.go:1002` |
| `<up>` / `k` | `PrevItem` / `PrevItemAlt` | `pkg/config/user_config.go:1003`, `:1005` |
| `<down>` / `j` | `NextItem` / `NextItemAlt` | `pkg/config/user_config.go:1004`, `:1006` |
| `,` / `.` | `PrevPage` / `NextPage` | `pkg/config/user_config.go:1007`-`:1008` |
| `H` / `L` | `ScrollLeft` / `ScrollRight` | `pkg/config/user_config.go:1009`-`:1010` |
| `<` / `>` (`<home>` / `<end>`) | `GotoTop` / `GotoBottom` (+ `*Alt`) | `pkg/config/user_config.go:1011`-`:1014` |
| `v` | `ToggleRangeSelect` | `pkg/config/user_config.go:1015` |
| `<shift+down>` / `<shift+up>` | `RangeSelectDown` / `RangeSelectUp` | `pkg/config/user_config.go:1016`-`:1017` |
| `<left>` / `h` / `<backtab>` | `PrevBlock`, `PrevBlockAlt`, `PrevBlockAlt2` | `pkg/config/user_config.go:1018`, `:1020`, `:1022` |
| `<right>` / `l` / `<tab>` | `NextBlock`, `NextBlockAlt`, `NextBlockAlt2` | `pkg/config/user_config.go:1019`, `:1021`, `:1023` |
| `1` `2` `3` `4` `5` | `JumpToBlock` (side panel 1..5) | `pkg/config/user_config.go:1024` |
| `0` | `FocusMainView` | `pkg/config/user_config.go:1025` |
| `n` / `N` | `NextMatch` / `PrevMatch` (search results) | `pkg/config/user_config.go:1026`-`:1027` |
| `/` | `StartSearch` | `pkg/config/user_config.go:1028` |
| `<alt+left>` / `<alt+right>` (macOS) | `MoveWordLeft` / `MoveWordRight` | `pkg/config/user_config.go:1029`-`:1030` |
| `<alt+backspace>` / `<alt+delete>` (macOS) | `BackspaceWord` / `ForwardDeleteWord` | `pkg/config/user_config.go:1031`-`:1032` |
| `?` | `OptionMenu` | `pkg/config/user_config.go:1033` |
| `<space>` | `Select` | `pkg/config/user_config.go:1034` |
| `<enter>` | `GoInto` / `Confirm` / `ConfirmMenu` / `ConfirmSuggestion` | `pkg/config/user_config.go:1035`-`:1038` |
| `<meta+enter>` (macOS) / `<ctrl+s>` | `ConfirmInEditor` / `ConfirmInEditorAlt` | `pkg/config/user_config.go:1039`-`:1040` |
| `d` / `n` / `w` / `e` / `o` | `Remove` / `New` / `NewWorktree` / `Edit` / `OpenFile` | `pkg/config/user_config.go:1041`-`:1045` |
| `<ctrl+r>` | `OpenRecentRepos` | `pkg/config/user_config.go:1046` |
| `<pgup>` / `K` / `<ctrl+u>` | `ScrollUpMain` (+ Alt1/Alt2) | `pkg/config/user_config.go:1047`, `:1049`, `:1051` |
| `<pgdown>` / `J` / `<ctrl+d>` | `ScrollDownMain` (+ Alt1/Alt2) | `pkg/config/user_config.go:1048`, `:1050`, `:1052` |
| `:` | `ExecuteShellCommand` | `pkg/config/user_config.go:1053` |
| `m` | `CreateRebaseOptionsMenu` | `pkg/config/user_config.go:1054` |
| `P` / `p` | `Push` / `Pull` | `pkg/config/user_config.go:1055`-`:1056` |
| `R` | `Refresh` | `pkg/config/user_config.go:1057` |
| `<ctrl+p>` | `CreatePatchOptionsMenu` | `pkg/config/user_config.go:1058` |
| `]` / `[` | `NextTab` / `PrevTab` | `pkg/config/user_config.go:1059`-`:1060` |
| `+` / `_` | `NextScreenMode` / `PrevScreenMode` | `pkg/config/user_config.go:1061`-`:1062` |
| `\|` / `\\` | `CycleDiffRenderers` / reverse | `pkg/config/user_config.go:1063`-`:1064` |
| `z` / `Z` | `Undo` / `Redo` | `pkg/config/user_config.go:1065`-`:1066` |
| `<ctrl+s>` | `FilteringMenu` | `pkg/config/user_config.go:1067` |
| `W` / `<ctrl+e>` | `DiffingMenu` / `DiffingMenuAlt` | `pkg/config/user_config.go:1068`-`:1069` |
| `<ctrl+o>` | `CopyToClipboard` | `pkg/config/user_config.go:1070` |
| `<enter>` | `SubmitEditorText` | `pkg/config/user_config.go:1071` |
| `@` | `ExtrasMenu` (command log options) | `pkg/config/user_config.go:1072` |
| `<ctrl+w>` | `ToggleWhitespaceInDiffView` | `pkg/config/user_config.go:1073` |
| `}` / `{` | `Increase/DecreaseContextInDiffView` | `pkg/config/user_config.go:1074`-`:1075` |
| `)` / `(` | `Increase/DecreaseRenameSimilarityThreshold` | `pkg/config/user_config.go:1076`-`:1077` |
| `<ctrl+t>` | `OpenDiffTool` | `pkg/config/user_config.go:1078` |
| `<alt+shift+c>` | `EditConfig` | `pkg/config/user_config.go:1079` |

Per-context default blocks follow immediately after: `Status:` at
`pkg/config/user_config.go:1081`, `Files:` at `pkg/config/user_config.go:1087`, and so on —
each generated table below has a matching struct there.

### b.2 Search / filter prompt context

The search prompt is its own context (`SEARCH_CONTEXT_KEY types.ContextKey = "search"`,
`pkg/gui/context/context.go:45`, registered at `pkg/gui/context/setup.go:107`). While it is
open lazygit **replaces the whole keymap** — no other binding is reachable
(`pkg/gui/keybindings.go:331`-`pkg/gui/keybindings.go:342`; the comment at
`pkg/gui/keybindings.go:332` states the intent explicitly). Its bindings
(`pkg/gui/controllers/search_prompt_controller.go:24`-`:43`):

| Key | Action | file:line |
|---|---|---|
| `<enter>` | Confirm search | `pkg/gui/controllers/search_prompt_controller.go:27` |
| `<esc>` (`Universal.Return`) | Cancel search | `pkg/gui/controllers/search_prompt_controller.go:31` |
| `<up>` / `k` (`Universal.PrevItem`) | Previous search history entry | `pkg/gui/controllers/search_prompt_controller.go:35` |
| `<down>` / `j` (`Universal.NextItem`) | Next search history entry | `pkg/gui/controllers/search_prompt_controller.go:39` |

`/` opens it from any searchable context (`pkg/gui/controllers/search_controller.go:36`-`:44`).

> **Native takeaway:** this "prompt swallows the keymap" behaviour is the single most important
> keymap rule to copy. In gpui, model it as a dedicated key context that is the *only* one
> active while the overlay has focus (see `lazygit-native-brief.md` §b and §f).

### b.3 Generated keybinding tables (verbatim from `docs/keybindings/Keybindings_en.md`)

#### Global keybindings

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+r> `` | Switch to a recent repo |  |
| `` <pgup>, K, <ctrl+u> (fn+up/shift+k) `` | Scroll up main window |  |
| `` <pgdown>, J, <ctrl+d> (fn+down/shift+j) `` | Scroll down main window |  |
| `` @ `` | View command log options | View options for the command log e.g. show/hide the command log and focus the command log. |
| `` P `` | Push | Push the current branch to its upstream branch. If no upstream is configured, you will be prompted to configure an upstream branch. |
| `` p `` | Pull | Pull changes from the remote for the current branch. If no upstream is configured, you will be prompted to configure an upstream branch. |
| `` ) `` | Increase rename similarity threshold | Increase the similarity threshold for a deletion and addition pair to be treated as a rename.<br><br>The default can be changed in the config file with the key 'git.renameSimilarityThreshold'. |
| `` ( `` | Decrease rename similarity threshold | Decrease the similarity threshold for a deletion and addition pair to be treated as a rename.<br><br>The default can be changed in the config file with the key 'git.renameSimilarityThreshold'. |
| `` } `` | Increase diff context size | Increase the amount of the context shown around changes in the diff view.<br><br>The default can be changed in the config file with the key 'git.diffContextSize'. |
| `` { `` | Decrease diff context size | Decrease the amount of the context shown around changes in the diff view.<br><br>The default can be changed in the config file with the key 'git.diffContextSize'. |
| `` : `` | Execute shell command | Bring up a prompt where you can enter a shell command to execute. |
| `` <ctrl+p> `` | View custom patch options |  |
| `` m `` | View merge/rebase options | View options to abort/continue/skip the current merge/rebase. |
| `` R `` | Refresh | Refresh the git state (i.e. run `git status`, `git branch`, etc in background to update the contents of panels). This does not run `git fetch`. |
| `` + `` | Next screen mode (normal/half/fullscreen) |  |
| `` _ `` | Prev screen mode |  |
| `` \| `` | Cycle diff renderers | Choose the next renderer in the list of configured diff renderers. |
| `` \ `` | Cycle diff renderers (reverse) | Choose the previous renderer in the list of configured diff renderers. |
| `` <esc> `` | Cancel |  |
| `` ? `` | Open keybindings menu |  |
| `` <ctrl+s> `` | View filter options | View options for filtering the commit log, so that only commits matching the filter are shown. |
| `` W, <ctrl+e> `` | View diffing options | View options relating to diffing two refs e.g. diffing against selected ref, entering ref to diff against, and reversing the diff direction. |
| `` q, <ctrl+c> `` | Quit |  |
| `` <ctrl+z> `` | Suspend the application |  |
| `` <ctrl+w> `` | Toggle whitespace | Toggle whether or not whitespace changes are shown in the diff view.<br><br>The default can be changed in the config file with the key 'git.ignoreWhitespaceInDiffView'. |
| `` <alt+shift+c> `` | Edit config file | Open file in external editor. |
| `` z `` | Undo | The reflog will be used to determine what git command to run to undo the last git command. This does not include changes to the working tree; only commits are taken into consideration. |
| `` Z `` | Redo | The reflog will be used to determine what git command to run to redo the last git command. This does not include changes to the working tree; only commits are taken into consideration. |

#### List panel navigation

| Key | Action | Info |
|-----|--------|-------------|
| `` , `` | Previous page |  |
| `` . `` | Next page |  |
| `` <, <home> `` | Scroll to top |  |
| `` >, <end> `` | Scroll to bottom |  |
| `` v `` | Toggle range select |  |
| `` <shift+down> `` | Range select down |  |
| `` <shift+up> `` | Range select up |  |
| `` / `` | Search the current view by text |  |
| `` H `` | Scroll left |  |
| `` L `` | Scroll right |  |
| `` ] `` | Next tab |  |
| `` [ `` | Previous tab |  |

#### Commit files

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy path to clipboard |  |
| `` y `` | Copy to clipboard |  |
| `` c `` | Checkout | Checkout file. This replaces the file in your working tree with the version from the selected commit. |
| `` d `` | Discard | Discard this commit's changes to this file. This runs an interactive rebase in the background, so you may get a merge conflict if a later commit also changes this file. |
| `` o `` | Open file | Open file in default application. |
| `` e `` | Edit | Open file in external editor. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` <space> `` | Toggle file included in patch | Toggle whether the file is included in the custom patch. See https://github.com/jesseduffield/lazygit#rebase-magic-custom-patches. |
| `` a `` | Toggle all files | Add/remove all commit's files to custom patch. See https://github.com/jesseduffield/lazygit#rebase-magic-custom-patches. |
| `` <enter> `` | Enter file / Toggle directory collapsed | If a file is selected, enter the file so that you can add/remove individual lines to the custom patch. If a directory is selected, toggle the directory. |
| `` ` `` | Toggle file tree view | Toggle file view between flat and tree layout. Flat layout shows all file paths in a single list, tree layout groups files by directory.<br><br>The default can be changed in the config file with the key 'gui.showFileTree'. |
| `` - `` | Collapse all files | Collapse all directories in the files tree |
| `` = `` | Expand all files | Expand all directories in the file tree |
| `` 0 `` | Focus main view |  |
| `` / `` | Filter the current view by text |  |

#### Commit summary

| Key | Action | Info |
|-----|--------|-------------|
| `` <enter> `` | Confirm |  |
| `` <esc> `` | Close |  |

#### Commits

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy abbreviated commit hash to clipboard |  |
| `` <ctrl+r> `` | Reset copied (cherry-picked) commits selection |  |
| `` b `` | View bisect options |  |
| `` s `` | Squash | Squash the selected commit into the commit below it. The selected commit's message will be appended to the commit below it. |
| `` f `` | Fixup | Meld the selected commit into the commit below it. Similar to squash, but the selected commit's message will be discarded. |
| `` c `` | Set fixup message | Set the message option for the fixup commit. The -C option means to use this commit's message instead of the target commit's message. |
| `` r `` | Reword | Reword the selected commit's message. |
| `` R `` | Reword with editor |  |
| `` d `` | Drop | Drop the selected commit. This will remove the commit from the branch via a rebase. If the commit makes changes that later commits depend on, you may need to resolve merge conflicts. |
| `` e `` | Edit (start interactive rebase) | Edit the selected commit. Use this to start an interactive rebase from the selected commit. When already mid-rebase, this will mark the selected commit for editing, which means that upon continuing the rebase, the rebase will pause at the selected commit to allow you to make changes. |
| `` i `` | Start interactive rebase | Start an interactive rebase for the commits on your branch. This will include all commits from the HEAD commit down to the first merge commit or main branch commit.<br>If you would instead like to start an interactive rebase from the selected commit, press `e`. |
| `` p `` | Pick | Mark the selected commit to be picked (when mid-rebase). This means that the commit will be retained upon continuing the rebase. |
| `` F `` | Create fixup commit | Create 'fixup!' commit for the selected commit. Later on, you can press `S` on this same commit to apply all above fixup commits. |
| `` S `` | Apply fixup commits | Squash all 'fixup!' commits, either above the selected commit, or all in current branch (autosquash). |
| `` <ctrl+j>, <alt+down> `` | Move commit down one |  |
| `` <ctrl+k>, <alt+up> `` | Move commit up one |  |
| `` V `` | Paste (cherry-pick) |  |
| `` B `` | Mark as base commit for rebase | Select a base commit for the next rebase. When you rebase onto a branch, only commits above the base commit will be brought across. This uses the `git rebase --onto` command. |
| `` A `` | Amend | Amend commit with staged changes. If the selected commit is the HEAD commit, this will perform `git commit --amend`. Otherwise the commit will be amended via a rebase. |
| `` a `` | Amend commit attribute | Set/Reset commit author or set co-author. |
| `` t `` | Revert | Create a revert commit for the selected commit, which applies the selected commit's changes in reverse. |
| `` T `` | Tag commit | Create a new tag pointing at the selected commit. You'll be prompted to enter a tag name and optional description. |
| `` <ctrl+l> `` | View log options | View options for commit log e.g. changing sort order, hiding the git graph, showing the whole git graph. |
| `` G `` | Open pull request in browser |  |
| `` <space> `` | Checkout | Checkout the selected commit as a detached HEAD. |
| `` y `` | Copy commit attribute to clipboard | Copy commit attribute to clipboard (e.g. hash, URL, diff, message, author). |
| `` o `` | Open commit in browser |  |
| `` n `` | Create new branch off of commit |  |
| `` N `` | Move commits to new branch | Create a new branch and move the unpushed commits of the current branch to it. Useful if you meant to start new work and forgot to create a new branch first.<br><br>Note that this disregards the selection, the new branch is always created either from the main branch or stacked on top of the current branch (you get to choose which). |
| `` w `` | New worktree |  |
| `` g `` | Reset | View reset options (soft/mixed/hard) for resetting onto selected item. |
| `` C `` | Copy (cherry-pick) | Mark commit as copied. Then, within the local commits view, you can press `V` to paste (cherry-pick) the copied commit(s) into your checked out branch. At any time you can press `<esc>` to cancel the selection. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` * `` | Select commits of current branch |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View files |  |
| `` / `` | Search the current view by text |  |

#### Confirmation panel

| Key | Action | Info |
|-----|--------|-------------|
| `` <enter> `` | Confirm |  |
| `` <esc> `` | Close/Cancel |  |
| `` <ctrl+o> `` | Copy to clipboard |  |

#### Files

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy path to clipboard |  |
| `` <space> `` | Stage | Toggle staged for selected file. |
| `` <ctrl+b> `` | Filter files by status |  |
| `` y `` | Copy to clipboard |  |
| `` c `` | Commit | Commit staged changes. |
| `` w `` | Commit changes without pre-commit hook |  |
| `` A `` | Amend last commit |  |
| `` C `` | Commit changes using git editor |  |
| `` <ctrl+f> `` | Find base commit for fixup | Find the commit that your current changes are building upon, for the sake of amending/fixing up the commit. This spares you from having to look through your branch's commits one-by-one to see which commit should be amended/fixed up. See docs: <https://github.com/jesseduffield/lazygit/tree/master/docs/Fixup_Commits.md> |
| `` e `` | Edit | Open file in external editor. |
| `` o `` | Open file | Open file in default application. |
| `` i `` | Ignore or exclude file |  |
| `` r `` | Refresh files |  |
| `` s `` | Stash | Stash all changes. For other variations of stashing, use the view stash options keybinding. |
| `` S `` | View stash options | View stash options (e.g. stash all, stash staged, stash unstaged). |
| `` a `` | Stage all | Toggle staged/unstaged for all files in working tree. |
| `` <enter> `` | Stage lines / Collapse directory | If the selected item is a file, focus the staging view so you can stage individual hunks/lines. If the selected item is a directory, collapse/expand it. |
| `` d `` | Discard | View options for discarding changes to the selected file. |
| `` g `` | View upstream reset options |  |
| `` D `` | Reset | View reset options for working tree (e.g. nuking the working tree). |
| `` ` `` | Toggle file tree view | Toggle file view between flat and tree layout. Flat layout shows all file paths in a single list, tree layout groups files by directory.<br><br>The default can be changed in the config file with the key 'gui.showFileTree'. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` M `` | View merge conflict options | View options for resolving merge conflicts. |
| `` f `` | Fetch | Fetch changes from remote. |
| `` - `` | Collapse all files | Collapse all directories in the files tree |
| `` = `` | Expand all files | Expand all directories in the file tree |
| `` 0 `` | Focus main view |  |
| `` / `` | Filter the current view by text |  |

#### Input prompt

| Key | Action | Info |
|-----|--------|-------------|
| `` <enter> `` | Confirm |  |
| `` <esc> `` | Close/Cancel |  |

#### Local branches

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy branch name to clipboard |  |
| `` i `` | Show git-flow options |  |
| `` <space> `` | Checkout | Checkout selected item. |
| `` n `` | New branch |  |
| `` N `` | Move commits to new branch | Create a new branch and move the unpushed commits of the current branch to it. Useful if you meant to start new work and forgot to create a new branch first.<br><br>Note that this disregards the selection, the new branch is always created either from the main branch or stacked on top of the current branch (you get to choose which). |
| `` w `` | New worktree |  |
| `` o `` | Create pull request |  |
| `` O `` | View create pull request options |  |
| `` G `` | Open pull request in browser |  |
| `` <ctrl+y> `` | Copy pull request URL to clipboard |  |
| `` c `` | Checkout by name | Checkout by name. In the input box you can enter '-' to switch to the previous branch. |
| `` - `` | Checkout previous branch |  |
| `` F `` | Force checkout | Force checkout selected branch. This will discard all local changes in your working directory before checking out the selected branch. |
| `` d `` | Delete | View delete options for local/remote branch. |
| `` r `` | Rebase | Rebase the checked-out branch onto the selected branch. |
| `` M `` | Merge | View options for merging the selected item into the current branch (regular merge, squash merge) |
| `` f `` | Fast-forward | Fast-forward selected branch from its upstream. |
| `` T `` | New tag |  |
| `` s `` | Sort order |  |
| `` g `` | Reset |  |
| `` R `` | Rename branch |  |
| `` u `` | View upstream options | View options relating to the branch's upstream e.g. setting/unsetting the upstream and resetting to the upstream. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View commits |  |
| `` / `` | Filter the current view by text |  |

#### Main panel (merging)

| Key | Action | Info |
|-----|--------|-------------|
| `` <space> `` | Pick hunk |  |
| `` b `` | Pick both hunks |  |
| `` <up>, k `` | Previous hunk |  |
| `` <down>, j `` | Next hunk |  |
| `` <left>, h `` | Previous conflict |  |
| `` <right>, l `` | Next conflict |  |
| `` z `` | Undo | Undo last merge conflict resolution. |
| `` e `` | Edit file | Open file in external editor. |
| `` o `` | Open file | Open file in default application. |
| `` M `` | View merge conflict options | View options for resolving merge conflicts. |
| `` <esc> `` | Return to files panel |  |

#### Main panel (normal)

| Key | Action | Info |
|-----|--------|-------------|
| `` <mouse wheel down> (fn+up) `` | Scroll down |  |
| `` <mouse wheel up> (fn+down) `` | Scroll up |  |
| `` <tab> `` | Switch view | Switch to other view (staged/unstaged changes). |
| `` <esc> `` | Exit back to side panel |  |
| `` / `` | Search the current view by text |  |

#### Main panel (patch building)

| Key | Action | Info |
|-----|--------|-------------|
| `` <left>, h `` | Go to previous hunk |  |
| `` <right>, l `` | Go to next hunk |  |
| `` v `` | Toggle range select |  |
| `` a `` | Toggle hunk selection | Toggle line-by-line vs. hunk selection mode. |
| `` <ctrl+o> `` | Copy selected text to clipboard |  |
| `` o `` | Open file | Open file in default application. |
| `` e `` | Edit file | Open file in external editor. |
| `` <space> `` | Toggle lines in patch |  |
| `` d `` | Remove lines from commit | Remove the selected lines from this commit. This runs an interactive rebase in the background, so you may get a merge conflict if a later commit also changes these lines. |
| `` <esc> `` | Exit custom patch builder |  |
| `` / `` | Search the current view by text |  |

#### Main panel (staging)

| Key | Action | Info |
|-----|--------|-------------|
| `` <left>, h `` | Go to previous hunk |  |
| `` <right>, l `` | Go to next hunk |  |
| `` v `` | Toggle range select |  |
| `` a `` | Toggle hunk selection | Toggle line-by-line vs. hunk selection mode. |
| `` <ctrl+o> `` | Copy selected text to clipboard |  |
| `` <space> `` | Stage | Toggle selection staged / unstaged. |
| `` d `` | Discard | When unstaged change is selected, discard the change using `git reset`. When staged change is selected, unstage the change. |
| `` o `` | Open file | Open file in default application. |
| `` e `` | Edit file | Open file in external editor. |
| `` <esc> `` | Return to files panel |  |
| `` <tab> `` | Switch view | Switch to other view (staged/unstaged changes). |
| `` E `` | Edit hunk | Edit selected hunk in external editor. |
| `` c `` | Commit | Commit staged changes. |
| `` w `` | Commit changes without pre-commit hook |  |
| `` C `` | Commit changes using git editor |  |
| `` <ctrl+f> `` | Find base commit for fixup | Find the commit that your current changes are building upon, for the sake of amending/fixing up the commit. This spares you from having to look through your branch's commits one-by-one to see which commit should be amended/fixed up. See docs: <https://github.com/jesseduffield/lazygit/tree/master/docs/Fixup_Commits.md> |
| `` / `` | Search the current view by text |  |

#### Menu

| Key | Action | Info |
|-----|--------|-------------|
| `` <enter> `` | Execute |  |
| `` <esc> `` | Close/Cancel |  |
| `` / `` | Filter the current view by text |  |

#### Reflog

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy abbreviated commit hash to clipboard |  |
| `` <space> `` | Checkout | Checkout the selected commit as a detached HEAD. |
| `` y `` | Copy commit attribute to clipboard | Copy commit attribute to clipboard (e.g. hash, URL, diff, message, author). |
| `` o `` | Open commit in browser |  |
| `` n `` | Create new branch off of commit |  |
| `` N `` | Move commits to new branch | Create a new branch and move the unpushed commits of the current branch to it. Useful if you meant to start new work and forgot to create a new branch first.<br><br>Note that this disregards the selection, the new branch is always created either from the main branch or stacked on top of the current branch (you get to choose which). |
| `` w `` | New worktree |  |
| `` g `` | Reset | View reset options (soft/mixed/hard) for resetting onto selected item. |
| `` C `` | Copy (cherry-pick) | Mark commit as copied. Then, within the local commits view, you can press `V` to paste (cherry-pick) the copied commit(s) into your checked out branch. At any time you can press `<esc>` to cancel the selection. |
| `` <ctrl+r> `` | Reset copied (cherry-picked) commits selection |  |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` * `` | Select commits of current branch |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View commits |  |
| `` / `` | Filter the current view by text |  |

#### Remote branches

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy branch name to clipboard |  |
| `` <space> `` | Checkout | Checkout a new local branch based on the selected remote branch, or the remote branch as a detached head. |
| `` n `` | New branch |  |
| `` w `` | New worktree |  |
| `` M `` | Merge | View options for merging the selected item into the current branch (regular merge, squash merge) |
| `` r `` | Rebase | Rebase the checked-out branch onto the selected branch. |
| `` d `` | Delete | Delete the remote branch from the remote. |
| `` u `` | Set as upstream | Set the selected remote branch as the upstream of the checked-out branch. |
| `` s `` | Sort order |  |
| `` g `` | Reset | View reset options (soft/mixed/hard) for resetting onto selected item. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View commits |  |
| `` / `` | Filter the current view by text |  |

#### Remotes

| Key | Action | Info |
|-----|--------|-------------|
| `` <enter> `` | View branches |  |
| `` n `` | New remote |  |
| `` d `` | Remove | Remove the selected remote. Any local branches tracking a remote branch from the remote will be unaffected. |
| `` e `` | Edit | Edit the selected remote's name or URL. |
| `` f `` | Fetch | Fetch updates from the remote repository. This retrieves new commits and branches without merging them into your local branches. |
| `` F `` | Add fork remote | Quickly add a fork remote by replacing the owner in the origin URL and optionally check out a branch from new remote. |
| `` / `` | Filter the current view by text |  |

#### Secondary

| Key | Action | Info |
|-----|--------|-------------|
| `` <tab> `` | Switch view | Switch to other view (staged/unstaged changes). |
| `` <esc> `` | Exit back to side panel |  |
| `` / `` | Search the current view by text |  |

#### Stash

| Key | Action | Info |
|-----|--------|-------------|
| `` <space> `` | Apply | Apply the stash entry to your working directory. |
| `` g `` | Pop | Apply the stash entry to your working directory and remove the stash entry. |
| `` d `` | Drop | Remove the stash entry from the stash list. |
| `` n `` | New branch | Create a new branch from the selected stash entry. This works by git checking out the commit that the stash entry was created from, creating a new branch from that commit, then applying the stash entry to the new branch as an additional commit. |
| `` w `` | New worktree |  |
| `` r `` | Rename stash |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View files |  |
| `` / `` | Filter the current view by text |  |

#### Status

| Key | Action | Info |
|-----|--------|-------------|
| `` e `` | Edit config file | Open file in external editor. |
| `` u `` | Check for update |  |
| `` <enter> `` | Switch to a recent repo |  |
| `` a `` | Show/cycle all branch logs |  |
| `` A `` | Show/cycle all branch logs (reverse) |  |
| `` 0 `` | Focus main view |  |

#### Sub-commits

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy abbreviated commit hash to clipboard |  |
| `` <space> `` | Checkout | Checkout the selected commit as a detached HEAD. |
| `` y `` | Copy commit attribute to clipboard | Copy commit attribute to clipboard (e.g. hash, URL, diff, message, author). |
| `` o `` | Open commit in browser |  |
| `` n `` | Create new branch off of commit |  |
| `` N `` | Move commits to new branch | Create a new branch and move the unpushed commits of the current branch to it. Useful if you meant to start new work and forgot to create a new branch first.<br><br>Note that this disregards the selection, the new branch is always created either from the main branch or stacked on top of the current branch (you get to choose which). |
| `` w `` | New worktree |  |
| `` g `` | Reset | View reset options (soft/mixed/hard) for resetting onto selected item. |
| `` C `` | Copy (cherry-pick) | Mark commit as copied. Then, within the local commits view, you can press `V` to paste (cherry-pick) the copied commit(s) into your checked out branch. At any time you can press `<esc>` to cancel the selection. |
| `` <ctrl+r> `` | Reset copied (cherry-picked) commits selection |  |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` * `` | Select commits of current branch |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View files |  |
| `` / `` | Search the current view by text |  |

#### Submodules

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy submodule name to clipboard |  |
| `` <enter> `` | Enter | Enter submodule. After entering the submodule, you can press `<esc>` to escape back to the parent repo. |
| `` d `` | Remove | Remove the selected submodule and its corresponding directory. |
| `` u `` | Update | Update selected submodule. |
| `` n `` | New submodule |  |
| `` e `` | Update submodule URL |  |
| `` i `` | Initialize | Initialize the selected submodule to prepare for fetching. You probably want to follow this up by invoking the 'update' action to fetch the submodule. |
| `` b `` | View bulk submodule options |  |
| `` / `` | Filter the current view by text |  |

#### Tags

| Key | Action | Info |
|-----|--------|-------------|
| `` <ctrl+o> `` | Copy tag to clipboard |  |
| `` <space> `` | Checkout | Checkout the selected tag as a detached HEAD. |
| `` n `` | New tag | Create new tag from current commit. You'll be prompted to enter a tag name and optional description. |
| `` w `` | New worktree |  |
| `` d `` | Delete | View delete options for local/remote tag. |
| `` P `` | Push tag | Push the selected tag to a remote. You'll be prompted to select a remote. |
| `` g `` | Reset | View reset options (soft/mixed/hard) for resetting onto selected item. |
| `` <ctrl+t> `` | Open external diff tool (git difftool) |  |
| `` 0 `` | Focus main view |  |
| `` <enter> `` | View commits |  |
| `` / `` | Filter the current view by text |  |

#### Worktrees

| Key | Action | Info |
|-----|--------|-------------|
| `` n `` | New worktree |  |
| `` <space> `` | Switch | Switch to the selected worktree. |
| `` o `` | Open in editor |  |
| `` d `` | Remove | Remove the selected worktree. This will both delete the worktree's directory, as well as metadata about the worktree in the .git directory. |
| `` / `` | Filter the current view by text |  |
## c. Visual language worth mirroring

### c.0 The column machinery every list shares

- Each row is a `[]string` of **columns**. `utils.RenderDisplayStrings` pads all but the last to
  the maximum ANSI-stripped width and appends **exactly one space**
  (`pkg/gui/context/list_renderer.go:134`-`:137`, `pkg/utils/formatting.go:55`-`:87`, where
  `columnPositions[i+1] = columnPositions[i] + padWidth + 1` at `:76`); width is
  `StringWidth(Decolorise(cell))` (`:159`-`:165`).
- **Columns that are blank in every row are removed entirely** — `excludeBlankColumns`
  (`pkg/utils/formatting.go:90`-`:126`). This is why the graph, bisect, date and icon columns
  silently vanish.
- Every column is left-aligned; `getColumnAlignments` is nil for every list except the menu
  (`pkg/gui/context/list_renderer.go:33`-`:34`, `pkg/gui/context/menu_context.go:44`).
- Section headers are `fmt.Sprintf("─── %s", label)`
  (`pkg/gui/context/list_renderer.go:13`-`:15`), with labels `"Pending rebase todos"`,
  `"Pending cherry-picks"`, `"Pending reverts"`, `"Commits"`
  (`pkg/i18n/english.go:1541`-`:1544`, used at
  `pkg/gui/context/local_commits_context.go:86`, `:100`-`:105`, `:127`).
- The commit drop indicator is
  `style.FgCyan.SetBold().Sprintf("━━━━━━ %s ━━━━━━", label)` at column 6
  (`pkg/gui/context/local_commits_context.go:197`-`:199`).
- `TruncateWithEllipsis` is a no-op when width ≤ limit; `limit <= 2` gives
  `strings.Repeat(".", limit)`; otherwise it cuts to `limit-1` and appends `"…"`
  (`pkg/utils/formatting.go:179`-`:200`).
- **Range selection has no printable marker** — it is the view's range highlight
  (`pkg/gui/context/list_context_trait.go:63`-`:69`).

> **Native mapping.** `Row` + `RowColumn::fixed_ch` reproduces this padding model directly, and
> `ColumnLadder`/`RowColumn::resolved` is the kit's answer to the blank-column elision: decide
> which columns exist once, per width, rather than per row.

### c.1 The files panel

**Where the two status characters come from.**
`git status <--untracked-files=X> --porcelain -z [--no-renames|--find-renames=N%]`
(`pkg/commands/git_commands/file_loader.go:217`-`:226`); each NUL record is sliced
`Change: original[:2]`, `Path: original[3:]` (`:252`-`:257`), and `FileStatus.Change` is
documented `// ??, MM, AM, ...` (`:201`). `R`/`C` records pull the next record as the previous
path and rebuild the status as
`fmt.Sprintf("%s %s -> %s", status.Change, status.PreviousPath, status.Path)` (`:260`-`:265`).

```go
// pkg/commands/models/file.go:26
ShortStatus string // e.g. 'AD', ' A', 'M ', '??'
```

`deriveStatusFields` (`pkg/commands/models/file.go:150`-`:168`) — **the first char is the index,
the second the worktree**:

```go
stagedChange   := shortStatus[0:1]   // :151  <- INDEX / staged column
unstagedChange := shortStatus[1:2]   // :152  <- WORKTREE / unstaged column
tracked        := !lo.Contains([]string{"??", "A ", "AM"}, shortStatus)                   // :153
hasStagedChanges := !lo.Contains([]string{" ", "U", "?"}, stagedChange)                   // :154
hasInlineMergeConflicts := lo.Contains([]string{"UU", "AA"}, shortStatus)                 // :155
hasMergeConflicts := hasInlineMergeConflicts ||
        lo.Contains([]string{"DD","AU","UA","UD","DU"}, shortStatus)                      // :156
HasUnstagedChanges: unstagedChange != " "                                                 // :160
Deleted: unstagedChange == "D" || stagedChange == "D"                                     // :162
Added:   unstagedChange == "A" || !tracked                                                // :163
```

Directory nodes aggregate from their descendants: `GetHasUnstagedChanges` is `SomeFile(...)`
(`pkg/gui/filetree/file_node.go:32`-`:34`), `GetHasStagedChanges` (`:45`-`:47`), and
`GetHasInlineMergeConflicts` re-checks the file on disk with
`mergeconflicts.FileHasConflictMarkers` (`:49`-`:57`).

**The file name colour has exactly three cases** (`pkg/gui/presentation/files.go:134`-`:140`):

```go
if hasStagedChanges && !hasUnstagedChanges { nameColor = style.FgGreen }        // :135 fully staged
else if hasStagedChanges                   { nameColor = style.FgYellow }       // :137 MIXED staged+unstaged
else                                       { nameColor = theme.DefaultTextColor } // :139
```

There is **no name-colour special case for conflicts, deletions or untracked files**.

**The two status characters are coloured independently** — `formatFileStatus`
(`pkg/gui/presentation/files.go:184`-`:201`):

```go
firstChar := file.ShortStatus[0:1]
firstCharCl := style.FgGreen
switch firstChar {
case "?": firstCharCl = theme.UnstagedChangesColor
case " ": firstCharCl = restColor            // restColor == nameColor
}
secondChar := file.ShortStatus[1:2]
secondCharCl := theme.UnstagedChangesColor
if secondChar == " " { secondCharCl = restColor }
return firstCharCl.Sprint(firstChar) + secondCharCl.Sprint(secondChar)
```

Consequences, worth spelling out because they are easy to get wrong:

| status | first char | second char |
|---|---|---|
| `??` | red (`UnstagedChangesColor`) | red |
| `A ` / `M ` / `D ` / `R ` / `C ` | green | name-coloured space |
| ` M` | name-coloured space | red |
| `MM` | **green `M`** | **red `M`** |
| `UU` / `AA` / `DD` | **green** (the switch handles only `"?"` and `" "`) | red |

`theme.UnstagedChangesColor` defaults to `unstagedChangesColor: []string{"red"}`
(`pkg/config/user_config.go:895`, wired at `pkg/theme/theme.go:66`-`:67`).

The numstat suffix (`gui.showNumstatInFilesView`, default `false`,
`pkg/config/user_config.go:908`) is `style.FgGreen.Sprintf("+%d", linesAdded)` and
`style.FgRed.Sprintf("-%d", linesDeleted)` joined by a space
(`pkg/gui/presentation/files.go:203`-`:218`, appended with a leading space at `:177`).

**Tree mode versus flat mode.** Indentation is **plain spaces only** —
`indentation := strings.Repeat("  ", visualDepth)`
(`pkg/gui/presentation/files.go:132`; commit files at `:229`). **There is no `├─` / `└─` / `│`
line art anywhere in lazygit.**

The collapse glyphs (`pkg/gui/presentation/files.go:17`-`:20`):

```go
const (
	EXPANDED_ARROW  = "▼"
	COLLAPSED_ARROW = "▶"
)
```

They appear on directory rows only (`file == nil`), styled `nameColor` and followed by a space
(`:142`-`:151`). The click target is `arrowStartCol := visualDepth * 2`,
`arrowEndCol := arrowStartCol + 1` (`pkg/gui/controllers/files_controller.go:232`-`:237`).

Line shapes:

- file: `<2*visualDepth spaces><statusChar0><statusChar1><space>[<icon><space>]<name>[" (submodule)"][" +N -M"]`
  (`pkg/gui/presentation/files.go:156`, `:163`-`:179`; the `" (submodule)"` literal at `:172`)
- directory: `<2*visualDepth spaces><▼|▶><space>[<icon><space>]<name>` (`:143`-`:151`, `:163`-`:169`)
- rename: `prevName + " → " + name` (U+2192, spaces both sides, `:323`); the root item is the
  literal `"/"` (`:304`-`:309`); intra-directory renames shorten the previous path (`:318`-`:321`)
- `utils.EscapeSpecialChars` maps `\n \r \t \b \f \v` to their escaped forms
  (`pkg/utils/lines.go:41`-`:50`)

Tree building is `BuildTreeFromFiles` → `root.Sort(cmp)` → `root.Compress()`
(`pkg/gui/filetree/build_tree.go:10`-`:68`, `:64`-`:65`).

**Single-child directory compression is what replaces branch glyphs** —
`Node.CompressionLevel` (`pkg/gui/filetree/node.go:26`-`:37`), `compressAux()` (`:320`-`:342`):

```go
for len(grandchildren) == 1 && !grandchildren[0].IsFile() {
    grandchildren[0].CompressionLevel = children[i].CompressionLevel + 1
    children[i] = grandchildren[0]
    grandchildren = children[i].Children
}
```

Recursion advances tree depth by `treeDepth+1+node.CompressionLevel` but *visual* depth by only
1 (`pkg/gui/presentation/files.go:106`, rationale `:73`-`:77`) — so `src/main/java` collapses to
one row at one indent level.

Flat mode is `BuildFlatTreeFromFiles`: it builds the tree, takes `GetLeaves()`, then stably
re-sorts merge-conflict files first, then tracked, then untracked
(`pkg/gui/filetree/build_tree.go:130`-`:170`). The switch is `FileTree.SetTree()`
(`pkg/gui/filetree/file_tree.go:198`-`:208`); the renderer short-circuits collapsed children
(`pkg/gui/presentation/files.go:101`-`:103`).

Config: `showFileTree` default `true`, toggled by `` ` `` non-persistently
(`pkg/config/user_config.go:151`-`:153`, `:904`, `:1100`); `showRootItemInFileTree` `true`
(`:905`); `fileTreeSortOrder` `"mixed"` (`mixed|filesFirst|foldersFirst`, `:906`, comparator
`pkg/gui/filetree/node.go:77`-`:113`); collapse-all `-`, expand-all `=` (`:1105`-`:1106`);
status filters `DisplayAll/Staged/Unstaged/Tracked/Untracked/Conflicted`
(`pkg/gui/filetree/file_tree.go:13`-`:23`, `:95`-`:123`).

**Icons** are enabled only when `gui.nerdFontsVersion` is `"2"`/`"3"` **and** `gui.showFileIcons`
(`pkg/gui/context/working_tree_context.go:30`,
`pkg/gui/presentation/icons/icons.go:20`-`:35`; defaults `""` / `true`,
`pkg/config/user_config.go:911`-`:912`). Tables: `nameIconMap`
(`pkg/gui/presentation/icons/file_icons.go:20`-`:221`, case-sensitive) and `extIconMap`
(`:223`-`:757`), with `DEFAULT_FILE_ICON = {"", "#878787"}` and friends (`:14`-`:18`).
**The icon colour is the table's hex and is never tinted by staged state**:

```go
// pkg/gui/presentation/files.go:163-167
paint := color.HEX(icon.Color, false)
output += paint.Sprint(icon.Icon) + nameColor.Sprint(" ")
```

Git icons, verbatim (`pkg/gui/presentation/icons/git_icons.go:9`-`:19`):

```go
BRANCH_ICON                  = "\U000f062c" // 󰘬
DETACHED_HEAD_ICON           = ""
TAG_ICON                     = ""
COMMIT_ICON                  = "\U000f0718" // 󰜘
MERGE_COMMIT_ICON            = "\U000f062d" // 󰘭
DEFAULT_REMOTE_ICON          = "\U000f02a2" // 󰊢
STASH_ICON                   = ""
LINKED_WORKTREE_ICON         = "\U000f0339" // 󰌹
MISSING_LINKED_WORKTREE_ICON = "\U000f033a" // 󰌺
```

**The commit-files panel** (custom patch) uses a different glyph set — `getCommitFileLine`
(`pkg/gui/presentation/files.go:220`-`:283`): `patch.WHOLE` → `style.FgGreen` + `"●"`;
`patch.PART` → `style.FgYellow` + `"◐"`; `patch.UNSELECTED` → `theme.DefaultTextColor` plus the
raw `ChangeStatus` letter coloured by `getColorForChangeStatus` (`"A"`→FgGreen,
`"M","R"`→FgYellow, `"D"`→UnstagedChangesColor, `"C"`→FgCyan, `"T"`→FgMagenta) —
`:238`-`:268`, `:285`-`:300`.

> **Native mapping.** Render the two status characters as **two sibling `Text::data` runs** in a
> `RowColumn::fixed_ch(2.0)` leading column — never one string, because they carry different
> colours (brief §f.5 gotcha 5). Use `Text::data` in `ansi[2]`/`ansi[1]` for green/red. For tree
> mode, `RowColumn::flex` with a leading `"  ".repeat(depth)` prefix baked into the string is
> faithful and cheapest.

### c.2 Branch rows

Column order, in append order (`pkg/gui/presentation/branches.go:140`-`:191`):

1. `recencyColor.Sprint(b.Recency)` (`:141`)
2. `coloredPrIcon` — often empty, so the column is dropped (`:160`)
3. the short hash, only when `fullDescription || gui.showBranchCommitHash` (`:162`-`:164`)
4. `coloredName` = name + worktree marker + branch status + right-aligned divergence (`:179`)
5. full-description only: `fmt.Sprintf("%s %s", style.FgYellow.Sprint(b.UpstreamRemote), style.FgYellow.Sprint(b.UpstreamBranch))` (`:184`-`:187`)
6. full-description only: `utils.TruncateWithEllipsis(b.Subject, 60)` (`:188`)

Real test rows: `[]string{"1m", "", "branch_name"}` and
`[]string{"1m", "", "12345678", "branch_name ✓", "origin branch_name", "commit title"}`
(`pkg/gui/presentation/branches_test.go:135`, `:263`). **There is no branch-icon column** —
`icons.IconForBranch` exists (`pkg/gui/presentation/icons/git_icons.go:49`-`:54`) but the branch
list does not use it.

**Recency is always three characters plus one space** —
`// Recency is always three characters, plus one for the space` /
`availableWidth := viewWidth - 4` (`:64`-`:65`). Colour is `style.FgCyan`, or `style.FgGreen`
when `b.Recency == "  *"` (`:135`-`:138`).

> **The current-branch marker IS the recency string `"  *"`** — two spaces plus an asterisk
> (`pkg/commands/git_commands/branch_loader.go:108`, `:118`). There is no separate marker glyph.

Other recency values come from `utils.UnixToTimeAgo` → `"%d%s"` with the unit labels
`s m h d w M y` (`pkg/utils/date.go:28`-`:36`, `:45`-`:55`).

**Upstream ahead/behind — the exact characters are U+2193 `↓` and U+2191 `↑`, not triangles**
(`BranchStatus`, `pkg/gui/presentation/branches.go:221`-`:251`):

```go
result = style.FgRed.Sprint(tr.UpstreamGone)                                         // :236  "(upstream gone)"
result = style.FgGreen.Sprint("✓")                                                   // :238  matches upstream
result = style.FgMagenta.Sprint("?")                                                 // :240  remote branch not stored locally
result = style.FgYellow.Sprintf("↓%s↑%s", branch.BehindForPull, branch.AheadForPull)  // :242
result = style.FgYellow.Sprintf("↓%s", branch.BehindForPull)                          // :244
result = style.FgYellow.Sprintf("↑%s", branch.AheadForPull)                           // :246
```

**Behind comes first.** The status is only computed when `branch.IsTrackingRemote()` (`:234`);
`MatchesUpstream` means stored locally with both counts `"0"`
(`pkg/commands/models/branch.go:106`-`:108`), and `RemoteBranchNotStoredLocally` means both
counts are `"?"` (`:102`-`:104`). An in-progress item operation preempts everything:
`style.FgCyan.Sprintf("%s %s", itemOperationStr, Loader(now, userConfig.Gui.Spinner))` (`:230`).
The status is appended to the name with one space (`:131`-`:133`).

**Divergence from the base branch** — `divergenceStr` (`:253`-`:272`): `"arrowAndNumber"` gives
`fmt.Sprintf("↓%d", behind)`, otherwise a bare `"↓"`; the config is
`gui.showDivergenceFromBaseBranch` (`none|onlyArrow|arrowAndNumber`, default `"none"`,
`pkg/config/user_config.go:189`, `:917`). It is right-aligned *inside* the branch column with
`strings.Repeat(" ", paddingNeededForDivergence)` then `style.FgCyan.Sprint(divergence)`
(`:166`-`:178`). Test rows: `"branch_name    ↓"`, `"branch_name ✓   ↓2"`,
`"branch_name ↓5↑3    ↓2"` (`pkg/gui/presentation/branches_test.go:201`, `:219`, `:235`).

**The worktree marker** (`:87`-`:114`, `:128`-`:130`) fires when
`git_commands.CheckedOutByOtherWorktree(b, worktrees)` (`:59`). With icons and enough room:
`fmt.Sprintf("(%s %s)", icons.LINKED_WORKTREE_ICON, wt.Name)` (`:91`); without icons:
`fmt.Sprintf("(%s %s)", tr.LcWorktree, wt.Name)` = `"(worktree other-worktree)"` (`:93`); when it
does not fit, the bare icon (`:100`) or `"(worktree)"` (`:102`, `:109`). Test rows:
`"branch_name (worktree other-worktree)"`, `"branch_name (󰌹 other-worktree)"`,
`"bra… (worktree)"`, `"branc… 󰌹"` (`pkg/gui/presentation/branches_test.go:145`, `:155`,
`:293`, `:303`).

**The name colour** is `GetBranchTextStyle(name)`: the first matching pattern from
`gui.branchColorPatterns` (or the deprecated `branchColors`), else `theme.DefaultTextColor`
(`:195`-`:219`, config `pkg/config/user_config.go:74`-`:77`). A diffing ref overrides to
`theme.DiffTerminalColor` (= `style.FgMagenta`) at `:117`-`:119`.

The truncation budget is `viewWidth - 4`, minus divergence+1, minus
`COMMIT_HASH_SHORT_SIZE+1` (8+1) when showing the hash, minus 2 when any PRs exist, minus the
status width+1, minus the worktree marker+1 (`:65`-`:84`, `:113`).

PR icons: with icons `icons.IconForRemoteUrl(pr.Url)`, without, the literal `"●"`
(`:147`-`:151`); `WithPrColor` RGB values are OPEN `0x43,0x84,0x40`, CLOSED `0xC9,0x45,0x3C`,
MERGED `0x82,0x59,0xDD`, DRAFT `0x67,0x6C,0x75` (`:281`-`:294`). Check glyphs (`:344`-`:359`):
`"✓"` FgGreen (SUCCESS), `"●"` FgYellow (PENDING), `"✗"` FgRed (FAILURE), `"!"` FgRed (ERROR),
`"○"` FgDefault (EXPECTED).

> **Native mapping.** `Row::new().column(RowColumn::fixed_ch(4.0, recency))` then
> `RowColumn::flex(name)` then `RowColumn::fixed_ch(8.0, ahead_behind).align(ColumnAlign::Right)`.
> Build the ahead/behind cell as sibling `Text` runs (`↓` + count + `↑` + count) so the arrows
> and numbers can carry the yellow together while `✓` stays green. Use `Truncate::Middle` for
> the branch name.

### c.3 Commit rows

Column order — the seven `cols` slots (`pkg/gui/presentation/commits.go:442`-`:452`):

```go
cols = append(cols,
    divergenceString,   // ↑ / ↓, or the commit/merge icon when icons are on
    hashString,
    bisectString,
    descriptionString,  // date, full-description mode only
    actionString,       // rebase todo verb
    author,
    graphLine+mark+tagString+theme.DefaultTextColor.Sprint(name),
)
```

Test rows: `hash1 commit1`, `hash1 tag1 tag2 commit1`, `hash2 * commit2`,
`hash1 ◎─╮ commit1`, `↓ hash1r ○ commit1`,
`hash1 2:03AM     Jesse Duffield    commit1`
(`pkg/gui/presentation/commits_test.go:69`, `:86`, `:114`, `:205`, `:350`, `:524`).

**Short-sha length** is `common.UserConfig().Gui.CommitHashLength`, default **8**
(`:363`, `pkg/config/user_config.go:915`). `hashLength >= len(hash)` gives the full hash;
`> 0` gives `commit.Hash()[:hashLength]`; `<= 0` with icons off gives the literal `"*"`;
otherwise empty (`:364`-`:370`).

**The divergence column** (`:372`-`:377`) is
`hashColor.Sprint(lo.Ternary(commit.Divergence == models.DivergenceLeft, "↑", "↓"))` (`:374`);
when divergence is none and icons are on, the slot instead holds
`hashColor.Sprint(icons.IconForCommit(commit))` (`:375`-`:377`), which is `MERGE_COMMIT_ICON`
for a merge and `COMMIT_ICON` otherwise
(`pkg/gui/presentation/icons/git_icons.go:64`-`:69`).

**The bisect column** — `getBisectStatusText` (`:315`-`:339`) gives `"<-- " + NewTerm()`,
`"<-- " + OldTerm()`, `"<-- current"`, `"<-- skipped"`, `"?"` or `""`; colours from
`getBisectStatusColor` (`:457`-`:475`): None `style.FgBlack`, New `style.FgRed`, Old
`style.FgGreen`, Skipped `style.FgYellow`, Current `style.FgMagenta`, Candidate `style.FgBlue`.

**The date column** (full description only, `:379`-`:384`) is
`style.FgBlue.Sprint(utils.UnixToDateSmart(now, commit.UnixTimestamp, timeFormat, shortTimeFormat))`.
`UnixToDateSmart` uses `shortTimeFormat` for the same day/month/year and `longTimeFormat`
otherwise (`pkg/utils/date.go:59`-`:67`); defaults are `TimeFormat: "02 Jan 06"` and
`ShortTimeFormat: time.Kitchen` (`pkg/config/user_config.go:882`-`:883`). "Full description"
means screen mode ≠ normal (`pkg/gui/context/local_commits_context.go:64`).

**The author column** is `CommitAuthorShortLength` (default **2**) wide, or
`CommitAuthorLongLength` (default **17**) in full-description mode (`:436`-`:439`,
`pkg/config/user_config.go:913`-`:914`). `AuthorWithLength`: `< 2` → `""`; `== 2` → initials;
else `LongAuthor`, which is `WithPadding` then `TruncateWithEllipsis`, so the column is *exactly*
that many characters (`pkg/gui/presentation/authors/authors.go:51`-`:74`). Initials are the
first grapheme cluster for wide scripts, `LimitStr(name, 2)` for a single word, else the first
letter of each of the first two words (`:112`-`:128`).

**The author colour is hashed, not palette-cycled**
(`pkg/gui/presentation/authors/authors.go:93`-`:98`):

```go
hash := md5.Sum([]byte(str))
c := colorful.Hsl(randFloat(hash[0:4])*360.0, 0.6+0.4*randFloat(hash[4:8]), 0.4+randFloat(hash[8:12])*0.2)
return style.New().SetFg(style.NewRGBColor(color.RGB(uint8(c.R*255), uint8(c.G*255), uint8(c.B*255))))
```

**The hash colour is the pushed/unpushed/merged signal** — `getHashColor`
(`:477`-`:513`):

```go
if bisectInfo.Started() { return getBisectStatusColor(bisectStatus) }   // :484-486
StatusUnpushed                                       -> style.FgRed     // :491-492
StatusPushed                                         -> style.FgYellow  // :493-494
StatusMerged                                         -> style.FgGreen   // :495-496
StatusRebasing/CherryPickingOrReverting/Conflicted   -> style.FgBlue    // :497-498
StatusReflog                                         -> style.FgBlue    // :499-500
default                                              -> theme.DefaultTextColor  // :489, :501
// then overrides, in order:
diffed        -> theme.DiffTerminalColor              // :504-505
cherryPicked  -> theme.CherryPickedCommitTextStyle    // :506-507
DivergenceRight && !Merged -> style.FgBlue            // :508-509
```

The status enum is `StatusNone, StatusUnpushed, StatusPushed, StatusMerged, StatusRebasing,
StatusCherryPickingOrReverting, StatusConflicted, StatusReflog`
(`pkg/commands/models/commit.go:14`-`:25`), assigned unmerged+unpushed → Unpushed,
unmerged+pushed → Pushed, else Merged
(`pkg/commands/git_commands/commit_loader.go:561`-`:577`; the underlying `rev-list` calls are in
§d.3).

> **This three-way red / yellow / green on the sha column is lazygit's single most recognisable
> signal.** Reproduce it exactly.

**Rebase-todo verbs.** The strings come from `commandToString`
(`vendor/github.com/stefanhaller/git-todo-parser/todo/todo.go:37`-`:39`), mapped at `:41`-`:57`:
`Pick: "pick"` (`:42`), `Revert: "revert"` (`:43`), `Edit: "edit"` (`:44`),
`Reword: "reword"` (`:45`), `Fixup: "fixup"` (`:46`), `Squash: "squash"` (`:47`),
`Exec: "exec"` (`:48`), `Break: "break"` (`:49`), `Label: "label"` (`:50`),
`Reset: "reset"` (`:51`), `Merge: "merge"` (`:52`), `NoOp: "noop"` (`:53`),
`Drop: "drop"` (`:54`), `UpdateRef: "update-ref"` (`:55`), `Comment: "comment"` (`:56`). The
enum starts at 1 (`Pick TodoCommand = iota + 1`, `:5`-`:24`), which is why lazygit reserves 0 for
`ActionNone` (`pkg/commands/models/commit.go:27`-`:31`).

`actionColorMap(action, status)` — `pkg/gui/presentation/commits.go:515`-`:532`:

- `status == models.StatusConflicted` → `style.FgRed`, **overriding the verb** (`:516`-`:518`)
- `pick` → `style.FgCyan` (`:521`-`:522`); `drop` → `style.FgRed` (`:523`-`:524`);
  `edit` → `style.FgGreen` (`:525`-`:526`); `fixup` → `style.FgMagenta` (`:527`-`:528`)
- `squash`, `reword`, `revert`, `update-ref`, `merge`, `exec`, `break`, `label`, `reset`,
  `noop`, `comment` → `style.FgYellow` (the default branch, `:529`-`:530`)

The text is `commit.Action.String()`, with `" " + commit.ActionFlag` appended **only** for
`todo.Fixup` (giving e.g. `fixup -C`) — `:386`-`:394`; `ActionFlag` is documented
`// e.g. "-C" for fixup -C` (`pkg/commands/models/commit.go:63`).

**Marks** (`:424`-`:434`): conflicted →
`style.FgRed.Sprintf("<-- %s ---", common.Tr.ConflictLabel)` = `<-- CONFLICT ---`; a marked base
commit → `style.FgYellow.Sprint(common.Tr.MarkedCommitMarker)` =
`"↑↑↑ Will rebase from here ↑↑↑"`; commits that will not be rebased →
`style.FgYellow.Sprint("✓")`.

**Tag and branch-head decorations** (`:396`-`:414`): in full description,
`style.FgMagenta.SetBold().Sprint(commit.ExtraInfo) + " "` (`:399`), where `ExtraInfo` looks like
`HEAD -> master, tag: v0.15.2` (`pkg/commands/models/commit.go:49`); in normal mode with tags,
`theme.DiffTerminalColor.SetBold().Sprint(strings.Join(commit.Tags, " ")) + " "` (`:403`); a
local-branch head is prefixed with
`style.FgCyan.SetBold().Sprint(lo.Ternary(icons.IsIconEnabled(), icons.BRANCH_ICON, "*") + " " + tagString)`
(`:411`-`:412`). Markers are limited to branches that are not the current branch, not in
`git.mainBranches`, and with a non-empty CommitHash (`:164`-`:178`), and are suppressed on
`StatusMerged` (`:406`-`:410`).

**The subject** is `commit.Name`, with `update-ref` stripping `"refs/heads/"` and
`emoji.Sprint(name)` applied when `parseEmoji` (`:416`-`:422`), rendered in
`theme.DefaultTextColor` (`:451`).

#### The commit graph

Each cell renders **exactly two characters** (`pkg/gui/presentation/graph/cell.go:31`-`:64`;
the buffer is `len(cells)*2`, `graph.go:372`).

There are only **two node glyphs** in the whole package — no `⏣`, `◯` or `●`
(`cell.go:11`-`:14`):

```go
const (
	MergeSymbol  = '◎'   // U+25CE
	CommitSymbol = '○'   // U+25CB
)
```

`isMerge := startCount > 1` (more than one STARTS pipe on the row) selects `◎`
(`graph.go:283`-`:295`, `:363`-`:368`).

The box-drawing table — `getBoxDrawingChars(up, down, left, right)`
(`cell.go:147`-`:183`):

| up | down | left | right | first | second | line |
|---|---|---|---|---|---|---|
|✓|✓|✓|✓|`│`|`─`|`:148-149`|
|✓|✓|✓|✗|`│`|`" "`|`:150-151`|
|✓|✓|✗|✓|`│`|`─`|`:152-153`|
|✓|✓|✗|✗|`│`|`" "`|`:154-155`|
|✓|✗|✓|✓|`┴`|`─`|`:156-157`|
|✓|✗|✓|✗|`╯`|`" "`|`:158-159`|
|✓|✗|✗|✓|`╰`|`─`|`:160-161`|
|✓|✗|✗|✗|`╵`|`" "`|`:162-163`|
|✗|✓|✓|✓|`┬`|`─`|`:164-165`|
|✗|✓|✓|✗|`╮`|`" "`|`:166-167`|
|✗|✓|✗|✓|`╭`|`─`|`:168-169`|
|✗|✓|✗|✗|`╷`|`" "`|`:170-171`|
|✗|✗|✓|✓|`─`|`─`|`:172-173`|
|✗|✗|✓|✗|`─`|`" "`|`:174-175`|
|✗|✗|✗|✓|`╶`|`─`|`:176-177`|
|✗|✗|✗|✗|`" "`|`" "`|`:178-179`|

Rendered examples: `◎─╮`, `○ │`, `○─╯`, `○`
(`pkg/gui/presentation/graph/graph_test.go:45`-`:55`).

Pipes are `TERMINATES | STARTS | CONTINUES` (`graph.go:19`-`:23`) with
`Pipe{fromHash, toHash, style, fromPos, toPos, kind}` (`:25`-`:32`). The seed pipe is
`Pipe{fromPos: 0, toPos: 0, fromHash: &StartCommitHash, toHash: commits[0].HashPtr(), kind: STARTS, style: &style.FgDefault}`
(`:64`); `StartCommitHash = "START"` (`:36`-`:38`), and the empty-tree hash
`4b825dc642cb6eb9a060e54bf8d69288fbee4904` stands in as a root commit's parent (`:36`,
`:144`-`:148`).

**Graph colours are per-author, not a cycling palette**:
`getStyle := func(commit *models.Commit) *style.TextStyle { return authors.AuthorStyle(commit.AuthorName) }`
(`pkg/gui/presentation/commits.go:260`-`:262`, consumed at `graph.go:59`, `:149`-`:156`,
`:229`-`:236`); continuing and terminating pipes inherit the originating pipe's style.

The selected-commit highlight is `highlightStyle = style.FgLightWhite.SetBold()`
(`graph.go:35`); pipes originating at the selected commit render last, after
`cells[i].reset()`, so they override, and the node cell gets `setStyle(&highlightStyle)`
(`:335`-`:361`).

Visibility: `git.log.showGraph` is `always|never|when-maximised`, default `"always"`
(`pkg/config/user_config.go:422`, `:951`), resolved at
`pkg/gui/context/local_commits_context.go:326`-`:350`.

> **Native mapping.** The graph is a fixed-width `RowColumn::fixed_ch(n)` of mono text where `n`
> is twice the pipe count. Because each cell is two characters and each can carry a different
> colour, build the graph cell as a flex row of per-cell `Text::data` runs — this is the single
> place in the UI where the "one `Text`, one colour" limit bites hardest. Reproduce the
> author-hash colouring with the same md5→HSL formula so the same author gets the same hue as in
> lazygit.

### c.4 Reflog and stash rows

**Reflog** (`pkg/gui/presentation/reflog_commits.go`):

- normal, 2 columns:
  `[reflogHashColor(...).Sprint(c.ShortHash()), theme.DefaultTextColor.Sprint(name)]`
  (`:73`-`:83`)
- full description, 3 columns: the hash, then
  `style.FgMagenta.Sprint(utils.UnixToDateSmart(now, c.UnixTimestamp, timeFormat, shortTimeFormat))`,
  then the name (`:60`-`:71`)
- `reflogHashColor(cherryPicked, diffed)`: diffed → `theme.DiffTerminalColor`; base
  `style.FgBlue`; cherry-picked → `theme.CherryPickedCommitTextStyle` (`:38`-`:49`)
- `ShortHash()` is the first 8 characters (`pkg/commands/models/commit.go:107`-`:109`)

**Stash** (`pkg/gui/presentation/stash_entries.go:19`-`:33`):
`[style.FgCyan.Sprint(s.Recency), (icons.IconForStash(s) if icons on), textStyle.Sprint(s.Name)]`,
where `textStyle` is `theme.DefaultTextColor` or `theme.DiffTerminalColor` when
`stashEntry.RefName() == diffName` (`:20`-`:23`). `RefName()` is
`fmt.Sprintf("stash@{%d}", s.Index)` (`pkg/commands/models/stash_entry.go:17`-`:19`).

**Tags** (`pkg/gui/presentation/tags.go:41`-`:52`):
`[(TAG_ICON), textStyle.Sprint(t.Name), style.FgYellow.Sprint(t.Description())]`.
**Remotes** (`pkg/gui/presentation/remotes.go:44`-`:53`):
`[(icon), textStyle.Sprint(r.Name), style.FgBlue.Sprintf("%d branches", branchCount)]`.
**Remote branches** (`pkg/gui/presentation/remote_branches.go:24`-`:29`):
`[(BRANCH_ICON), GetBranchTextStyle(b.Name).Sprint(b.Name)]`.
**Worktrees** (`pkg/gui/presentation/worktrees.go:37`-`:55`):
`["  *" in FgGreen if current else "" in FgCyan, (icon), name, branch + mainWorktreeLabel]`; a
missing path turns the whole row `style.FgRed` (`:32`-`:35`) and, without icons, appends
`" (missing)"` (`:44`-`:46`); a detached worktree shows
`style.FgYellow.Sprint("HEAD detached at " + utils.ShortHash(worktree.Head))` (`:51`-`:52`).

### c.5 Diff colouring — two completely different paths

**(a) The main and secondary diff views: git colours them, lazygit does not.** Every UI diff is
a PTY task running git with `--color=<arg>`:

- `DiffCmdObj`: `Arg(fmt.Sprintf("--color=%s", self.diffRendererConfigManager.GetColorArg()))`
  (`pkg/commands/git_commands/diff.go:27`)
- `WorktreeFileDiffCmdObj`: `colorArg := ...GetColorArg(); if plain { colorArg = "never" }`
  (`pkg/commands/git_commands/working_tree.go:396`-`:417`)
- commit show: `Arg("--color=" + self.diffRendererConfigManager.GetColorArg())`
  (`pkg/commands/git_commands/commit.go:248`)
- stash show (`pkg/commands/git_commands/stash.go:90`)
- `GetColorArg()` returns `"always"` unless a `stdinFilter` renderer overrides it
  (`pkg/config/diff_renderer_config_manager.go:77`-`:88`)
- non-UI diffs use `Arg("--no-ext-diff", "--no-color")`
  (`pkg/commands/git_commands/diff.go:45`)

**lazygit sets no `color.diff` / `color.ui` / `diff-highlight` git config anywhere in `pkg/`.**
So the added/removed/context/hunk-header colours in the main panel come from the *user's git
config*, not from lazygit.

**(b) The staging and patch-building views: lazygit colours the patch itself**
(`pkg/commands/patch/format.go`):

```go
func (self *patchPresenter) patchLineStyle(patchLine *PatchLine) style.TextStyle {   // :111
	switch patchLine.Kind {
	case ADDITION: return style.FgGreen                  // :113-114
	case DELETION: return style.FgRed                    // :115-116
	default:       return theme.DefaultTextColor         // :117-118
	}
}
```

- patch header lines → `theme.DefaultTextColor.SetBold()` (`:76`-`:79`)
- **the hunk header is split into two runs on one line**: `hunk.formatHeaderStart()` in
  `style.FgCyan` (`:85`), then `hunk.headerContext` in `theme.DefaultTextColor` (`:93`)
- **every line is emitted as two style runs — the first character, then the rest**:
  `firstCharStyle.Sprint(str[:1]) + textStyle.Sprint(str[1:])` (`:145`)

Line kinds (`pkg/commands/patch/patch_line.go:5`-`:14`):

```go
const (
	PATCH_HEADER PatchLineKind = iota
	HUNK_HEADER
	ADDITION
	DELETION
	CONTEXT
	NEWLINE_MESSAGE
)
```

`PatchLine{Kind, Content}` carries the comment
`// something like '+ hello' (note the first character is not removed)` (`:16`-`:19`). The kind
comes from the first byte: `" "`→CONTEXT, `"+"`→ADDITION, `"-"`→DELETION, `"\\"`→NEWLINE_MESSAGE
(`pkg/commands/patch/parse.go:54`-`:85`); hunks are detected by
`strings.HasPrefix(line, "@@")` (`:20`-`:36`) with the regex
`` `(?m)^@@ -(\d+)[^\+]+\+(\d+)[^@]+@@(.*)$` `` (`:10`), and the header is rebuilt as
`fmt.Sprintf("@@ -%d,%d +%d%s @@", …)` (`pkg/commands/patch/hunk.go:59`-`:68`).

The **merge-conflict view** is also lazygit-coloured: conflict marker lines (`<<<<<<<`,
`=======`, `>>>>>>>`) in `style.FgRed`, everything else `theme.DefaultTextColor`
(`pkg/gui/mergeconflicts/rendering.go:19`-`:22`).

> **Native mapping.** `fleet-git` returns a **parsed** `Diff` with typed `LineKind`
> (`crates/fleet-git/src/model.rs:411`), so path (b) is the model to follow for *both* cases:
> colour the diff yourself from the parsed kinds rather than interpreting git's ANSI output.
> Emit each line as `[gutter, marker, payload]` sibling runs — matching lazygit's own two-run
> split — and use `ansi[2]` / `ansi[1]` / default / `ansi[6]` for addition / deletion / context /
> hunk header.

### c.6 Staging-mode selection

The three modes (`pkg/gui/patch_exploring/state.go:39`-`:46`):

```go
// these represent what select mode we're in
type selectMode int

const (
	LINE selectMode = iota
	RANGE
	HUNK
)
```

Predicates: `SelectingHunk()` (`:188`-`:190`); `SelectingRange()` =
`RANGE && (rangeIsSticky || rangeStartLineIdx != selectedLineIdx)` (`:196`-`:198`);
`SelectingLine()` (`:200`-`:202`). Transitions: `ToggleSelectHunk()` (`:156`-`:168`, HUNK↔LINE,
snapping to the next change line), `ToggleSelectRange(sticky)` (`:174`-`:182`),
`SelectPreviousHunk` / `SelectNextHunk` (`:259`-`:293`). The initial mode is `LINE`, or `HUNK`
when `useHunkModeByDefault && !patch.IsSingleHunkForWholeFile()` (`:69`-`:72`); a click forces
`RANGE` (`:80`-`:86`).

Keys: `main.toggleSelectHunk` default `"a"` (`pkg/config/user_config.go:1173`);
`universal.toggleRangeSelect` default `"v"` (`:1015`); `rangeSelectUp`/`rangeSelectDown` =
`<shift+up>` / `<shift+down>` (`:1016`-`:1017`); `main.prevHunk`/`nextHunk` = `<left>,h` /
`<right>,l` (`:1171`-`:1172`); `main.editSelectHunk` = `"E"` (`:1175`). Escape demotes RANGE or a
user-enabled HUNK to LINE *before* popping the context
(`pkg/gui/controllers/staging_controller.go:171`-`:194`).

**The range computation** — `SelectedViewRange()`
(`pkg/gui/patch_exploring/state.go:353`-`:368`):

```go
switch s.selectMode {
case HUNK:  return s.selectionRangeForCurrentBlockOfChanges()
case RANGE: if s.rangeStartLineIdx > s.selectedLineIdx { return s.selectedLineIdx, s.rangeStartLineIdx }
            return s.rangeStartLineIdx, s.selectedLineIdx
case LINE:  return s.selectedLineIdx, s.selectedLineIdx
default:    return 0, 0
}
```

> **HUNK mode selects the contiguous block of *change lines* around the cursor, not the whole
> `@@` hunk.** `selectionRangeForCurrentBlockOfChanges()` walks back and forward while
> `patchLines[i].IsChange()`, then extends over wrapped continuation lines (`:329`-`:351`). The
> true hunk bounds (`CurrentHunkBounds()`, `:322`-`:327`) are used only by `editSelectHunk`
> (`E`).

**How the range is drawn** — all three modes are expressed to the view as a range
(`pkg/gui/context/patch_explorer_context.go:106`-`:110`):

```go
startIdx, endIdx := state.SelectedViewRange()
// As far as the view is concerned, we are always selecting a range
view.SetRangeSelectStart(startIdx)
view.SetCursorY(endIdx - newOriginY)
```

The pixel-level style is the same one used for list selection — bright fg (+8) + `AttrBold` +
`SelBgColor` — quoted in §a.7 (`pkg/gocui/view.go:668`-`:689`).

**The included-lines marker exists only in the custom-patch builder** — `formatLineAux`
(`pkg/commands/patch/format.go:131`-`:146`):

```go
firstCharStyle := textStyle
if included { firstCharStyle = firstCharStyle.MergeStyle(style.BgGreen) }   // :136-139
if len(str) < 2 { return firstCharStyle.Sprint(str) }
return firstCharStyle.Sprint(str[:1]) + textStyle.Sprint(str[1:])            // :145
```

That is **`style.BgGreen` on the `+`/`-` sign column only**, with no extra glyph. The feed
closure for `Staging` and `StagingSecondary` is `func() []int { return nil }`
(`pkg/gui/context/setup.go:48`, `:55`), so **the staging view never draws included-line
markers**; only the custom-patch builder does.

Staging apply — `StagingController.applySelection(reverse)`
(`pkg/gui/controllers/staging_controller.go:239`-`:283`) — is quoted in §d.11.

> **Native mapping.** `PatchCursor { hunk, line: Option<usize>, anchor }` (brief §f.2) maps
> one-to-one: `line: None` is HUNK mode, `Some` is LINE, and `anchor` is RANGE. Draw the
> selection as a background band across the whole diff row (`colors.row_selected`) rather than
> re-colouring the text, and remember that HUNK mode highlights a *change block*, not the whole
> `@@` hunk.

### c.7 The status panel and the mode words

`FormatStatus` (`pkg/gui/presentation/status.go:15`-`:49`) builds, in order:

1. `BranchStatus(...)` plus a space, when `currentBranch.IsRealBranch()` (`:26`-`:31`)
2. `style.FgYellow.Sprintf("(%s) ", workingTreeState.LowerCaseTitle(tr))` when a mode is active
   (`:33`-`:35`) — **yellow, lowercase, parenthesised, with a trailing space**
3. the branch name styled `GetBranchTextStyle(currentBranch.Name)` (`:37`)
4. for a linked worktree,
   `repoName = fmt.Sprintf("%s(%s%s)", repoName, icon, style.FgCyan.Sprint(linkedWorktreeName))`
   (`:39`-`:45`)
5. `status += fmt.Sprintf("%s → %s", repoName, name)` (`:46`)

**The working-tree state model** (`pkg/commands/models/working_tree_state.go`):
`WorkingTreeState{Rebasing, Merging, CherryPicking, Reverting bool}` (`:9`-`:14`) — **several can
be true at once** (`:5`-`:8`). `Effective()` precedence is **Reverting > CherryPicking > Merging
> Rebasing > None** (`:45`-`:59`). `CommandName()` gives
`"rebase" / "merge" / "cherry-pick" / "revert"` (`:97`-`:104`). **There is no "bisecting"
working-tree state** — bisect is a separate mode.

**The exact mode-word strings** (`pkg/i18n/english.go`):

| key | value | line |
|---|---|---|
| `RebasingStatus` | `"Rebasing"` | `:1590` |
| `MergingStatus` | `"Merging"` | `:1591` |
| `LowercaseRebasingStatus` | `"rebasing"` (*"lowercase because it shows up in parentheses"*) | `:1592` |
| `LowercaseMergingStatus` | `"merging"` | `:1593` |
| `LowercaseCherryPickingStatus` | `"cherry-picking"` | `:1594` |
| `LowercaseRevertingStatus` | `"reverting"` | `:1595` |
| `CherryPickingStatus` | `"Cherry-picking"` | `:1597` |
| `RevertingStatus` | `"Reverting"` | `:1604` |
| `Bisect.Bisecting` | `"Bisecting"` | `:2272` |
| `ShowingGitDiff` | `"Showing output for:"` | `:1856` |
| `BuildingPatch` | `"Building patch"` | `:1887` |
| `FilteringBy` | `"Filtering by"` | `:1830` |
| `MarkedBaseCommitStatus` | `"Marked a base commit for rebase"` | `:2098` |
| `ResetInParentheses` | `"(Reset)"` | `:1831` |
| `ConflictLabel` | `"CONFLICT"` | `:1540` |
| `MarkedCommitMarker` | `"↑↑↑ Will rebase from here ↑↑↑"` | `:2102` |
| `UpstreamGone` | `"(upstream gone)"` | `:2008` |
| `MissingWorktree` / `MainWorktree` | `"(missing)"` / `"(main worktree)"` | `:2072`-`:2073` |
| `LcWorktree` | `"worktree"` | `:2091` |

**The information view's seven modes** — `Statuses()`, first active wins
(`pkg/gui/controllers/helpers/mode_helper.go`):

| # | mode | label | style | lines |
|---|---|---|---|---|
| 1 | Diffing | `"Showing output for: git diff <args>"` | `style.FgMagenta` | `:51`-`:67` (style `:60`) |
| 2 | Patch building | `"Building patch"` | `style.FgYellow.SetBold()` | `:68`-`:77` (`:71`) |
| 3 | Filtering | `"Filtering by '<x>'"` | `style.FgRed` | `:78`-`:95` (`:88`) |
| 4 | Marked base commit | `"Marked a base commit for rebase"` | `style.FgCyan` | `:96`-`:108` (`:101`) |
| 5 | Cherry-picking | `"<n> commits copied"` | `style.FgCyan` | `:109`-`:131` (`:124`) |
| 6 | Working tree state | `"Rebasing"` / `"Merging"` / `"Cherry-picking"` / `"Reverting"` | `style.FgYellow` | `:132`-`:146` (`:139`) |
| 7 | Bisecting | `"Bisecting"` | `style.FgGreen` | `:147`-`:158` (`:152`) |

Every label is wrapped by (`:162`-`:168`):

```go
func (self *ModeHelper) withResetButton(content string, textStyle style.TextStyle) string {
	return textStyle.Sprintf("%s %s", content, style.AttrUnderline.Sprint(self.c.Tr.ResetInParentheses))
}
```

so it renders as `<content> (Reset)` in the mode's colour, with `(Reset)` **underlined** and
clickable (`pkg/gui/information_panel.go:35`-`:40`).

**The spinner** (`pkg/gui/presentation/loader.go:9`-`:14`):

```go
func Loader(now time.Time, config config.SpinnerConfig) string {
	milliseconds := now.UnixMilli()
	index := milliseconds / int64(config.Rate) % int64(len(config.Frames))
	return config.Frames[index]
}
```

The frames come from config: `Frames: []string{"●∙∙", "∙●∙", "∙∙●", "∙●∙"}`, `Rate: 180`
(`pkg/config/user_config.go:930`-`:931`) — four frames of three runes (`●` U+25CF, `∙` U+2219),
advancing every 180 ms and **phase-locked to `UnixMilli()` so every spinner animates in sync**.
It is always paired with cyan text.

> **Native mapping.** Put the repo name and branch in `StatusBar::breadcrumb` with the `→`
> separator, and the mode word in `StatusBar::mode_element` as
> `ModeWord::word("REBASING").tone(Tone::Warning)`. `OperationState` maps directly onto lazygit's
> precedence — but note `fleet-git`'s `OperationState` is an enum (one state), whereas lazygit's
> is four booleans; use lazygit's precedence order if you ever need to display more than one.
> The kit's spinner is one turn per 1000 ms (`theme.motion.spinner`) rather than 4 frames at
> 180 ms; use it, but give it a stable `ElementId`.

### c.8 The semantic colour palette

The style constants (`pkg/gui/style/basic_styles.go:9`-`:52`) are the plain ANSI set: `FgWhite`,
`FgLightWhite`, `FgBlack`, `FgBlackLighter`, `FgCyan`, `FgRed`, `FgGreen`, `FgBlue`, `FgYellow`,
`FgMagenta`, `FgDefault` (`:10`-`:20`), the `Bg*` equivalents (`:22`-`:30`), plus
`Nothing = New()` (`:32`-`:33`, *"will not print any colour escape codes, including the reset
escape code"*), `AttrUnderline` (`:35`) and `AttrBold` (`:36`).

**There are no `Fg*Bold` constants** — bold is only ever produced by chaining `.SetBold()`.

Theme variables (`pkg/theme/theme.go:9`-`:48`) include `DefaultTextColor` (`:11`),
`ActiveBorderColor` (`:17`), `InactiveBorderColor` (`:20`), `SearchingActiveBorderColor` (`:23`),
`SelectedLineBgColor` (`:33`), `CherryPickedCommitTextStyle` (`:38`), `OptionsFgColor` (`:43`),
**`DiffTerminalColor = style.FgMagenta` (`:45`, hard-coded and never reassigned by
`UpdateTheme`)** and `UnstagedChangesColor` (`:47`).

Defaults (`pkg/config/user_config.go:884`-`:897`):

```go
ActiveBorderColor:               []string{"green", "bold"},   // :885
SearchingActiveBorderColor:      []string{"cyan", "bold"},    // :886
InactiveBorderColor:             []string{"default"},         // :887
OptionsTextColor:                []string{"blue"},            // :888
SelectedLineBgColor:             []string{"blue"},            // :889
InactiveViewSelectedLineBgColor: []string{"bold"},            // :890
CherryPickedCommitBgColor:       []string{"cyan"},            // :891
CherryPickedCommitFgColor:       []string{"blue"},            // :892
MarkedBaseCommitBgColor:         []string{"yellow"},          // :893
MarkedBaseCommitFgColor:         []string{"blue"},            // :894
UnstagedChangesColor:            []string{"red"},             // :895
DefaultFgColor:                  []string{"default"},         // :896
```

**What each colour means, semantically:**

- **green** — staged / merged / added / good: the staged file name
  (`pkg/gui/presentation/files.go:135`), patch additions
  (`pkg/commands/patch/format.go:113`-`:114`), `+N` counts
  (`pkg/gui/presentation/files.go:207`), the merged commit hash
  (`pkg/gui/presentation/commits.go:495`-`:496`), `todo.Edit` (`:525`-`:526`), the bisect
  old/good marker (`:463`-`:464`), a branch in sync with its upstream `✓`
  (`pkg/gui/presentation/branches.go:238`), the current-branch/worktree `"  *"` (`:136`-`:137`),
  and the focused border (`pkg/config/user_config.go:885`).
- **red** — unstaged / unpushed / removed / destructive: `unstagedChangesColor`
  (`pkg/config/user_config.go:895`) and its uses
  (`pkg/gui/presentation/files.go:189`, `:195`, `:292`), patch deletions
  (`pkg/commands/patch/format.go:115`-`:116`), `-N` counts
  (`pkg/gui/presentation/files.go:214`), the unpushed commit hash
  (`pkg/gui/presentation/commits.go:491`-`:492`), `todo.Drop` and the conflicted override
  (`:516`-`:518`, `:523`-`:524`), the `<-- CONFLICT ---` mark (`:426`), merge-conflict marker
  lines (`pkg/gui/mergeconflicts/rendering.go:21`), `(upstream gone)`
  (`pkg/gui/presentation/branches.go:236`), and a missing worktree
  (`pkg/gui/presentation/worktrees.go:33`).
- **yellow** — partially staged / pushed / pending / caution: the mixed staged+unstaged file
  name (`pkg/gui/presentation/files.go:137`), the commit-file `PART` state (`:241`-`:242`), the
  pushed commit hash (`pkg/gui/presentation/commits.go:493`-`:494`), the default rebase verb
  (`:529`-`:530`), the bisect skipped marker (`:465`-`:466`), the `↑`/`↓` upstream counts
  (`pkg/gui/presentation/branches.go:242`-`:246`), the tag description
  (`pkg/gui/presentation/tags.go:45`), `(rebasing)` in the status panel
  (`pkg/gui/presentation/status.go:34`) and the mode label
  (`pkg/gui/controllers/helpers/mode_helper.go:139`), and the command-log action lines
  (`pkg/gui/command_log_panel.go:41`).
- **cyan** — navigational / informational / in-progress / copied: branch recency
  (`pkg/gui/presentation/branches.go:135`), stash recency
  (`pkg/gui/presentation/stash_entries.go:26`), divergence text
  (`pkg/gui/presentation/branches.go:176`), **hunk headers**
  (`pkg/commands/patch/format.go:85`), the in-progress spinner text
  (`pkg/gui/presentation/branches.go:230`), the branch-head marker in bold
  (`pkg/gui/presentation/commits.go:411`-`:412`), `todo.Pick` (`:521`-`:522`), the cherry-picked
  background (`pkg/config/user_config.go:891`), the searching border (`:886`), and the linked
  worktree name (`pkg/gui/presentation/status.go:44`).
- **blue** — neutral metadata / rebase in flight / chrome: the options-bar text
  (`pkg/config/user_config.go:888`), the selected-line background (`:889`), the commit date
  (`pkg/gui/presentation/commits.go:381`), the reflog hash
  (`pkg/gui/presentation/reflog_commits.go:43`), the rebasing/cherry-picking/conflicted/reflog
  hashes (`pkg/gui/presentation/commits.go:497`-`:500`), and remote branch counts
  (`pkg/gui/presentation/remotes.go:48`).
- **magenta** — "the thing you singled out" / non-runnable:
  `theme.DiffTerminalColor = style.FgMagenta` (`pkg/theme/theme.go:45`), applied to the diffing
  ref across branches, remotes, tags, stash, reflog and commit hashes; tag decorations in bold
  (`pkg/gui/presentation/commits.go:403`); the diffing mode label
  (`pkg/gui/controllers/helpers/mode_helper.go:60`); `todo.Fixup`
  (`pkg/gui/presentation/commits.go:527`-`:528`); the `?` for a remote branch not stored locally
  (`pkg/gui/presentation/branches.go:240`); and non-command-line log lines
  (`pkg/gui/command_log_panel.go:55`).
- **default / other** — `defaultFgColor: [default]` for ordinary list text, commit subjects and
  patch context lines; `inactiveBorderColor: [default]` for unfocused frames;
  `style.FgLightWhite.SetBold()` is the graph's `highlightStyle`
  (`pkg/gui/presentation/graph/graph.go:35`); bold-only is the unfocused selection
  (`pkg/config/user_config.go:890`); commit authors and graph pipes are md5→HSL truecolor
  hashes (`pkg/gui/presentation/authors/authors.go:76`-`:98`).

### c.9 Twelve gotchas the port will hit

1. **There is no tree line art in the files panel** — only `"  "` per level plus `▼`/`▶`
   (`pkg/gui/presentation/files.go:132`, `:17`-`:20`).
2. **Fully-unstaged files are not red in the name** — only the second status character is
   (`:139` versus `:194`-`:198`). Merge statuses (`UU`/`AA`/`DD`) get a **green** first character
   because the switch handles only `"?"` and `" "` (`:186`-`:192`).
3. **The current-branch marker is the recency string `"  *"`**, not a separate glyph
   (`pkg/commands/git_commands/branch_loader.go:108`).
4. **Main-panel diff colours come from git, not lazygit** (`--color=always` into a PTY). Only the
   staging and patch-building views are coloured in-process
   (`pkg/commands/git_commands/working_tree.go:396`-`:417` versus
   `pkg/commands/patch/format.go:111`-`:120`).
5. **HUNK select mode selects a contiguous change block, not the whole `@@` hunk**
   (`pkg/gui/patch_exploring/state.go:329`-`:351`).
6. **The selection highlight is bright-fg + bold + background swap**, not reverse video
   (`pkg/gocui/view.go:677`-`:688`); it **dims when the panel is unfocused** and when the
   *terminal window* loses focus (`pkg/gocui/gui.go:2031`-`:2035`).
7. **Columns blank in every row are deleted**, silently shifting the layout
   (`pkg/utils/formatting.go:90`-`:126`).
8. **The command log lives inside the right column**, under the main panel, not full width
   (`pkg/gui/controllers/helpers/window_arrangement_helper.go:199`-`:204`).
9. **Graph branch colours are md5-of-author-name → HSL truecolor**, not a cycling palette
   (`pkg/gui/presentation/authors/authors.go:93`-`:98`).
10. **Side panels are user-configurable groups**, and the window name is the first tab's name —
    so "the branches window" may be showing `remotes` or `tags`
    (`pkg/gui/controllers/helpers/window_helper.go:144`-`:148`).
11. **Every diff line is emitted as two style runs** — the first character, then the rest — even
    when it is not "included" (`pkg/commands/patch/format.go:145`).
12. **The staging view never shows included-line markers**; only the custom-patch builder does
    (`pkg/gui/context/setup.go:48`, `:55`).
## d. The git command lines lazygit runs

### d.0 How to read these

lazygit never writes a shell string. It builds argv with a fluent builder and executes it
directly. `ToArgv` is what turns a builder into the final argv
(`pkg/commands/git_commands/git_command_builder.go:140`):

```go
func (self *GitCommandBuilder) ToArgv() []string { return append([]string{"git"}, self.args...) }
```

Argument-order rules you need in order to reconstruct a command correctly:

| Builder call | Effect | file:line |
|---|---|---|
| `Config(v)` | **prepends** `-c v` before the subcommand | `pkg/commands/git_commands/git_command_builder.go:63`-`:68` |
| `Dir(p)` | **prepends** `-C p` | `pkg/commands/git_commands/git_command_builder.go:79`-`:84` |
| `GitDir(p)` | prepends `--git-dir p` | `pkg/commands/git_commands/git_command_builder.go:111` |
| `Worktree(p)` | prepends `--work-tree p` | `pkg/commands/git_commands/git_command_builder.go:95` |

Reconstructions below assume lazygit's defaults: `DiffContextSize: 3`,
`RenameSimilarityThreshold: 50` (`pkg/config/user_config.go:967`-`:968`),
`Git.Log.Order: "topo-order"` (`pkg/config/user_config.go:950`),
local/remote branch sort order `"date"` (`pkg/config/user_config.go:954`-`:955`), and
`GetColorArg() == "always"` (`pkg/config/diff_renderer_config_manager.go:77`-`:88`).

### d.1 Global args and environment

**There are no always-on `-c` overrides.** `-c commit.gpgSign=false`, `--no-optional-locks` and
`-c core.quotepath=` do not appear anywhere in the tree. Instead lazygit sets one environment
variable on every command (`pkg/commands/git_cmd_obj_builder.go:38`, `:57`-`:59`):

```go
var defaultEnvVar = git_commands.OptionalLocksEnvVar + "=0"
...
func (self *gitCmdObjBuilder) New(args []string) *oscommands.CmdObj {
	return self.innerBuilder.New(args).AddEnvVars(self.envVars...).SetWd(self.repoDir)
}
```

```go
// pkg/commands/git_commands/git_command_builder.go:20
const OptionalLocksEnvVar = "GIT_OPTIONAL_LOCKS"
```

`envVars` is `defaultEnvVar` plus the repo-location vars
(`pkg/commands/git_cmd_obj_builder.go:53`), which are only set for linked worktrees and
submodules (`pkg/commands/git_commands/repo_paths.go:201`-`:215`):

```go
// pkg/env/env.go:12-15
const (
	GitDirEnvVar      = "GIT_DIR"
	GitWorkTreeEnvVar = "GIT_WORK_TREE"
)
```

So every invocation is effectively:

```sh
GIT_OPTIONAL_LOCKS=0 [GIT_DIR=… GIT_WORK_TREE=…] git …
```

There is exactly one opt-out: a *foreground* status refresh removes the variable so the index
lock is taken normally (`pkg/commands/git_commands/file_loader.go:228`-`:236`):

```go
cmdObj := self.cmd.New(cmdArgs).DontLog()
if !opts.Background {
	cmdObj.RemoveEnvVar(OptionalLocksEnvVar)
}
```

**The shared diff-args helper**, used by nearly every diff-producing command
(`pkg/commands/git_commands/git_command_builder.go:126`-`:138`):

```go
func (self *GitCommandBuilder) AddCommonDiffArgs(diffRendererConfigManager *config.DiffRendererConfigManager, userConfig *config.UserConfig, forUI bool) *GitCommandBuilder {
	contextSize := userConfig.Git.DiffContextSize
	extDiffCmd := diffRendererConfigManager.GetExternalDiffCommand(contextSize)
	useExtDiff := forUI && diffRendererConfigManager.GetDiffRendererType() == config.DiffRendererType_ExtDiff

	return self.
		ConfigIf(forUI && extDiffCmd != "", "diff.external="+extDiffCmd).
		ArgIfElse(useExtDiff, "--ext-diff", "--no-ext-diff").
		Arg(fmt.Sprintf("--unified=%d", contextSize)).
		ArgIf(forUI && userConfig.Git.IgnoreWhitespaceInDiffView, "--ignore-all-space").
		Arg(fmt.Sprintf("--find-renames=%d%%", userConfig.Git.RenameSimilarityThreshold)).
		ArgIf(forUI, diffRendererConfigManager.GetRawGitArgs()...)
}
```

With defaults this expands to `--no-ext-diff --unified=3 --find-renames=50%`. Note the two
config-driven knobs the UI exposes as keys: `}`/`{` change `--unified=N`, `<ctrl+w>` toggles
`--ignore-all-space`, and `)`/`(` change `--find-renames=N%`.

Long path lists are batched under a 30 KB argv cap by `runGitCmdOnPaths`
(`pkg/commands/git_commands/git_command_builder.go:151`-`:172`), emitting
`git <subcommand> -- <paths…>` repeatedly.

Version probe: `git --version` (`pkg/commands/git_commands/version.go:18`).

### d.2 Status and working-tree state

```go
// pkg/commands/git_commands/file_loader.go:216-226
func (self *FileLoader) gitStatus(opts GitStatusOptions) ([]FileStatus, error) {
	cmdArgs := NewGitCmd("status").
		Arg(opts.UntrackedFilesArg).
		Arg("--porcelain").
		Arg("-z").
		ArgIfElse(
			opts.NoRenames,
			"--no-renames",
			fmt.Sprintf("--find-renames=%d%%", self.UserConfig().Git.RenameSimilarityThreshold),
		).
		ToArgv()
```

`UntrackedFilesArg` is built at `pkg/commands/git_commands/file_loader.go:49`-`:54` and
defaults to `all` when git config `status.showUntrackedFiles` is unset
(`pkg/commands/git_commands/config.go:70`-`:72`).

```sh
# normal refresh
GIT_OPTIONAL_LOCKS=0 git status --untracked-files=all --porcelain -z --find-renames=50%

# used to resolve a rename back into its before/after pair
GIT_OPTIONAL_LOCKS=0 git status --untracked-files=all --porcelain -z --no-renames

# foreground refresh: GIT_OPTIONAL_LOCKS is removed (file_loader.go:235)
```

There is **no `--ignore-submodules`**. Parsing is at
`pkg/commands/git_commands/file_loader.go:243`-`:268`: split on `"\x00"`,
`Change = original[:2]`, `Path = original[3:]`, and an `R`/`C` record consumes the *next* NUL
field as `PreviousPath`.

Two sidecar commands complete the files panel:

```sh
# +/- line counts — pkg/commands/git_commands/file_loader.go:206-214
git diff --numstat -z HEAD

# per-path conflict-marker size — pkg/commands/git_commands/file_loader.go:122-134
printf 'path1\0path2\0' | git check-attr -z --stdin conflict-marker-size
```

**Working-tree state is detected with filesystem probes, not git.**

```go
// pkg/commands/git_commands/status.go:25-32
func (self *StatusCommands) WorkingTreeState() models.WorkingTreeState {
	result := models.WorkingTreeState{}
	result.Rebasing, _ = self.IsInRebase()
	result.Merging, _ = self.IsInMergeState()
	result.CherryPicking, _ = self.IsInCherryPick()
	result.Reverting, _ = self.IsInRevert()
	return result
}
```

| State | Probed path | file:line |
|---|---|---|
| rebasing | `<worktreeGitDir>/rebase-merge`, else `<worktreeGitDir>/rebase-apply` | `pkg/commands/git_commands/status.go:38`-`:44` |
| merging | `<worktreeGitDir>/MERGE_HEAD` | `pkg/commands/git_commands/status.go:47`-`:49` |
| cherry-picking | `<worktreeGitDir>/CHERRY_PICK_HEAD`, disambiguated against `rebase-merge/stopped-sha` | `pkg/commands/git_commands/status.go:51`-`:81` |
| reverting | `<worktreeGitDir>/REVERT_HEAD` | `pkg/commands/git_commands/status.go:83`-`:85` |
| bisecting | `<gitDir>/BISECT_START` (+ `BISECT_TERMS`, `refs/bisect/*`, `BISECT_EXPECTED_REV`) | `pkg/commands/git_commands/bisect.go:31`-`:98` |
| branch being rebased | `<worktreeGitDir>/{rebase-merge,rebase-apply}/head-name` | `pkg/commands/git_commands/status.go:149`-`:156` |

Rebase progress files are read directly too: `rebase-merge/git-rebase-todo`
(`pkg/commands/git_commands/commit_loader.go:359`), `rebase-merge/done` (`:404`),
`rebase-merge/amend` and `rebase-merge/message` (`:416`-`:417`), `sequencer/todo` (`:501`),
`CHERRY_PICK_HEAD`/`REVERT_HEAD` (`:544`).

A cheap "did anything change?" fingerprint is taken before deciding to refresh
(`pkg/commands/git_commands/status.go:94`-`:97`), with fallbacks for reftable repos
(`:134`, `:139`):

```sh
git for-each-ref --format=%(objectname) %(refname) refs/heads
git symbolic-ref HEAD
git rev-parse HEAD
```

> **Native note:** `crates/fleet-git` already covers all of this —
> `OperationState` (`crates/fleet-git/src/model.rs:114`) mirrors the probe table above,
> including `Rebasing { done, total }` from `rebase-merge/done` vs the todo file.

### d.3 Commit log (the commits panel)

**The pretty-format string, verbatim** (`pkg/commands/git_commands/commit_loader.go:625`):

```go
const prettyFormat = `--pretty=format:+%H%x00%at%x00%aN%x00%ae%x00%P%x00%m%x00%D%x00%s`
```

The separator is **NUL** (`%x00`) and every record is prefixed with a literal `+` so that
multi-line `--name-status` output can be told apart from a new record
(`pkg/commands/git_commands/commit_loading_shared.go:52`, `if line[0] == '+'`). Fields in
order: hash, author unix date, author name, author email, parent hashes, left/right mark, ref
decorations, subject — parsed with `strings.SplitN(line, "\x00", 8)`
(`pkg/commands/git_commands/commit_loader.go:192`-`:216`).

```go
// pkg/commands/git_commands/commit_loader.go:598-623
cmdArgs := NewGitCmd("log").
	Arg(refSpec).
	ArgIf(gitLogOrder != "default", "--"+gitLogOrder).
	ArgIf(opts.All, "--all").
	Arg("--oneline").
	Arg(prettyFormat).
	Arg("--abbrev=40").
	ArgIf(opts.FilterAuthor != "", "--author="+opts.FilterAuthor).
	ArgIf(opts.Limit, "-300").
	ArgIf(opts.FilterPath != "", "--follow", "--name-status").
	Arg("--no-show-signature").
	ArgIf(opts.RefToShowDivergenceFrom != "", "--left-right").
	Arg("--").
	ArgIf(opts.FilterPath != "", opts.FilterPath).
	ToArgv()
```

```sh
# default commits panel
git log HEAD --topo-order --oneline \
  '--pretty=format:+%H%x00%at%x00%aN%x00%ae%x00%P%x00%m%x00%D%x00%s' \
  --abbrev=40 -300 --no-show-signature --

# with --all, author filter, path filter and divergence view
git log HEAD...origin/master --topo-order --all --oneline \
  '--pretty=format:+%H%x00%at%x00%aN%x00%ae%x00%P%x00%m%x00%D%x00%s' \
  --abbrev=40 --author=jesse -300 --follow --name-status \
  --no-show-signature --left-right -- some/path
```

Commits referenced only by an in-progress rebase todo are hydrated separately
(`pkg/commands/git_commands/commit_loader.go:305`-`:311`):

```sh
git -c log.showSignature=false show --no-patch --oneline --abbrev=20 \
  '--pretty=format:+%H%x00%at%x00%aN%x00%ae%x00%P%x00%m%x00%D%x00%s' <hash…>
```

**Pushed / unpushed / merged detection** uses a single primitive — `rev-list <ref> ^<excluded…>`
(`pkg/commands/git_commands/commit_loader.go:579`-`:595`), called twice
(`pkg/commands/git_commands/commit_loader.go:110`-`:121`):

```sh
# "unmerged": reachable from the panel ref but not from any main branch
git rev-list HEAD ^refs/remotes/origin/master ^refs/remotes/origin/main

# "unpushed": reachable from the branch but not from its upstream nor any main branch
git rev-list refs/heads/mybranch ^mybranch@{u} ^refs/remotes/origin/master
```

Status assignment (`pkg/commands/git_commands/commit_loader.go:561`-`:577`):
in `unmerged` ∧ in `unpushed` → `StatusUnpushed`; in `unmerged` ∧ ¬`unpushed` → `StatusPushed`;
¬`unmerged` → `StatusMerged`. **This three-way classification is what drives the commit-row
colouring** described in §c.

Merge-base is used for the base-branch/divergence display, not for pushed status
(`pkg/commands/git_commands/main_branches.go:72`-`:76`):

```sh
git merge-base refs/heads/mybranch refs/remotes/origin/master refs/remotes/origin/main

# main-branch resolution — main_branches.go:92, :102, :113
git rev-parse --symbolic-full-name master@{u}
git rev-parse --verify --quiet refs/remotes/origin/master
git rev-parse --verify --quiet refs/heads/master

# fast-forward ancestry — pkg/commands/git_commands/branch.go:290-293
git merge-base --is-ancestor HEAD <ref>

# is this branch merged? — pkg/commands/git_commands/branch.go:339-346
git rev-list --max-count=1 <branch> ^HEAD ^<branch>@{upstream} ^<main…> --
```

### d.4 Branches (`for-each-ref`)

```go
// pkg/commands/git_commands/branch_loader.go:419-428
var branchFields = []string{
	"HEAD",
	"refname:short",
	"upstream:short",
	"upstream:track",
	"push:track",
	"subject",
	"objectname",
	"committerdate:unix",
}
```

The command (`pkg/commands/git_commands/branch_loader.go:392`-`:417`) joins those with `%00`
and picks the sort order from config (`recency`/`date` → `-committerdate`, `alphabetical` →
`refname`, default `refname`):

```sh
git for-each-ref --sort=-committerdate \
  '--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(push:track)%00%(subject)%00%(objectname)%00%(committerdate:unix)' \
  refs/heads
```

Parsing at `pkg/commands/git_commands/branch_loader.go:379`-`:385`.

**Ahead/behind versus upstream come from the format placeholders, not from `rev-list`.**
`%(upstream:track)` / `%(push:track)` are regex-scraped
(`pkg/commands/git_commands/branch_loader.go:466`-`:491`):

```go
ahead := parseDifference(track, `ahead (\d+)`)
behind := parseDifference(track, `behind (\d+)`)
```

`[gone]` yields `("?", "?", true)` (`:474`-`:476`); no upstream yields `("?", "?", false)`
(`:467`-`:472`). **Those `?` values are what the branch row renders when the upstream is gone
or unset** — see §c.

Behind-base-branch counts have two paths. On git ≥ 2.41 a single `for-each-ref`
(`pkg/commands/git_commands/branch_loader.go:158`-`:161`, builder at `:283`-`:295`):

```sh
git for-each-ref '--format=%(refname)%00%(ahead-behind:refs/remotes/origin/master)%00%(ahead-behind:refs/remotes/origin/main)' refs/heads
```

On older git, one call per branch (`pkg/commands/git_commands/branch_loader.go:180`-`:186`):

```sh
git rev-list --left-right --count refs/heads/mybranch...refs/remotes/origin/master   # -> "<ahead>\t<behind>"
```

Supporting queries:

```sh
git for-each-ref --contains <mergeBase> --format=%(refname) refs/remotes/origin/master refs/remotes/origin/main  # branch_loader.go:346-352
git symbolic-ref --short HEAD                                              # branch.go:65-68
git branch --points-at=HEAD '--format=%(HEAD)%00%(objectname)%00%(refname)' # branch.go:78-82
git branch --show-current                                                   # branch.go:107-109
git rev-parse --symbolic-full-name @{-1}                                    # branch.go:122-126
git symbolic-ref -q HEAD                                                    # branch.go:239  (exit code == detached?)
git config --local --get-regexp '^branch\.'                                 # config.go:81-82
git rev-list <from>..<to> --count                                           # branch.go:230-234
```

### d.5 Remotes and tags

```sh
# remotes — pkg/commands/git_commands/remote_loader.go:71-72
git config --local --get-regexp '^remote\.[^.]+\.(url|pushurl)$'

# remote branches — pkg/commands/git_commands/remote_loader.go:117-131
git for-each-ref --sort=-committerdate --format=%(refname) refs/remotes
```

Remote-branch lines are split on `/` into 4 parts (`remote_loader.go:136`) and `HEAD` is
skipped (`remote_loader.go:143`).

Remote mutations (`pkg/commands/git_commands/remote.go`):

```sh
git remote add <name> <url>                                       # :22-24
git remote remove <name>                                          # :30-32
git remote rename <old> <new>                                     # :38-40
git remote set-url <name> <url>                                   # :46-48
git push <remote> --delete refs/heads/<b1> refs/heads/<b2>        # :54-57
git push <remote> --delete refs/tags/<tag>                        # :63-65
git show-ref --verify -- refs/remotes/origin/<branch>             # :72-74
git ls-remote --get-url <remote>                                  # :84-86
```

Tags (`pkg/commands/git_commands/tag_loader.go:31`, parsed with `^([^\s]+)(\s+)?(.*)$` at
`:39`), then `pkg/commands/git_commands/tag.go`:

```sh
git tag --list -n --sort=-creatordate                             # tag_loader.go:31
git tag [--force] -- <tagName> [<ref>]                            # tag.go:21-25  (lightweight)
git tag <tagName> [--force] [<ref>] -m <msg>                      # tag.go:31-35  (annotated)
git show-ref --tags --quiet --verify -- refs/tags/<tag>           # tag.go:41-44
git tag -d <tag>                                                  # tag.go:50
git push <remote> tag <tag>                                       # tag.go:57
git for-each-ref '--format=Tagger:     %(taggername) %(taggeremail)%0aTaggerDate: %(taggerdate)%0a%0a%(contents)' refs/tags/<tag>   # tag.go:72-75
git cat-file -t refs/tags/<tag>                                   # tag.go:81-84
```

### d.6 Stash

```sh
# pkg/commands/git_commands/stash_loader.go:69
git stash list -z '--pretty=%H|%ct|%gs'

# path-filtered variant — pkg/commands/git_commands/stash_loader.go:35
git stash list --name-only '--pretty=%gd:%H|%ct|%gs'
```

Fields are split on `|` into hash, committer unix time and the `%gs` reflog subject
(`pkg/commands/git_commands/stash_loader.go:83`-`:100`); entries match
`^stash@\{(\d+)\}:(.*)$` (`:44`).

```sh
# main-panel stash diff — pkg/commands/git_commands/stash.go:85-93
git -C <worktree> stash show --no-ext-diff --unified=3 --find-renames=50% -p --stat -u --color=always refs/stash@{N}

git stash drop                                       # stash.go:29
git stash drop refs/stash@{N}                        # stash.go:35
git stash pop refs/stash@{N}                         # stash.go:42
git stash apply refs/stash@{N}                       # stash.go:49
git stash push -m <msg>                              # stash.go:57
git stash push --keep-index -m <msg>                 # stash.go:99
git stash push --staged -m <msg>                     # stash.go:128  (git >= 2.35)
git stash push --include-untracked -m <msg>          # stash.go:190
git stash store [-m <msg>] <hash>                    # stash.go:66-69
git rev-parse refs/stash@{N}                         # stash.go:75-77
```

Pre-2.35, the "stash staged only" variant is emulated by piping `git stash show -p` into
`git apply -R` (`pkg/commands/git_commands/stash.go:158`-`:163`).

### d.7 Reflog

It is **`git log -g`**, not `git reflog`
(`pkg/commands/git_commands/reflog_commit_loader.go:28`-`:34`):

```sh
git -c log.showSignature=false log -g '--format=+%H%x00%ct%x00%gs%x00%P'

# with filters
git -c log.showSignature=false log -g '--format=+%H%x00%ct%x00%gs%x00%P' \
    --author=jesse --follow --name-status -- some/path
```

NUL-separated, `+`-prefixed: hash, committer unix time, reflog subject `%gs`, parents
(`pkg/commands/git_commands/reflog_commit_loader.go:69`-`:89`). Loading is incremental: it
stops as soon as it reaches the previously-seen entry
(`pkg/commands/git_commands/reflog_commit_loader.go:50`-`:54`).

### d.8 File diffs (working tree)

```go
// pkg/commands/git_commands/working_tree.go:396-417
noIndex := !node.GetIsTracked() && !node.GetHasStagedChanges() && !cached && node.GetIsFile()

cmdArgs := NewGitCmd("diff").
	AddCommonDiffArgs(self.diffRendererConfigManager, self.UserConfig(), !plain).
	Arg("--submodule").
	Arg(fmt.Sprintf("--color=%s", colorArg)).
	ArgIf(cached, "--cached").
	ArgIf(noIndex, "--no-index").
	Arg("--").
	ArgIf(noIndex, "/dev/null").
	Arg(paths...).
	Dir(self.repoPaths.worktreePath).
	ToArgv()
```

```sh
# (a) unstaged worktree diff of a tracked file
git -C /path/to/worktree diff --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always -- src/foo.go

# (b) staged
git -C /path/to/worktree diff --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always --cached -- src/foo.go

# (c) untracked
git -C /path/to/worktree diff --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always --no-index -- /dev/null new.txt
```

`plain=true` swaps `--color=always` for `--color=never` and drops the UI-only extras
(`--ignore-all-space`, external diff, raw args). A rename passes **both** paths
(`pkg/commands/models/file.go:49`-`:55`, call at
`pkg/commands/git_commands/working_tree.go:388`). Note this particular command does **not**
carry `-c diff.noprefix=false`, unlike the ref-to-ref variants below.

Ref-to-ref file diff, used by the patch builder and the commit-file view
(`pkg/commands/git_commands/working_tree.go:431`-`:451`):

```sh
git -C /worktree -c diff.noprefix=false diff --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always <from> <to> [-R] -- src/foo.go [old/path.go]
```

Generic diff command objects (`pkg/commands/git_commands/diff.go`):

```sh
# main-view range diffs — diff.go:21-32
git -C /worktree -c diff.noprefix=false diff --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always <diffArgs…>

# plain, e.g. copy-to-clipboard — diff.go:41-51
git -C /worktree -c diff.noprefix=false diff --no-ext-diff --no-color [--staged] <additionalArgs…>

# diff.go:91-97
git -c diff.noprefix=false diff-index --submodule --no-ext-diff --no-color --patch <diffArgs…>

# external difftool — diff.go:79-88
git difftool --no-prompt [--dir-diff] [--cached] [<from>] [<to>] [-R] -- <path>
```

### d.9 Commit diff and commit file list

```go
// pkg/commands/git_commands/commit.go:243-259
cmdArgs := NewGitCmd("show").
	Config("diff.noprefix=false").
	AddCommonDiffArgs(self.diffRendererConfigManager, self.UserConfig(), true).
	Arg("--submodule").
	Arg("--color=" + self.diffRendererConfigManager.GetColorArg()).
	Arg("--stat").
	Arg("--decorate").
	Arg("-p").
	Arg(hash).
	Arg("--").
	Arg(filterPaths...).
	Dir(self.repoPaths.worktreePath).
	ToArgv()
```

```sh
git -C /worktree -c diff.noprefix=false show --no-ext-diff --unified=3 --find-renames=50% \
    --submodule --color=always --stat --decorate -p <hash> -- [filterPaths…]
```

The commit's **file list is a `diff --name-status -z`, not `diff-tree`**
(`pkg/commands/git_commands/commit_file_loader.go:25`-`:36`):

```sh
git -c diff.noprefix=false diff --submodule --no-ext-diff --name-status -z --find-renames=50% [-R] <from> <to>
# for one commit lazygit passes from="<hash>^" to="<hash>"  (see working_tree.go:419)
```

Output looks like `"MM\x00file1\x00R100\x00old\x00new\x00"` and is parsed at
`pkg/commands/git_commands/commit_file_loader.go:51`-`:79`.

Other `show` variants (`pkg/commands/git_commands/commit.go`):

```sh
git show --no-color <hash>                                    # :178
git show --no-patch '--pretty=format:%an%x00%ae' <hash>       # :190-191
git show --no-patch --pretty=format:%s <hash…>                # :213-215
git show --no-patch --oneline <hash…>                         # :222-223
git show <hash>:<path>                                        # :262-263
git -c log.showsignature=false log --format=%B --max-count=1 <hash>   # :158-160
git -c log.showsignature=false log --format=%s --max-count=1 <hash>   # :168-170
git log -1 --skip=<N> --pretty=%H                             # :305

git blame -l -L<first>,+<num> <commit> -- <file>              # pkg/commands/git_commands/blame.go:25-30
git show :<stage>:<path>                                      # working_tree.go:535-536
git rev-parse :<stage>:<path>                                 # working_tree.go:543-544
```

### d.10 Stage, unstage, discard

All from `pkg/commands/git_commands/working_tree.go` unless noted.

| Action | Command | file:line |
|---|---|---|
| stage paths | `git add [<extraArgs>] -- <paths…>` | `:45`-`:52` |
| stage all | `git add -u` (tracked only) / `git add -A` | `:56`-`:62` |
| unstage everything | `git reset` | `:65`-`:67` |
| unstage tracked | `git reset HEAD -- <paths…>` | `:79`-`:81` |
| unstage a newly added file | `git rm --cached --force -- <paths…>` | `:83`-`:85` |
| discard unstaged, one file | `git checkout -- <path>` | `:359`-`:362` |
| discard all unstaged | `git checkout -- .` | `:462`-`:467` |
| checkout file from a commit | `git checkout <hash> -- <file>` | `:454`-`:459` |
| untrack but keep on disk | `git rm -r --cached -- <name>` | `:470`-`:475` |
| remove a conflicted file | `git rm -- <name>` | `:477`-`:482` |
| remove untracked | `git clean -fd` | `:485`-`:489` |
| reset working tree | `git reset --hard\|--soft\|--mixed <ref>` | `:512`-`:532` |
| reset onto a commit (panel `g`) | `git reset --<strength> <hash>` (with `GIT_TERMINAL_PROMPT=0`) | `pkg/commands/git_commands/commit.go:79`-`:86` |
| list tracked files | `git ls-files -z` | `:581`-`:583` |
| merge tool | `git mergetool` | `:36`-`:38` |

**`git restore` is never used** — discard is always `checkout` / `reset` / `rm` / `clean`.

The discard edge cases matter and are easy to get wrong
(`pkg/commands/git_commands/working_tree.go:123`-`:180`):

```go
if file.ShortStatus == "AA" {
	// git checkout --ours -- <p>  then  git add -- <p>
}
if file.ShortStatus == "DU" {
	// git rm -- <p>
}
if file.HasStagedChanges || file.HasMergeConflicts {
	// git reset -- <p>   (first)
}
if file.ShortStatus == "DD" || file.ShortStatus == "AU" {
	return nil          // nothing further
}
if file.Added {
	return self.os.RemoveFile(file.Path)   // untracked: delete from disk, no git
}
return self.DiscardUnstagedFileChanges(file)   // git checkout -- <p>
```

Renames recurse through `BeforeAndAfterFileForRename`
(`pkg/commands/git_commands/working_tree.go:87`-`:120`), which re-runs status with
`--no-renames`. Directory-level discard batches
`reset` → remove files → `checkout`
(`pkg/commands/git_commands/working_tree.go:190`-`:249`, `:251`-`:277`).

### d.11 Partial staging: hunk and line patches

**The patch is built in Go, in-process — git is not asked to produce it.**
(`pkg/gui/controllers/staging_controller.go:249`-`:272`):

```go
firstLineIdx, lastLineIdx := state.SelectedPatchRange()
patchToApply := patch.
	Parse(state.GetDiff()).
	Transform(patch.TransformOpts{
		Reverse:             reverse,
		IncludedLineIndices: patch.ExpandRange(firstLineIdx, lastLineIdx),
		FileNameOverride:    path,
	}).
	FormatPlain()
...
err := self.c.Git().Patch.ApplyPatch(
	patchToApply,
	git_commands.ApplyPatchOpts{
		Reverse: reverse,
		Cached:  !reverse || self.staged,
	},
)
```

The crucial subtlety is documented on the option itself
(`pkg/commands/patch/transform.go:14`-`:45`):

```go
// Create a patch that will applied in reverse with `git apply --reverse`.
// This affects how unselected lines are treated when only parts of a hunk
// are selected: usually, for unselected lines we change '-' lines to
// context lines and remove '+' lines, but when Reverse is true we need to
// turn '+' lines into context lines and remove '-' lines.
Reverse bool
```

Header rewriting is at `pkg/commands/patch/transform.go:76`-`:104` (`FileNameOverride` replaces
the header with exactly `--- a/<path>` / `+++ b/<path>`;
`TurnAddedFilesIntoDiffAgainstEmptyFile` drops `new file mode` and rewrites `--- /dev/null`)
and `:111`ff (`stripRenameFromHeader`). Hunk headers are re-counted in Go
(`pkg/commands/patch/hunk.go`, `pkg/commands/patch/patch_line.go`); output formatting is
`formatPlain` (`pkg/commands/patch/format.go:22`-`:29`).

**Application goes through a temp file, never stdin**
(`pkg/commands/git_commands/patch.go:60`-`:88`):

```go
cmdArgs := NewGitCmd("apply").
	ArgIf(opts.ThreeWay, "--3way").
	ArgIf(opts.Cached, "--cached").
	ArgIf(opts.Index, "--index").
	ArgIf(opts.Reverse, "--reverse").
	Arg(filepath).
	ToArgv()
```

**The complete flag set is `--3way`, `--cached`, `--index`, `--reverse`.**
`--unidiff-zero`, `--recount` and `--whitespace=…` appear nowhere in the repository.

```sh
# stage selected lines   (reverse=false, staged=false -> Cached=true)
git apply --cached "$TMPDIR/<repo>/Jan  5 12.34.56.000000000.patch"

# unstage selected lines (reverse=true, staged=true  -> Cached=true)
git apply --cached --reverse "$TMPDIR/<repo>/….patch"

# discard selected lines (reverse=true, staged=false -> Cached=false)
git apply --reverse "$TMPDIR/<repo>/….patch"

# custom patch builder / move-patch flows (patch.go:50-58, :198, :255, :301)
git apply --3way --index [--reverse] "$TMPDIR/<repo>/….patch"
```

The patch file path is
`filepath.Join(tempDir, repoName, time.Now().Format("Jan _2 15.04.05.000000000")+".patch")`
(`pkg/commands/git_commands/patch.go:88`ff). Editing a hunk in `$EDITOR` (`E`) writes the
transformed patch to a temp file, opens the editor, re-parses the edited text and applies it
with `{Reverse: self.staged, Cached: true}`
(`pkg/gui/controllers/staging_controller.go:296`-`:353`).

> **Native note:** `fleet-git` exposes this as
> `Repository::apply_patch_selection(selection: PatchSelection, action: PatchAction)`
> (`crates/fleet-git/src/mutation.rs:92`), with `HunkSelection.lines: Option<Vec<usize>>`
> — `None` for hunk mode, `Some` for line mode
> (`crates/fleet-git/src/model.rs:497`-`:527`).

### d.12 Push, pull, fetch

```go
// pkg/commands/git_commands/sync.go:31-46
cmdArgs := NewGitCmd("push").
	ArgIf(opts.Force, "--force").
	ArgIf(opts.ForceWithLease, "--force-with-lease").
	ArgIf(opts.SetUpstream, "--set-upstream").
	ArgIf(opts.UpstreamRemote != "", opts.UpstreamRemote).
	ArgIf(opts.UpstreamBranch != "", fmt.Sprintf("refs/heads/%s:%s", opts.CurrentBranch, opts.UpstreamBranch)).
	ToArgv()
```

```sh
git push
git push --force-with-lease
git push --set-upstream origin refs/heads/mybranch:mybranch
git push --force origin refs/heads/mybranch:otherbranch
```

Fetch always passes `--no-write-fetch-head`, with the reason stated inline
(`pkg/commands/git_commands/sync.go:57`-`:63`): *"avoid writing to .git/FETCH_HEAD; this allows
running a pull concurrently without getting errors"*.

```sh
git fetch --all --no-write-fetch-head                                              # sync.go:65-84
git fetch --no-write-fetch-head <remote>                                           # sync.go:127-132
git fetch --no-write-fetch-head <remote> refs/heads/<remoteBranch>:<localBranch>   # sync.go:113-125
```

```go
// pkg/commands/git_commands/sync.go:98-111
cmdArgs := NewGitCmd("pull").
	Arg("--no-edit").
	ArgIf(opts.FastForwardOnly, "--ff-only").
	ArgIf(opts.RemoteName != "", opts.RemoteName).
	ArgIf(opts.BranchName != "", "refs/heads/"+opts.BranchName).
	GitDirIf(opts.WorktreeGitDir != "", opts.WorktreeGitDir).
	WorktreePathIf(opts.WorktreePath != "", opts.WorktreePath).
	ToArgv()

// setting GIT_SEQUENCE_EDITOR to ':' as a way of skipping it, in case the user
// has 'pull.rebase = interactive' configured.
return self.cmd.New(cmdArgs).AddEnvVars("GIT_SEQUENCE_EDITOR=:").PromptOnCredentialRequest(task).Run()
```

```sh
GIT_SEQUENCE_EDITOR=: git [--work-tree <wt>] [--git-dir <gd>] pull --no-edit [--ff-only] [<remote>] [refs/heads/<branch>]
```

**There is no `--rebase` flag on lazygit's pull.** Rebase-versus-merge is left to the user's
`pull.rebase` config, which is exactly why the `GIT_SEQUENCE_EDITOR=:` guard exists.

**Credential handling.** `PromptOnCredentialRequest(task)`
(`pkg/commands/oscommands/cmd_obj.go:217`) selects the `PROMPT` strategy;
`FailOnCredentialRequest()` (`:225`) selects `FAIL`. The runner then forces English output so
it can pattern-match the prompt (`pkg/commands/oscommands/cmd_obj_runner.go:373`-`:374`):

```go
// setting the output to english so we can parse it for a username/password request
cmdObj.AddEnvVars("LANG=C", "LC_ALL=C", "LC_MESSAGES=C")
```

Recognised credential kinds are `Password, Username, Passphrase, PIN, Token`
(`pkg/commands/oscommands/cmd_obj_runner.go:331`-`:337`); the command runs in a PTY and the
answer is written to stdin (`:376`-`:420`), with the process killed if the prompt callback
returns `nil` (`:341`-`:343`, `:399`-`:406`).

**`GIT_ASKPASS` is never set.** `GIT_TERMINAL_PROMPT=0` appears in exactly two places, both
with the comment *"prevents git from prompting us for input which would freeze the program"*:
`pkg/commands/git_commands/branch.go:157`-`:162` (`Checkout`) and
`pkg/commands/git_commands/commit.go:81`-`:86` (`ResetToCommit`).

### d.13 Merge, rebase and conflict detection

```go
// pkg/commands/git_commands/branch.go:262-286
cmdArgs := NewGitCmd("merge").
	Arg("--no-edit").
	Arg(strings.Fields(self.UserConfig().Git.Merging.Args)...).
	Arg(extraArgs...).
	Arg(branchName).
	ToArgv()
```

```sh
git merge --no-edit [<user Git.Merging.Args…>] <branch>   # MERGE_VARIANT_REGULAR
git merge --no-edit --ff <branch>                          # MERGE_VARIANT_FAST_FORWARD
git merge --no-edit --no-ff <branch>                       # MERGE_VARIANT_NON_FAST_FORWARD
git merge --no-edit --squash --ff <branch>                 # MERGE_VARIANT_SQUASH
```

```sh
# squash-all-above-fixups — pkg/commands/git_commands/rebase.go:396-398
git rebase --interactive --rebase-merges --autostash --autosquash <hash>^   # or --root
```

Continue / abort / skip share one helper
(`pkg/commands/git_commands/rebase.go:466`-`:478`):

```go
func (self *RebaseCommands) GenericMergeOrRebaseActionCmdObj(commandType string, command string) *oscommands.CmdObj {
	cmdArgs := NewGitCmd(commandType).Arg("--" + command).ToArgv()
	return self.cmd.New(cmdArgs)
}
```

```sh
git rebase --continue          # rebase.go:472-474
git rebase --abort             # rebase.go:476-478
git rebase --skip
git merge --continue / --abort / --skip     # same helper with commandType == "merge"
```

All of them are wrapped in `runSkipEditorCommand` (see §d.14), and
`"no rebase in progress"` on stderr is swallowed
(`pkg/commands/git_commands/rebase.go:485`-`:488`).

```go
// pkg/commands/git_commands/rebase.go:559-569
cmdArgs := NewGitCmd("cherry-pick").
	Arg("--allow-empty").
	ArgIf(self.version.IsAtLeast(2, 45, 0), "--empty=keep", "--keep-redundant-commits").
	ArgIf(hasMergeCommit, "-m1").
	Arg(lo.Reverse(...)...).
	ToArgv()
```

```sh
git cherry-pick --allow-empty --empty=keep --keep-redundant-commits [-m1] <hash…>
git revert [-m 1] <hash…>          # pkg/commands/git_commands/commit.go:272-279
```

**Conflicts are detected purely from the porcelain status codes — no extra git call**
(`pkg/commands/models/file.go:150`-`:167`):

```go
stagedChange := shortStatus[0:1]
unstagedChange := shortStatus[1:2]
tracked := !lo.Contains([]string{"??", "A ", "AM"}, shortStatus)
hasStagedChanges := !lo.Contains([]string{" ", "U", "?"}, stagedChange)
hasInlineMergeConflicts := lo.Contains([]string{"UU", "AA"}, shortStatus)
hasMergeConflicts := hasInlineMergeConflicts || lo.Contains([]string{"DD", "AU", "UA", "UD", "DU"}, shortStatus)
```

- **inline (marker-bearing) conflicts:** `UU`, `AA`
- **other conflicts:** `DD`, `AU`, `UA`, `UD`, `DU` (human descriptions at
  `pkg/commands/models/file.go:109`-`:123`)

Marker parsing lives in `pkg/gui/mergeconflicts/find_conflicts.go`: `LineType` is
`START, ANCESTOR, TARGET, END, NOT_A_MARKER` (`:16`-`:22`), with
`const defaultConflictMarkerSize = 7` (`:26`) and the per-file override coming from the
`git check-attr` call in §d.2 (`:32`-`:38`).

```sh
# conflict resolution helpers — pkg/commands/git_commands/working_tree.go:555-575
git merge-file <strategy> --stdout <ours> <base> <theirs>
git merge-file <strategy> --stdout --object-id <oursID> <baseID> <theirsID>   # git >= 2.43
```

### d.14 Interactive rebase via `GIT_SEQUENCE_EDITOR` (the daemon mechanism)

This is the trick that makes lazygit's commit operations possible, and it is worth copying
exactly.

**The env var constants, verbatim** (`pkg/app/daemon/daemon.go:44`-`:49`):

```go
const (
	DaemonKindEnvKey string = "LAZYGIT_DAEMON_KIND"

	// Contains json-encoded arguments to the daemon
	DaemonInstructionEnvKey string = "LAZYGIT_DAEMON_INSTRUCTION"
)
```

```go
// pkg/app/daemon/daemon.go:133-138
func ToEnvVars(instruction Instruction) []string {
	return []string{
		fmt.Sprintf("%s=%d", DaemonKindEnvKey, instruction.Kind()),
		fmt.Sprintf("%s=%s", DaemonInstructionEnvKey, instruction.SerializedInstructions()),
	}
}
```

**The daemon-kind enum** (`pkg/app/daemon/daemon.go:27`-`:42`):

```go
type DaemonKind int

const (
	// for when we fail to parse the daemon kind
	DaemonKindUnknown DaemonKind = iota

	DaemonKindExitImmediately
	DaemonKindRemoveUpdateRefsForCopiedBranch
	DaemonKindMoveTodosUp
	DaemonKindMoveTodosDown
	DaemonKindInsertBreak
	DaemonKindChangeTodoActions
	DaemonKindDropMergeCommit
	DaemonKindMoveFixupCommitDown
	DaemonKindWriteRebaseTodo
)
```

Numerically: `Unknown=0`, `ExitImmediately=1`, `RemoveUpdateRefsForCopiedBranch=2`,
`MoveTodosUp=3`, `MoveTodosDown=4`, `InsertBreak=5`, `ChangeTodoActions=6`,
`DropMergeCommit=7`, `MoveFixupCommitDown=8`, `WriteRebaseTodo=9`. Dispatch table at
`pkg/app/daemon/daemon.go:54`-`:66`; mode detection at `:81`-`:92`:

```go
func InDaemonMode() bool { return getDaemonKind() != DaemonKindUnknown }

func getDaemonKind() DaemonKind {
	intValue, err := strconv.Atoi(os.Getenv(DaemonKindEnvKey))
	if err != nil { return DaemonKindUnknown }
	return DaemonKind(intValue)
}
```

**Re-executing its own binary** (`pkg/commands/oscommands/os.go:367`-`:373`):

```go
func GetLazygitPath() string {
	ex, err := os.Executable() // get the executable path for git to use
	if err != nil {
		ex = os.Args[0] // fallback to the first call argument if needed
	}
	return `"` + filepath.ToSlash(ex) + `"`
}
```

Note the returned string is **already double-quoted and slash-normalised**, because git treats
`GIT_SEQUENCE_EDITOR` as a shell command line, not as an argv[0].

**The rebase command and its full environment**
(`pkg/commands/git_commands/rebase.go:216`-`:260`):

```go
cmdArgs := NewGitCmd("rebase").
	Arg("--interactive").
	Arg("--autostash").
	Arg("--keep-empty").
	ArgIf(opts.keepCommitsThatBecomeEmpty, "--empty=keep").
	Arg("--no-autosquash").
	Arg("--rebase-merges").
	ArgIf(opts.onto != "", "--onto", opts.onto).
	Arg(opts.baseHashOrRoot).
	ToArgv()
...
cmdObj.AddEnvVars(
	"DEBUG="+debug,
	"LANG=C",        // Force using English language
	"LC_ALL=C",      // Force using English language
	"LC_MESSAGES=C", // Force using English language
	"GIT_SEQUENCE_EDITOR="+gitSequenceEditor,
)

if opts.overrideEditor {
	cmdObj.AddEnvVars("GIT_EDITOR=" + ex)
}
```

A typical squash/edit flow reconstructs to:

```sh
GIT_OPTIONAL_LOCKS=0 \
LAZYGIT_DAEMON_KIND=6 \
LAZYGIT_DAEMON_INSTRUCTION='{"Changes":[{"Hash":"abc123…","NewAction":10,"Flag":""}]}' \
DEBUG=FALSE \
LANG=C LC_ALL=C LC_MESSAGES=C \
GIT_SEQUENCE_EDITOR='"/usr/local/bin/lazygit"' \
GIT_EDITOR='"/usr/local/bin/lazygit"' \
git rebase --interactive --autostash --keep-empty --no-autosquash --rebase-merges <baseHash>
```

With `--onto`:

```sh
git rebase --interactive --autostash --keep-empty --empty=keep --no-autosquash --rebase-merges --onto <targetBranch> <baseCommit>
```

`baseHashOrRoot` is `commits[index].Hash()`, or the literal `--root` when the index runs past
the loaded commits (`pkg/commands/git_commands/rebase.go:580`-`:590`).

**Editing an in-progress todo** (`pkg/commands/git_commands/rebase.go:263`-`:291`):

```sh
LAZYGIT_DAEMON_KIND=9 LAZYGIT_DAEMON_INSTRUCTION='{"TodosFileContent":"<base64>"}' \
DEBUG=FALSE LANG=C LC_ALL=C LC_MESSAGES=C \
GIT_EDITOR='"…/lazygit"' GIT_SEQUENCE_EDITOR='"…/lazygit"' \
git rebase --edit-todo
```

**Skipping the editor entirely** — used by continue/abort/skip and autosquash
(`pkg/commands/git_commands/rebase.go:505`-`:517`):

```go
func (self *RebaseCommands) runSkipEditorCommand(cmdObj *oscommands.CmdObj) error {
	instruction := daemon.NewExitImmediatelyInstruction()
	lazyGitPath := oscommands.GetLazygitPath()
	return cmdObj.
		AddEnvVars(
			"GIT_EDITOR="+lazyGitPath,
			"GIT_SEQUENCE_EDITOR="+lazyGitPath,
			"EDITOR="+lazyGitPath,
			"VISUAL="+lazyGitPath,
		).
		AddEnvVars(daemon.ToEnvVars(instruction)...).
		Run()
}
```

```sh
GIT_EDITOR='"…/lazygit"' GIT_SEQUENCE_EDITOR='"…/lazygit"' \
EDITOR='"…/lazygit"' VISUAL='"…/lazygit"' \
LAZYGIT_DAEMON_KIND=1 LAZYGIT_DAEMON_INSTRUCTION='{}' \
git rebase --continue
```

**What the child process does when git invokes it**
(`pkg/app/daemon/rebase.go:20`-`:39`):

```go
func handleInteractiveRebase(common *common.Common, f func(path string) error) error {
	common.Log.Info("Lazygit invoked as interactive rebase demon")
	common.Log.Info("args: ", os.Args)
	path := os.Args[1]

	if strings.HasSuffix(path, "git-rebase-todo") {
		err := utils.RemoveUpdateRefsForCopiedBranch(path, getCommentChar())
		if err != nil {
			return err
		}
		return f(path)
	} else if strings.HasSuffix(path, filepath.Join(gitDir(), "COMMIT_EDITMSG")) { // TODO: test
		// if we are rebasing and squashing, we'll see a COMMIT_EDITMSG
		// but in this case we don't need to edit it, so we'll just return
	} else {
		common.Log.Info("Lazygit demon did not match on any use cases")
	}

	return nil
}
```

Git passes the file to edit as `os.Args[1]`. The comment character is resolved with a raw
`exec.Command`, bypassing the builder (`pkg/app/daemon/daemon.go:94`-`:101`):

```go
cmd := exec.Command("git", "config", "--get", "--null", "core.commentChar")
```

**Todo-file rewriting** uses `github.com/stefanhaller/git-todo-parser/todo`, pinned at
`go.mod:40` (`v0.0.7-0.20250905083220-c50528f08304`), imported by
`pkg/commands/git_commands/commit_loader.go:20`, `pkg/app/daemon/rebase.go:11` and
`pkg/utils/rebase_todo.go:11`. lazygit's own wrappers are in `pkg/utils/rebase_todo.go`:
`EditRebaseTodo` (`:27`), `ReadRebaseTodoFile` (`:71`), `WriteRebaseTodoFile` (`:85`),
`PrependStrToTodoFile` (`:104`), `DeleteTodos` (`:119`), `MoveTodos` (`:154`),
`MoveFixupCommitDown` (`:233`), `RemoveUpdateRefsForCopiedBranch` (`:278`),
`DropMergeCommit` (`:309`).

Some edits bypass git entirely and rewrite
`<worktreeGitDir>/rebase-merge/git-rebase-todo` in place
(`pkg/commands/git_commands/rebase.go:339`-`:353` for `EditRebaseTodo`, `:380`-`:387` for
`MoveTodos`); others rewrite the file and then run `git rebase --edit-todo`
(`pkg/commands/git_commands/rebase.go:355`-`:370`).

> **Native note:** `crates/fleet-git` implements the same mechanism with its own names —
> `SEQUENCE_INSTRUCTION_ENV = "FLEET_GIT_SEQUENCE_INSTRUCTION"`
> (`crates/fleet-git/src/rebase.rs:15`), `maybe_run_from_env()`
> (`crates/fleet-git/src/rebase.rs:194`) and `run_sequence_editor(todo_path, encoded_plan)`
> (`crates/fleet-git/src/rebase.rs:178`), driven by a typed `RebasePlan`
> (`crates/fleet-git/src/rebase.rs:76`-`:87`) instead of a JSON blob. The UI's `main()` must
> call `fleet_git::sequence_editor::maybe_run_from_env()` before anything else — see the
> companion brief §f.1.
