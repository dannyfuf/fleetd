//! The task guides Help teaches with: what to do, in order, and a button for each step.
//!
//! Every guide describes what Fleet really does today. A step's buttons name actions from the
//! key table, and clicking one runs exactly what its key runs (`help::run`), so reading a guide
//! and doing the work are the same click. The prose never spells a key: the chip on each button
//! is read from the live keymap, which is the one place a key may be written down.
//!
//! When a guide changes, check it against the code, not the mockup: `docs/UX-SPEC.md` §3.8.7
//! states the rule, and the tests below hold every action to the key table.

use fleet_ui_kit::Icon;

/// One task, as a short recipe.
#[derive(Debug)]
pub(crate) struct Guide {
    /// Stable identifier, for tests and logs.
    pub id: &'static str,
    /// The task, as a person would say it: "Start work on a task".
    pub title: &'static str,
    /// One line saying what the guide covers, shown beside it as a related guide.
    pub summary: &'static str,
    /// The glyph in the guide list.
    pub icon: Icon,
    /// A sentence or two of context above the steps.
    pub intro: &'static str,
    /// The steps, in the order they are done.
    pub steps: &'static [Step],
    /// Whether the steps happen inside a worktree's terminals, where Fleet's keys need the
    /// prefix, so the guide ends with the callout that says so.
    pub terminal_callout: bool,
}

/// One step of a guide.
#[derive(Debug)]
pub(crate) struct Step {
    /// What to do, as an imperative.
    pub title: &'static str,
    /// What happens, and what to know while doing it.
    pub body: &'static str,
    /// The buttons that do the step. May be empty for a step done in a form.
    pub actions: &'static [ActionRef],
}

/// A button that runs an action from the key table.
#[derive(Debug)]
pub(crate) struct ActionRef {
    /// The button's label.
    pub label: &'static str,
    /// Fully qualified action names, most specific first. The button runs the first one whose
    /// key works where Help was opened from, and otherwise shows the first one's key.
    pub actions: &'static [&'static str],
}

const fn act(label: &'static str, actions: &'static [&'static str]) -> ActionRef {
    ActionRef { label, actions }
}

const fn step(title: &'static str, body: &'static str, actions: &'static [ActionRef]) -> Step {
    Step {
        title,
        body,
        actions,
    }
}

/// Every guide, in the order Help lists them.
pub(crate) const GUIDES: &[Guide] = &[
    Guide {
        id: "start-work",
        title: "Start work on a task",
        summary: "pick a card, give it a worktree, hand it to an agent",
        icon: Icon::GitBranchPlus,
        intro: "Each task gets its own worktree: a separate checkout of the repository with its \
                own terminals and agents. Nothing you start in it stops when you close Fleet.",
        steps: &[
            step(
                "Pick a card on the board",
                "The board lists your tasks. Or skip it and create a worktree straight from a \
                 branch name.",
                &[
                    act("Go to the board", &["board::GoBoard"]),
                    act("New worktree", &["worktrees::Create"]),
                ],
            ),
            step(
                "Create a worktree for it",
                "Fleet names the branch after the card and, unless the board says otherwise, \
                 moves a card that has not started to the first started column.",
                &[act(
                    "Start a worktree",
                    &["board::CreateWorktree", "card_detail::CreateWorktree"],
                )],
            ),
            step(
                "Open it",
                "You land in its workspace: terminals, the git tab and agent threads, side by \
                 side as tabs.",
                &[act("Open", &["board::OpenWorktree", "worktrees::Open"])],
            ),
            step(
                "Hand it to an agent",
                "Start a Claude or Codex thread in the worktree. When it needs a decision, \
                 “needs you” shows at the top of the window.",
                &[
                    act("Claude", &["native_agent::NewClaude"]),
                    act("Codex", &["native_agent::NewCodex"]),
                ],
            ),
            step(
                "Go back to the hub",
                "The worktree keeps running. Its row in the list shows its session and its pull \
                 request.",
                &[act("Back to the hub", &["prefix::GoHub"])],
            ),
        ],
        terminal_callout: true,
    },
    Guide {
        id: "agent",
        title: "Work with an agent",
        summary: "start one, answer its questions, review what it changed",
        icon: Icon::Bot,
        intro: "An agent thread is a Claude or Codex conversation working inside a worktree. It \
                runs in fleetd, so it keeps going while you switch tabs, go back to the hub or \
                close Fleet.",
        steps: &[
            step(
                "Start a thread",
                "Open a worktree first. The thread opens as a tab next to its terminals.",
                &[
                    act("Claude", &["native_agent::NewClaude"]),
                    act("Codex", &["native_agent::NewCodex"]),
                ],
            ),
            step(
                "Say what you want",
                "Type @ to mention a file, $ to use a skill, and / at the start of a line for a \
                 command.",
                &[act("Send", &["native_agent::Send"])],
            ),
            step(
                "Choose how it works",
                "Switch between building and planning, pick the model, and set what the agent \
                 may do without asking.",
                &[
                    act("Build or plan", &["native_agent::PlanMode"]),
                    act("Model", &["native_agent::Model"]),
                    act("Access", &["native_agent::AccessMode"]),
                ],
            ),
            step(
                "Answer its requests",
                "Unless it has full access, the agent stops and asks before it runs a command \
                 or edits a file, and “needs you” shows until you answer.",
                &[
                    act("Allow once", &["native_agent::AllowOnce"]),
                    act("Allow for this session", &["native_agent::AllowSession"]),
                    act("Deny", &["native_agent::Deny"]),
                ],
            ),
            step(
                "Review its plan",
                "In plan mode it proposes a plan before it touches anything. Implement it as it \
                 stands, or send it back to refine it.",
                &[
                    act("Implement", &["native_agent::Implement"]),
                    act("Refine", &["native_agent::Refine"]),
                ],
            ),
            step(
                "Steer or stop it",
                "While it works, a message you send steers the current turn straight away. \
                 Stopping ends the turn; the thread stays open.",
                &[
                    act("Steer", &["native_agent::Steer"]),
                    act("Stop", &["native_agent::Stop"]),
                ],
            ),
            step(
                "See what it changed",
                "Scroll the conversation to focus a row, then show its changes, open the file \
                 in your editor, or undo a turn's file changes.",
                &[
                    act("Scroll", &["native_agent::Scroll"]),
                    act("Show the changes", &["native_agent::DiffRow"]),
                ],
            ),
        ],
        terminal_callout: true,
    },
    Guide {
        id: "review-pr",
        title: "Review a pull request",
        summary: "find it, check it out in its own worktree, read the diff",
        icon: Icon::GitPullRequest,
        intro: "Fleet lists the pull requests you opened and the ones waiting for your review. \
                Opening one checks it out in a worktree of its own, so a review never disturbs \
                your work.",
        steps: &[
            step(
                "Go to pull requests",
                "The screen has two lists: yours, and the ones waiting for your review.",
                &[act("Pull requests", &["hub::GoPrs"])],
            ),
            step(
                "Switch to the review list",
                "The count on each list says how many are waiting.",
                &[act("Next list", &["prs::NextTab"])],
            ),
            step(
                "Open it",
                "The first time, Fleet creates a worktree for the pull request's branch; after \
                 that it opens the same one.",
                &[
                    act("Open", &["prs::Open"]),
                    act("Create without opening", &["prs::CreateWithoutOpening"]),
                ],
            ),
            step(
                "Read the changes",
                "The git tab, third by default, shows the branch's commits and diffs. The \
                 terminal tabs are there to run it.",
                &[act("Git tab", &["prefix::SelectTab3"])],
            ),
            step(
                "Leave your review",
                "Open the pull request in the browser to comment, approve or ask for changes.",
                &[act("Open in the browser", &["hub::OpenInBrowser"])],
            ),
        ],
        terminal_callout: true,
    },
    Guide {
        id: "board",
        title: "Plan on the board",
        summary: "add cards, fill them in, move them along, sync",
        icon: Icon::Flag,
        intro: "Each context has a board of cards, and every worktree has one too, on its board \
                tab. A board can stay local or mirror a tracker such as Jira.",
        steps: &[
            step(
                "Open the board",
                "The hub's board belongs to the context you are in.",
                &[
                    act("Go to the board", &["board::GoBoard"]),
                    act("Worktree's board", &["prefix::OpenBoard"]),
                ],
            ),
            step(
                "Add a card",
                "Give it a title and, if you like, a description.",
                &[act("New card", &["board::NewCard"])],
            ),
            step(
                "Fill it in",
                "Status, priority, assignee, labels and estimate each have a picker. Open the \
                 card to edit its text and read its history.",
                &[
                    act("Status", &["board::PickStatus"]),
                    act("Priority", &["board::PickPriority"]),
                    act("Open the card", &["board::OpenCard"]),
                ],
            ),
            step(
                "Move it along",
                "Move the focused card one column left or right.",
                &[
                    act("Left", &["board::MovePrevColumn"]),
                    act("Right", &["board::MoveNextColumn"]),
                ],
            ),
            step(
                "Find cards",
                "Filter by title, key, assignee or label.",
                &[act("Filter", &["board::Filter"])],
            ),
            step(
                "Sync with your tracker",
                "On a board linked to a tracker, sync pulls its changes and pushes yours. A \
                 local board has nothing to sync with.",
                &[act("Sync", &["board::Sync"])],
            ),
        ],
        terminal_callout: false,
    },
    Guide {
        id: "automate",
        title: "Automate a board column",
        summary: "run an agent on every card that enters a column",
        icon: Icon::Zap,
        intro: "A column on a worktree's board can run an agent on each card that enters it, and \
                move the card on when the run succeeds. Automation needs a worktree to run in, \
                so it works on worktree boards only.",
        steps: &[
            step(
                "Open the worktree's board",
                "It is a tab of the worktree, next to its terminals.",
                &[act("Worktree's board", &["prefix::OpenBoard"])],
            ),
            step(
                "Edit its columns",
                "Board settings open on the Columns section. Open the column to automate, or \
                 add one.",
                &[act("Columns", &["board::Columns"])],
            ),
            step(
                "Say what runs on enter",
                "Set On enter to prompt to run the card as the agent's brief, or to \
                 skill:<name> to run one of the agent's skills. A ⚡ marks a column that runs \
                 something.",
                &[],
            ),
            step(
                "Pick the agent and brief it",
                "Provider, model, effort and mode choose the agent and what it may do. \
                 Instructions are added to the card's brief, and Expect is added at the end as \
                 what the card expects.",
                &[],
            ),
            step(
                "Say where the card goes next",
                "On success moves the card to a later column when its run succeeds. When \
                 unblocked releases a waiting card once every card blocking it is done.",
                &[],
            ),
            step(
                "Save, then send it a card",
                "Save the settings and move a card into the column. From the card you can \
                 watch its run, start it now or stop it.",
                &[
                    act("Watch the run", &["board::AttachRun"]),
                    act("Run now", &["board::RunNow"]),
                ],
            ),
        ],
        terminal_callout: false,
    },
    Guide {
        id: "terminals",
        title: "Terminals: copy, paste, scroll",
        summary: "copy and paste, scroll back, zoom, watch sub-agents, restart",
        icon: Icon::SquareTerminal,
        intro: "Every key you type in a terminal goes to the program running in it. The \
                clipboard keys and the prefix key are the only ones Fleet keeps.",
        steps: &[
            step(
                "Copy",
                "Drag to select, double-click for a word, triple-click for a line: the text is \
                 copied when you let go. With nothing selected, the copy key goes to the \
                 program.",
                &[act("Copy", &["workspace::CopySelection"])],
            ),
            step(
                "Paste",
                "Pastes the clipboard into the program, bracketed when the program asks for it.",
                &[act(
                    "Paste",
                    &["workspace::PasteClipboard", "prefix::Paste"],
                )],
            ),
            step(
                "Scroll back",
                "The wheel scrolls the output. Scroll mode moves through it from the keyboard, \
                 and selects and copies lines.",
                &[
                    act("Scroll mode", &["prefix::EnterScroll"]),
                    act("Page up", &["scroll::TerminalPageUp"]),
                ],
            ),
            step(
                "Zoom",
                "Hide the session header and the tab strip to give the terminal the room.",
                &[act("Zoom", &["prefix::ToggleZoom"])],
            ),
            step(
                "Watch sub-agents",
                "When an agent starts helpers, the watch pane follows them. Show or hide it, \
                 and step through the watches.",
                &[
                    act("Watch pane", &["prefix::ToggleWatchPane"]),
                    act("Next watch", &["prefix::NextWatch"]),
                ],
            ),
            step(
                "Manage tabs",
                "Open another shell in the worktree, rename a tab, or close it. Closing asks \
                 first when an agent or a server is running in it.",
                &[
                    act("New terminal", &["prefix::NewTerminal"]),
                    act("Rename", &["prefix::RenameTerminal"]),
                    act("Close", &["prefix::CloseTerminal"]),
                ],
            ),
            step(
                "Restart a command",
                "When a tab's program has exited, run it again in the same tab.",
                &[act("Restart", &["prefix::RestartCommand"])],
            ),
        ],
        terminal_callout: true,
    },
    Guide {
        id: "keeps-running",
        title: "What keeps running",
        summary: "what closing Fleet leaves alone, and the three things that stop work",
        icon: Icon::Clock,
        intro: "Closing Fleet never stops your work. Terminals, agents and jobs run inside \
                fleetd, a background service, and Fleet only shows them. Terminals even \
                survive a restart of fleetd and reattach on their own.",
        steps: &[
            step(
                "Quit, close, go back: all safe",
                "Quitting Fleet, closing a dialog or going back to the hub leaves everything \
                 running. It is all there when you come back.",
                &[act("Quit Fleet", &["fleet::Quit"])],
            ),
            step(
                "Cancelling a job stops it",
                "Clones, refreshes and hooks run as jobs. Cancel one, or all of them, from the \
                 jobs panel.",
                &[act("Show jobs", &["fleet::OpenJobs"])],
            ),
            step(
                "Ending a session stops its terminals",
                "Ends the worktree's session and everything running in its terminals. Fleet \
                 asks first.",
                &[act("End its session", &["worktrees::Kill"])],
            ),
            step(
                "Stopping fleetd stops everything",
                "Quitting and stopping fleetd ends every terminal, agent and job. Fleet lists \
                 them and asks first.",
                &[act("Quit and stop fleetd", &["fleet::QuitAndStopDaemon"])],
            ),
            step(
                "Sleep is gentler",
                "A sleeping worktree closes its idle terminals and keeps the ones running an \
                 agent, a server or an editor with unsaved changes. Opening another worktree \
                 puts the one you leave to sleep, unless you open it keeping the last one awake.",
                &[
                    act("Sleep", &["worktrees::Sleep", "prefix::SleepAndGoHub"]),
                    act("Open, keep awake", &["worktrees::OpenKeepAwake"]),
                ],
            ),
        ],
        terminal_callout: false,
    },
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::{action_catalogue, keymap};

    #[test]
    fn every_step_button_runs_a_bound_catalogued_action() {
        let bound: HashSet<&str> = keymap::table().iter().map(|spec| spec.action).collect();
        for guide in GUIDES {
            for step in guide.steps {
                for button in step.actions {
                    assert!(!button.actions.is_empty(), "{}: an empty button", guide.id);
                    for action in button.actions {
                        assert!(
                            bound.contains(action),
                            "{}: `{action}` has no key, so its button could not run it",
                            guide.id
                        );
                        assert!(
                            action_catalogue::info(action).is_some(),
                            "{}: `{action}` has no catalogue entry",
                            guide.id
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn guides_are_distinct_and_readable() {
        let mut ids = HashSet::new();
        for guide in GUIDES {
            assert!(ids.insert(guide.id), "two guides are called {}", guide.id);
            assert!(!guide.steps.is_empty(), "{} has no steps", guide.id);
            assert!(
                guide.steps.len() <= 7,
                "{} is a manual, not a guide",
                guide.id
            );
            for text in [guide.intro]
                .into_iter()
                .chain(guide.steps.iter().map(|step| step.body))
            {
                assert!(
                    text.ends_with('.'),
                    "{}: `{text}` is not a sentence",
                    guide.id
                );
                // Keys are chips from the live keymap; prose that spells one drifts from it.
                for spelled in ["ctrl-", "cmd-", "⌃", "⌘", "^s"] {
                    assert!(
                        !text.contains(spelled),
                        "{}: `{text}` spells a key; put it on a button instead",
                        guide.id
                    );
                }
            }
        }
        assert_eq!(GUIDES.len(), 7);
    }
}
