//! The composer's rules: the submit truth table, the mode capability table, the control tiers,
//! the restart decision, and the send-time strip.

use fleet_core::agents::{
    AgentKind, GateId, ItemId, ModelSelection, PermissionMode, ThreadProjection, TurnOutcome,
};
use fleet_ui_kit::MetadataSegment;

use super::fixtures::{assistant, projection, settled_turn, user};
use crate::screens::agent_thread::{
    composer::{
        ComposerMode, ControlDraft, InteractionMode, Refusal, RestartInputs, Submit,
        restart_with_resume, strip_send_time_context, submit_gate, submit_intent,
    },
    decisions::PLAN_IMPLEMENTATION_PROMPT_PREFIX,
    presentation::{self, TabBadge, key_hint_set, mode_label, tab_badge, tab_title},
};

#[test]
fn the_submit_truth_table() {
    // Shift is the only newline modifier, and it is not a keymap binding at all.
    assert_eq!(submit_intent(true, false, false), None);
    assert_eq!(submit_intent(true, true, true), None);
    assert_eq!(submit_intent(false, false, true), Some(Submit::Foreground));
    assert_eq!(
        submit_intent(false, true, true),
        Some(Submit::Background),
        "⌘⏎ on an unstarted thread starts it in the background"
    );
    assert_eq!(
        submit_intent(false, true, false),
        Some(Submit::Foreground),
        "on a started thread ⌘⏎ is identical to ⏎"
    );
}

#[test]
fn the_submit_gate_never_refuses_a_running_turn() {
    // A message sent while a turn runs is a steer, dispatched immediately: `turn == Running` is
    // deliberately not an input to this decision.
    assert_eq!(submit_gate("steer", 0, 0, false), Ok(()));
    assert_eq!(submit_gate("   ", 0, 0, false), Err(Refusal::Empty));
    assert_eq!(
        submit_gate("", 1, 0, false),
        Ok(()),
        "an attachment is content"
    );
    assert_eq!(
        submit_gate("again", 0, 1, false),
        Err(Refusal::Unacknowledged)
    );
    assert_eq!(
        submit_gate("anything", 0, 0, true),
        Err(Refusal::Unreachable)
    );
}

#[test]
fn the_composer_mode_capability_table() {
    let gate = GateId::new();
    let plan = ItemId::new();
    let modes = [
        ComposerMode::Normal,
        ComposerMode::Approval(gate),
        ComposerMode::Question(gate),
        ComposerMode::PlanFollowUp(plan),
    ];

    // editor enabled: yes / no / yes unless choice-only / yes
    let enabled: Vec<bool> = modes
        .iter()
        .map(|mode| mode.editor_enabled(false))
        .collect();
    assert_eq!(enabled, [true, false, true, true]);
    assert!(
        !ComposerMode::Question(gate).editor_enabled(true),
        "a choice-only question disables the editor rather than inviting a refusal"
    );

    // the metadata strip is unmounted under an approval, not dimmed
    let metadata: Vec<bool> = modes.iter().map(|mode| mode.metadata_shown()).collect();
    assert_eq!(metadata, [true, false, true, true]);

    // triggers, history and attachments
    let triggers: Vec<bool> = modes.iter().map(|mode| mode.triggers_enabled()).collect();
    assert_eq!(triggers, [true, false, true, true]);
    let history: Vec<bool> = modes.iter().map(|mode| mode.history_enabled()).collect();
    assert_eq!(
        history,
        [true, false, false, true],
        "recalling a prompt into an answer field would answer with an unrelated message"
    );
    let attachments: Vec<bool> = modes
        .iter()
        .map(|mode| mode.attachments_enabled(true))
        .collect();
    assert_eq!(attachments, [true, false, true, true]);
    assert!(!ComposerMode::Question(gate).attachments_enabled(false));

    // only the two draft-bound modes bind the thread draft
    let binds: Vec<bool> = modes.iter().map(|mode| mode.binds_draft()).collect();
    assert_eq!(binds, [true, false, false, true]);

    // and the two answering modes name their gate
    assert_eq!(ComposerMode::Normal.gate(), None);
    assert_eq!(ComposerMode::Approval(gate).gate(), Some(gate));
    assert_eq!(ComposerMode::Question(gate).gate(), Some(gate));
    assert_eq!(ComposerMode::PlanFollowUp(plan).gate(), None);
}

#[test]
fn a_human_pick_is_never_overwritten_by_a_seed() {
    let mut draft = ControlDraft::default();
    let instance = gpui::SharedString::new_static("claude");
    let seeded = ModelSelection {
        model: "claude-sonnet-5".to_owned(),
        effort: None,
        provider: None,
    };
    let picked = ModelSelection {
        model: "claude-opus-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    };

    draft.seed_model(instance.clone(), seeded.clone());
    assert!(!draft.is_explicit());
    assert_eq!(draft.model(), Some(&seeded));
    assert_eq!(draft.instance(), Some(&instance));

    draft.pick_model(instance.clone(), picked.clone());
    assert!(draft.is_explicit());
    draft.seed_model(instance, seeded);
    assert_eq!(
        draft.model(),
        Some(&picked),
        "a later seed may replace a seed, never a human's pick"
    );
}

#[test]
fn the_two_axes_collapse_onto_the_one_field_the_wire_carries() {
    let mut draft = ControlDraft::default();

    // A thread parked in plan mode has no access ladder of its own on the wire, so the base
    // mode is the conservative one until the user picks another.
    assert_eq!(draft.access(PermissionMode::Plan), PermissionMode::Ask);
    assert_eq!(
        draft.interaction(PermissionMode::Plan),
        InteractionMode::Plan
    );
    assert_eq!(draft.wire_mode(PermissionMode::Plan), PermissionMode::Plan);

    // Leaving plan mode restores the **base** access ladder rather than a hardcoded default.
    draft.set_access(PermissionMode::FullAccess);
    draft.set_interaction(InteractionMode::Plan);
    assert_eq!(draft.wire_mode(PermissionMode::Ask), PermissionMode::Plan);
    draft.set_interaction(InteractionMode::Build);
    assert_eq!(
        draft.wire_mode(PermissionMode::Ask),
        PermissionMode::FullAccess
    );
}

#[test]
fn the_restart_rule_stated_once() {
    let none = RestartInputs {
        mode_changed: false,
        cwd_changed: false,
        instance_changed: false,
        model_changed: false,
        can_switch_model: true,
        controls_ride_the_turn: true,
    };
    assert!(!restart_with_resume(none));

    for change in [
        RestartInputs {
            mode_changed: true,
            ..none
        },
        RestartInputs {
            cwd_changed: true,
            ..none
        },
        RestartInputs {
            instance_changed: true,
            ..none
        },
    ] {
        assert!(restart_with_resume(change), "{change:?}");
    }

    // Codex's controls ride the turn, so a model change alone needs no restart.
    assert!(!restart_with_resume(RestartInputs {
        model_changed: true,
        ..none
    }));
    // Claude's effort, fast mode, thinking and context window are `query()` construction
    // options, so any change to the whole selection restarts with the resume cursor.
    assert!(restart_with_resume(RestartInputs {
        model_changed: true,
        controls_ride_the_turn: false,
        ..none
    }));
    assert!(restart_with_resume(RestartInputs {
        model_changed: true,
        can_switch_model: false,
        ..none
    }));
}

#[test]
fn the_bubble_shows_what_the_user_typed() {
    // A plan implementation recalls as empty rather than as a wall of markdown.
    assert_eq!(
        strip_send_time_context(&format!(
            "{PLAN_IMPLEMENTATION_PROMPT_PREFIX}# Plan\n\nsteps"
        )),
        ""
    );
    // Fleet's own appended expansion block is stripped; the user's prose is kept verbatim.
    assert_eq!(
        strip_send_time_context("look at this\n\n```\nsrc/lib.rs:1-4\n```"),
        "look at this"
    );
    assert_eq!(strip_send_time_context("plain prompt"), "plain prompt");
    assert_eq!(
        strip_send_time_context("a fence\n```rust\nfn main() {}\n```"),
        "a fence\n```rust\nfn main() {}\n```",
        "a user's own fence on the next line is not Fleet's block"
    );
}

#[test]
fn the_metadata_row_invents_no_segment() {
    let mut projection = projection();
    // A fresh tab that has published no model at all has the mode and the interaction axis and
    // nothing else — no placeholder where a number would be.
    let segments = presentation::metadata_segments(&projection, InteractionMode::Build);
    let texts: Vec<String> = segments
        .iter()
        .map(|segment| segment.text.to_string())
        .collect();
    assert_eq!(
        texts,
        [
            mode_label(PermissionMode::Ask).to_owned(),
            "build".to_owned()
        ]
    );
    assert!(presentation::trailing_segments(&projection).is_empty());

    projection.model = Some(ModelSelection {
        model: "claude-opus-5".to_owned(),
        effort: None,
        provider: None,
    });
    let segments = presentation::metadata_segments(&projection, InteractionMode::Plan);
    let texts: Vec<String> = segments
        .iter()
        .map(|segment| segment.text.to_string())
        .collect();
    assert_eq!(
        texts,
        [
            "claude-opus-5".to_owned(),
            mode_label(PermissionMode::Ask).to_owned(),
            "plan".to_owned()
        ],
        "a tab that has not published an effort has three segments, not four"
    );
    // Losing which model is answering is worse than losing its name's tail.
    assert_eq!(
        segments.first().map(|segment| segment.collapsible),
        Some(false)
    );
}

#[test]
fn a_harness_that_reports_no_cost_contributes_no_segment() {
    let turn = fleet_core::agents::TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        assistant(turn, "done", fleet_core::agents::ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.context_pct = 34.0;
    projection.last_activity = Some(super::fixtures::at(120));

    let texts: Vec<String> = presentation::trailing_segments(&projection)
        .iter()
        .map(|segment| segment.text.to_string())
        .collect();
    assert_eq!(texts, ["34%".to_owned(), "2m".to_owned()]);

    projection.cumulative_cost_usd = Some(0.42);
    let texts: Vec<String> = presentation::trailing_segments(&projection)
        .iter()
        .map(|segment| segment.text.to_string())
        .collect();
    assert_eq!(
        texts,
        ["34%".to_owned(), "$0.42".to_owned(), "2m".to_owned()]
    );
}

#[test]
fn the_status_key_set_follows_the_context_not_the_badge() {
    let idle: Vec<&str> = key_hint_set(false, false)
        .iter()
        .map(|(key, _)| *key)
        .collect();
    assert!(
        idle.contains(&"\u{21e7}\u{21e5}"),
        "plan mode is bound on idle"
    );
    let working: Vec<&str> = key_hint_set(true, false)
        .iter()
        .map(|(_, label)| *label)
        .collect();
    assert!(
        working.contains(&"steer") && working.contains(&"interrupt"),
        "⏎ while working is a steer, and esc interrupts: {working:?}"
    );
    // §12: a frozen tail beats everything, including an open gate, so its keys win the bar.
    let scrolling: Vec<&str> = key_hint_set(true, true)
        .iter()
        .map(|(_, label)| *label)
        .collect();
    assert!(scrolling.contains(&"row") && scrolling.contains(&"leave"));
}

#[test]
fn the_composer_placeholder_states_what_the_mode_is_for() {
    let gate = GateId::new();
    assert_eq!(
        presentation::composer_placeholder(ComposerMode::Normal, AgentKind::Claude, None, false),
        "message claude\u{2026} (@ files \u{b7} $ skills \u{b7} / commands)"
    );
    assert_eq!(
        presentation::composer_placeholder(
            ComposerMode::Approval(gate),
            AgentKind::Claude,
            Some("git push --force"),
            false
        ),
        "git push --force",
        "under an approval the placeholder is the payload under review"
    );
    assert_eq!(
        presentation::composer_placeholder(
            ComposerMode::Question(gate),
            AgentKind::Codex,
            None,
            true
        ),
        "choose an option above"
    );
    assert_eq!(
        presentation::composer_placeholder(
            ComposerMode::PlanFollowUp(ItemId::new()),
            AgentKind::Codex,
            None,
            false
        ),
        "add feedback to refine, or leave blank to implement"
    );
}

#[test]
fn the_tab_badge_follows_the_attention_table() {
    use fleet_core::agents::{Attention, AttentionKind};

    assert_eq!(
        tab_badge(Attention::NeedsYou(AttentionKind::Permission), None),
        TabBadge::NeedsYou
    );
    assert_eq!(tab_badge(Attention::Working, None), TabBadge::Spinner);
    // A parked usage window is a gray spinner with a countdown, never an amber dot: nothing the
    // user can do releases it.
    assert_eq!(tab_badge(Attention::Waiting, None), TabBadge::Spinner);
    assert_eq!(
        tab_badge(Attention::Failed, Some(137)),
        TabBadge::Exited(Some(137))
    );
    assert_eq!(tab_badge(Attention::Unread, None), TabBadge::Unread);
    assert_eq!(tab_badge(Attention::Idle, None), TabBadge::None);
}

#[test]
fn an_untitled_thread_is_named_by_its_harness_alone() {
    let projection: ThreadProjection = projection();
    let summary = projection.summary(fleet_core::agents::Seq::default());
    assert_eq!(tab_title(&summary), "claude");
}

#[test]
fn a_metadata_segment_estimates_its_own_width() {
    // The fit is a pure function of data the caller already has, which is what lets the memo be
    // keyed by `(width, revision)` instead of measured per frame.
    let short = MetadataSegment::new("high");
    let long = MetadataSegment::new("claude-opus-5[1m]");
    assert!(long.width > short.width);
    assert!(short.collapsible);
    assert!(!MetadataSegment::pinned("claude-opus-5").collapsible);
}
