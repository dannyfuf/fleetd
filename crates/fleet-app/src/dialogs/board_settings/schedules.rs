//! Board settings › Schedules (BOARD §12, UX-SPEC Board settings): the board's schedules as a
//! list level and a form level, gated on the daemon's `schedules` capability.
//!
//! The pane is shaped like Columns — a list, `⏎` drills into one row's form, `esc` leaves it —
//! with one difference that follows from where the data lives. A column is part of the board
//! document and waits in the draft for `^s`; a schedule is its own record in the daemon, so
//! every list key (`space`, `r`, `d`) is one request, sent at once, and the form's `^s` is one
//! `CreateSchedule` or `UpdateSchedule`. None of them touches the board draft, which is why a
//! schedule saved here never makes the board "unsaved".
//!
//! The list and the requests that fill it live here; the mirror they fill is
//! [`crate::state::SchedulesMirror`], beside the board state, because the board header's
//! schedules strip reads it too. The draft keeps a prepared copy of the open board's rows,
//! rebuilt whenever the mirror's revision moves, so `render` formats nothing.

use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use fleet_core::{
    ids::ScheduleId,
    schedule::{
        Cadence, SCHEDULE_DEFAULT_TIMEOUT_MINUTES, STARTER_PROMPT_GITHUB_REVIEWS, Schedule,
        ScheduleAgent, ScheduleDraft, ScheduleOutcome, SchedulePatch,
    },
};
use std::cell::Cell;

use super::*;
use crate::state::SchedulesMirror;

mod form;
pub(super) mod requests;
#[cfg(test)]
mod tests;
mod time;
mod view;

pub(super) use form::*;
pub(super) use requests::*;
pub(super) use time::*;
pub(super) use view::schedules_pane;

/// The cadence a new schedule starts with, and the starter's.
const DEFAULT_EVERY_MINUTES: u32 = 15;
/// The GitHub review starter's name.
const STARTER_NAME: &str = "GitHub reviews";
/// How a one-off time is typed and shown, in local time.
const ONCE_FORMAT: &str = "%Y-%m-%d %H:%M";
/// How many runs the form's read-only `Last runs` block lists.
const LAST_RUNS_SHOWN: usize = 5;
/// The client's refusal sentence for a daemon without `schedules` (contracts C6).
pub(crate) const SCHEDULES_UNSUPPORTED: &str =
    "this daemon does not support schedules; run `fleet daemon restart`";

thread_local! {
    /// A Schedules opening waiting for the dialog's seed: `Some(starter)`.
    ///
    /// `T` and the Review board's empty state open the dialog and then ask for this section
    /// in the same key press, before the dialog host has seeded the draft. The draft is rebuilt
    /// from scratch by that seed, so the request waits here and the seed takes it — the same
    /// reason the remembered section lives beside the draft rather than in it.
    static PENDING_OPEN: Cell<Option<bool>> = const { Cell::new(None) };
}

/// One line of the form's `Last runs` block.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RunLine {
    /// The outcome glyph and its tone.
    pub(super) outcome: Outcome,
    /// When it started, local time, then its summary.
    pub(super) text: String,
    /// Where its log is, when it wrote one.
    pub(super) log_path: Option<String>,
}

/// An outcome as the list and the form draw it: a glyph, a tone and a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Outcome {
    /// `✓`, `✗`, `⤼`, or `…` while the run is live.
    pub(super) glyph: &'static str,
    /// The glyph's tone.
    pub(super) tone: Tone,
    /// What the run is called when it wrote no summary.
    pub(super) word: &'static str,
}

impl Outcome {
    /// The glyph, tone and word of a run's outcome; `None` is a run still going.
    #[must_use]
    pub(super) const fn of(outcome: Option<ScheduleOutcome>) -> Self {
        let (glyph, tone, word) = match outcome {
            Some(ScheduleOutcome::Succeeded) => ("\u{2713}", Tone::Success, "succeeded"),
            Some(ScheduleOutcome::Failed) => ("\u{2717}", Tone::Danger, "failed"),
            Some(ScheduleOutcome::TimedOut) => ("\u{2717}", Tone::Danger, "timed out"),
            Some(ScheduleOutcome::Skipped) => ("\u{293c}", Tone::Muted, "skipped"),
            None => ("\u{2026}", Tone::Muted, "running"),
        };
        Self { glyph, tone, word }
    }
}

/// One prepared row of the Schedules list: `● name   every 15m   next 14:05   last ✓ …`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScheduleListRow {
    /// The schedule itself, for the keys that act on the row.
    pub(super) schedule: Schedule,
    /// `●` when enabled, `○` when not.
    pub(super) dot: &'static str,
    /// `every 15m` or `once 2026-09-23 09:00`.
    pub(super) cadence: String,
    /// `next 14:05`, when the daemon has a next run for it.
    pub(super) next: Option<String>,
    /// The last run's outcome and its summary, when it has run.
    pub(super) last: Option<(Outcome, String)>,
}

/// One prepared row of a schedule's form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScheduleFormRow {
    /// Which field this is.
    pub(super) field: ScheduleField,
    /// What it draws.
    pub(super) value: ScheduleValue,
}

/// What one form row draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScheduleValue {
    /// A closed choice: `h` / `l` step it and a click picks an option.
    Choice {
        /// The value shown.
        value: String,
        /// Every option, in cycle order.
        options: Vec<String>,
        /// Where the value sits.
        at: usize,
    },
    /// Free text; `⏎` opens an editor over it.
    Text {
        /// The value, cut to one line.
        value: String,
    },
    /// A flag.
    Flag(bool),
}

/// The Schedules section's own draft: the prepared list, and the form it drilled into.
#[derive(Debug, Clone, Default)]
pub(super) struct SchedulesPane {
    /// The open board's schedules, prepared from the mirror.
    pub(super) list: Rc<Vec<ScheduleListRow>>,
    /// The mirror revision [`Self::list`] was prepared at.
    pub(super) revision: Option<u64>,
    /// Whether the mirror is loading this board's schedules.
    pub(super) loading: bool,
    /// Why the last load failed.
    pub(super) load_error: Option<String>,
    /// The form `⏎` or `n` opened, if any.
    pub(super) form: Option<ScheduleForm>,
    /// The form's rows, prepared whenever it changes.
    pub(super) prepared: Vec<ScheduleFormRow>,
    /// The schedule `d` was pressed on once; a second `d` deletes it.
    pub(super) pending_delete: Option<ScheduleId>,
    /// A schedule request is in flight; further keys wait for its answer.
    pub(super) busy: bool,
    /// Whether `esc` has already asked about the unsaved form.
    pub(super) discard_armed: bool,
}

impl SchedulesPane {
    /// The rows the section draws: the list, or the open form's fields.
    #[must_use]
    pub(super) fn rows(&self) -> Vec<SettingRow> {
        if self.form.is_some() {
            return self
                .prepared
                .iter()
                .map(|row| SettingRow::ScheduleField(row.field))
                .collect();
        }
        (0..self.list.len()).map(SettingRow::Schedule).collect()
    }

    /// Recomputes the form's rows (`docs/APP-CONTRACTS.md`: render prepares nothing).
    pub(super) fn prepare(&mut self) {
        self.prepared = self.form.as_ref().map_or_else(Vec::new, prepare_form);
    }

    /// Leaves the form and forgets an armed delete, for a section change.
    pub(super) fn leave(&mut self) {
        self.form = None;
        self.prepared.clear();
        self.pending_delete = None;
        self.discard_armed = false;
    }

    /// The schedule on list row `index`.
    #[must_use]
    pub(super) fn schedule(&self, index: usize) -> Option<&Schedule> {
        self.list.get(index).map(|row| &row.schedule)
    }
}

impl BoardSettingsState {
    /// The rail's sections: Schedules only when the daemon advertises `schedules`.
    #[must_use]
    pub(super) fn sections(&self) -> &'static [BoardSection] {
        if self.schedules_supported {
            BoardSection::ALL
        } else {
            BoardSection::WITHOUT_SCHEDULES
        }
    }

    /// Whether the Schedules pane is showing its list rather than a form.
    #[must_use]
    pub(super) fn in_schedule_list(&self) -> bool {
        self.section == BoardSection::Schedules && self.schedules.form.is_none()
    }

    /// Reads the capability and the open board's schedules out of the app, at seed time.
    pub(super) fn seed_schedules(&mut self, app: &AppState) {
        self.schedules_supported = app.board_takes_schedules();
        self.schedules_refusal = app.board_schedules_refusal();
        if self.section == BoardSection::Schedules && !self.schedules_supported {
            self.section = BoardSection::General;
        }
        self.sync_schedules(&app.schedules, Local::now());
        if let Some(starter) = PENDING_OPEN.take() {
            self.open_section(starter, Local::now());
        }
    }

    /// Rebuilds the prepared list when the mirror moved since it was last prepared.
    ///
    /// Returns whether anything was rebuilt.
    pub(super) fn sync_schedules(
        &mut self,
        mirror: &SchedulesMirror,
        now: DateTime<Local>,
    ) -> bool {
        let revision = mirror.revision;
        if self.schedules.revision == Some(revision) {
            return false;
        }
        self.schedules.revision = Some(revision);
        let Some(board) = self.board_id.as_ref() else {
            return true;
        };
        let entry = mirror.entry(board);
        self.schedules.loading = entry.is_some_and(|entry| entry.loading);
        self.schedules.load_error = entry.and_then(|entry| entry.error.clone());
        self.schedules.list = Rc::new(prepare_list(mirror.for_board(board), now));
        // An open form's read-only `Last runs` follows the mirror too — a run it started, a
        // scheduled fire or a run that finished — while its fields and baseline stay as typed.
        if let Some(form) = self.schedules.form.as_mut()
            && let Some(id) = form.id.as_ref()
            && let Some(schedule) = mirror
                .for_board(board)
                .iter()
                .find(|schedule| &schedule.id == id)
        {
            let runs = run_lines(schedule, now);
            if form.runs != runs {
                form.runs = runs;
                self.schedules.prepare();
            }
        }
        if self.in_schedule_list() {
            self.row = self.row.min(self.schedules.list.len().saturating_sub(1));
        }
        true
    }

    /// Moves to the Schedules section; `starter` also opens the new-schedule form.
    fn open_section(&mut self, starter: bool, now: DateTime<Local>) {
        if !self.schedules_supported {
            self.notice = Some(
                self.schedules_refusal
                    .clone()
                    .unwrap_or_else(|| SCHEDULES_UNSUPPORTED.to_owned()),
            );
            return;
        }
        self.section = BoardSection::Schedules;
        self.opened_column = None;
        self.pending_delete = None;
        self.editing = false;
        self.schedules.leave();
        self.row = 0;
        if starter {
            self.open_new_schedule(now);
        }
    }

    /// Whether the board being configured is a Reviews board, which is what the starter
    /// pre-fills for.
    #[must_use]
    fn is_reviews_board(&self) -> bool {
        self.board
            .as_ref()
            .is_some_and(|board| !board.kind.is_tasks())
    }

    /// `n`: the new-schedule form, pre-filled with the GitHub review starter on a Reviews
    /// board and with an empty prompt on any other.
    pub(super) fn open_new_schedule(&mut self, now: DateTime<Local>) {
        let fields = ScheduleFields::new(self.is_reviews_board(), now);
        self.open_form(ScheduleForm {
            id: None,
            baseline: fields.clone(),
            fields,
            runs: Vec::new(),
        });
    }

    /// `⏎` on a list row: that schedule's form.
    fn open_schedule(&mut self, schedule: &Schedule, now: DateTime<Local>) {
        let fields = ScheduleFields::load(schedule, now);
        self.open_form(ScheduleForm {
            id: Some(schedule.id.clone()),
            baseline: fields.clone(),
            fields,
            runs: run_lines(schedule, now),
        });
    }

    fn open_form(&mut self, form: ScheduleForm) {
        self.schedules.form = Some(form);
        self.schedules.pending_delete = None;
        self.schedules.discard_armed = false;
        self.schedules.prepare();
        self.row = 0;
        self.editing = false;
        self.notice = None;
        self.error = None;
    }

    /// The Schedules steps of `esc`, from the deepest outwards; `None` when the section has
    /// nothing of its own to leave.
    pub(super) fn escape_schedules(&mut self) -> Option<EscapeStep> {
        if self.section != BoardSection::Schedules {
            return None;
        }
        if self.schedules.pending_delete.take().is_some() {
            self.notice = None;
            return Some(EscapeStep::Delete);
        }
        let form = self.schedules.form.as_ref()?;
        if form.dirty() && !self.schedules.discard_armed {
            self.schedules.discard_armed = true;
            self.error = Some("unsaved schedule \u{2014} esc again to discard it".to_owned());
            return Some(EscapeStep::Ask);
        }
        let id = form.id.clone();
        self.schedules.leave();
        self.error = None;
        self.row = id
            .and_then(|id| {
                self.schedules
                    .list
                    .iter()
                    .position(|row| row.schedule.id == id)
            })
            .unwrap_or(0);
        Some(EscapeStep::Schedule)
    }

    /// Whether an unsaved schedule form refuses a section change (`⇥`, `⇧⇥`, a rail click).
    ///
    /// The form lives only while the section is shown, so leaving would throw it away. The
    /// first attempt asks, as `esc` does, and arms the discard; the next one (or `esc`) leaves.
    pub(super) fn holds_unsaved_schedule(&mut self) -> bool {
        // A save in flight is not unsaved: its answer lands whichever section is shown.
        if self.section != BoardSection::Schedules
            || self.schedules.discard_armed
            || self.schedules.busy
        {
            return false;
        }
        if !self
            .schedules
            .form
            .as_ref()
            .is_some_and(ScheduleForm::dirty)
        {
            return false;
        }
        self.schedules.discard_armed = true;
        self.error = Some("unsaved schedule \u{2014} esc again to discard it".to_owned());
        true
    }

    /// The seed text of a form row's editor, once `⏎` has opened it.
    #[must_use]
    pub(super) fn schedule_text(&self, field: ScheduleField) -> Option<String> {
        if !self.editing || !field.is_text() {
            return None;
        }
        self.schedules.form.as_ref()?.fields.text(field)
    }

    /// Mirrors a form editor's text back into the form.
    pub(super) fn set_schedule_text(&mut self, field: ScheduleField, text: &str) {
        let Some(form) = self.schedules.form.as_mut() else {
            return;
        };
        form.fields.set_text(field, text);
        self.schedules.discard_armed = false;
        self.schedules.prepare();
    }

    /// `h` / `l` on a form row. Returns whether anything changed.
    pub(super) fn cycle_schedule(&mut self, field: ScheduleField, delta: isize) -> bool {
        if self.schedules.busy {
            return false;
        }
        let Some(form) = self.schedules.form.as_mut() else {
            return false;
        };
        if !form.fields.cycle(field, delta) {
            return false;
        }
        self.schedules.discard_armed = false;
        self.schedules.prepare();
        // The cadence cycler swaps the row under it, so the cursor is kept on the cycler.
        self.row = self
            .row
            .min(self.schedules.prepared.len().saturating_sub(1));
        true
    }

    /// `space` on the form's `Enabled` row.
    pub(super) fn toggle_schedule_flag(&mut self) -> bool {
        let Some(form) = self.schedules.form.as_mut() else {
            return false;
        };
        if self.schedules.busy {
            return false;
        }
        form.fields.enabled = !form.fields.enabled;
        self.schedules.discard_armed = false;
        self.schedules.prepare();
        true
    }
}

/// Prepares the list rows for `schedules`, formatting every time in local time.
#[must_use]
pub(super) fn prepare_list(schedules: &[Schedule], now: DateTime<Local>) -> Vec<ScheduleListRow> {
    schedules
        .iter()
        .map(|schedule| ScheduleListRow {
            dot: if schedule.enabled {
                "\u{25cf}"
            } else {
                "\u{25cb}"
            },
            cadence: cadence_text(&schedule.cadence, now),
            next: schedule
                .next_run_at
                .as_deref()
                .and_then(|at| local_time(at, now))
                .map(|at| format!("next {at}")),
            last: schedule.runs.last().map(|run| {
                let outcome = Outcome::of(run.outcome);
                // Capped as the header strip's clause is: a run with no `SUMMARY:` line
                // carries its last output line, and the row keeps its name column readable.
                let text = run
                    .summary
                    .as_deref()
                    .filter(|summary| !summary.trim().is_empty())
                    .map_or_else(
                        || outcome.word.to_owned(),
                        |summary| {
                            fleet_ui_kit::truncate(
                                summary.trim(),
                                crate::views::board_screen::SCHEDULE_SUMMARY_BUDGET,
                                fleet_ui_kit::Truncate::Tail,
                            )
                            .to_string()
                        },
                    );
                (outcome, text)
            }),
            schedule: schedule.clone(),
        })
        .collect()
}

/// What a run-now answer says when the daemon recorded the fire as `Skipped` because the
/// schedule's previous run was still going (BOARD §12): `skipped {name}: {summary}`. `None`
/// when the answer's newest run is the one it started.
#[must_use]
pub(crate) fn skipped_run_notice(schedule: &Schedule) -> Option<String> {
    let run = schedule.runs.last()?;
    if run.outcome != Some(ScheduleOutcome::Skipped) {
        return None;
    }
    let name = &schedule.name;
    Some(match run.summary.as_deref().map(str::trim) {
        Some(summary) if !summary.is_empty() => format!("skipped {name}: {summary}"),
        _ => format!("skipped {name}"),
    })
}

/// The `Last runs` block of a schedule: its last five runs, newest first.
#[must_use]
fn run_lines(schedule: &Schedule, now: DateTime<Local>) -> Vec<RunLine> {
    schedule
        .runs
        .iter()
        .rev()
        .take(LAST_RUNS_SHOWN)
        .map(|run| {
            let outcome = Outcome::of(run.outcome);
            let started =
                local_time(&run.started_at, now).unwrap_or_else(|| run.started_at.clone());
            let summary = run
                .summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| outcome.word.to_owned());
            RunLine {
                outcome,
                text: format!("{started}  {summary}"),
                log_path: run.log_path.clone(),
            }
        })
        .collect()
}
