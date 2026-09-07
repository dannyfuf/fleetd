use super::*;

impl Lazygit {
    pub(super) fn pull(&mut self, _: &global::Pull, _: &mut Window, cx: &mut Context<Self>) {
        self.mutate("pull", Mutation::Pull(PullRequest::default()));
        cx.notify();
    }

    pub(super) fn push(&mut self, _: &global::Push, _: &mut Window, cx: &mut Context<Self>) {
        let branch = self.state.head_branch().map(str::to_owned);
        let upstream = self
            .state
            .branches()
            .iter()
            .find(|candidate| candidate.is_head)
            .and_then(|candidate| candidate.upstream.clone());
        match (branch, upstream) {
            (Some(_), Some(_)) => {
                self.mutate("push", Mutation::Push(PushRequest::default()));
            }
            (Some(branch), None) => {
                self.open_prompt(Prompt {
                    title: "Push and set upstream".to_owned(),
                    subtitle: Some(format!("`{branch}` has no upstream. Name the remote.")),
                    buffer: crate::state::Buffer::single_line().with_text("origin"),
                    kind: PromptKind::PushSetUpstream,
                });
            }
            (None, _) => self.toast("HEAD is detached; nothing to push.", Icon::TriangleAlert),
        }
        cx.notify();
    }

    pub(super) fn fetch(&mut self, _: &global::Fetch, _: &mut Window, cx: &mut Context<Self>) {
        self.mutate(
            "fetch",
            Mutation::Fetch(FetchRequest {
                remote: None,
                prune: true,
                all: false,
            }),
        );
        cx.notify();
    }

    pub(super) fn operation_menu(
        &mut self,
        _: &global::OperationMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let operation = self.state.operation();
        let items = match operation {
            OperationState::None => {
                self.toast("Nothing in progress.", Icon::CircleDot);
                cx.notify();
                return;
            }
            OperationState::Rebasing { .. } => vec![
                menu_request(
                    "c",
                    "continue rebase",
                    "rebase continue",
                    Mutation::RebaseContinue,
                ),
                menu_request("s", "skip commit", "rebase skip", Mutation::RebaseSkip),
                menu_request("a", "abort rebase", "rebase abort", Mutation::RebaseAbort),
            ],
            OperationState::Merging => vec![
                menu_request(
                    "c",
                    "continue merge",
                    "merge continue",
                    Mutation::MergeContinue,
                ),
                menu_request("a", "abort merge", "merge abort", Mutation::MergeAbort),
            ],
            OperationState::CherryPicking | OperationState::Reverting => vec![
                menu_request(
                    "c",
                    "continue cherry-pick",
                    "cherry-pick continue",
                    Mutation::CherryPickContinue,
                ),
                menu_request(
                    "a",
                    "abort cherry-pick",
                    "cherry-pick abort",
                    Mutation::CherryPickAbort,
                ),
            ],
            OperationState::Bisecting => {
                self.toast("Bisect is not supported yet.", Icon::TriangleAlert);
                cx.notify();
                return;
            }
        };
        self.open_menu(Menu::new("Merge / rebase options", items));
        cx.notify();
    }
}

/// A menu row that fires one mutation.
pub(super) fn menu_request(
    key: &str,
    label: &str,
    mutation_label: &str,
    mutation: Mutation,
) -> MenuItem {
    MenuItem {
        key: key.to_owned(),
        label: label.to_owned(),
        action: MenuAction::Request(Box::new(GitRequest::Mutate {
            label: mutation_label.to_owned(),
            mutation: Box::new(mutation),
        })),
    }
}
