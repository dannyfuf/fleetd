# Board usage checklist (fleet-board-planning)

Run through the relevant list before the first `fleet board` command of a pass, and the review
list before declaring the pass done. Each item names the rule in `SKILL.md`.

## Before writing to a board

- [ ] I know which board I am writing to and I named it explicitly (`--board`, `--worktree=…`,
      or `--context`), or I am in a worktree terminal or a native agent thread and a bare
      `--worktree` resolves. (Rule 1)
- [ ] If I need a worktree other than the one my terminal or thread belongs to, I passed
      `--worktree=<owner/name#slug>` with the `=`. (Rule 1)
- [ ] I ran `fleet board show` once (human output) and read what is already planned and what is
      in progress before adding anything. (Rules 2, 3)
- [ ] Every card I am about to create is one verifiable outcome with an imperative title and a
      description that says goal, scope, and done-when. (Rule 4)
- [ ] The plan is a handful of cards, not a transcript of steps. (Rule 4)
- [ ] Any label I will use already exists on the board, or I added it once with
      `board set --add-label`. (Rule 10)
- [ ] I am not creating a board just to see whether one exists. (Rule 15)

## Before building a chain (a board that runs its own cards)

- [ ] The plan has real dependencies and every card's brief is complete enough for an agent to
      act on alone; otherwise I am doing this work myself. ("Building a chain")
- [ ] The columns carry their actions and routes already — `columns preset workflow`, then
      `columns edit`, then `columns` read back — before any card can reach one.
- [ ] Every card was created in Todo and every `--blocked-by` / `--blocks` link was written
      **before** the card reached an action column. (Chain rule 1)
- [ ] The head of the chain is the one card I move by hand, or start with `card run`; nothing
      releases a card that nothing blocks. (Chain rule 2)
- [ ] I am watching with `board show` at phase boundaries, not polling; `card wait` is a barrier
      for one run, never for a chain. (Chain rule 4)
- [ ] I am not moving a card that has a live run; `--cancel-run` is the deliberate way to say
      stop it. (Chain rule 5)
- [ ] `board set --max-live-runs N` above 1 is a decision I made about one shared checkout, not a
      default. (Chain rule 6)

## While working

- [ ] I moved a card to `in-progress` when I started it, and I have one card in progress. (Rule 6)
- [ ] I comment decisions, blockers, and hand-offs only, in a few lines. (Rule 7)
- [ ] Edits to one card are one `card edit` with every flag, not several. (Rule 8)
- [ ] I did not follow a mutating command with a `show` to confirm it. (Rule 9)
- [ ] I did not run `board show --json` more than once per phase. (Rule 3)
- [ ] Branch work started from the card with `card worktree`, not with a bare `fleet create`
      beside it. (Rule 11)

## Before moving a card to `done`

- [ ] The verification the card's description names actually ran, and I saw it pass. (Rule 6)
- [ ] If a subagent did the work, I verified its report myself. (Rule 6, With subagents)
- [ ] Anything dropped is `canceled` with a comment or archived, not deleted. (Rule 14)

## Remote-backed boards only

- [ ] I read the real column names with `fleet board describe` before moving cards. (Model 3)
- [ ] I am not syncing after every edit; one `sync --wait` at the end of the pass. (Rule 12)
- [ ] I treated `Last error:` on a successful sync as a warning and read it. (Rule 12)
- [ ] Conflicts were resolved with `card resolve`, not edited around. (Rule 13)
- [ ] I know whether `push_new_cards` is on before expecting a local card to appear remotely.
      (Rule 12)

## Review (when checking someone else's board usage)

- Cards in `done` whose verification did not run → finding.
- More than one card in progress per agent → finding.
- A comment thread used as chat → finding.
- `card delete` on a card that had work recorded → finding.
- A bare `--worktree` in a script that may run outside a worktree terminal or agent thread →
  finding.
- A card that reached an action column before its links were written, or a chain left waiting in
  Ready with no head moved by hand → finding.
