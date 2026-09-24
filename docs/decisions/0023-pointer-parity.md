# 0023 — Pointer parity: every action has a key and a visible control

**Adopted** for every surface in `fleet-app` and every interactive component in `fleet-ui-kit`.
Every action reachable by a key is also reachable by a visible control, the control shows its key,
and no key is taken away. The status-bar mode word is retired. This reverses `DESIGN-SYSTEM.md` §1.6
and §1.7 as they stood before this record.

## Context

Fleet was designed as a keyboard-only app that drew no button anywhere. Every affordance was a
`KeyHint`, footers were `KeyHintRow` legends, dialogs were confirmed only from the keyboard, and a
fixed 84 px `ModeWord` (`NORMAL`, `TERMINAL`, `^S`, `DIALOG`…) sat in the status bar on every
screen.

In practice this left most of the app undiscoverable. Hub rows had no click handler. The context
bar's chips counted things you could not open. `^S` showed an amber word and nothing about what came
next. Help was about 470 auto-generated labels ("select tab1"). Dialogs had no close control and no
button row: `dialog.button[N]` was a harness name that nothing painted. Features you could not see
were features nobody used, and the mode word told people nothing they needed to act on.

The design source is the "Fleet beauty pass" mockup canvas
(https://claude.ai/artifact/2ZWjEb8WTTrHQbpzQAked6). Each row shows today's screen, the proposed
one, and a note on what changed and why. Its principles are the ones adopted here.

## Decision

- **Parity.** Every action bound in `keymap::table()` is reachable by pointer: the actions a
  surface is about from a control on that surface (a button, a menu item, a clickable row or
  chip), and every action from the ⌃S command menu, the palette or Help. Hover may reveal a row's
  actions, but none lives only behind hover: each is also in the row's right-click menu, and in
  its detail panel where it has one.
- **The key is shown on the control.** A control that runs an action shows that action's key as a
  `Kbd` chip, inside the button, at the right of the menu item, or in the tooltip of an icon-only
  button. The chip is resolved from the live keymap for the focused context and is never typed by
  hand. An action with no binding in that context shows no chip.
- **One path.** A control dispatches the same action its key dispatches. No surface has a
  pointer-only code path for something a key also does.
- **Controls are not focusable.** Buttons and menu triggers have no tab stop and no focus ring.
  The keyboard path to a control's action is its key. `Tab`, `j`/`k`, `h`/`l` and pane focus keep
  exactly the meaning `KEYMAP.md` gives them. Menus and popovers, once open, are navigated with
  `↑`/`↓`/`⏎`/`esc` like any list.
- **No mode word.** The status bar shows where you are (the daemon's state and the breadcrumb) and
  what is running. A state that changes what keys do is shown on the surface that has it: the
  scroll pill, the filter bar, the open overlay, the focused pane, the ⌃S command menu. The key
  contexts themselves and the harness snapshot's `mode` field are unchanged.
- **Hints live in controls, not legends.** Footer key legends are removed. A `KeyHintRow` remains
  only where there is no control to put a key in: the terminal exit strip and the scroll pill.
- **Colour.** `accent` is also the fill of a surface's one primary button. A destructive action's
  strong form (the `Y` confirm) is a `danger` button. Risk shows in the button's colour; the
  `y`/`Y` rule that decides it is unchanged.
- **Pointer conventions.** A click on a row selects it; a double-click or `⏎` opens it; a
  right-click opens its menu. Clicking outside a dialog or sheet closes it through the same cancel
  action as `esc`. The command palette also opens with ⌘K on macOS and `ctrl-k` elsewhere
  (`KEYMAP.md`), and from a visible command field in the title bar.

## Alternatives rejected

- **Keep the legends and add hover only.** Hover states and tooltips on today's layout make
  nothing new visible: a feature still exists only for people who already know its key. The
  mockup's point is that the control *is* the teaching, so the key has to be on the thing you can
  see.
- **Make buttons focusable with `Tab` / `⏎`.** Fleet already has a focus model: `Tab` and `h`/`l`
  move between panes, `j`/`k` move a list cursor, and the detail panel is never in the cycle. A
  second, per-control focus ring would compete with it, make `Tab` mean two things, and add a
  keystroke to every action that already has one. The key is the keyboard path.
- **Keep the mode word next to the new chrome.** It cost 84 px of every status bar to show a word
  that duplicated what the surface already showed, and the one real case it served, knowing that
  the prefix is armed, is now the ⌃S command menu.

## Consequences

- `DESIGN-SYSTEM.md` §1.4, §1.6, §1.7, §4 and §7, and `UX-SPEC.md` §1.4, §1.9 and §5, are rewritten
  by this record. `KEYMAP.md` states the ⌘K palette key.
- The kit grows `Kbd`, `Button`, `IconButton`, `Tooltip`, menus and pointer-first rows. Dialogs
  and sheets gain a close ✕, button footers and click-outside dismissal. `ModeWord`, `ContextBar`
  and `PrefixHint` leave when their replacements land.
- Labels are hand-written once in an action catalogue that Help, the palette, the ⌃S menu,
  tooltips and buttons all read. Nothing user-visible is generated from an action's type name.
- The harness vocabulary grows: `dialog.button[N]` is finally painted, and each new control gets
  a target name recorded in `TESTING-HARNESS.md`. No existing name is renamed or removed.
- Until a surface is rebuilt, its section in `UX-SPEC.md` §2–§3 and `DESIGN-SYSTEM.md` §6 describes
  what ships, even where it still names the mode word, a footer legend or "no button pair". This
  record's rules govern new work, and each section is rewritten in the commit that rebuilds its
  surface.
