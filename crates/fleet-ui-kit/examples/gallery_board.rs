//! The visual and behavioural test bench for the **board** group of `fleet-ui-kit`.
//!
//! `KanbanBoard` · `KanbanColumn` · `CardTile` · `PriorityGlyph` · `MarkdownText` ·
//! `TextArea` / `TextAreaState`.
//!
//! Every component appears in every state it can be in, in both themes, and the interactive
//! ones are *live*: the cursor really moves between columns, `[` / `]` really move the card,
//! and the description editor really edits — multi-line, with `↑` / `↓` keeping their column.
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

use fleet_ui_kit::KitAssets;
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, KeyBinding,
    KeyDownEvent, Menu, MenuItem, MouseDownEvent, ScrollHandle, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, px, size,
};

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

// ---------------------------------------------------------------------------------- fixtures

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

/// One column of the demo board, with the scroll handle its body is tracked by.
struct DemoColumn {
    title: SharedString,
    category: Category,
    cards: Vec<DemoCard>,
    scroll: ScrollHandle,
}

impl DemoColumn {
    fn new(title: &str, category: Category, cards: Vec<DemoCard>) -> Self {
        Self {
            title: title.to_string().into(),
            category,
            cards,
            scroll: ScrollHandle::new(),
        }
    }
}

/// The markdown every card description is a variation of.
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
                    "TextArea: the multi-line sibling of TextField",
                    "`↑` / `↓` keep the preferred column.\n\nTab inserts two spaces.",
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

// ------------------------------------------------------------------------------------- view

struct BoardGallery {
    focus_handle: FocusHandle,
    columns: Vec<DemoColumn>,
    column: usize,
    row: usize,
    board_scroll: ScrollHandle,
    editor: TextAreaState,
    editor_scroll: gpui::ScrollHandle,
    editing: bool,
    read_mode: bool,
}

impl BoardGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let columns = fixtures();
        let mut gallery = Self {
            focus_handle: cx.focus_handle(),
            columns,
            column: 1,
            row: 0,
            board_scroll: ScrollHandle::new(),
            editor: TextAreaState::new(),
            editor_scroll: gpui::ScrollHandle::new(),
            editing: false,
            read_mode: true,
        };
        gallery.load_editor();
        gallery
    }

    /// The card under the cursor, if the column has any.
    fn selected(&self) -> Option<&DemoCard> {
        self.columns.get(self.column)?.cards.get(self.row)
    }

    /// Refill the editor from the selected card, which is what selecting a card does in the
    /// real card detail too.
    fn load_editor(&mut self) {
        let description = self
            .selected()
            .map(|card| card.description.clone())
            .unwrap_or_default();
        self.editor = TextAreaState::from_text(description);
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
        self.load_editor();
        cx.notify();
    }

    fn move_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.columns[self.column].cards.len().saturating_sub(1);
        self.row = self.row.saturating_add_signed(delta).min(last);
        self.editing = false;
        self.load_editor();
        cx.notify();
    }

    /// `[` / `]`: move the selected card to the neighbouring column, cursor following it.
    fn move_card(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.columns.len().saturating_sub(1);
        let target = self.column.saturating_add_signed(delta).min(last);
        if target == self.column || self.columns[self.column].cards.is_empty() {
            return;
        }
        let card = self.columns[self.column].cards.remove(self.row);
        self.columns[target].cards.push(card);
        self.column = target;
        self.row = self.columns[target].cards.len() - 1;
        self.clamp_row();
        cx.notify();
    }

    // ------------------------------------------------------------------ actions

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
        if let Some(card) = self.columns[column].cards.get_mut(row) {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected().is_some() {
            self.read_mode = false;
            self.editing = true;
            cx.notify();
        }
    }

    fn toggle_read_mode(
        &mut self,
        _: &ToggleReadMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.read_mode = !self.read_mode;
        self.editing = false;
        cx.notify();
    }

    fn escape(&mut self, _: &Escape, _window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        cx.notify();
    }

    /// Every key the editor owns while it is capturing, and nothing else.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        if self.editor.handle_keystroke(&event.keystroke) {
            self.editor.reveal_cursor(EDITOR_ROWS as usize);
            cx.stop_propagation();
            cx.notify();
        }
    }
}

impl Focusable for BoardGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// How many rows the description editor shows.
const EDITOR_ROWS: u32 = 8;

/// The width of the detail panel, which is the design system's detail width.
fn detail_width(theme: &Theme) -> gpui::Pixels {
    theme.metrics.detail_w
}

impl BoardGallery {
    /// The live board: one column per fixture, one tile per card.
    fn board(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let column_index = self.column;
        let row_index = self.row;

        let columns: Vec<AnyElement> = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let focused = index == column_index;
                let tiles: Vec<AnyElement> = column
                    .cards
                    .iter()
                    .enumerate()
                    .map(|(row, card)| {
                        let selected = focused && row == row_index;
                        let on_click =
                            cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                                this.column = index;
                                this.row = row;
                                this.editing = false;
                                this.load_editor();
                                cx.notify();
                            });
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
                        .on_click(on_click)
                        .into_any_element()
                    })
                    .collect();

                KanbanColumn::new(
                    SharedString::from(format!("column-{index}")),
                    column.title.clone(),
                )
                .count(column.cards.len())
                .accent(Some(column.category.accent(&theme)))
                .focused(focused)
                .empty_hint("No cards here.")
                .scroll_handle(column.scroll.clone())
                .children(tiles)
                .into_any_element()
            })
            .collect();

        KanbanBoard::new("board")
            .scroll_handle(self.board_scroll.clone())
            .columns(columns)
            .into_any_element()
    }

    /// The detail panel: the card's properties, then its description in read or edit mode.
    fn detail(&self, theme: &Theme) -> AnyElement {
        let Some(card) = self.selected() else {
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .w(detail_width(theme))
                .h_full()
                .border_l(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .child(EmptyState::new("No card selected.").action("h l  pick a column"))
                .into_any_element();
        };

        let description = if self.read_mode && !self.editing {
            div()
                .w_full()
                .child(MarkdownText::new(card.description.clone()))
                .into_any_element()
        } else {
            TextArea::new(self.editor.shared_text())
                .label("description")
                .placeholder("Describe the card. Markdown is rendered in read mode.")
                .cursor(self.editor.cursor())
                .focused(self.editing)
                .rows(EDITOR_ROWS)
                .max_rows(EDITOR_ROWS)
                .scroll_row(self.editor.scroll_row())
                .scroll("gallery-board-editor-scroll", self.editor_scroll.clone())
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(detail_width(theme))
            .h_full()
            .gap(theme.space.sm)
            .p(theme.space.md)
            .border_l(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(Text::data_small(card.key.clone()).faint())
            .child(Text::title(card.title.clone()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.space.md)
                    .child(PriorityGlyph::new(card.priority).with_label(true))
                    .children(
                        card.assignee
                            .clone()
                            .map(|assignee| Chip::new().text(assignee).filled(true)),
                    ),
            )
            .child(SectionHeader::new(if self.read_mode && !self.editing {
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

    /// Every priority mark at once, so a reviewer can compare the ladder in one glance.
    fn priorities(&self, theme: &Theme) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(theme.space.lg)
            .h(theme.metrics.strip_h)
            .px(theme.space.md)
            .children(
                PriorityLevel::ALL
                    .into_iter()
                    .map(|level| PriorityGlyph::new(level).with_label(true)),
            )
            .into_any_element()
    }
}

impl Render for BoardGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cards: usize = self.columns.iter().map(|column| column.cards.len()).sum();
        let board = self.board(cx);
        let detail = self.detail(&theme);
        let priorities = self.priorities(&theme);
        let editing = self.editing;

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("board", cards)])
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if theme.mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if theme.mode.is_dark() {
                            "dark"
                        } else {
                            "light"
                        },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("gallery-board")
                    .key_context(if editing {
                        "BoardTyping"
                    } else {
                        "BoardNormal"
                    })
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
                    .on_key_down(cx.listener(Self::on_key_down))
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .bg(theme.colors.bg)
                    .text_color(theme.colors.text)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(div().flex_1().min_w_0().h_full().child(board))
                            .child(detail),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .w_full()
                            .border_t(theme.metrics.hairline)
                            .border_color(theme.colors.border)
                            .child(priorities),
                    ),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit · board components, every state")
                    .mode(if editing { Mode::Dialog } else { Mode::Normal })
                    .ticker(
                        KeyHintRow::new()
                            .key("h l", "column")
                            .key("j k", "card")
                            .key("[ ]", "move")
                            .key("p", "priority")
                            .key("ctrl-t", "theme"),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(|cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
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
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.set_menus(vec![Menu {
                name: "fleet-ui-kit".into(),
                items: vec![MenuItem::action("Quit", Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1240.0), px(860.0)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("fleet-ui-kit · board gallery".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_window, cx| {
                    let view: Entity<BoardGallery> = cx.new(BoardGallery::new);
                    view
                },
            );

            if let Ok(window) = window {
                window
                    .update(cx, |view, window, cx| {
                        window.focus(&view.focus_handle(cx), cx);
                    })
                    .ok();
            }

            cx.activate(true);
        });
}
