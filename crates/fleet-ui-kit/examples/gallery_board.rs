//! The visual and behavioural test bench for the **board** group of `fleet-ui-kit`.
//!
//! `KanbanBoard` · `KanbanColumn` · `CardTile` · `PriorityGlyph` · `MarkdownText` ·
//! multi-line `TextInput`.
//!
//! Every component appears in every state it can be in, in both themes, and the interactive
//! ones are *live*: the cursor really moves between columns, `[` / `]` really move the card,
//! and the description editor really edits — multi-line, with `↑` / `↓` moving by visual row.
//! If a state is not visible or not operable here, it is not implemented.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_board
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `h` `l` | previous / next column |
//! | `j` `k` | previous / next card |
//! | `[` `]` | move the selected card one column left / right |
//! | `p` | cycle the selected card's priority |
//! | `i` | edit the description · `esc` leaves it |
//! | `m` | toggle the description between read and edit mode |
//! | `ctrl-t` | toggle light / dark |
//! | `ctrl-q` / `cmd-q` | quit |
//!
//! Bare-letter keys are bound only in the `BoardNormal` context. While the description editor
//! owns the keyboard the root switches to `BoardTyping`, so `h`, `i`, `p` and friends are typed
//! instead of fired — the same rule §3.10 states for the real Hub.

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 150.0,
    column: true,
    divided: false,
    compact: false,
};
use std::rc::Rc;

use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Hsla, KeyBinding, ListState,
    MouseDownEvent, Pixels, ScrollHandle, SharedString, Window, actions, div, px,
};
use support::layout::strip;

actions!(
    gallery_board,
    [
        ToggleTheme,
        Quit,
        NextColumn,
        PrevColumn,
        NextCard,
        PrevCard,
        MoveRight,
        MoveLeft,
        CyclePriority,
        EditDescription,
        ToggleReadMode,
        Escape,
    ]
);

/// How many visual rows the description editor shows.
const EDITOR_ROWS: usize = 8;

/// The height the live board is staged at, so a column really scrolls inside the gallery.
const BOARD_H: f32 = 420.0;

/// The width one static card panel is measured at; a real column is `COLUMN_WIDTH_CH` wide.
const TILE_W: f32 = 260.0;

/// The width a static text-area panel is measured at.
const AREA_W: f32 = 320.0;

/// The status category a column belongs to, which is the only thing that picks its accent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Backlog,
    Started,
    Completed,
}

impl Category {
    /// The token this category's accent bar uses. The mapping is domain knowledge, which is
    /// why `KanbanColumn::accent` takes a resolved color and not a name.
    fn accent(self, theme: &Theme) -> Hsla {
        match self {
            Category::Backlog => theme.colors.text_muted,
            Category::Started => theme.colors.warning,
            Category::Completed => theme.colors.success,
        }
    }
}

/// One card of the demo board.
#[derive(Clone)]
struct DemoCard {
    key: SharedString,
    title: SharedString,
    priority: PriorityLevel,
    labels: Vec<(SharedString, Option<SharedString>)>,
    assignee: Option<SharedString>,
    estimate: Option<u32>,
    due: Option<SharedString>,
    worktree: bool,
    dirty: bool,
    conflict: bool,
    description: String,
}

impl DemoCard {
    fn new(key: &str, title: &str, description: &str) -> Self {
        Self {
            key: key.to_string().into(),
            title: title.to_string().into(),
            priority: PriorityLevel::None,
            labels: Vec::new(),
            assignee: None,
            estimate: None,
            due: None,
            worktree: false,
            dirty: false,
            conflict: false,
            description: description.to_string(),
        }
    }

    fn priority(mut self, priority: PriorityLevel) -> Self {
        self.priority = priority;
        self
    }

    fn label(mut self, name: &str, token: Option<&str>) -> Self {
        self.labels.push((
            name.to_string().into(),
            token.map(|token| token.to_string().into()),
        ));
        self
    }

    fn assignee(mut self, assignee: &str) -> Self {
        self.assignee = Some(assignee.to_string().into());
        self
    }

    fn estimate(mut self, estimate: u32) -> Self {
        self.estimate = Some(estimate);
        self
    }

    fn due(mut self, due: &str) -> Self {
        self.due = Some(due.to_string().into());
        self
    }

    fn worktree(mut self, dirty: bool) -> Self {
        self.worktree = true;
        self.dirty = dirty;
        self
    }

    fn conflict(mut self) -> Self {
        self.conflict = true;
        self
    }
}

/// One column of the demo board, with the list state its virtualized body is drawn through.
struct DemoColumn {
    title: SharedString,
    category: Category,
    /// Shared with the row closure, so a frame hands the column a refcount and not a copy.
    cards: Rc<Vec<DemoCard>>,
    list: ListState,
}

impl DemoColumn {
    fn new(title: &str, category: Category, cards: Vec<DemoCard>) -> Self {
        let list = KanbanColumn::list_state();
        list.splice(0..0, cards.len());
        Self {
            title: title.to_string().into(),
            category,
            cards: Rc::new(cards),
            list,
        }
    }
}

/// The markdown every card description is a variation of: every construct the parser knows.
const DESCRIPTION: &str = "\
# Board sync
The reconciler is **pure**: it takes the local card, the remote card and the policy, and
returns ops. A bare URL such as https://fleet.dev/docs/board renders in the accent color.

## Steps
1. `fleet board sync` fetches the remote cards.
2. A conflict stops at the card, never at the board.

- Unknown syntax such as ~~strike~~ stays text.
- An unclosed **bold survives as two asterisks.

```sh
fleet board sync --board work
```";

fn fixtures() -> Vec<DemoColumn> {
    vec![
        DemoColumn::new(
            "Backlog",
            Category::Backlog,
            vec![
                DemoCard::new(
                    "FLT-31",
                    "Board backend adapter for Jira",
                    "Map native issues onto `RemoteCard`.\n\nNative-only fields ride in `properties`.",
                )
                .priority(PriorityLevel::Medium)
                .label("backend", Some("info"))
                .estimate(5),
                DemoCard::new(
                    "FLT-32",
                    "Decide the conflict policy defaults",
                    "Three policies: **keep local**, **take remote**, **ask**.",
                )
                .label("spec", None),
                DemoCard::new(
                    "FLT-33",
                    "A card with nothing but a key and a title, to prove a bare tile costs no meta row",
                    "",
                ),
            ],
        ),
        DemoColumn::new(
            "In progress",
            Category::Started,
            vec![
                DemoCard::new("FLT-12", "Kanban column and card tile in the kit", DESCRIPTION)
                    .priority(PriorityLevel::Urgent)
                    .label("ui-kit", Some("accent"))
                    .label("board", Some("success"))
                    .assignee("Danny Fuentes")
                    .estimate(3)
                    .due("Mar 4")
                    .worktree(true),
                DemoCard::new(
                    "FLT-13",
                    "Markdown read mode for card descriptions",
                    "The parser is pure and unit-tested.\n\n- headings\n- lists\n- fenced code",
                )
                .priority(PriorityLevel::High)
                .assignee("ana.perez")
                .worktree(false)
                .conflict(),
            ],
        ),
        DemoColumn::new(
            "In review",
            Category::Started,
            vec![
                DemoCard::new(
                    "FLT-08",
                    "Multi-line TextInput: soft wrap and visual rows",
                    "`↑` / `↓` move by visual row.\n\n`tab` is the surface's, not the editor's.",
                )
                .priority(PriorityLevel::Low)
                .label("ui-kit", Some("accent"))
                .assignee("dannyfuf")
                .estimate(2)
                .due("2026-03-06"),
            ],
        ),
        DemoColumn::new("Done", Category::Completed, Vec::new()),
    ]
}

/// A bordered stage a live surface is shown at its real height inside.
fn stage(t: &Theme, height: Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .h(height)
        .rounded(t.radii.sm)
        .bg(t.colors.bg)
        .border(t.metrics.hairline)
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

/// A fixed-width panel one static specimen is measured in.
fn panel(width: f32, child: impl IntoElement) -> AnyElement {
    div().w(px(width)).child(child).into_any_element()
}

/// A wrapping row of fixed-width panels.
fn row_of(t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_start()
        .gap(t.space.md)
        .children(children)
        .into_any_element()
}

struct BoardGallery {
    focus_handle: FocusHandle,
    columns: Vec<DemoColumn>,
    column: usize,
    row: usize,
    board_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    page_scroll: ScrollHandle,
    /// The description editor: one live entity, reseeded when the selection moves.
    editor: Entity<TextInput>,
    /// The static specimens of the multi-line box, live so every state is operable.
    specimens: Vec<(&'static str, Entity<TextInput>)>,
    editing: bool,
    read_mode: bool,
}

impl BoardGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let columns = fixtures();
        let editor = cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: EDITOR_ROWS,
                    max_rows: EDITOR_ROWS,
                },
                cx,
            );
            input.set_label(Some("description".into()), cx);
            input.set_placeholder("Describe the card. Markdown is rendered in read mode.", cx);
            input
        });
        let mut gallery = Self {
            focus_handle: cx.focus_handle(),
            columns,
            column: 1,
            row: 0,
            board_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            page_scroll: ScrollHandle::new(),
            editor,
            specimens: specimens(cx),
            editing: false,
            read_mode: true,
        };
        gallery.load_editor(cx);
        gallery
    }

    /// The card under the cursor, if the column has any.
    fn selected(&self) -> Option<&DemoCard> {
        self.columns.get(self.column)?.cards.get(self.row)
    }

    /// Refill the editor from the selected card, which is what selecting a card does in the
    /// real card detail too.
    fn load_editor(&mut self, cx: &mut Context<Self>) {
        let description = self
            .selected()
            .map(|card| card.description.clone())
            .unwrap_or_default();
        self.editor
            .update(cx, |input, cx| input.set_text(description, cx));
    }

    /// Whether any live editor on the page owns the keyboard.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.editor.read(cx).focus_handle().is_focused(window)
            || self
                .specimens
                .iter()
                .any(|(_, input)| input.read(cx).focus_handle().is_focused(window))
    }

    /// Keep the cursor inside the column it just landed in.
    fn clamp_row(&mut self) {
        let len = self.columns[self.column].cards.len();
        self.row = self.row.min(len.saturating_sub(1));
    }

    fn move_column(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.columns.len().saturating_sub(1);
        self.column = self.column.saturating_add_signed(delta).min(last);
        self.clamp_row();
        self.editing = false;
        self.load_editor(cx);
        cx.notify();
    }

    fn move_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.columns[self.column].cards.len().saturating_sub(1);
        self.row = self.row.saturating_add_signed(delta).min(last);
        self.editing = false;
        self.load_editor(cx);
        cx.notify();
    }

    /// `[` / `]`: move the selected card to the neighbouring column, cursor following it.
    fn move_card(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.columns.len().saturating_sub(1);
        let target = self.column.saturating_add_signed(delta).min(last);
        if target == self.column || self.columns[self.column].cards.is_empty() {
            return;
        }
        let card = Rc::make_mut(&mut self.columns[self.column].cards).remove(self.row);
        // Only the row that left and the row that arrived are re-measured; every other tile
        // keeps the height the list already has for it.
        self.columns[self.column]
            .list
            .splice(self.row..self.row + 1, 0);
        let appended = self.columns[target].cards.len();
        Rc::make_mut(&mut self.columns[target].cards).push(card);
        self.columns[target].list.splice(appended..appended, 1);
        self.column = target;
        self.row = self.columns[target].cards.len() - 1;
        self.clamp_row();
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn next_column(&mut self, _: &NextColumn, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_column(1, cx);
    }

    fn prev_column(&mut self, _: &PrevColumn, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_column(-1, cx);
    }

    fn next_card(&mut self, _: &NextCard, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_row(1, cx);
    }

    fn prev_card(&mut self, _: &PrevCard, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_row(-1, cx);
    }

    fn move_card_right(&mut self, _: &MoveRight, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_card(1, cx);
    }

    fn move_card_left(&mut self, _: &MoveLeft, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_card(-1, cx);
    }

    fn cycle_priority(&mut self, _: &CyclePriority, _window: &mut Window, cx: &mut Context<Self>) {
        let (column, row) = (self.column, self.row);
        if let Some(card) = Rc::make_mut(&mut self.columns[column].cards).get_mut(row) {
            let next = PriorityLevel::ALL
                .iter()
                .position(|level| *level == card.priority)
                .map_or(0, |index| (index + 1) % PriorityLevel::ALL.len());
            card.priority = PriorityLevel::ALL[next];
            cx.notify();
        }
    }

    fn edit_description(
        &mut self,
        _: &EditDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected().is_some() {
            self.read_mode = false;
            self.editing = true;
            self.editor.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        }
    }

    fn toggle_read_mode(
        &mut self,
        _: &ToggleReadMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.read_mode = !self.read_mode;
        self.stop_editing(window, cx);
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_editing(window, cx);
    }

    /// Leave the editor and hand the keyboard back, so a surface that stops drawing the editor
    /// never leaves focus on an element nobody paints.
    fn stop_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
}

impl Focusable for BoardGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The live board: one column per fixture, one tile per card, the cursor really moving.
fn live_board(gallery: &BoardGallery, t: &Theme, cx: &mut Context<BoardGallery>) -> AnyElement {
    let column_index = gallery.column;
    let row_index = gallery.row;

    let weak = cx.weak_entity();
    let columns: Vec<AnyElement> = gallery
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let focused = index == column_index;
            // The lazy row API, which is the one a real board has to use: the closure runs only
            // for the tiles the column's viewport can show.
            let cards = Rc::clone(&column.cards);
            let weak = weak.clone();
            KanbanColumn::new(
                SharedString::from(format!("column-{index}")),
                column.title.clone(),
            )
            .count(column.cards.len())
            // One automated column, so the ⚡ can be read against its plain neighbours: a card
            // moved into "In progress" starts a run.
            .action(index == 1)
            .accent(Some(column.category.accent(t)))
            .focused(focused)
            .empty_hint("No cards here.")
            .rows(
                column.list.clone(),
                cards.len(),
                move |row, _window, _cx| {
                    let Some(card) = cards.get(row) else {
                        return div().into_any_element();
                    };
                    let selected = focused && row == row_index;
                    let weak = weak.clone();
                    CardTile::new(
                        SharedString::from(format!("card-{}", card.key)),
                        card.key.clone(),
                        card.title.clone(),
                    )
                    .priority(card.priority)
                    .labels(card.labels.clone())
                    .assignee(card.assignee.clone())
                    .estimate(card.estimate)
                    .due(card.due.clone())
                    .worktree(card.worktree)
                    .dirty(card.dirty)
                    .conflict(card.conflict)
                    .selected(selected)
                    .focused(selected)
                    .on_click(move |_event: &MouseDownEvent, window, cx| {
                        weak.update(cx, |this, cx| {
                            this.column = index;
                            this.row = row;
                            this.stop_editing(window, cx);
                            this.load_editor(cx);
                        })
                        .ok();
                    })
                    .into_any_element()
                },
            )
            .into_any_element()
        })
        .collect();

    KanbanBoard::new("board")
        .scroll_handle(gallery.board_scroll.clone())
        .columns(columns)
        .into_any_element()
}

/// The detail panel: the card's properties, then its description in read or edit mode.
fn live_detail(gallery: &BoardGallery, t: &Theme) -> AnyElement {
    let Some(card) = gallery.selected() else {
        return div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.metrics.detail_w)
            .h_full()
            .border_l(t.metrics.hairline)
            .border_color(t.colors.border)
            .child(EmptyState::new("No card selected.").action("h l  pick a column"))
            .into_any_element();
    };

    let reading = gallery.read_mode && !gallery.editing;
    let description = if reading {
        div()
            .w_full()
            .child(MarkdownText::new(card.description.clone()))
            .into_any_element()
    } else {
        gallery.editor.clone().into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(t.metrics.detail_w)
        .h_full()
        .gap(t.space.sm)
        .p(t.space.md)
        .border_l(t.metrics.hairline)
        .border_color(t.colors.border)
        .child(Text::data_small(card.key.clone()).faint())
        .child(Text::title(card.title.clone()))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(t.space.md)
                .child(PriorityGlyph::new(card.priority).with_label(true))
                .children(
                    card.assignee
                        .clone()
                        .map(|assignee| Chip::new().text(assignee).filled(true)),
                ),
        )
        .child(SectionHeader::new(if reading {
            "description · read"
        } else {
            "description · edit"
        }))
        .child(
            div()
                .id("detail-description")
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .track_scroll(&gallery.detail_scroll)
                .child(description),
        )
        .child(
            KeyHintRow::new()
                .key("i", "edit")
                .key("m", "read / edit")
                .key("esc", "leave"),
        )
        .into_any_element()
}

fn live_board_section(
    gallery: &BoardGallery,
    t: &Theme,
    cx: &mut Context<BoardGallery>,
) -> AnyElement {
    let board = live_board(gallery, t, cx);
    let detail = live_detail(gallery, t);
    let children = vec![
        LAYOUT.labeled(
            "board · detail",
            t,
            stage(
                t,
                px(BOARD_H),
                div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(board))
                    .child(detail),
            ),
        ),
    ];
    LAYOUT.section("live board", t, children)
}

/// The word a [`RunMark`] is called, here and in a harness dump.
fn mark_word(mark: RunMark) -> &'static str {
    match mark {
        RunMark::Pending => "pending",
        RunMark::Stalled => "stalled",
        RunMark::Working => "working",
        RunMark::NeedsYou => "needs you",
        RunMark::Succeeded => "done",
    }
}

fn card_tile_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();

    let bare = panel(
        TILE_W,
        CardTile::new(
            "tile-bare",
            "FLT-40",
            "A bare card: key and title, no meta row at all",
        ),
    );
    let full = panel(
        TILE_W,
        CardTile::new(
            "tile-full",
            "FLT-41",
            "Every meta slot at once, with a title long enough to clip at two lines",
        )
        .priority(PriorityLevel::High)
        .labels(vec![
            ("bug".into(), Some("danger".into())),
            ("infra".into(), Some("warning".into())),
        ])
        .assignee(Some("Danny Fuentes".into()))
        .estimate(Some(8))
        .due(Some("2026-03-04".into()))
        .worktree(true)
        .dirty(true)
        .conflict(true)
        .extras(vec!["QA: pending".into()]),
    );
    let selected = panel(
        TILE_W,
        CardTile::new("tile-selected", "FLT-42", "Selected, keyboard elsewhere")
            .priority(PriorityLevel::Low)
            .selected(true),
    );
    let focused = panel(
        TILE_W,
        CardTile::new("tile-focused", "FLT-43", "Selected and focused")
            .priority(PriorityLevel::Urgent)
            .selected(true)
            .focused(true),
    );
    let unknown_token = panel(
        TILE_W,
        CardTile::new(
            "tile-token",
            "FLT-44",
            "An unknown color token falls back to neutral",
        )
        .labels(vec![
            ("from-jira".into(), Some("chartreuse".into())),
            ("no-token".into(), None),
        ])
        .assignee(Some("ñandú ávila".into())),
    );

    let runs: Vec<AnyElement> = RunMark::ALL
        .into_iter()
        .enumerate()
        .map(|(index, mark)| {
            let word = mark_word(mark);
            panel(
                TILE_W,
                CardTile::new(
                    SharedString::from(format!("tile-run-{index}")),
                    SharedString::from(format!("FLT-5{index}")),
                    SharedString::from(format!(
                        "A run mark at the right end of the key line: {word}"
                    )),
                )
                .run(mark),
            )
        })
        .collect();

    let blocked: Vec<AnyElement> = BlockedTone::ALL
        .into_iter()
        .enumerate()
        .map(|(index, tone)| {
            let word = match tone {
                BlockedTone::Muted => "still moving",
                BlockedTone::Warning => "needs a person",
            };
            panel(
                TILE_W,
                CardTile::new(
                    SharedString::from(format!("tile-blocked-{index}")),
                    SharedString::from(format!("FLT-6{index}")),
                    SharedString::from(format!("Waiting on two cards: {word}")),
                )
                .blocked(2, tone)
                .priority(PriorityLevel::High)
                .assignee(Some("Danny Fuentes".into())),
            )
        })
        .collect();

    let run_wins = panel(
        TILE_W,
        CardTile::new(
            "tile-run-wins",
            "FLT-62",
            "Blocked and running at once: the run mark wins",
        )
        .blocked(2, BlockedTone::Warning)
        .run(RunMark::Working),
    );

    let children = vec![
        LAYOUT.labeled("bare · every slot", &t, row_of(&t, vec![bare, full])),
        LAYOUT.labeled("run marks", &t, row_of(&t, runs)),
        LAYOUT.labeled("blocked · run wins", &t, {
            let mut tiles = blocked;
            tiles.push(run_wins);
            row_of(&t, tiles)
        }),
        LAYOUT.labeled(
            "selected · focused",
            &t,
            row_of(&t, vec![selected, focused]),
        ),
        LAYOUT.labeled("label tokens", &t, row_of(&t, vec![unknown_token])),
    ];
    LAYOUT.section("card tile", &t, children)
}

fn priority_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();

    let with_label = strip(
        &t,
        PriorityLevel::ALL
            .into_iter()
            .map(|level| {
                PriorityGlyph::new(level)
                    .with_label(true)
                    .into_any_element()
            })
            .collect(),
    );
    let marks_only = strip(
        &t,
        PriorityLevel::ALL
            .into_iter()
            .map(|level| PriorityGlyph::new(level).into_any_element())
            .collect(),
    );

    let children = vec![
        LAYOUT.labeled("with label", &t, with_label),
        LAYOUT.labeled("mark only (on a tile)", &t, marks_only),
    ];
    LAYOUT.section("priority glyph", &t, children)
}

fn markdown_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();

    let framed = |child: MarkdownText| {
        div()
            .w_full()
            .p(t.space.md)
            .rounded(t.radii.sm)
            .bg(t.colors.surface)
            .border(t.metrics.hairline)
            .border_color(t.colors.border)
            .child(child)
            .into_any_element()
    };

    let children = vec![
        LAYOUT.labeled("document", &t, framed(MarkdownText::new(DESCRIPTION))),
        LAYOUT.labeled(
            "muted",
            &t,
            framed(MarkdownText::new(DESCRIPTION).muted(true)),
        ),
        LAYOUT.labeled(
            "unclosed marks stay text",
            &t,
            framed(MarkdownText::new(
                "**bold and `code never close, and #### deep is a paragraph.",
            )),
        ),
    ];
    LAYOUT.section("markdown", &t, children)
}

/// The multi-line box in every state it has, one live entity each.
fn specimens(cx: &mut Context<BoardGallery>) -> Vec<(&'static str, Entity<TextInput>)> {
    let build = |cx: &mut Context<BoardGallery>,
                 label: &'static str,
                 rows: usize,
                 text: &str,
                 placeholder: Option<&'static str>,
                 mono: bool,
                 invalid: Option<&'static str>| {
        cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: rows,
                    max_rows: rows,
                },
                cx,
            );
            input.set_label(Some(label.into()), cx);
            if let Some(placeholder) = placeholder {
                input.set_placeholder(placeholder, cx);
            }
            input.set_mono(mono, cx);
            input.set_invalid(invalid.map(Into::into), cx);
            input.set_text(text, cx);
            input
        })
    };
    vec![
        (
            "filled",
            build(
                cx,
                "description",
                5,
                "Reproduce with `fleet board sync`.\nThe second line wraps as soon as the box is narrower than the sentence it holds.",
                None,
                false,
                None,
            ),
        ),
        (
            "placeholder",
            build(
                cx,
                "comment",
                3,
                "",
                Some("Leave a comment. ctrl-s saves, esc cancels."),
                false,
                None,
            ),
        ),
        (
            "resting",
            build(
                cx,
                "resting",
                3,
                "Unfocused: no caret, resting border.",
                None,
                false,
                None,
            ),
        ),
        (
            "mono \u{b7} invalid",
            build(
                cx,
                "mono \u{b7} invalid",
                3,
                "fleet board move FLT-12 done\nfleet worktree new --card FLT-12",
                None,
                true,
                Some("the second command names no board"),
            ),
        ),
        (
            "capped at 3 rows",
            build(
                cx,
                "capped at 3 rows",
                3,
                "one\ntwo\nthree\nfour\nfive\nsix\nseven",
                None,
                false,
                None,
            ),
        ),
    ]
}

fn text_input_section(gallery: &BoardGallery, cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let panels: Vec<AnyElement> = gallery
        .specimens
        .iter()
        .map(|(_, input)| panel(AREA_W, input.clone()))
        .collect();
    let labels = gallery
        .specimens
        .iter()
        .map(|(label, _)| *label)
        .collect::<Vec<_>>()
        .join(" \u{b7} ");

    let children = vec![LAYOUT.labeled(labels.as_str(), &t, row_of(&t, panels))];
    LAYOUT.section("multi-line text input", &t, children)
}

impl Render for BoardGallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let cards: usize = self.columns.iter().map(|column| column.cards.len()).sum();
        let typing = self.typing(window, cx);

        let sections = vec![
            live_board_section(self, &t, cx),
            card_tile_section(cx),
            priority_section(cx),
            markdown_section(cx),
            text_input_section(self, cx),
        ];

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("board", cards)])
                    .active(0)
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if t.mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if t.mode.is_dark() { "dark" } else { "light" },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("gallery-board-scroll")
                    .key_context(if typing { "BoardTyping" } else { "BoardNormal" })
                    .track_focus(&self.focus_handle)
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::quit))
                    .on_action(cx.listener(Self::next_column))
                    .on_action(cx.listener(Self::prev_column))
                    .on_action(cx.listener(Self::next_card))
                    .on_action(cx.listener(Self::prev_card))
                    .on_action(cx.listener(Self::move_card_right))
                    .on_action(cx.listener(Self::move_card_left))
                    .on_action(cx.listener(Self::cycle_priority))
                    .on_action(cx.listener(Self::edit_description))
                    .on_action(cx.listener(Self::toggle_read_mode))
                    .on_action(cx.listener(Self::escape))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .p(t.space.xl)
                    .flex()
                    .flex_col()
                    .bg(t.colors.bg)
                    .text_color(t.colors.text)
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit › board")
                    .mode(if typing { Mode::Dialog } else { Mode::Normal })
                    .ticker(
                        KeyHintRow::new()
                            .key("h l", "column")
                            .key("j k", "card")
                            .key("[ ]", "move")
                            .key("p", "priority")
                            .key("i m", "describe")
                            .key("ctrl-t", "theme"),
                    ),
            )
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit · board gallery",
        (1240.0, 900.0),
        Quit,
        |cx| {
            cx.bind_keys(support::input::bindings());
            cx.bind_keys([
                // Always available, in both modes.
                KeyBinding::new("ctrl-t", ToggleTheme, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("escape", Escape, None),
                // Bare letters exist only while the editor does not own the keyboard.
                KeyBinding::new("h", PrevColumn, Some("BoardNormal")),
                KeyBinding::new("l", NextColumn, Some("BoardNormal")),
                KeyBinding::new("left", PrevColumn, Some("BoardNormal")),
                KeyBinding::new("right", NextColumn, Some("BoardNormal")),
                KeyBinding::new("j", NextCard, Some("BoardNormal")),
                KeyBinding::new("k", PrevCard, Some("BoardNormal")),
                KeyBinding::new("down", NextCard, Some("BoardNormal")),
                KeyBinding::new("up", PrevCard, Some("BoardNormal")),
                KeyBinding::new("[", MoveLeft, Some("BoardNormal")),
                KeyBinding::new("]", MoveRight, Some("BoardNormal")),
                KeyBinding::new("p", CyclePriority, Some("BoardNormal")),
                KeyBinding::new("i", EditDescription, Some("BoardNormal")),
                KeyBinding::new("m", ToggleReadMode, Some("BoardNormal")),
            ]);
        },
        BoardGallery::new,
    );
}
