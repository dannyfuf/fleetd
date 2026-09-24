//! Every catalogue entry, written by hand.
//!
//! Rules for a new entry (`docs/KEYMAP.md` § *Action catalogue*):
//!
//! * The label is a sentence-case verb phrase a person who has never read the keymap
//!   understands: specific, short, no type names, no key names.
//! * The description is one line saying what happens, including the safety that matters
//!   ("asks first", "keeps running in fleetd").
//! * A numbered range (`SelectTab1` … `9`) is one entry with a range label.
//! * The order is the order Help and the `^s` menu list entries in: by place, then by heading,
//!   the most useful first.

use super::{Entry, Group as G, Place as P};

const fn e(
    actions: &'static [&'static str],
    place: P,
    group: G,
    label: &'static str,
    description: &'static str,
) -> Entry {
    Entry::new(actions, place, group, label, description)
}

pub(super) const ENTRIES: &[Entry] = &[
    // ── Everywhere ────────────────────────────────────────────────────────────────────────
    e(
        &["fleet::OpenHelp"],
        P::Everywhere,
        G::Panels,
        "Show help and shortcuts",
        "Open this guide: what you can do here, and every key.",
    )
    .short("All shortcuts")
    .palette(),
    e(
        &["fleet::OpenJobs"],
        P::Everywhere,
        G::Panels,
        "Show jobs",
        "Open the jobs panel: clones, refreshes and other background work, with their logs.",
    )
    .short("Jobs")
    .palette(),
    e(
        &["fleet::FocusStickyError"],
        P::Everywhere,
        G::Panels,
        "Show the last error",
        "Open the jobs panel on the job that failed, to read its log or retry it.",
    )
    .short("Last error"),
    e(
        &["fleet::DismissStickyError"],
        P::Hub,
        G::Panels,
        "Dismiss the last error",
        "Clear the error from the status bar. The failed jobs stay in the jobs panel.",
    )
    .short("Dismiss error"),
    e(
        &["fleet::Quit"],
        P::Everywhere,
        G::App,
        "Quit Fleet",
        "Close the app. Your terminals, agents and jobs keep running in fleetd.",
    )
    .palette(),
    e(
        &["fleet::QuitAndStopDaemon"],
        P::Everywhere,
        G::App,
        "Quit and stop fleetd",
        "Close the app and stop fleetd, which ends every terminal, agent and job. Asks first.",
    )
    .destructive()
    .palette(),
    // ── Hub ───────────────────────────────────────────────────────────────────────────────
    e(
        &["fleet::OpenPalette"],
        P::Hub,
        G::Panels,
        "Open the command palette",
        "Search commands, worktrees, cards, pull requests and agents by name, and run one.",
    )
    .featured(80)
    .short("Command palette"),
    e(
        &["fleet::OpenSettings"],
        P::Hub,
        G::App,
        "Open settings",
        "Change Fleet's preferences and see how fleetd is doing.",
    )
    .featured(85)
    .short("Settings")
    .palette(),
    e(
        &["fleet::Refresh"],
        P::Hub,
        G::App,
        "Refresh everything",
        "Check worktree status, pull requests and repositories again, in the background.",
    )
    .short("Refresh")
    .palette(),
    e(
        &["fleet::UpdateFleet"],
        P::Hub,
        G::App,
        "Update Fleet",
        "Install the new version of Fleet in the background.",
    )
    .palette(),
    e(
        &["first_run::Import"],
        P::Hub,
        G::App,
        "Import from swarm",
        "Bring in the repositories and worktrees swarm knows about, as a background job.",
    )
    .featured(91),
    e(
        &["hub::MoveDown"],
        P::Hub,
        G::Navigation,
        "Move down",
        "Select the next row.",
    ),
    e(
        &["hub::MoveUp"],
        P::Hub,
        G::Navigation,
        "Move up",
        "Select the previous row.",
    ),
    e(
        &["hub::GoTop"],
        P::Hub,
        G::Navigation,
        "Go to the first row",
        "Select the first row of the list.",
    ),
    e(
        &["hub::GoBottom"],
        P::Hub,
        G::Navigation,
        "Go to the last row",
        "Select the last row of the list.",
    ),
    e(
        &["hub::HalfPageDown"],
        P::Hub,
        G::Navigation,
        "Move down half a page",
        "Jump the cursor half a screen down the list.",
    ),
    e(
        &["hub::HalfPageUp"],
        P::Hub,
        G::Navigation,
        "Move up half a page",
        "Jump the cursor half a screen up the list.",
    ),
    e(
        &["hub::FocusPrevPane"],
        P::Hub,
        G::Navigation,
        "Focus the previous pane",
        "Move between the repositories list and the list beside it.",
    ),
    e(
        &["hub::FocusNextPane"],
        P::Hub,
        G::Navigation,
        "Focus the next pane",
        "Move between the repositories list and the list beside it.",
    ),
    e(
        &["hub::GoRepos"],
        P::Hub,
        G::Navigation,
        "Go to repositories",
        "Focus the list of repositories on the left.",
    ),
    e(
        &["hub::GoWorktrees"],
        P::Hub,
        G::Navigation,
        "Go to worktrees",
        "Show the list of worktrees.",
    )
    .palette(),
    e(
        &["hub::GoPrs"],
        P::Hub,
        G::Navigation,
        "Go to pull requests",
        "Show your pull requests and the ones waiting for your review.",
    )
    .palette(),
    e(
        &["hub::TogglePrScreen"],
        P::Hub,
        G::Navigation,
        "Switch between worktrees and pull requests",
        "Flip the main list between your worktrees and your pull requests.",
    )
    .featured(75),
    e(
        &["board::GoBoard"],
        P::Hub,
        G::Navigation,
        "Go to the board",
        "Show this context's board of cards.",
    )
    .featured(70)
    .short("Go to board")
    .palette(),
    e(
        &["hub::GoJobs"],
        P::Hub,
        G::Navigation,
        "Go to jobs",
        "Open the jobs panel.",
    ),
    e(
        &["hub::GoAllRepos"],
        P::Hub,
        G::Navigation,
        "Show every repository",
        "List the worktrees of all the repositories in this context.",
    ),
    e(
        &["hub::OpenFilter"],
        P::Hub,
        G::Navigation,
        "Filter the list",
        "Type to narrow the list you are in.",
    )
    .featured(40)
    .short("Filter"),
    e(
        &["fleet::Cancel"],
        P::Hub,
        G::Navigation,
        "Clear or close",
        "Clear the filter if there is one, otherwise close what is open on top. Never quits.",
    ),
    e(
        &["hub::OpenInBrowser"],
        P::Hub,
        G::Navigation,
        "Open in the browser",
        "Open the selected pull request or repository on its website.",
    )
    .featured(65),
    e(
        &["hub::ToggleDetail"],
        P::Hub,
        G::Panels,
        "Show or hide details",
        "Toggle the panel that describes the selected row.",
    )
    .featured(50),
    e(
        &["hub::ToggleRepoRail"],
        P::Hub,
        G::Panels,
        "Collapse or expand the repositories list",
        "Shrink the list of repositories to icons to make room, or bring it back.",
    ),
    e(
        &["fleet::OpenAgentClaude"],
        P::Hub,
        G::Agents,
        "Open the Claude agent window",
        "Show the floating Claude terminal. Press again to hide it; it keeps running in fleetd.",
    )
    .featured(30)
    .short("Claude window")
    .palette(),
    e(
        &["fleet::OpenAgentCodex"],
        P::Hub,
        G::Agents,
        "Open the Codex agent window",
        "Show the floating Codex terminal. Press again to hide it; it keeps running in fleetd.",
    )
    .featured(62)
    .short("Codex window")
    .palette(),
    e(
        &[
            "hub::SelectContext1",
            "hub::SelectContext2",
            "hub::SelectContext3",
            "hub::SelectContext4",
            "hub::SelectContext5",
            "hub::SelectContext6",
            "hub::SelectContext7",
            "hub::SelectContext8",
            "hub::SelectContext9",
        ],
        P::Hub,
        G::Context,
        "Switch to context 1–9",
        "Jump straight to one of your first nine contexts.",
    )
    .short("Context 1–9"),
    e(
        &["hub::NextContext"],
        P::Hub,
        G::Context,
        "Switch to the next context",
        "Show the next context's repositories, worktrees and board.",
    ),
    e(
        &["hub::PrevContext"],
        P::Hub,
        G::Context,
        "Switch to the previous context",
        "Show the previous context's repositories, worktrees and board.",
    ),
    e(
        &["hub::NewContext"],
        P::Hub,
        G::Context,
        "New context",
        "Create a context: a named group of repositories with its own board.",
    )
    .featured(90)
    .palette(),
    e(
        &["hub::EditContext"],
        P::Hub,
        G::Context,
        "Edit this context",
        "Rename the current context or change who owns it.",
    )
    .palette(),
    e(
        &["hub::DeleteContext"],
        P::Hub,
        G::Context,
        "Delete this context",
        "Remove the current context from Fleet. Asks first, and needs the strong yes.",
    )
    .destructive()
    .palette(),
    e(
        &["repos::Open"],
        P::Hub,
        G::Repository,
        "Open the repository",
        "Show this repository's worktrees.",
    )
    .featured(10),
    e(
        &["repos::Clone"],
        P::Hub,
        G::Repository,
        "Clone a repository",
        "Search GitHub and clone a repository into this context, as a background job.",
    )
    .short("Clone repo")
    .featured(20)
    .palette(),
    e(
        &["repos::MoveToContext"],
        P::Hub,
        G::Repository,
        "Move to another context",
        "Put the selected repository in a different context.",
    )
    .palette(),
    e(
        &["repos::EditHooks"],
        P::Hub,
        G::Repository,
        "Edit setup commands",
        "Change the commands that run when one of its worktrees is prepared or created.",
    )
    .featured(60),
    e(
        &["repos::DismissClone"],
        P::Hub,
        G::Repository,
        "Dismiss the failed clone",
        "Remove a clone that failed from the list.",
    ),
    e(
        &["repos::Delete"],
        P::Hub,
        G::Repository,
        "Delete the repository",
        "Remove the repository and all of its worktrees. Asks first, and needs the strong yes.",
    )
    .destructive(),
    e(
        &["filter::Accept"],
        P::Hub,
        G::Navigation,
        "Open the highlighted row",
        "Open the row under the cursor without leaving the filter first.",
    ),
    e(
        &["filter::CursorDown"],
        P::Hub,
        G::Navigation,
        "Move down the filtered list",
        "Move the cursor while you keep typing.",
    ),
    e(
        &["filter::CursorUp"],
        P::Hub,
        G::Navigation,
        "Move up the filtered list",
        "Move the cursor while you keep typing.",
    ),
    e(
        &["filter::Escape"],
        P::Hub,
        G::Navigation,
        "Leave or clear the filter",
        "The first press keeps the filter and returns to the list; the second clears it.",
    ),
    // ── Worktrees ─────────────────────────────────────────────────────────────────────────
    e(
        &["worktrees::Open"],
        P::Worktrees,
        G::Worktree,
        "Open the selected worktree",
        "Go to its terminals and agents. The worktree you were in goes to sleep.",
    )
    .featured(10)
    .short("Open"),
    e(
        &["worktrees::OpenKeepAwake"],
        P::Worktrees,
        G::Worktree,
        "Open it and keep the last one awake",
        "Open the selected worktree without putting the one you were in to sleep.",
    )
    .short("Open, keep awake"),
    e(
        &["worktrees::Create"],
        P::Worktrees,
        G::Worktree,
        "New worktree",
        "Create a separate checkout of a branch, with its own terminals and agents.",
    )
    .featured(20)
    .palette(),
    e(
        &["worktrees::Delete"],
        P::Worktrees,
        G::Worktree,
        "Delete the worktree safely",
        "Remove it after showing what would be lost. Asks first, and you can undo it.",
    )
    .featured(60)
    .short("Delete")
    .destructive()
    .palette(),
    e(
        &["worktrees::UndoDelete"],
        P::Worktrees,
        G::Worktree,
        "Undo the last delete",
        "Bring back the worktree you just deleted, while it is still in the trash.",
    )
    .short("Undo delete"),
    e(
        &["worktrees::Prune"],
        P::Worktrees,
        G::Worktree,
        "Clean up finished worktrees",
        "Preview the worktrees that are safe to remove, then remove them. Asks first.",
    )
    .destructive()
    .palette(),
    e(
        &["worktrees::Sleep"],
        P::Worktrees,
        G::Worktree,
        "Put its session to sleep",
        "Stop its terminals to save resources. Opening the worktree again brings them back.",
    )
    .short("Sleep")
    .palette(),
    e(
        &["worktrees::Kill"],
        P::Worktrees,
        G::Worktree,
        "End its session",
        "Stop every terminal and agent running in this worktree. Asks first.",
    )
    .short("Kill session")
    .destructive()
    .palette(),
    e(
        &["worktrees::Inspect"],
        P::Worktrees,
        G::Worktree,
        "Check what would be lost",
        "Look again for uncommitted changes, unpushed commits and running programs.",
    )
    .short("Inspect")
    .palette(),
    e(
        &["worktrees::CopyPath"],
        P::Worktrees,
        G::Worktree,
        "Copy the worktree's path",
        "Put the folder path of the selected worktree on the clipboard.",
    )
    .short("Copy path"),
    e(
        &["worktrees::CopyBranch"],
        P::Worktrees,
        G::Worktree,
        "Copy the branch name",
        "Put the selected worktree's branch name on the clipboard.",
    )
    .short("Copy branch"),
    // ── Pull requests ─────────────────────────────────────────────────────────────────────
    e(
        &["prs::Open"],
        P::PullRequests,
        G::PullRequests,
        "Open the pull request's worktree",
        "Check out its branch in a worktree, creating it if needed. The one you were in sleeps.",
    )
    .featured(10)
    .short("Open"),
    e(
        &["prs::OpenKeepAwake"],
        P::PullRequests,
        G::PullRequests,
        "Open it and keep the last one awake",
        "Open the pull request's worktree without putting the one you were in to sleep.",
    )
    .short("Open, keep last awake"),
    e(
        &["prs::CreateWithoutOpening"],
        P::PullRequests,
        G::PullRequests,
        "Create its worktree without opening it",
        "Check out the pull request's branch in a worktree and stay on this list.",
    )
    .short("Create worktree only"),
    e(
        &["prs::NextTab"],
        P::PullRequests,
        G::PullRequests,
        "Next list",
        "Switch between your pull requests and the ones waiting for your review.",
    )
    .featured(20),
    e(
        &["prs::PrevTab"],
        P::PullRequests,
        G::PullRequests,
        "Previous list",
        "Switch between your pull requests and the ones waiting for your review.",
    ),
    e(
        &["prs::Inspect"],
        P::PullRequests,
        G::PullRequests,
        "Check its worktree",
        "Look for uncommitted changes and running programs in the pull request's worktree.",
    )
    .short("Check worktree"),
    e(
        &["prs::CopyUrl"],
        P::PullRequests,
        G::PullRequests,
        "Copy the pull request's link",
        "Put the pull request's web address on the clipboard.",
    )
    .short("Copy link"),
    e(
        &["prs::Refresh"],
        P::PullRequests,
        G::PullRequests,
        "Refresh pull requests",
        "Fetch both lists again from GitHub.",
    )
    .short("Refresh"),
    e(
        &["prs::Back"],
        P::PullRequests,
        G::PullRequests,
        "Back to worktrees",
        "Leave the pull requests for the list of worktrees.",
    )
    .featured(58),
    // ── Board ─────────────────────────────────────────────────────────────────────────────
    e(
        &["board::OpenCard"],
        P::Board,
        G::Board,
        "Open the card",
        "Show the card's details, comments and agent runs.",
    )
    .featured(10)
    .short("Open")
    .palette(),
    e(
        &["board::NewCard"],
        P::Board,
        G::Board,
        "New card",
        "Add a card to the board.",
    )
    .featured(20)
    .palette(),
    e(
        &["board::CreateWorktree"],
        P::Board,
        G::Board,
        "Start a worktree for the card",
        "Create a worktree whose branch is named after the card.",
    )
    .featured(25)
    .short("New worktree")
    .palette(),
    e(
        &["board::OpenWorktree"],
        P::Board,
        G::Board,
        "Open the card's worktree",
        "Go to the worktree this card is being worked on in.",
    )
    .short("Open worktree")
    .featured(27)
    .palette(),
    e(
        &["board::NextCard"],
        P::Board,
        G::Board,
        "Next card",
        "Select the card below in this column.",
    )
    .palette(),
    e(
        &["board::PrevCard"],
        P::Board,
        G::Board,
        "Previous card",
        "Select the card above in this column.",
    )
    .palette(),
    e(
        &["board::NextColumn"],
        P::Board,
        G::Board,
        "Next column",
        "Select the column to the right.",
    )
    .palette(),
    e(
        &["board::PrevColumn"],
        P::Board,
        G::Board,
        "Previous column",
        "Select the column to the left.",
    )
    .palette(),
    e(
        &["board::MoveNextColumn"],
        P::Board,
        G::Board,
        "Move the card right",
        "Move the selected card to the next column.",
    )
    .short("Move to next column")
    .featured(35)
    .palette(),
    e(
        &["board::MovePrevColumn"],
        P::Board,
        G::Board,
        "Move the card left",
        "Move the selected card to the previous column.",
    )
    .short("Move to previous column")
    .palette(),
    e(
        &["board::PickStatus"],
        P::Board,
        G::Board,
        "Change the status",
        "Pick the card's status, which is also its column.",
    )
    .short("Status")
    .palette(),
    e(
        &["board::PickPriority"],
        P::Board,
        G::Board,
        "Change the priority",
        "Pick how urgent the card is.",
    )
    .short("Priority")
    .palette(),
    e(
        &["board::PickAssignee"],
        P::Board,
        G::Board,
        "Assign the card",
        "Pick who the card is assigned to.",
    )
    .short("Assignee")
    .palette(),
    e(
        &["board::PickLabels"],
        P::Board,
        G::Board,
        "Change the labels",
        "Pick the labels on the card.",
    )
    .short("Labels")
    .palette(),
    e(
        &["board::PickEstimate"],
        P::Board,
        G::Board,
        "Change the estimate",
        "Pick how big the card's work is.",
    )
    .short("Estimate")
    .palette(),
    e(
        &["board::PickBlockedBy"],
        P::Board,
        G::Board,
        "Mark what blocks the card",
        "Pick the cards that have to be finished before this one.",
    )
    .short("Blocked by")
    .palette(),
    e(
        &["board::PickAgent"],
        P::Board,
        G::Board,
        "Choose the agent for a card",
        "Pick which agent works on this card when its column runs one.",
    )
    .short("Agent")
    .palette(),
    e(
        &["board::AttachRun"],
        P::Board,
        G::Board,
        "Watch the card's agent run",
        "Open the agent working on this card in a tab.",
    )
    .short("Attach run")
    .palette(),
    e(
        &["board::RunNow"],
        P::Board,
        G::Board,
        "Run the column's agent now",
        "Start the column's automation on this card without waiting.",
    )
    .short("Run now")
    .palette(),
    e(
        &["board::CancelRun"],
        P::Board,
        G::Board,
        "Stop the card's agent run",
        "Cancel the agent working on this card.",
    )
    .short("Cancel run")
    .palette(),
    e(
        &["board::OpenRemote"],
        P::Board,
        G::Board,
        "Open the card's issue in the browser",
        "Open the issue this card mirrors on its tracker's website.",
    )
    .short("Open remote issue")
    .palette(),
    e(
        &["board::DeleteCard"],
        P::Board,
        G::Board,
        "Delete the card",
        "Remove the card from the board. Asks first.",
    )
    .short("Delete")
    .destructive()
    .palette(),
    e(
        &["board::Filter"],
        P::Board,
        G::Board,
        "Filter the cards",
        "Type to show only the cards that match.",
    )
    .short("Filter")
    .featured(40)
    .palette(),
    e(
        &["board::Sync"],
        P::Board,
        G::Board,
        "Sync with the tracker",
        "Bring the board up to date with the tracker it mirrors, such as Jira or GitHub.",
    )
    .short("Sync")
    .palette(),
    e(
        &["board::FullSync"],
        P::Board,
        G::Board,
        "Sync everything again",
        "Read every card from the tracker again, not only what changed.",
    )
    .short("Full sync")
    .palette(),
    e(
        &["board::Reload"],
        P::Board,
        G::Board,
        "Reload the board",
        "Read the board again from fleetd.",
    )
    .short("Reload")
    .palette(),
    e(
        &["board::Settings"],
        P::Board,
        G::Board,
        "Board settings",
        "Change the board's columns, fields and automation.",
    )
    .palette(),
    e(
        &["board::Columns"],
        P::Board,
        G::Board,
        "Edit the board's columns",
        "Open board settings on its list of columns.",
    )
    .short("Columns")
    .palette(),
    e(
        &["board::Schedules"],
        P::Board,
        G::Board,
        "Edit the board's schedules",
        "Open board settings on its list of schedules.",
    )
    .short("Schedules")
    .palette(),
    e(
        &["board::RunSchedules"],
        P::Board,
        G::Board,
        "Run the board's schedules now",
        "Run every enabled schedule of the board now, without waiting for its next time.",
    )
    .short("Run schedules")
    .palette(),
    e(
        &["board::OpenPullRequest"],
        P::Board,
        G::Board,
        "Open the card's pull request in the browser",
        "Open the pull request this review card is about on GitHub.",
    )
    .short("Open pull request")
    .palette(),
    e(
        &["board::CopyPullRequestUrl"],
        P::Board,
        G::Board,
        "Copy the card's pull request link",
        "Put the review card's pull request web address on the clipboard.",
    )
    .short("Copy pull request link")
    .palette(),
    // ── Card ──────────────────────────────────────────────────────────────────────────────
    e(
        &["card_detail::Close"],
        P::CardDetail,
        G::Card,
        "Close the card",
        "Go back to the board. While you are typing, cancel the edit instead.",
    )
    .palette(),
    e(
        &["card_detail::EditTitle"],
        P::CardDetail,
        G::Card,
        "Edit the title",
        "Change the card's title.",
    )
    .palette(),
    e(
        &["card_detail::EditDescription"],
        P::CardDetail,
        G::Card,
        "Edit the description",
        "Change the card's description.",
    )
    .palette(),
    e(
        &["card_detail::AddComment"],
        P::CardDetail,
        G::Card,
        "Add a comment",
        "Write a comment on the card.",
    )
    .palette(),
    e(
        &["card_detail::NextProperty"],
        P::CardDetail,
        G::Card,
        "Next field",
        "Select the card field below.",
    )
    .palette(),
    e(
        &["card_detail::PrevProperty"],
        P::CardDetail,
        G::Card,
        "Previous field",
        "Select the card field above.",
    )
    .palette(),
    e(
        &["card_detail::EditProperty"],
        P::CardDetail,
        G::Card,
        "Edit the selected field",
        "Change the selected field, or expand every folded run report.",
    )
    .palette(),
    e(
        &["card_detail::Save"],
        P::CardDetail,
        G::Card,
        "Save the edit",
        "Keep the text you changed.",
    )
    .palette(),
    e(
        &["card_detail::CreateWorktree"],
        P::CardDetail,
        G::Card,
        "Start a worktree for the card",
        "Create a worktree whose branch is named after the card.",
    )
    .short("New worktree")
    .palette(),
    e(
        &["card_detail::OpenRemote"],
        P::CardDetail,
        G::Card,
        "Open the card's issue in the browser",
        "Open the issue this card mirrors on its tracker's website.",
    )
    .palette(),
    e(
        &["card_detail::KeepLocal"],
        P::CardDetail,
        G::Card,
        "Keep your version",
        "Settle the conflict with the changes made here; the tracker gets them.",
    )
    .palette(),
    e(
        &["card_detail::TakeRemote"],
        P::CardDetail,
        G::Card,
        "Take the tracker's version",
        "Settle the conflict with the tracker's changes; the ones made here are dropped.",
    )
    .palette(),
    e(
        &["board::CreateAndOpen"],
        P::CardDetail,
        G::Card,
        "Create and open the card",
        "Add the new card and show its details.",
    ),
    // ── Terminal ──────────────────────────────────────────────────────────────────────────
    e(
        &["workspace::EnterPrefix"],
        P::Terminal,
        G::Terminal,
        "Start a Fleet command",
        "Every Fleet key in a terminal starts with this one; everything else goes to the program.",
    ),
    e(
        &[
            "prefix::SelectTab1",
            "prefix::SelectTab2",
            "prefix::SelectTab3",
            "prefix::SelectTab4",
            "prefix::SelectTab5",
            "prefix::SelectTab6",
            "prefix::SelectTab7",
            "prefix::SelectTab8",
            "prefix::SelectTab9",
        ],
        P::Terminal,
        G::Tabs,
        "Go to tab 1–9",
        "Switch to one of the worktree's first nine tabs.",
    )
    .short("Go to tab"),
    e(
        &["prefix::NewTerminal"],
        P::Terminal,
        G::Tabs,
        "New terminal",
        "Open a new shell tab in this worktree.",
    )
    .featured(30),
    e(
        &["prefix::CloseTerminal"],
        P::Terminal,
        G::Tabs,
        "Close the tab",
        "Close this terminal. Asks first if a program is still running in it.",
    )
    .short("Close tab"),
    e(
        &["prefix::RenameTerminal"],
        P::Terminal,
        G::Tabs,
        "Rename the tab",
        "Give this terminal tab a name of your own.",
    )
    .short("Rename tab"),
    e(
        &["prefix::PrevTab"],
        P::Terminal,
        G::Tabs,
        "Previous tab",
        "Switch to the tab on the left.",
    ),
    e(
        &["prefix::NextTab"],
        P::Terminal,
        G::Tabs,
        "Next tab",
        "Switch to the tab on the right.",
    ),
    e(
        &["prefix::LastTab"],
        P::Terminal,
        G::Tabs,
        "Go to the last tab you used",
        "Flip back to the tab you were on before this one.",
    )
    .short("Last tab"),
    e(
        &["prefix::GoHub"],
        P::Terminal,
        G::Session,
        "Back to the hub",
        "Leave this worktree for the hub. Everything here keeps running.",
    )
    .featured(10)
    .short("Back to hub"),
    e(
        &["prefix::SleepAndGoHub"],
        P::Terminal,
        G::Session,
        "Sleep this session and go back to the hub",
        "Stop this worktree's terminals and return to the hub. Opening it again brings them back.",
    )
    .short("Sleep, back to hub"),
    e(
        &["prefix::LastSession"],
        P::Terminal,
        G::Session,
        "Go to the last session you used",
        "Jump back to the worktree you were in before this one.",
    )
    .short("Last session"),
    e(
        &["prefix::SessionSwitcher"],
        P::Terminal,
        G::Session,
        "Switch to another session",
        "Open the palette with only your worktree sessions listed.",
    )
    .short("Switch session…"),
    e(
        &["prefix::CopyWorktreePath"],
        P::Terminal,
        G::Session,
        "Copy the worktree's path",
        "Put this worktree's folder path on the clipboard.",
    )
    .short("Copy path"),
    e(
        &["prefix::EnterScroll"],
        P::Terminal,
        G::Terminal,
        "Scroll back through the output",
        "Freeze the terminal to scroll, select and copy its history.",
    )
    .featured(40)
    .short("Scroll back"),
    e(
        &["prefix::Paste"],
        P::Terminal,
        G::Terminal,
        "Paste into the terminal",
        "Paste the clipboard as if you had typed it.",
    )
    .featured(60)
    .short("Paste"),
    e(
        &["prefix::ToggleZoom"],
        P::Terminal,
        G::Terminal,
        "Zoom the pane",
        "Hide the header and tabs so the terminal gets the whole window. Press again to undo.",
    )
    .featured(55)
    .short("Zoom pane"),
    e(
        &["prefix::RestartCommand"],
        P::Terminal,
        G::Terminal,
        "Restart the command",
        "Run this tab's command again after it has exited.",
    )
    .featured(57)
    .short("Restart command"),
    e(
        &["prefix::SendLiteral"],
        P::Terminal,
        G::Terminal,
        "Send the prefix key to the program",
        "Pass the prefix key itself through to the program in the terminal.",
    )
    .short("Send prefix key"),
    e(
        &["prefix::Cancel"],
        P::Terminal,
        G::Terminal,
        "Cancel the command",
        "Do nothing and give the keys back to the terminal.",
    )
    .short("Cancel"),
    e(
        &["workspace::CopySelection"],
        P::Terminal,
        G::Terminal,
        "Copy the selection",
        "Copy the selected text. With nothing selected, the key goes to the program.",
    )
    .short("Copy"),
    e(
        &["workspace::PasteClipboard"],
        P::Terminal,
        G::Terminal,
        "Paste the clipboard",
        "Paste into the terminal the way the program running in it expects.",
    )
    .short("Paste"),
    e(
        &["scroll::TerminalPageUp"],
        P::Terminal,
        G::Terminal,
        "Scroll up a page",
        "Scroll the terminal's history. In a full-screen program, the key goes to the program.",
    ),
    e(
        &["scroll::TerminalPageDown"],
        P::Terminal,
        G::Terminal,
        "Scroll down a page",
        "Scroll the terminal's history. In a full-screen program, the key goes to the program.",
    ),
    e(
        &["scroll::TerminalTop"],
        P::Terminal,
        G::Terminal,
        "Scroll to the oldest output",
        "Jump to the start of the terminal's history.",
    ),
    e(
        &["scroll::TerminalBottom"],
        P::Terminal,
        G::Terminal,
        "Scroll to the newest output",
        "Jump back to the live bottom of the terminal.",
    ),
    e(
        &["native_agent::NewClaude"],
        P::Terminal,
        G::Agents,
        "New Claude thread in a tab",
        "Start a Claude agent working in this worktree, in its own tab.",
    )
    .featured(20)
    .short("New Claude thread"),
    e(
        &["native_agent::NewCodex"],
        P::Terminal,
        G::Agents,
        "New Codex thread in a tab",
        "Start a Codex agent working in this worktree, in its own tab.",
    )
    .short("New Codex thread"),
    e(
        &["prefix::AgentsPicker"],
        P::Terminal,
        G::Agents,
        "Pick from all running agents",
        "Open the palette with only agent threads listed.",
    )
    .short("Agents…"),
    e(
        &["prefix::UpToCaller"],
        P::Terminal,
        G::Agents,
        "Go to the agent that started this one",
        "From a sub-agent's thread, select the thread that handed it the work.",
    )
    .short("Up to caller"),
    e(
        &["native_agent::TerminalFallback"],
        P::Terminal,
        G::Agents,
        "Use the agent's terminal window",
        "Open the floating terminal version of the agent instead of a thread tab.",
    )
    .short("Agent terminal"),
    e(
        &["prefix::OpenBoard"],
        P::Terminal,
        G::Panels,
        "Open the worktree's board",
        "Show this worktree's board in a tab, creating it the first time.",
    )
    .featured(50)
    .short("Board")
    .palette(),
    e(
        &["prefix::ToggleWatchPane"],
        P::Terminal,
        G::Panels,
        "Show or hide the watch pane",
        "Toggle the read-only view of the sub-agents this session started.",
    )
    .short("Watch pane"),
    e(
        &["prefix::ToggleChanges"],
        P::Terminal,
        G::Panels,
        "Show or hide the Changes panel",
        "List what this worktree changed against its base: files, commits ahead, and each file's diff.",
    )
    .featured(52)
    .short("Changes"),
    e(
        &["prefix::NextWatch"],
        P::Terminal,
        G::Panels,
        "Next watch",
        "Show the next sub-agent in the watch pane.",
    ),
    e(
        &["prefix::PrevWatch"],
        P::Terminal,
        G::Panels,
        "Previous watch",
        "Show the previous sub-agent in the watch pane.",
    ),
    e(
        &["prefix::DismissWatch"],
        P::Terminal,
        G::Panels,
        "Dismiss the watch",
        "Remove a finished sub-agent from the watch pane, or hide the pane while it runs.",
    ),
    // ── Agent thread ──────────────────────────────────────────────────────────────────────
    e(
        &["native_agent::Send"],
        P::AgentThread,
        G::Agents,
        "Send the message",
        "Send what you typed to the agent.",
    )
    .featured(5)
    .short("Send"),
    e(
        &["native_agent::Steer"],
        P::AgentThread,
        G::Agents,
        "Steer the agent",
        "Send a message while the agent works; it reads it straight away.",
    )
    .featured(5)
    .short("Steer"),
    e(
        &["native_agent::SendBackground"],
        P::AgentThread,
        G::Agents,
        "Start the thread in the background",
        "Send the first message and carry on elsewhere while the agent works.",
    ),
    e(
        &["native_agent::Stop"],
        P::AgentThread,
        G::Agents,
        "Stop the agent",
        "Interrupt what the agent is doing now. The thread stays open.",
    )
    .featured(6)
    .short("Stop"),
    e(
        &["native_agent::PlanMode"],
        P::AgentThread,
        G::Agents,
        "Switch between build and plan",
        "In plan mode the agent proposes a plan before it changes anything.",
    )
    .featured(15),
    e(
        &["native_agent::Model"],
        P::AgentThread,
        G::Agents,
        "Switch the agent's model",
        "Pick the model this thread uses.",
    )
    .featured(25)
    .short("Model…"),
    e(
        &["native_agent::Traits"],
        P::AgentThread,
        G::Agents,
        "Change the model's options",
        "Adjust what the selected model offers, such as how hard it thinks.",
    )
    .short("Options…"),
    e(
        &["native_agent::AccessMode"],
        P::AgentThread,
        G::Agents,
        "Change what the agent may do",
        "Choose how much the agent can do without asking you first.",
    )
    .short("Access…"),
    e(
        &["native_agent::History"],
        P::AgentThread,
        G::Agents,
        "Previous suggestion or message",
        "Move up an open suggestion list, or bring back your previous message.",
    ),
    e(
        &["native_agent::HistoryNext"],
        P::AgentThread,
        G::Agents,
        "Next suggestion or message",
        "Move down an open suggestion list, or the cursor down a line.",
    ),
    e(
        &["native_agent::CloseTab"],
        P::AgentThread,
        G::Agents,
        "Close the thread's tab",
        "Close this agent tab; a sub-agent's tab is detached instead.",
    )
    .short("Close tab"),
    e(
        &["native_agent::AllowOnce"],
        P::AgentThread,
        G::Decisions,
        "Allow the agent's request once",
        "Let the agent run this one command or make this one change.",
    )
    .featured(1)
    .short("Allow once"),
    e(
        &["native_agent::AllowSession"],
        P::AgentThread,
        G::Decisions,
        "Allow it for the rest of the session",
        "Stop asking about requests like this one for now.",
    )
    .featured(2)
    // Never "always" (NATIVE-AGENTS §6.2): the grant lasts for this session, and says so.
    .short("Allow for this session"),
    e(
        &["native_agent::Deny"],
        P::AgentThread,
        G::Decisions,
        "Deny the request",
        "Refuse it; the agent carries on without it.",
    )
    .featured(3)
    .short("Deny"),
    e(
        &["native_agent::EditCommand"],
        P::AgentThread,
        G::Decisions,
        "Edit the command first",
        "Change the command the agent wants to run before answering.",
    )
    .short("Edit"),
    e(
        &["native_agent::DenyAndStop"],
        P::AgentThread,
        G::Decisions,
        "Deny and stop the agent",
        "Refuse the request and interrupt what the agent is doing.",
    )
    .featured(4),
    e(
        &[
            "native_agent::Choose1",
            "native_agent::Choose2",
            "native_agent::Choose3",
            "native_agent::Choose4",
            "native_agent::Choose5",
        ],
        P::AgentThread,
        G::Decisions,
        "Choose answer 1–5",
        "Pick one of the options the agent offered.",
    )
    .short("Choose"),
    e(
        &["native_agent::Toggle"],
        P::AgentThread,
        G::Decisions,
        "Tick or untick the option",
        "For a question that takes more than one answer.",
    ),
    e(
        &["native_agent::Answer"],
        P::AgentThread,
        G::Decisions,
        "Send the answer",
        "Answer the question, or go on to the next one.",
    )
    .featured(2),
    e(
        &["native_agent::Previous"],
        P::AgentThread,
        G::Decisions,
        "Previous question",
        "Go back to the question before this one.",
    ),
    e(
        &["native_agent::Implement"],
        P::AgentThread,
        G::Decisions,
        "Go ahead with the plan",
        "Let the agent carry out the plan it proposed.",
    )
    .featured(1)
    .short("Implement"),
    e(
        &["native_agent::Refine"],
        P::AgentThread,
        G::Decisions,
        "Ask for changes to the plan",
        "Write what should change; the agent revises the plan.",
    )
    .featured(2)
    .short("Refine"),
    e(
        &["native_agent::Scroll"],
        P::AgentThread,
        G::Scroll,
        "Scroll the conversation",
        "Freeze the conversation to move through it row by row. Press again to stop.",
    )
    .short("Scroll back"),
    e(
        &["native_agent::ScrollLineDown"],
        P::AgentThread,
        G::Scroll,
        "Next row",
        "Focus the next row of the conversation.",
    ),
    e(
        &["native_agent::ScrollLineUp"],
        P::AgentThread,
        G::Scroll,
        "Previous row",
        "Focus the previous row of the conversation.",
    ),
    e(
        &["native_agent::ScrollHalfPageDown"],
        P::AgentThread,
        G::Scroll,
        "Down half a page",
        "Move half a screen down the conversation.",
    ),
    e(
        &["native_agent::ScrollHalfPageUp"],
        P::AgentThread,
        G::Scroll,
        "Up half a page",
        "Move half a screen up the conversation.",
    ),
    e(
        &["native_agent::ScrollPageDown"],
        P::AgentThread,
        G::Scroll,
        "Down a page",
        "Move a screen down the conversation.",
    ),
    e(
        &["native_agent::ScrollPageUp"],
        P::AgentThread,
        G::Scroll,
        "Up a page",
        "Move a screen up the conversation.",
    ),
    e(
        &["native_agent::ScrollTop"],
        P::AgentThread,
        G::Scroll,
        "Go to the oldest row",
        "Jump to the start of the conversation.",
    ),
    e(
        &["native_agent::ScrollBottom"],
        P::AgentThread,
        G::Scroll,
        "Go to the newest row",
        "Jump to the end of the conversation, still scrolling.",
    ),
    e(
        &["native_agent::ScrollExit"],
        P::AgentThread,
        G::Scroll,
        "Stop scrolling",
        "Go back to the live conversation and follow it again.",
    )
    .featured(4),
    e(
        &["native_agent::ExpandRow"],
        P::AgentThread,
        G::Agents,
        "Expand or collapse the row",
        "Show or hide the focused row's details; on a sub-agent row, open its thread.",
    )
    .featured(1),
    e(
        &["native_agent::DiffRow"],
        P::AgentThread,
        G::Agents,
        "Show the changes",
        "Open the diff of the focused edit or turn.",
    )
    .featured(2),
    e(
        &["native_agent::OpenInEditor"],
        P::AgentThread,
        G::Agents,
        "Open the file in your editor",
        "Open the file the focused row is about.",
    )
    .featured(3),
    e(
        &["native_agent::CopyRow"],
        P::AgentThread,
        G::Agents,
        "Copy the row",
        "Put the focused row's content on the clipboard.",
    ),
    e(
        &["native_agent::Revert"],
        P::AgentThread,
        G::Agents,
        "Undo the agent's file changes",
        "Put the files back as they were before this turn. The conversation is kept.",
    ),
    e(
        &["native_agent::CancelDelegation"],
        P::AgentThread,
        G::Agents,
        "Cancel the sub-agent",
        "Stop the work handed off on the focused row.",
    ),
    e(
        &[
            "native_agent::SelectTab1",
            "native_agent::SelectTab2",
            "native_agent::SelectTab3",
            "native_agent::SelectTab4",
            "native_agent::SelectTab5",
            "native_agent::SelectTab6",
            "native_agent::SelectTab7",
            "native_agent::SelectTab8",
            "native_agent::SelectTab9",
        ],
        P::AgentThread,
        G::Tabs,
        "Go to tab 1–9",
        "Switch to one of the worktree's first nine tabs.",
    )
    .short("Go to tab"),
    e(
        &["native_agent::LastTab"],
        P::AgentThread,
        G::Tabs,
        "Go to the last tab you used",
        "Flip back to the tab you were on before this one.",
    )
    .short("Last tab"),
    e(
        &["native_agent::LastSession"],
        P::AgentThread,
        G::Session,
        "Go to the last session you used",
        "Jump back to the worktree you were in before this one.",
    )
    .short("Last session"),
    // ── Agent window ──────────────────────────────────────────────────────────────────────
    e(
        &["agent::Hide"],
        P::AgentPopup,
        G::Agents,
        "Hide the agent window",
        "Put the floating agent away. It keeps running in fleetd.",
    )
    .featured(10)
    .short("Hide"),
    e(
        &["agent::EnterPrefix"],
        P::AgentPopup,
        G::Terminal,
        "Start a Fleet command",
        "Every Fleet key in the agent window starts with this one; the rest go to the agent.",
    ),
    e(
        &["agent::CopySelection"],
        P::AgentPopup,
        G::Terminal,
        "Copy the selection",
        "Copy the selected text. With nothing selected, the key goes to the agent.",
    )
    .short("Copy"),
    e(
        &["agent::PasteClipboard"],
        P::AgentPopup,
        G::Terminal,
        "Paste the clipboard",
        "Paste into the agent the way it expects.",
    )
    .short("Paste"),
    // ── Scrolling ─────────────────────────────────────────────────────────────────────────
    e(
        &["scroll::LineDown"],
        P::Scroll,
        G::Scroll,
        "Scroll down a line",
        "Move the view one line towards the newest output.",
    ),
    e(
        &["scroll::LineUp"],
        P::Scroll,
        G::Scroll,
        "Scroll up a line",
        "Move the view one line towards the oldest output.",
    ),
    e(
        &["scroll::HalfPageDown"],
        P::Scroll,
        G::Scroll,
        "Scroll down half a page",
        "Move the view half a screen towards the newest output.",
    ),
    e(
        &["scroll::HalfPageUp"],
        P::Scroll,
        G::Scroll,
        "Scroll up half a page",
        "Move the view half a screen towards the oldest output.",
    )
    .featured(30),
    e(
        &["scroll::PageDown"],
        P::Scroll,
        G::Scroll,
        "Scroll down a page",
        "Move the view a screen towards the newest output.",
    ),
    e(
        &["scroll::PageUp"],
        P::Scroll,
        G::Scroll,
        "Scroll up a page",
        "Move the view a screen towards the oldest output.",
    ),
    e(
        &["scroll::Top"],
        P::Scroll,
        G::Scroll,
        "Go to the oldest output",
        "Jump to the start of the history.",
    )
    .featured(40),
    e(
        &["scroll::Bottom"],
        P::Scroll,
        G::Scroll,
        "Go to the newest output",
        "Jump to the live bottom, still scrolling.",
    )
    .featured(45),
    e(
        &["scroll::StartSelection"],
        P::Scroll,
        G::Scroll,
        "Start selecting",
        "Mark where a selection begins; move to extend it.",
    )
    .featured(20),
    e(
        &["scroll::Yank"],
        P::Scroll,
        G::Scroll,
        "Copy the selection",
        "Put the selected text on the clipboard.",
    )
    .featured(25)
    .short("Copy"),
    e(
        &["scroll::Escape"],
        P::Scroll,
        G::Scroll,
        "Clear the selection or stop scrolling",
        "Drop the selection if there is one, otherwise go back to the live terminal.",
    ),
    e(
        &["scroll::Exit"],
        P::Scroll,
        G::Scroll,
        "Stop scrolling",
        "Go back to the live terminal at the bottom.",
    )
    .featured(10),
    e(
        &["scroll::Search"],
        P::Scroll,
        G::Scroll,
        "Search the output",
        "Kept for searching the history, which is not available yet.",
    ),
    e(
        &["scroll::SearchNext"],
        P::Scroll,
        G::Scroll,
        "Next match",
        "Kept for searching the history, which is not available yet.",
    ),
    e(
        &["scroll::SearchPrev"],
        P::Scroll,
        G::Scroll,
        "Previous match",
        "Kept for searching the history, which is not available yet.",
    ),
    // ── Jobs ──────────────────────────────────────────────────────────────────────────────
    e(
        &["jobs::ToggleLog"],
        P::Jobs,
        G::Jobs,
        "Show or hide the job's log",
        "Expand the selected job to read its output as it runs.",
    )
    .short("Show log"),
    e(
        &["jobs::Retry"],
        P::Jobs,
        G::Jobs,
        "Retry the job",
        "Run a failed job again with the same settings.",
    )
    .short("Retry"),
    e(
        &["jobs::CancelJob"],
        P::Jobs,
        G::Jobs,
        "Cancel the job",
        "Stop the selected job, if it can be stopped.",
    )
    .short("Cancel"),
    e(
        &["jobs::CancelAll"],
        P::Jobs,
        G::Jobs,
        "Cancel every job",
        "Stop every job that can be stopped. Asks first.",
    )
    .short("Cancel all")
    .destructive(),
    e(
        &["jobs::DismissFinished"],
        P::Jobs,
        G::Jobs,
        "Clear finished jobs",
        "Remove finished and failed jobs from the list.",
    )
    .short("Clear finished"),
    e(
        &["jobs::CycleFilter"],
        P::Jobs,
        G::Jobs,
        "Change which jobs are shown",
        "Cycle between all, running, failed and done jobs. In an open log, turn following on or off.",
    ),
    e(
        &["jobs::CopyLogPath"],
        P::Jobs,
        G::Jobs,
        "Copy the log's path",
        "Put the selected job's log file path on the clipboard.",
    )
    .short("Copy log path"),
    e(
        &["jobs::MoveDown"],
        P::Jobs,
        G::Jobs,
        "Next job",
        "Select the next job, or scroll down an open log.",
    ),
    e(
        &["jobs::MoveUp"],
        P::Jobs,
        G::Jobs,
        "Previous job",
        "Select the previous job, or scroll up an open log.",
    ),
    e(
        &["jobs::Top"],
        P::Jobs,
        G::Jobs,
        "Go to the first job",
        "Select the first job in the list.",
    ),
    e(
        &["jobs::Bottom"],
        P::Jobs,
        G::Jobs,
        "Go to the last job",
        "Select the last job; in an open log, jump to the end and follow it.",
    )
    .short("Jump to end"),
    e(
        &["jobs::CollapseLog"],
        P::Jobs,
        G::Jobs,
        "Collapse the log",
        "Fold the open log away without closing the panel.",
    )
    .short("Back"),
    e(
        &["jobs::Close"],
        P::Jobs,
        G::Jobs,
        "Close the jobs panel",
        "Go back to exactly where you were.",
    )
    .short("Close"),
    // ── Dialogs ───────────────────────────────────────────────────────────────────────────
    e(
        &["dialog::Confirm"],
        P::Dialog,
        G::Dialog,
        "Confirm",
        "Do what the dialog offers.",
    ),
    e(
        &["dialog::Cancel"],
        P::Dialog,
        G::Dialog,
        "Cancel",
        "Close the dialog without changing anything.",
    ),
    e(
        &["dialog::NextField"],
        P::Dialog,
        G::Dialog,
        "Next field",
        "Move to the dialog's next field.",
    ),
    e(
        &["dialog::PrevField"],
        P::Dialog,
        G::Dialog,
        "Previous field",
        "Move to the dialog's previous field.",
    ),
    e(
        &["dialog::CursorDown"],
        P::Dialog,
        G::Dialog,
        "Next option",
        "Move the selection down the dialog's list.",
    ),
    e(
        &["dialog::CursorUp"],
        P::Dialog,
        G::Dialog,
        "Previous option",
        "Move the selection up the dialog's list.",
    ),
    e(
        &["palette::Run"],
        P::Dialog,
        G::Dialog,
        "Run the highlighted result",
        "Go there, or do it. A command that deletes something still asks first.",
    )
    .short("Run"),
    e(
        &["palette::CursorDown"],
        P::Dialog,
        G::Dialog,
        "Next result",
        "Highlight the palette's next result.",
    ),
    e(
        &["palette::CursorUp"],
        P::Dialog,
        G::Dialog,
        "Previous result",
        "Highlight the palette's previous result.",
    ),
    e(
        &["palette::Close"],
        P::Dialog,
        G::Dialog,
        "Close the palette",
        "Close it without running anything.",
    ),
    e(
        &["confirm::Accept"],
        P::Dialog,
        G::Dialog,
        "Yes, do it",
        "Confirm, when every safety check came back known.",
    )
    .short("Yes"),
    e(
        &["confirm::AcceptStrong"],
        P::Dialog,
        G::Dialog,
        "Yes, do it anyway",
        "Confirm although a safety check is unknown; also the only yes for a repository or context.",
    )
    .short("Yes, anyway")
    .destructive(),
    e(
        &["confirm::Reject"],
        P::Dialog,
        G::Dialog,
        "No, keep it",
        "Cancel; nothing is changed.",
    )
    .short("No"),
    e(
        &["confirm::Recheck"],
        P::Dialog,
        G::Dialog,
        "Check again",
        "Refresh the safety checks before you decide.",
    ),
    e(
        &["confirm::ToggleKeep"],
        P::Dialog,
        G::Dialog,
        "Show or hide what is kept",
        "In a clean-up preview, list the worktrees that will stay.",
    ),
    e(
        &["create_worktree::CreateWithoutOpening"],
        P::Dialog,
        G::Dialog,
        "Create without opening",
        "Create the worktree and stay where you are.",
    ),
    e(
        &["create_worktree::HostPrev"],
        P::Dialog,
        G::Dialog,
        "Previous machine",
        "Pick the machine the worktree is created on.",
    ),
    e(
        &["create_worktree::HostNext"],
        P::Dialog,
        G::Dialog,
        "Next machine",
        "Pick the machine the worktree is created on.",
    ),
    e(
        &["context_dialog::Delete"],
        P::Dialog,
        G::Dialog,
        "Delete this context",
        "Remove the context you are editing from Fleet. Asks first, and needs the strong yes.",
    )
    .destructive(),
    e(
        &["settings::MoveDown"],
        P::Dialog,
        G::Dialog,
        "Next setting",
        "Select the row below.",
    ),
    e(
        &["settings::MoveUp"],
        P::Dialog,
        G::Dialog,
        "Previous setting",
        "Select the row above.",
    ),
    e(
        &["settings::Toggle"],
        P::Dialog,
        G::Dialog,
        "Turn on or off",
        "Flip the selected switch, or tick the highlighted item in a picker.",
    ),
    e(
        &["settings::CycleNext"],
        P::Dialog,
        G::Dialog,
        "Next choice",
        "Change the selected setting to its next value.",
    ),
    e(
        &["settings::CyclePrev"],
        P::Dialog,
        G::Dialog,
        "Previous choice",
        "Change the selected setting to its previous value.",
    ),
    e(
        &["settings::OpenConfigFile"],
        P::Dialog,
        G::Dialog,
        "Open the settings file",
        "Edit config.json in a new terminal tab.",
    ),
    e(
        &["settings::RunDoctor"],
        P::Dialog,
        G::Dialog,
        "Run a health check",
        "Check fleetd and the tools Fleet relies on, and show a report.",
    )
    .short("Run doctor"),
    e(
        &["settings::Search"],
        P::Dialog,
        G::Dialog,
        "Search settings",
        "Find a setting in any section by its name or what it does.",
    ),
    e(
        &["settings::EndSearch"],
        P::Dialog,
        G::Dialog,
        "Clear the settings search",
        "Empty the search and go back to the section's settings.",
    ),
    e(
        &["board_settings::Save"],
        P::Dialog,
        G::Dialog,
        "Save board settings",
        "Keep the changes made to the board.",
    )
    .short("Save"),
    e(
        &["board_settings::NewColumn"],
        P::Dialog,
        G::Dialog,
        "Add a column",
        "Add a column to the board. Nothing changes until you save.",
    ),
    e(
        &["board_settings::DeleteColumn"],
        P::Dialog,
        G::Dialog,
        "Remove the column",
        "Take the focused column out. Nothing changes until you save.",
    ),
    e(
        &["board_settings::MoveColumnDown"],
        P::Dialog,
        G::Dialog,
        "Move the column later",
        "Move the focused column one place to the right.",
    ),
    e(
        &["board_settings::MoveColumnUp"],
        P::Dialog,
        G::Dialog,
        "Move the column earlier",
        "Move the focused column one place to the left.",
    ),
    e(
        &["board_settings::ApplyPreset"],
        P::Dialog,
        G::Dialog,
        "Add the preset's missing columns",
        "Add the columns the workflow preset expects and the board lacks.",
    ),
    e(
        &["board_settings::RunScheduleNow"],
        P::Dialog,
        G::Dialog,
        "Run the schedule now",
        "Run the focused schedule once now, without waiting for its next time.",
    ),
    e(
        &["help::Close"],
        P::Dialog,
        G::Dialog,
        "Close help",
        "Go back to where you were.",
    ),
    e(
        &["help::SwitchTab"],
        P::Dialog,
        G::Dialog,
        "Switch between guides and shortcuts",
        "Show the other half of Help: the task guides, or every shortcut.",
    )
    .short("Switch tab"),
    e(
        &["quit_dialog::Accept"],
        P::Dialog,
        G::Dialog,
        "Quit",
        "Close the app. The work listed keeps running in fleetd.",
    ),
    e(
        &["quit_dialog::Reject"],
        P::Dialog,
        G::Dialog,
        "Don't quit",
        "Stay in Fleet.",
    ),
    e(
        &["quit_dialog::OpenJobs"],
        P::Dialog,
        G::Dialog,
        "Show jobs instead",
        "Look at the running jobs rather than quitting.",
    ),
    e(
        &["quit_dialog::NeverWarn"],
        P::Dialog,
        G::Dialog,
        "Quit and don't ask again",
        "Turn this warning off and quit.",
    ),
    e(
        &["quit_daemon_dialog::Accept"],
        P::Dialog,
        G::Dialog,
        "Stop fleetd and quit",
        "End every terminal, agent and job, then close the app.",
    )
    .destructive(),
    e(
        &["quit_daemon_dialog::Reject"],
        P::Dialog,
        G::Dialog,
        "Keep fleetd running",
        "Cancel; nothing stops.",
    ),
    // ── Open menus (a row's ⋯ or right-click menu, a dropdown's options) ──────────────────
    e(
        &["fleet_menu::SelectNext"],
        P::Dialog,
        G::Dialog,
        "Next menu item",
        "Highlight the next item of the open menu.",
    )
    .short("Next item"),
    e(
        &["fleet_menu::SelectPrevious"],
        P::Dialog,
        G::Dialog,
        "Previous menu item",
        "Highlight the previous item of the open menu.",
    )
    .short("Previous item"),
    e(
        &["fleet_menu::Confirm"],
        P::Dialog,
        G::Dialog,
        "Choose the menu item",
        "Close the menu and do what the highlighted item says.",
    )
    .short("Choose"),
    e(
        &["fleet_menu::Cancel"],
        P::Dialog,
        G::Dialog,
        "Close the menu",
        "Close the open menu without doing anything.",
    )
    .short("Close"),
    // ── Editing text ──────────────────────────────────────────────────────────────────────
    e(
        &["text_input::MoveLeft"],
        P::EditingText,
        G::EditingText,
        "Move left",
        "Move the cursor one character left.",
    ),
    e(
        &["text_input::MoveRight"],
        P::EditingText,
        G::EditingText,
        "Move right",
        "Move the cursor one character right.",
    ),
    e(
        &["text_input::MoveWordLeft"],
        P::EditingText,
        G::EditingText,
        "Move to the previous word",
        "Move the cursor back to the start of a word.",
    ),
    e(
        &["text_input::MoveWordRight"],
        P::EditingText,
        G::EditingText,
        "Move to the next word",
        "Move the cursor forward to the end of a word.",
    ),
    e(
        &["text_input::MoveToRowStart"],
        P::EditingText,
        G::EditingText,
        "Move to the start of the row",
        "Go to the start of the row as it is shown, wrapped.",
    ),
    e(
        &["text_input::MoveToRowEnd"],
        P::EditingText,
        G::EditingText,
        "Move to the end of the row",
        "Go to the end of the row as it is shown, wrapped.",
    ),
    e(
        &["text_input::MoveToLineStart"],
        P::EditingText,
        G::EditingText,
        "Move to the start of the line",
        "Go to the start of the line, ignoring wrapping.",
    ),
    e(
        &["text_input::MoveToLineEnd"],
        P::EditingText,
        G::EditingText,
        "Move to the end of the line",
        "Go to the end of the line, ignoring wrapping.",
    ),
    e(
        &["text_input::MoveUp"],
        P::EditingText,
        G::EditingText,
        "Move up a row",
        "Move the cursor to the row above.",
    ),
    e(
        &["text_input::MoveDown"],
        P::EditingText,
        G::EditingText,
        "Move down a row",
        "Move the cursor to the row below.",
    ),
    e(
        &["text_input::MoveToStart"],
        P::EditingText,
        G::EditingText,
        "Move to the start of the text",
        "Go to the very beginning.",
    ),
    e(
        &["text_input::MoveToEnd"],
        P::EditingText,
        G::EditingText,
        "Move to the end of the text",
        "Go to the very end.",
    ),
    e(
        &["text_input::SelectLeft"],
        P::EditingText,
        G::EditingText,
        "Select left",
        "Extend the selection one character left.",
    ),
    e(
        &["text_input::SelectRight"],
        P::EditingText,
        G::EditingText,
        "Select right",
        "Extend the selection one character right.",
    ),
    e(
        &["text_input::SelectWordLeft"],
        P::EditingText,
        G::EditingText,
        "Select to the previous word",
        "Extend the selection back to the start of a word.",
    ),
    e(
        &["text_input::SelectWordRight"],
        P::EditingText,
        G::EditingText,
        "Select to the next word",
        "Extend the selection forward to the end of a word.",
    ),
    e(
        &["text_input::SelectToRowStart"],
        P::EditingText,
        G::EditingText,
        "Select to the start of the row",
        "Extend the selection to the start of the row as it is shown.",
    ),
    e(
        &["text_input::SelectToRowEnd"],
        P::EditingText,
        G::EditingText,
        "Select to the end of the row",
        "Extend the selection to the end of the row as it is shown.",
    ),
    e(
        &["text_input::SelectToLineStart"],
        P::EditingText,
        G::EditingText,
        "Select to the start of the line",
        "Extend the selection to the start of the line, ignoring wrapping.",
    ),
    e(
        &["text_input::SelectToLineEnd"],
        P::EditingText,
        G::EditingText,
        "Select to the end of the line",
        "Extend the selection to the end of the line, ignoring wrapping.",
    ),
    e(
        &["text_input::SelectUp"],
        P::EditingText,
        G::EditingText,
        "Select up a row",
        "Extend the selection to the row above.",
    ),
    e(
        &["text_input::SelectDown"],
        P::EditingText,
        G::EditingText,
        "Select down a row",
        "Extend the selection to the row below.",
    ),
    e(
        &["text_input::SelectToStart"],
        P::EditingText,
        G::EditingText,
        "Select to the start of the text",
        "Extend the selection to the very beginning.",
    ),
    e(
        &["text_input::SelectToEnd"],
        P::EditingText,
        G::EditingText,
        "Select to the end of the text",
        "Extend the selection to the very end.",
    ),
    e(
        &["text_input::SelectAll"],
        P::EditingText,
        G::EditingText,
        "Select all",
        "Select all of the text in the field.",
    ),
    e(
        &["text_input::Backspace"],
        P::EditingText,
        G::EditingText,
        "Delete the previous character",
        "Delete the character before the cursor, or the selection.",
    ),
    e(
        &["text_input::Delete"],
        P::EditingText,
        G::EditingText,
        "Delete the next character",
        "Delete the character after the cursor, or the selection.",
    ),
    e(
        &["text_input::DeleteWordBackward"],
        P::EditingText,
        G::EditingText,
        "Delete the previous word",
        "Delete back to the start of the word before the cursor.",
    ),
    e(
        &["text_input::DeleteWordForward"],
        P::EditingText,
        G::EditingText,
        "Delete the next word",
        "Delete forward to the end of the word after the cursor.",
    ),
    e(
        &["text_input::DeleteToLineStart"],
        P::EditingText,
        G::EditingText,
        "Delete to the start of the line",
        "Delete everything before the cursor on this line.",
    ),
    e(
        &["text_input::DeleteToLineEnd"],
        P::EditingText,
        G::EditingText,
        "Delete to the end of the line",
        "Delete everything after the cursor on this line.",
    ),
    e(
        &["text_input::Copy"],
        P::EditingText,
        G::EditingText,
        "Copy",
        "Copy the selected text.",
    ),
    e(
        &["text_input::Cut"],
        P::EditingText,
        G::EditingText,
        "Cut",
        "Copy the selected text and remove it.",
    ),
    e(
        &["text_input::Paste"],
        P::EditingText,
        G::EditingText,
        "Paste",
        "Insert the clipboard's text.",
    ),
    e(
        &["text_input::Undo"],
        P::EditingText,
        G::EditingText,
        "Undo",
        "Take back the last change.",
    ),
    e(
        &["text_input::Redo"],
        P::EditingText,
        G::EditingText,
        "Redo",
        "Put back the change you undid.",
    ),
    e(
        &["text_input::Newline"],
        P::EditingText,
        G::EditingText,
        "New line",
        "Start a new line in a field that holds more than one.",
    ),
    // ── fleetd ────────────────────────────────────────────────────────────────────────────
    e(
        &["daemon::Retry"],
        P::Daemon,
        G::Daemon,
        "Try again",
        "Start fleetd again, or close the report and retry.",
    ),
    e(
        &["daemon::Reconnect"],
        P::Daemon,
        G::Daemon,
        "Reconnect now",
        "Try to reach fleetd again without waiting.",
    ),
    e(
        &["daemon::OpenLog"],
        P::Daemon,
        G::Daemon,
        "Open fleetd's log",
        "Read what fleetd wrote before it stopped.",
    ),
    e(
        &["daemon::RunDoctor"],
        P::Daemon,
        G::Daemon,
        "Run a health check",
        "Check fleetd and the tools Fleet relies on, and show a report.",
    )
    .short("Run doctor"),
    e(
        &["daemon::DismissBanner"],
        P::Daemon,
        G::Daemon,
        "Dismiss",
        "Hide the warning or the report. The status dot stays red until fleetd is back.",
    ),
];
