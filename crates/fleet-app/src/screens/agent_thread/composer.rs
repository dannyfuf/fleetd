//! The composer's modes, its submit rule, and the three tiers of control state (§7, `spec-B`
//! §B5/§B6).
//!
//! One enum expresses what the composer is for right now, and every capability query switches on
//! it — the alternative is the fifteen scattered booleans `spec-B` §B5.2 names. Nothing here
//! touches gpui, so the capability table and the submit truth table are unit-tested directly.

use std::collections::HashMap;

use fleet_core::agents::{GateId, ItemId, ModelSelection, PermissionMode};
use gpui::SharedString;

/// What the composer is for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerMode {
    /// Writing the next turn.
    Normal,
    /// A permission is open: the editor is disabled and `y`/`a`/`n`/`esc` own the keys.
    Approval(GateId),
    /// A question is open: the composer **is** the free-text answer field.
    Question(GateId),
    /// A plan is ready: `[y] implement` on an empty composer, `[n] refine` on a full one.
    PlanFollowUp(ItemId),
}

impl ComposerMode {
    /// Whether the editor accepts keystrokes.
    ///
    /// A question that forbids custom answers disables it too; the caller passes that in
    /// because only it knows the active question.
    pub(crate) const fn editor_enabled(self, choice_only: bool) -> bool {
        match self {
            ComposerMode::Approval(_) => false,
            ComposerMode::Question(_) => !choice_only,
            ComposerMode::Normal | ComposerMode::PlanFollowUp(_) => true,
        }
    }

    /// Whether the 22 px metadata strip is mounted.
    ///
    /// It is **unmounted** under an approval rather than dimmed: the drawer is showing an
    /// invocation to read, and the strip would compete with it for the same glance.
    pub(crate) const fn metadata_shown(self) -> bool {
        !matches!(self, ComposerMode::Approval(_))
    }

    /// Whether `@`, `$` and `/` report a trigger.
    pub(crate) const fn triggers_enabled(self) -> bool {
        !matches!(self, ComposerMode::Approval(_))
    }

    /// Whether `↑` recalls a prompt.
    ///
    /// Not under an approval and not under a question: recalling a past prompt into an answer
    /// field would answer the harness with an unrelated message.
    pub(crate) const fn history_enabled(self) -> bool {
        matches!(self, ComposerMode::Normal | ComposerMode::PlanFollowUp(_))
    }

    /// Whether attachments may be staged.
    ///
    /// Not under an approval: the approval sends no attachment, and a staged image would be
    /// silently dropped. Under a question only where the question accepts a custom answer.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the §B5.2 capability table is the         specification and is pinned by its own test; attachments themselves are not built yet,         and removing the row would let the table drift from the spec it encodes"
        )
    )]
    pub(crate) const fn attachments_enabled(self, allows_custom: bool) -> bool {
        match self {
            ComposerMode::Approval(_) => false,
            ComposerMode::Question(_) => allows_custom,
            ComposerMode::Normal | ComposerMode::PlanFollowUp(_) => true,
        }
    }

    /// Whether the composer's text binds to the thread draft rather than to an answer.
    pub(crate) const fn binds_draft(self) -> bool {
        matches!(self, ComposerMode::Normal | ComposerMode::PlanFollowUp(_))
    }

    /// The gate this mode is answering, when it is answering one.
    pub(crate) const fn gate(self) -> Option<GateId> {
        match self {
            ComposerMode::Approval(gate) | ComposerMode::Question(gate) => Some(gate),
            ComposerMode::Normal | ComposerMode::PlanFollowUp(_) => None,
        }
    }
}

/// What `⏎` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Submit {
    /// Send and stay on the thread.
    Foreground,
    /// Start an unstarted thread in the background, keeping the composer.
    Background,
}

/// The one function that decides `⏎`, and it takes no state beyond its arguments.
///
/// **Shift is the only newline modifier.** `⇧⏎` is not a keymap binding at all — the composer
/// owns it the way it owns `/`, `@`, `$` and `esc`.
#[must_use]
pub(crate) const fn submit_intent(shift: bool, cmd: bool, is_new_thread: bool) -> Option<Submit> {
    if shift {
        return None;
    }
    Some(if cmd && is_new_thread {
        Submit::Background
    } else {
        Submit::Foreground
    })
}

/// Why a send was refused.
///
/// It deliberately does **not** include "a turn is running": a message sent while a turn runs is
/// a steer, dispatched immediately (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Nothing to send.
    Empty,
    /// A dispatch of ours has not been acknowledged yet.
    Unacknowledged,
    /// The thread's machine is out of reach.
    Unreachable,
}

/// Whether this composer may send, and why not when it may not.
pub(crate) fn submit_gate(
    text: &str,
    attachments: usize,
    pending: usize,
    unreachable: bool,
) -> Result<(), Refusal> {
    if unreachable {
        return Err(Refusal::Unreachable);
    }
    if text.trim().is_empty() && attachments == 0 {
        return Err(Refusal::Empty);
    }
    if pending > 0 {
        return Err(Refusal::Unacknowledged);
    }
    Ok(())
}

/// Build ⇄ Plan, which is a **different axis** from the access mode (§7.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InteractionMode {
    /// The harness executes.
    #[default]
    Build,
    /// The harness proposes and waits.
    Plan,
}

/// The composer's own control draft: tier one of three (`spec-B` §B6.4).
///
/// Writing here is synchronous and repaints one row. Nothing reaches the daemon until the next
/// send, which is what keeps a picker instant over an SSH link — and it is why **nothing applies
/// mid-turn**.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ControlDraft {
    /// The model picked for each instance. `instance` is an **open** slug: parsing an unknown
    /// one always succeeds and the runtime marks it unavailable rather than crashing.
    models: HashMap<SharedString, ModelSelection>,
    /// The instance this thread routes to.
    instance: Option<SharedString>,
    /// True only when a human picked the model in the composer. A seeded selection may be
    /// replaced by a later seed; a human's pick never is.
    explicit: bool,
    /// The access ladder, which is **not** plan mode.
    access: Option<PermissionMode>,
    /// Build ⇄ Plan.
    interaction: Option<InteractionMode>,
}

impl ControlDraft {
    /// The instance this thread routes to.
    pub(crate) fn instance(&self) -> Option<&SharedString> {
        self.instance.as_ref()
    }

    /// The model picked for the active instance.
    pub(crate) fn model(&self) -> Option<&ModelSelection> {
        self.models.get(self.instance.as_ref()?)
    }

    /// Records a human's pick, which a later seed may not overwrite.
    pub(crate) fn pick_model(&mut self, instance: SharedString, model: ModelSelection) {
        self.instance = Some(instance.clone());
        self.models.insert(instance, model);
        self.explicit = true;
    }

    /// Seeds a selection from a project default or the sticky per-instance memory.
    ///
    /// Tier two of §B6.4: the last model chosen for each instance survives thread switches and
    /// seeds new threads — but it never replaces a pick a human made here.
    pub(crate) fn seed_model(&mut self, instance: SharedString, model: ModelSelection) {
        if self.explicit {
            return;
        }
        self.instance = Some(instance.clone());
        self.models.insert(instance, model);
    }

    /// Whether a human picked the current model.
    pub(crate) const fn is_explicit(&self) -> bool {
        self.explicit
    }

    /// The access ladder this thread will send next, falling back to the thread's own mode.
    ///
    /// Tier three is the thread projection, whose `mode` is a per-thread column with a non-null
    /// default, so the fallback is state and not a guess.
    pub(crate) fn access(&self, thread_mode: PermissionMode) -> PermissionMode {
        self.access.unwrap_or(match thread_mode {
            // A thread parked in plan mode has no access ladder of its own on the wire, so the
            // base mode is the conservative one until the user picks another.
            PermissionMode::Plan => PermissionMode::Ask,
            mode => mode,
        })
    }

    /// Records the access ladder, which takes effect at the next send.
    pub(crate) fn set_access(&mut self, access: PermissionMode) {
        self.access = Some(access);
    }

    /// Build or Plan, falling back to what the thread is already in.
    pub(crate) fn interaction(&self, thread_mode: PermissionMode) -> InteractionMode {
        self.interaction
            .unwrap_or(if thread_mode == PermissionMode::Plan {
                InteractionMode::Plan
            } else {
                InteractionMode::Build
            })
    }

    /// Records Build or Plan.
    pub(crate) fn set_interaction(&mut self, interaction: InteractionMode) {
        self.interaction = Some(interaction);
    }

    /// The one mode the wire carries, composed from the two axes Fleet keeps apart.
    ///
    /// `PermissionMode` is a four-value ladder with `Plan` inside it, so the *presentation* keeps
    /// access and interaction orthogonal and collapses them exactly here — which is also what
    /// makes leaving plan mode restore the **base** access ladder rather than a hardcoded
    /// default (§6.4).
    pub(crate) fn wire_mode(&self, thread_mode: PermissionMode) -> PermissionMode {
        match self.interaction(thread_mode) {
            InteractionMode::Plan => PermissionMode::Plan,
            InteractionMode::Build => self.access(thread_mode),
        }
    }
}

/// The one restart decision, stated once (`spec-B` §B6.3).
///
/// Logged as one structured line by the caller with every input, because the first time a user
/// reports "it lost my context" it pays for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RestartInputs {
    /// Whether the access ladder changed.
    pub(crate) mode_changed: bool,
    /// Whether the worktree changed.
    pub(crate) cwd_changed: bool,
    /// Whether the routing instance changed.
    pub(crate) instance_changed: bool,
    /// Whether any part of the model selection changed — options included.
    pub(crate) model_changed: bool,
    /// Whether the harness can switch models on a live session.
    pub(crate) can_switch_model: bool,
    /// Whether every control this harness exposes rides on the turn rather than on the process.
    ///
    /// True for Codex, false for Claude, whose effort, fast mode, thinking and context window
    /// are `query()` construction options and cannot change on a live query.
    pub(crate) controls_ride_the_turn: bool,
}

/// Whether the next send has to restart the harness with its resume cursor.
#[must_use]
pub(crate) const fn restart_with_resume(inputs: RestartInputs) -> bool {
    inputs.mode_changed
        || inputs.cwd_changed
        || inputs.instance_changed
        || (inputs.model_changed && !inputs.can_switch_model)
        || (inputs.model_changed && !inputs.controls_ride_the_turn)
}

/// Strips Fleet's own send-time additions, for display and for `↑` recall.
///
/// The bubble shows *what the user typed*. An attachment manifest, an `@file` expansion block
/// and the plan-implementation prefix are all Fleet's, so none of them may come back on `↑` —
/// and a recalled plan implementation recalls as empty rather than as a wall of markdown.
#[must_use]
pub(crate) fn strip_send_time_context(text: &str) -> String {
    // A recalled plan implementation recalls as *empty*: the prefix and the plan under it are
    // both Fleet's, so there is nothing of the user's left to hand back.
    if text.starts_with(super::decisions::PLAN_IMPLEMENTATION_PROMPT_PREFIX) {
        return String::new();
    }
    // The expansion block is appended after a blank line and opens with a fence, which is the
    // one shape Fleet itself writes; a user's own fence is never the *last* block after one.
    match text.split_once("\n\n```") {
        Some((head, _)) if head.trim().is_empty() => String::new(),
        Some((head, _)) => head.trim_end().to_owned(),
        None => text.to_owned(),
    }
}
