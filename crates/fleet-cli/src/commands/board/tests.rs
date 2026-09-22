//! Board CLI parsing and orchestration tests.

mod parsing {
    use crate::{
        args::{
            AgentChoice, AgentModeChoice, BoardArgs, BoardCardCommand, BoardColumnsCommand,
            BoardColumnsPreset, BoardCommand, BoardConflictPolicy, BoardPriority,
            BoardStatusCategory, BoardWorktreeSelector, Cli, Command,
        },
        envelope::{
            BoardBackendSchemaEnvelope, BoardBackendsEnvelope, BoardCardEnvelope, BoardEnvelope,
            BoardListEnvelope, BoardSyncEnvelope, BoardWorktreeEnvelope, PROTOCOL, to_json,
        },
        human,
    };
    use clap::Parser;
    use fleet_core::{
        board::{
            BackendCapabilities, BackendDescriptor, BackendRef, BackendSchema, BoardView, Card,
            Comment, Label, PropertyKind, PropertySchema, PropertySource, PropertyValue,
            RemoteLink, RemoteStatus, StatusCategory, new_board, summarize,
        },
        model::{Context, Worktree},
    };
    use fleet_proto::job::{JobKind, JobRecord, JobStatus};
    use serde_json::json;

    fn parse(arguments: &[&str]) -> BoardArgs {
        let argv = ["fleet", "board"]
            .into_iter()
            .chain(arguments.iter().copied());
        let Some(Command::Board(arguments)) = Cli::try_parse_from(argv).unwrap().command else {
            panic!("expected board command");
        };
        arguments
    }

    #[test]
    fn parses_board_show_selectors_and_json_at_each_depth() {
        for arguments in [
            vec!["--board", "work", "--json", "show"],
            vec!["show", "--board", "work", "--json"],
        ] {
            let args = parse(&arguments);
            assert!(matches!(args.command, BoardCommand::Show));
            assert_eq!(args.board.unwrap().as_str(), "work");
            assert!(args.json);
        }
        let args = parse(&["show", "--context", "work"]);
        assert_eq!(args.context.unwrap().as_str(), "work");
        assert!(parse(&["show"]).board.is_none());
    }

    #[test]
    fn parses_bare_and_explicit_worktree_selectors_and_their_conflicts() {
        assert_eq!(
            parse(&["--worktree", "show"]).worktree,
            Some(BoardWorktreeSelector::FromSession)
        );
        let args = parse(&["show", "--worktree=acme/api#feature"]);
        assert!(matches!(
            args.worktree,
            Some(BoardWorktreeSelector::Explicit(id)) if id.as_str() == "acme/api#feature"
        ));
        for arguments in [
            ["show", "--worktree=acme/api#feature", "--board", "work"].as_slice(),
            ["show", "--worktree=acme/api#feature", "--context", "work"].as_slice(),
        ] {
            let argv = ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied());
            assert!(Cli::try_parse_from(argv).is_err(), "accepted {arguments:?}");
        }
    }

    #[test]
    fn selector_values_may_match_board_subcommand_names() {
        let board = parse(&["--board", "list", "show"]);
        assert_eq!(board.board.as_ref().map(|id| id.as_str()), Some("list"));
        assert!(matches!(board.command, BoardCommand::Show));

        let context = parse(&["--context", "sync", "show"]);
        assert_eq!(context.context.as_ref().map(|id| id.as_str()), Some("sync"));
        assert!(matches!(context.command, BoardCommand::Show));

        let board = parse(&["--board", "card", "describe"]);
        assert_eq!(board.board.as_ref().map(|id| id.as_str()), Some("card"));
        assert!(matches!(board.command, BoardCommand::Describe));
    }

    #[test]
    fn parses_board_list() {
        let args = parse(&["list", "--json"]);
        assert!(matches!(args.command, BoardCommand::List));
        assert!(args.json);
    }

    #[test]
    fn parses_board_create() {
        let args = parse(&[
            "create",
            "--context",
            "work",
            "--name",
            "Fleet",
            "--prefix",
            "FLT",
            "--backend",
            "local",
            "--json",
        ]);
        assert!(args.json);
        assert_eq!(args.context.unwrap().as_str(), "work");
        let BoardCommand::Create(args) = args.command else {
            panic!("expected create")
        };
        assert_eq!(args.name.as_deref(), Some("Fleet"));
        assert_eq!(args.prefix.as_deref(), Some("FLT"));
        assert_eq!(args.backend.as_deref(), Some("local"));
        assert!(matches!(
            parse(&["create"]).command,
            BoardCommand::Create(_)
        ));
    }

    #[test]
    fn parses_board_set() {
        for (value, policy) in [
            ("manual", BoardConflictPolicy::Manual),
            ("remote_wins", BoardConflictPolicy::RemoteWins),
            ("local_wins", BoardConflictPolicy::LocalWins),
        ] {
            for enabled in ["true", "false"] {
                let args = parse(&[
                    "set",
                    "--name",
                    "Tasks",
                    "--prefix",
                    "TSK",
                    "--default-repo",
                    "acme/api",
                    "--start-on-worktree",
                    enabled,
                    "--conflict-policy",
                    value,
                ]);
                let BoardCommand::Set(args) = args.command else {
                    panic!("expected set")
                };
                assert_eq!(args.name.as_deref(), Some("Tasks"));
                assert_eq!(args.prefix.as_deref(), Some("TSK"));
                assert_eq!(args.default_repo.unwrap().as_str(), "acme/api");
                assert_eq!(args.start_on_worktree, Some(enabled == "true"));
                assert_eq!(args.conflict_policy, Some(policy));
            }
        }
    }

    /// The two board switches spell themselves the way `card edit --archive` does: bare means on,
    /// and the explicit value still works. `ArgAction::Set` made the bare form the clap error "a
    /// value is required" on two of the three booleans in this command group.
    #[test]
    fn board_switches_accept_the_bare_form_and_an_explicit_value() {
        for (arguments, start, push) in [
            (vec!["set", "--start-on-worktree"], Some(true), None),
            (vec!["set", "--push-new-cards"], None, Some(true)),
            (
                vec![
                    "set",
                    "--start-on-worktree",
                    "false",
                    "--push-new-cards",
                    "false",
                ],
                Some(false),
                Some(false),
            ),
            (
                vec!["set", "--push-new-cards", "--prefix", "TSK"],
                None,
                Some(true),
            ),
        ] {
            let BoardCommand::Set(args) = parse(&arguments).command else {
                panic!("expected set")
            };
            assert_eq!(args.start_on_worktree, start, "{arguments:?}");
            assert_eq!(args.push_new_cards, push, "{arguments:?}");
        }
    }

    #[test]
    fn parses_board_sync() {
        assert!(matches!(
            parse(&["sync"]).command,
            BoardCommand::Sync {
                wait: false,
                full: false
            }
        ));
        let args = parse(&["sync", "--wait", "--json"]);
        assert!(matches!(
            args.command,
            BoardCommand::Sync {
                wait: true,
                full: false
            }
        ));
        assert!(args.json);
        // A full sync is the escape hatch from a cursor that has drifted; it must be reachable
        // without also waiting for the job.
        assert!(matches!(
            parse(&["sync", "--full"]).command,
            BoardCommand::Sync {
                wait: false,
                full: true
            }
        ));
        assert!(matches!(
            parse(&["sync", "--wait", "--full"]).command,
            BoardCommand::Sync {
                wait: true,
                full: true
            }
        ));
    }

    fn card_command(arguments: &[&str]) -> BoardCardCommand {
        let args = parse(arguments);
        let BoardCommand::Card(args) = args.command else {
            panic!("expected card")
        };
        args.command
    }

    #[test]
    fn parses_card_new_all_fields_and_priority_values() {
        let command = card_command(&[
            "card",
            "new",
            "Fix login",
            "--desc",
            "Details",
            "--status",
            "in-progress",
            "--priority",
            "urgent",
            "--label",
            "bug",
            "--label",
            "auth",
            "--assignee",
            "Danny",
            "--estimate",
            "3",
            "--due",
            "2026-09-30",
            "--repo",
            "acme/api",
            "--json",
        ]);
        let BoardCardCommand::New { title, fields, .. } = command else {
            panic!("expected new")
        };
        assert_eq!(title, "Fix login");
        assert_eq!(fields.desc.as_deref(), Some("Details"));
        assert_eq!(fields.status.as_deref(), Some("in-progress"));
        assert_eq!(fields.priority, Some(BoardPriority::Urgent));
        assert_eq!(fields.labels, ["bug", "auth"]);
        assert_eq!(fields.assignee.as_deref(), Some("Danny"));
        assert_eq!(fields.estimate, Some(3));
        assert_eq!(fields.due.as_deref(), Some("2026-09-30"));
        assert_eq!(fields.repo.unwrap().as_str(), "acme/api");
        for value in ["urgent", "high", "medium", "low", "none"] {
            assert!(matches!(
                card_command(&["card", "new", "Task", "--priority", value]),
                BoardCardCommand::New { .. }
            ));
        }
    }

    #[test]
    fn parses_card_show_and_propagates_global_flags() {
        let args = parse(&["card", "show", "flt-12", "--context", "work", "--json"]);
        assert!(args.json);
        assert_eq!(args.context.unwrap().as_str(), "work");
        let BoardCommand::Card(args) = args.command else {
            panic!("expected card")
        };
        assert!(matches!(args.command, BoardCardCommand::Show { key } if key == "flt-12"));
    }

    #[test]
    fn parses_card_edit_all_fields() {
        let command = card_command(&[
            "card",
            "edit",
            "FLT-12",
            "--title",
            "Fixed",
            "--desc",
            "New details",
            "--status",
            "done",
            "--priority",
            "none",
            "--label",
            "bug",
            "--assignee",
            "Pat",
            "--estimate",
            "0",
            "--due",
            "2026-10-01",
            "--repo",
            "acme/api",
            "--archive",
        ]);
        let BoardCardCommand::Edit {
            key,
            title,
            fields,
            archive,
            ..
        } = command
        else {
            panic!("expected edit")
        };
        assert_eq!(key, "FLT-12");
        assert_eq!(title.as_deref(), Some("Fixed"));
        assert_eq!(fields.desc.as_deref(), Some("New details"));
        assert_eq!(fields.status.as_deref(), Some("done"));
        assert_eq!(fields.priority, Some(BoardPriority::None));
        assert_eq!(fields.labels, ["bug"]);
        assert_eq!(fields.assignee.as_deref(), Some("Pat"));
        assert_eq!(fields.estimate, Some(0));
        assert_eq!(fields.due.as_deref(), Some("2026-10-01"));
        assert_eq!(fields.repo.unwrap().as_str(), "acme/api");
        assert_eq!(archive, Some(true));
    }

    #[test]
    fn archive_is_a_switch_that_also_restores() {
        for (arguments, expected) in [
            (vec!["card", "edit", "FLT-12", "--archive"], Some(true)),
            (
                vec!["card", "edit", "FLT-12", "--archive", "true"],
                Some(true),
            ),
            // Without `--archive false` an archived card is unreachable from every surface.
            (
                vec!["card", "edit", "FLT-12", "--archive", "false"],
                Some(false),
            ),
            (vec!["card", "edit", "FLT-12", "--title", "T"], None),
        ] {
            let BoardCardCommand::Edit { archive, .. } = card_command(&arguments) else {
                panic!("expected edit")
            };
            assert_eq!(archive, expected, "{arguments:?}");
        }
    }

    #[test]
    fn parses_card_move() {
        assert!(
            matches!(card_command(&["card", "move", "FLT-12", "done", "--index", "0"]), BoardCardCommand::Move { key, status, index: Some(0), .. } if key == "FLT-12" && status == "done")
        );
    }

    #[test]
    fn parses_card_comment() {
        assert!(
            matches!(card_command(&["card", "comment", "FLT-12", "First\nSecond"]), BoardCardCommand::Comment { key, body } if key == "FLT-12" && body == "First\nSecond")
        );
    }

    #[test]
    fn parses_card_delete() {
        assert!(
            matches!(card_command(&["card", "delete", "FLT-12"]), BoardCardCommand::Delete { key } if key == "FLT-12")
        );
    }

    #[test]
    fn parses_card_worktree() {
        let command = card_command(&[
            "card",
            "worktree",
            "FLT-12",
            "--repo",
            "acme/api",
            "--base",
            "origin/main",
            "--host",
            "devbox",
        ]);
        let BoardCardCommand::Worktree {
            key,
            repo,
            base,
            host,
        } = command
        else {
            panic!("expected worktree")
        };
        assert_eq!(key, "FLT-12");
        assert_eq!(repo.unwrap().as_str(), "acme/api");
        assert_eq!(base.as_deref(), Some("origin/main"));
        assert_eq!(host.as_deref(), Some("devbox"));
    }

    #[test]
    fn parses_card_resolve() {
        for resolution in ["keep-local", "take-remote"] {
            assert!(matches!(
                card_command(&["card", "resolve", "FLT-12", resolution]),
                BoardCardCommand::Resolve { .. }
            ));
        }
    }

    /// Every `columns` verb and every flag it takes reach the command tree intact. The
    /// verb-less form is the listing, not a usage error: `fleet board columns` prints a table.
    #[test]
    fn parses_the_columns_family_and_every_flag_its_verbs_take() {
        let columns = |arguments: &[&str]| {
            let argv: Vec<&str> = ["columns"]
                .into_iter()
                .chain(arguments.iter().copied())
                .collect();
            let BoardCommand::Columns(args) = parse(&argv).command else {
                panic!("expected columns")
            };
            args.command
        };
        assert_eq!(columns(&[]), None);

        let Some(BoardColumnsCommand::Add {
            name,
            id,
            category,
            after,
            before,
        }) = columns(&[
            "add",
            "In review",
            "--id",
            "in-review",
            "--category",
            "started",
            "--after",
            "in-progress",
        ])
        else {
            panic!("expected add")
        };
        assert_eq!(name, "In review");
        assert_eq!(id.as_deref(), Some("in-review"));
        assert_eq!(category, Some(BoardStatusCategory::Started));
        assert_eq!(after.as_deref(), Some("in-progress"));
        assert_eq!(before, None);
        // Every category the contract names is spelled the same way on the command line as it
        // is in the document the daemon writes.
        for (value, expected) in [
            ("backlog", BoardStatusCategory::Backlog),
            ("unstarted", BoardStatusCategory::Unstarted),
            ("started", BoardStatusCategory::Started),
            ("completed", BoardStatusCategory::Completed),
            ("canceled", BoardStatusCategory::Canceled),
        ] {
            let Some(BoardColumnsCommand::Add {
                category, before, ..
            }) = columns(&["add", "Column", "--category", value, "--before", "done"])
            else {
                panic!("expected add")
            };
            assert_eq!(category, Some(expected));
            assert_eq!(before.as_deref(), Some("done"));
        }

        let Some(BoardColumnsCommand::Edit {
            id,
            name,
            category,
            color,
            on_enter,
            provider,
            model,
            effort,
            mode,
            instructions,
            instructions_file,
            expect,
            on_success,
            no_on_success,
            when_unblocked,
            no_when_unblocked,
            env,
            clear_env,
        }) = columns(&[
            "edit",
            "in-progress",
            "--name",
            "In Progress",
            "--category",
            "started",
            "--color",
            "#ff8800",
            "--on-enter",
            "skill:deep-review:--fast",
            "--provider",
            "codex",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "high",
            "--mode",
            "accept-edits",
            "--instructions",
            "Implement {key}",
            "--expect",
            "make test is green",
            "--on-success",
            "in-review",
            "--when-unblocked",
            "ready",
            "--env",
            "RUST_LOG=debug",
            "--env",
            "CI=1",
        ])
        else {
            panic!("expected edit")
        };
        assert_eq!(id, "in-progress");
        assert_eq!(name.as_deref(), Some("In Progress"));
        assert_eq!(category, Some(BoardStatusCategory::Started));
        assert_eq!(color.as_deref(), Some("#ff8800"));
        // `--on-enter` is free text the verb parses itself: the CLI never has to know which
        // skills exist, and `skill:<name>:<args>` survives with its colons.
        assert_eq!(on_enter.as_deref(), Some("skill:deep-review:--fast"));
        assert_eq!(provider, Some(AgentChoice::Codex));
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(effort.as_deref(), Some("high"));
        assert_eq!(mode, Some(AgentModeChoice::AcceptEdits));
        assert_eq!(instructions.as_deref(), Some("Implement {key}"));
        assert_eq!(instructions_file, None);
        assert_eq!(expect.as_deref(), Some("make test is green"));
        assert_eq!(on_success.as_deref(), Some("in-review"));
        assert_eq!(when_unblocked.as_deref(), Some("ready"));
        assert_eq!(env, ["RUST_LOG=debug", "CI=1"]);
        assert!(!no_on_success && !no_when_unblocked && !clear_env);

        // The clearing half of the same verb: three switches and the file form of the
        // instructions, none of which can be given beside the value flag it replaces.
        let Some(BoardColumnsCommand::Edit {
            on_enter,
            instructions,
            instructions_file,
            no_on_success,
            no_when_unblocked,
            clear_env,
            env,
            ..
        }) = columns(&[
            "edit",
            "ready",
            "--on-enter",
            "none",
            "--instructions-file",
            "notes/brief.md",
            "--no-on-success",
            "--no-when-unblocked",
            "--clear-env",
        ])
        else {
            panic!("expected edit")
        };
        assert_eq!(on_enter.as_deref(), Some("none"));
        assert_eq!(instructions, None);
        assert_eq!(
            instructions_file.as_deref(),
            Some(std::path::Path::new("notes/brief.md"))
        );
        assert!(no_on_success && no_when_unblocked && clear_env);
        assert!(env.is_empty());

        for (arguments, expected_after, expected_before) in [
            (
                vec!["move", "in-review", "--after", "in-progress"],
                Some("in-progress"),
                None,
            ),
            (
                vec!["move", "in-review", "--before", "done"],
                None,
                Some("done"),
            ),
        ] {
            let Some(BoardColumnsCommand::Move { id, after, before }) = columns(&arguments) else {
                panic!("expected move")
            };
            assert_eq!(id, "in-review");
            assert_eq!(after.as_deref(), expected_after);
            assert_eq!(before.as_deref(), expected_before);
        }

        for (arguments, expected) in [
            (vec!["remove", "ready"], None),
            (
                vec!["remove", "ready", "--move-cards-to", "todo"],
                Some("todo"),
            ),
        ] {
            let Some(BoardColumnsCommand::Remove { id, move_cards_to }) = columns(&arguments)
            else {
                panic!("expected remove")
            };
            assert_eq!(id, "ready");
            assert_eq!(move_cards_to.as_deref(), expected);
        }

        assert_eq!(
            columns(&["preset", "workflow"]),
            Some(BoardColumnsCommand::Preset {
                which: BoardColumnsPreset::Workflow
            })
        );
    }

    /// The five run verbs take a key and nothing else, except `wait`, whose timeout defaults to
    /// the 540 seconds that sit under a caller's own shell-tool timeout.
    #[test]
    fn parses_the_five_run_verbs_and_the_waits_default_timeout() {
        for (arguments, expected) in [
            (
                vec!["card", "run", "FLT-12"],
                BoardCardCommand::Run {
                    key: "FLT-12".into(),
                },
            ),
            (
                vec!["card", "cancel", "flt-12"],
                BoardCardCommand::Cancel {
                    key: "flt-12".into(),
                },
            ),
            (
                vec!["card", "runs", "Card-12"],
                BoardCardCommand::Runs {
                    key: "Card-12".into(),
                },
            ),
            (
                vec!["card", "attach", "FLT-12"],
                BoardCardCommand::Attach {
                    key: "FLT-12".into(),
                },
            ),
            (
                vec!["card", "wait", "FLT-12"],
                BoardCardCommand::Wait {
                    key: "FLT-12".into(),
                    timeout: 540,
                },
            ),
            (
                vec!["card", "wait", "FLT-12", "--timeout", "30"],
                BoardCardCommand::Wait {
                    key: "FLT-12".into(),
                    timeout: 30,
                },
            ),
        ] {
            assert_eq!(card_command(&arguments), expected, "{arguments:?}");
        }
    }

    /// `card new` and `card edit` carry the agent block, the description file and both
    /// directions of a link; `card move` carries the flag that cancels a live run.
    #[test]
    fn parses_the_card_agent_link_and_run_flags() {
        let command = card_command(&[
            "card",
            "new",
            "Ship it",
            "--desc-file",
            "notes/brief.md",
            "--provider",
            "codex",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "high",
            "--blocked-by",
            "FLT-11",
            "--blocked-by",
            "FLT-12",
            "--blocks",
            "FLT-13",
        ]);
        let BoardCardCommand::New {
            title,
            fields,
            blocked_by,
            blocks,
        } = command
        else {
            panic!("expected new")
        };
        assert_eq!(title, "Ship it");
        assert_eq!(
            fields.desc_file.as_deref(),
            Some(std::path::Path::new("notes/brief.md"))
        );
        assert_eq!(fields.desc, None);
        assert_eq!(fields.provider, Some(AgentChoice::Codex));
        assert_eq!(fields.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(fields.effort.as_deref(), Some("high"));
        assert!(!fields.clear_agent);
        assert_eq!(blocked_by, ["FLT-11", "FLT-12"]);
        assert_eq!(blocks, ["FLT-13"]);

        let command = card_command(&[
            "card",
            "edit",
            "FLT-12",
            "--provider",
            "claude",
            "--add-blocked-by",
            "FLT-11",
            "--remove-blocked-by",
            "FLT-10",
            "--add-blocks",
            "FLT-13",
            "--remove-blocks",
            "FLT-14",
        ]);
        let BoardCardCommand::Edit {
            key,
            fields,
            add_blocked_by,
            remove_blocked_by,
            clear_blocked_by,
            add_blocks,
            remove_blocks,
            ..
        } = command
        else {
            panic!("expected edit")
        };
        assert_eq!(key, "FLT-12");
        assert_eq!(fields.provider, Some(AgentChoice::Claude));
        assert_eq!(add_blocked_by, ["FLT-11"]);
        assert_eq!(remove_blocked_by, ["FLT-10"]);
        assert!(!clear_blocked_by);
        assert_eq!(add_blocks, ["FLT-13"]);
        assert_eq!(remove_blocks, ["FLT-14"]);

        // The two flags that stand alone: clearing the agent block, and clearing the blockers.
        let BoardCardCommand::Edit {
            fields,
            clear_blocked_by,
            ..
        } = card_command(&[
            "card",
            "edit",
            "FLT-12",
            "--clear-agent",
            "--clear-blocked-by",
        ])
        else {
            panic!("expected edit")
        };
        assert!(fields.clear_agent);
        assert_eq!(fields.clear_flag(), Some("--clear-agent"));
        assert!(clear_blocked_by);

        assert_eq!(
            card_command(&["card", "move", "FLT-12", "done", "--cancel-run"]),
            BoardCardCommand::Move {
                key: "FLT-12".into(),
                status: "done".into(),
                index: None,
                cancel_run: true,
            }
        );
        let BoardCardCommand::Move { cancel_run, .. } =
            card_command(&["card", "move", "FLT-12", "done"])
        else {
            panic!("expected move")
        };
        assert!(!cancel_run);
    }

    /// The board's run ceiling is an ordinary `board set` flag, so a board that runs more than
    /// one card at a time is one command away.
    #[test]
    fn parses_board_set_max_live_runs() {
        let BoardCommand::Set(args) = parse(&["set", "--max-live-runs", "3"]).command else {
            panic!("expected set")
        };
        assert_eq!(args.max_live_runs, Some(3));
        let BoardCommand::Set(args) = parse(&["set", "--name", "Fleet"]).command else {
            panic!("expected set")
        };
        assert_eq!(args.max_live_runs, None);
    }

    /// Each pair here says two contradictory things about one column, card or run, and clap
    /// refuses it before a request is built — the cheapest place for a caller to learn.
    #[test]
    fn refuses_the_workflow_flag_pairs_that_contradict_each_other() {
        for arguments in [
            // A column cannot land both after and before another one, and a move must say which.
            vec![
                "columns", "add", "Ready", "--after", "todo", "--before", "done",
            ],
            vec!["columns", "move", "ready"],
            vec![
                "columns", "move", "ready", "--after", "todo", "--before", "done",
            ],
            // Instructions come from the flag or from the file, never from both.
            vec![
                "columns",
                "edit",
                "ready",
                "--instructions",
                "Go",
                "--instructions-file",
                "brief.md",
            ],
            // Setting a route and clearing it in one command.
            vec![
                "columns",
                "edit",
                "ready",
                "--on-success",
                "done",
                "--no-on-success",
            ],
            vec![
                "columns",
                "edit",
                "ready",
                "--when-unblocked",
                "done",
                "--no-when-unblocked",
            ],
            // Values the CLI's own vocabulary does not contain.
            vec!["columns", "edit", "ready", "--provider", "gemini"],
            vec!["columns", "edit", "ready", "--mode", "yolo"],
            vec!["columns", "add", "Ready", "--category", "in-progress"],
            vec!["columns", "preset", "kanban"],
            // Every verb that names a column needs one named.
            vec!["columns", "add"],
            vec!["columns", "edit"],
            vec!["columns", "move"],
            vec!["columns", "remove"],
            vec!["columns", "preset"],
            // A description from the flag or from the file, never both.
            vec![
                "card",
                "new",
                "Task",
                "--desc",
                "Go",
                "--desc-file",
                "brief.md",
            ],
            // Clearing the agent block while asking it for something.
            vec![
                "card",
                "new",
                "Task",
                "--provider",
                "claude",
                "--clear-agent",
            ],
            vec!["card", "new", "Task", "--model", "opus", "--clear-agent"],
            vec!["card", "new", "Task", "--effort", "high", "--clear-agent"],
            vec!["card", "new", "Task", "--provider", "gemini"],
            // Clearing the blockers while editing them.
            vec![
                "card",
                "edit",
                "FLT-12",
                "--clear-blocked-by",
                "--add-blocked-by",
                "FLT-11",
            ],
            vec![
                "card",
                "edit",
                "FLT-12",
                "--clear-blocked-by",
                "--remove-blocked-by",
                "FLT-11",
            ],
            // Every run verb names a card, and a timeout is a count of seconds.
            vec!["card", "run"],
            vec!["card", "cancel"],
            vec!["card", "runs"],
            vec!["card", "attach"],
            vec!["card", "wait"],
            vec!["card", "wait", "FLT-12", "--timeout", "-1"],
            vec!["set", "--max-live-runs", "-1"],
            vec!["set", "--max-live-runs", "many"],
        ] {
            let argv = ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied());
            assert!(Cli::try_parse_from(argv).is_err(), "accepted {arguments:?}");
        }
    }

    #[test]
    fn rejects_invalid_board_arguments() {
        for arguments in [
            vec![],
            vec!["card"],
            vec!["card", "new"],
            vec!["card", "show"],
            vec!["card", "edit"],
            vec!["card", "move", "FLT-12"],
            vec!["card", "comment", "FLT-12"],
            vec!["card", "delete"],
            vec!["card", "worktree"],
            vec!["card", "resolve", "FLT-12", "local"],
            vec!["show", "--board", "work", "--context", "personal"],
            vec!["card", "new", "Task", "--priority", "critical"],
            vec!["card", "new", "Task", "--estimate", "-1"],
            vec!["card", "new", "Task", "--repo", "invalid"],
            vec!["card", "move", "FLT-12", "done", "--index", "-1"],
            vec!["set", "--start-on-worktree", "yes"],
            vec!["set", "--conflict-policy", "remote-wins"],
            vec!["show", "--unknown"],
        ] {
            let argv = ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied());
            assert!(Cli::try_parse_from(argv).is_err(), "accepted {arguments:?}");
        }
    }

    fn view() -> BoardView {
        let mut board = new_board(
            &Context {
                id: "work".parse().unwrap(),
                name: "Fleet".into(),
                owners: vec![],
                created_at: "now".into(),
            },
            "now",
        );
        board.prefix = "FLT".into();
        board.labels.push(Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        });
        let card: Card = serde_json::from_value(json!({
            "id": "card-12", "boardId": "work", "number": 12, "title": "Fix login", "statusId": "todo", "priority": "high",
            "labels": ["bug"], "assignee": "Danny", "description": "**Details**\nMore", "createdAt": "now", "updatedAt": "now",
            "comments": [{"id": "comment-1", "author": "Pat", "body": "Looks good", "createdAt": "now"}]
        })).unwrap();
        BoardView {
            board,
            cards: vec![card],
            live_runs: Vec::new(),
        }
    }

    #[test]
    fn board_and_card_envelopes_have_exact_protocol_one_shapes() {
        let view = view();
        let summaries = vec![summarize(
            &view.board,
            &view.cards,
            &[],
            "2026-09-06T12:00:00Z",
        )];
        for (actual, expected) in [
            (
                to_json(&BoardEnvelope {
                    protocol: PROTOCOL,
                    board: &view.board,
                    cards: &view.cards,
                    live_runs: &view.live_runs,
                })
                .unwrap(),
                json!({"protocol": 1, "board": view.board, "cards": view.cards}),
            ),
            (
                to_json(&BoardListEnvelope {
                    protocol: PROTOCOL,
                    boards: &summaries,
                })
                .unwrap(),
                json!({"protocol": 1, "boards": summaries}),
            ),
            (
                to_json(&BoardCardEnvelope {
                    protocol: PROTOCOL,
                    card: &view.cards[0],
                })
                .unwrap(),
                json!({"protocol": 1, "card": view.cards[0]}),
            ),
        ] {
            assert_eq!(actual.lines().count(), 1);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn board_worktree_envelope_preserves_create_fields() {
        let view = view();
        let worktree: Worktree = serde_json::from_value(json!({
            "id": "acme/api#flt-12", "repoId": "acme/api", "slug": "flt-12", "branch": "flt-12", "baseRef": "main", "path": "/tmp/flt-12", "session": "api/flt-12", "createdAt": "now"
        })).unwrap();
        let actual = serde_json::to_value(BoardWorktreeEnvelope {
            protocol: PROTOCOL,
            created: true,
            card: &view.cards[0],
            worktree: &worktree,
        })
        .unwrap();
        assert_eq!(
            actual,
            json!({"protocol": 1, "created": true, "card": view.cards[0], "worktree": worktree})
        );
    }

    #[test]
    fn board_sync_envelopes_cover_submitted_and_completed_jobs() {
        let job = JobRecord {
            id: "sync-1".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: "work".into(),
            title: "Sync".into(),
            status: JobStatus::Succeeded,
            progress: Some("pulled 2".into()),
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: Some("later".into()),
            cancellable: false,
            retryable: false,
        };
        let view = view();
        let summary = summarize(&view.board, &view.cards, &[], "2026-09-06T12:00:00Z");
        assert_eq!(
            to_json(&BoardSyncEnvelope {
                protocol: PROTOCOL,
                job_id: &job.id,
                job: None,
                summary: None
            })
            .unwrap(),
            r#"{"protocol":1,"jobId":"sync-1"}"#
        );
        assert_eq!(
            serde_json::to_value(BoardSyncEnvelope {
                protocol: PROTOCOL,
                job_id: &job.id,
                job: Some(&job),
                summary: Some(&summary)
            })
            .unwrap(),
            json!({"protocol":1,"jobId":"sync-1","job":job,"summary":summary})
        );
        assert!(human::board_sync(&job, &summary).contains("pulled 2"));
    }

    #[test]
    fn human_board_groups_orders_and_hides_archived_cards() {
        let mut view = view();
        let mut first = view.cards[0].clone();
        first.title = "First".into();
        first.position = 0;
        first.number = 1;
        view.cards[0].position = 10;
        let mut archived = first.clone();
        archived.archived = true;
        archived.title = "Hidden".into();
        view.cards.extend([first, archived]);
        let text = human::board(&view, None, 0);
        assert!(text.contains("FLT-12  high  Fix login  [Bug]  @Danny"));
        assert!(text.find("FLT-1 ").unwrap() < text.find("FLT-12 ").unwrap());
        assert!(!text.contains("Hidden"));
        assert!(text.find("Backlog").unwrap() < text.find("Todo").unwrap());
    }

    #[test]
    fn human_card_shows_properties_description_and_comments() {
        let mut view = view();
        view.cards[0]
            .properties
            .insert("score".into(), PropertyValue::Number(2.5));
        let text = human::board_card(&view.board, &view.cards, &view.cards[0]);
        for expected in [
            "FLT-12  Fix login",
            "Status: Todo",
            "Priority: High",
            "score: 2.5",
            "**Details**\nMore",
            "@Pat\nLooks good",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
    }

    #[test]
    fn an_empty_label_list_prints_the_em_dash_every_other_empty_field_uses() {
        let mut view = view();
        view.cards[0].labels.clear();
        let text = human::board_card(&view.board, &view.cards, &view.cards[0]);
        assert!(text.contains("Labels: \u{2014}"), "{text:?}");
    }

    /// The board listing is not the one-per-line report `card_labels` writes its em dash for: the
    /// bracket only belongs there when the card has labels, and testing the *string* printed
    /// `[\u{2014}]` beside every unlabeled card on the board.
    #[test]
    fn an_unlabeled_card_carries_no_label_bracket_in_the_board_listing() {
        let mut view = view();
        view.cards[0].labels.clear();
        let text = human::board(&view, None, 0);
        assert!(text.contains("FLT-12"), "{text:?}");
        assert!(!text.contains('['), "{text:?}");
        // A card that does have one still shows it.
        view.cards[0].labels = vec!["bug".parse().unwrap()];
        assert!(human::board(&view, None, 0).contains("[Bug]"));
    }

    /// A backend names its properties whatever it likes, and Jira calls one of them `Created` —
    /// which is also a row of this report, holding a different date. Two `Created:` rows with no
    /// way to tell them apart is worse than a row named by its key.
    #[test]
    fn a_property_whose_name_collides_with_a_row_answers_to_its_key() {
        let mut view = view();
        view.board.properties.push(PropertySchema {
            key: "jira.created".into(),
            name: "Created".into(),
            kind: PropertyKind::Date,
            options: vec![],
            editable: false,
            source: PropertySource::Backend,
            show_on_card: false,
        });
        view.cards[0].properties.insert(
            "jira.created".into(),
            PropertyValue::Date("2026-08-30".into()),
        );
        let text = human::board_card(&view.board, &view.cards, &view.cards[0]);
        assert!(text.contains("jira.created: 2026-08-30"), "{text:?}");
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("Created:"))
                .count(),
            1,
            "{text:?}"
        );
    }

    #[test]
    fn a_parent_is_named_by_the_key_the_user_types() {
        let mut view = view();
        let parent = view.cards[0].clone();
        let mut child = parent.clone();
        child.id = "child".parse().unwrap();
        child.number = 13;
        child.parent_id = Some(parent.id.clone());
        view.cards.push(child.clone());
        let text = human::board_card(&view.board, &view.cards, &child);
        assert!(text.contains("Parent: FLT-12"), "{text:?}");
        // A parent the view no longer carries still says something a user can look up.
        let text = human::board_card(&view.board, &view.cards[1..], &child);
        assert!(text.contains(&format!("Parent: {}", parent.id)), "{text:?}");
    }

    #[test]
    fn a_card_title_cannot_repaint_the_terminal_that_prints_it() {
        let mut view = view();
        view.cards[0].title = "Evil \u{1b}[31mRED\u{1b}[0m title".into();
        view.cards[0].assignee = Some("\u{1b}]0;pwned\u{7}Danny".into());
        view.cards[0].description = "Fine\n\u{1b}[2JCleared".into();
        view.cards[0].comments[0].body = "Hi\u{1b}[31m".into();
        for text in [
            human::board(&view, None, 0),
            human::board_card(&view.board, &view.cards, &view.cards[0]),
        ] {
            assert!(!text.contains('\u{1b}'), "{text:?}");
            assert!(text.contains("Evil [31mRED[0m title"), "{text:?}");
        }
    }

    #[test]
    fn nothing_a_backend_names_reaches_the_terminal_unsanitised() {
        let mut view = view();
        // Everything a remote chooses: the issue key that heads every row, the column and
        // board names it maps, its own timestamps, and the free text it parks in a property.
        view.board.name = "Fleet \u{1b}[2J".into();
        view.board.statuses[0].name = "To do \u{1b}[31m".into();
        view.board.properties.push(PropertySchema {
            key: "sprint".into(),
            name: "Sprint \u{1b}[7m".into(),
            kind: PropertyKind::Text,
            options: vec![],
            editable: true,
            show_on_card: true,
            source: PropertySource::Backend,
        });
        view.cards[0].remote = Some(RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: "SP-4\u{1b}[2K".into(),
            url: Some("https://example.test/\u{1b}]0;x\u{7}".into()),
            version: None,
            synced_at: "now\u{1b}[1A".into(),
            remote_updated_at: None,
        });
        view.cards[0].properties.insert(
            "sprint".into(),
            PropertyValue::Text("Sprint 9 \u{1b}[5m".into()),
        );
        let mut summary = summarize(&view.board, &view.cards, &[], "2026-09-06T12:00:00Z");
        summary.last_error = Some("acli said \u{1b}[2Jboom".into());
        let job = JobRecord {
            id: "sync-1".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: "work".into(),
            title: "Sync".into(),
            status: JobStatus::Succeeded,
            progress: Some("pulled 2 \u{1b}[31m".into()),
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: Some("later".into()),
            cancellable: false,
            retryable: false,
        };
        for text in [
            human::board(&view, None, 0),
            human::board_card(&view.board, &view.cards, &view.cards[0]),
            human::boards(&[summary.clone()]),
            human::board_sync(&job, &summary),
        ] {
            assert!(!text.contains('\u{1b}'), "{text:?}");
        }
    }

    #[test]
    fn board_list_pads_every_column_to_the_widest_cell() {
        let view = view();
        let mut short = summarize(&view.board, &view.cards, &[], "2026-09-06T12:00:00Z");
        short.id = "ops".parse().unwrap();
        short.name = "Ops".into();
        let mut long = short.clone();
        long.id = "platform-migration".parse().unwrap();
        long.name = "Platform migration".into();
        long.last_error = Some("acli timed out".into());

        let text = human::boards(&[short, long]);
        let lines: Vec<&str> = text.lines().collect();
        // Header, then one row per board, each starting its second column at the same offset.
        assert_eq!(lines.len(), 3);
        let context = lines[0].find("CONTEXT").unwrap();
        for line in &lines[1..] {
            assert_eq!(line.find("work"), Some(context), "{line:?}");
        }
        // The trailing error cell lands past the last counter column, not glued to it.
        assert!(
            lines[2].ends_with("  error: acli timed out"),
            "{:?}",
            lines[2]
        );
        // Padding never leaks past the last cell a row carries.
        for line in &lines {
            assert_eq!(*line, line.trim_end(), "{line:?}");
        }
    }

    #[test]
    fn board_list_names_context_and_worktree_scopes() {
        let view = view();
        let context = summarize(&view.board, &view.cards, &[], "2026-09-06T12:00:00Z");
        let mut worktree = context.clone();
        worktree.id = "wt-feature".parse().unwrap();
        worktree.worktree_id = Some("acme/api#feature".parse().unwrap());
        let text = human::boards(&[context, worktree]);
        assert!(text.lines().next().unwrap().contains("SCOPE"), "{text}");
        assert!(text.lines().nth(1).unwrap().contains("context"), "{text}");
        assert!(
            text.lines().nth(2).unwrap().contains("acme/api#feature"),
            "{text}"
        );
    }

    #[test]
    fn worktree_board_header_names_its_worktree() {
        let mut view = view();
        view.board.worktree_id = Some("acme/api#feature".parse().unwrap());
        assert!(
            human::board(&view, None, 0)
                .lines()
                .next()
                .unwrap()
                .contains("worktree acme/api#feature")
        );
    }

    #[test]
    fn card_header_stays_one_line_and_an_author_less_comment_names_nobody() {
        let mut view = view();
        view.cards[0].title = "Fix login\nand logout".into();
        let comment = view.cards[0].comments[0].clone();
        view.cards[0].comments.push(Comment {
            id: "local".into(),
            author: None,
            body: "Mine".into(),
            ..comment
        });
        let text = human::board_card(&view.board, &view.cards, &view.cards[0]);
        assert!(text.starts_with("FLT-12  Fix login and logout\n"));
        // The daemon never sets an author on a locally written comment; "@unknown" invents one.
        assert!(!text.contains("@unknown"));
        assert!(text.contains("@Pat"));
    }

    #[test]
    fn parses_board_backends_and_describe() {
        assert!(matches!(
            parse(&["backends"]).command,
            BoardCommand::Backends
        ));
        let args = parse(&["backends", "--json"]);
        assert!(args.json);
        assert!(matches!(
            parse(&["describe", "--board", "work"]).command,
            BoardCommand::Describe
        ));
        let args = parse(&["describe", "--context", "work", "--json"]);
        assert!(matches!(args.command, BoardCommand::Describe));
        assert_eq!(args.context.unwrap().as_str(), "work");
        assert!(args.json);
    }

    #[test]
    fn parses_board_set_backend_and_repeated_settings() {
        let args = parse(&[
            "set",
            "--backend",
            "jira",
            "--setting",
            "project=SP",
            "--setting",
            "jql=sprint in openSprints()",
            "--setting",
            "maxConcurrency=8",
        ]);
        let BoardCommand::Set(args) = args.command else {
            panic!("expected set")
        };
        assert_eq!(args.backend.as_deref(), Some("jira"));
        assert_eq!(
            args.settings,
            [
                ("project".to_owned(), "SP".to_owned()),
                // Only the first `=` separates: a JQL clause is full of them.
                ("jql".to_owned(), "sprint in openSprints()".to_owned()),
                ("maxConcurrency".to_owned(), "8".to_owned()),
            ]
        );
        // `--setting` alone is the merge-into-current-settings form.
        let BoardCommand::Set(args) = parse(&["set", "--setting", "jql="]).command else {
            panic!("expected set")
        };
        assert!(args.backend.is_none());
        assert_eq!(args.settings, [("jql".to_owned(), String::new())]);
    }

    #[test]
    fn parses_board_create_settings_which_require_a_backend() {
        let BoardCommand::Create(args) = parse(&[
            "create",
            "--backend",
            "jira",
            "--setting",
            "project=SP",
            "--setting",
            "site=example.atlassian.net",
        ])
        .command
        else {
            panic!("expected create")
        };
        assert_eq!(args.backend.as_deref(), Some("jira"));
        assert_eq!(
            args.settings,
            [
                ("project".to_owned(), "SP".to_owned()),
                ("site".to_owned(), "example.atlassian.net".to_owned()),
            ]
        );
        let BoardCommand::Create(args) = parse(&["create", "--name", "Fleet"]).command else {
            panic!("expected create")
        };
        assert!(args.settings.is_empty());
    }

    #[test]
    fn rejects_malformed_settings_and_a_backendless_create_setting() {
        for arguments in [
            // A bare word is not a pair, and a pair without a key names nothing.
            vec!["set", "--setting", "project"],
            vec!["set", "--setting", "=SP"],
            vec!["set", "--setting", " =SP"],
            vec!["set", "--setting"],
            // `create --setting` without `--backend` would silently configure the local backend.
            vec!["create", "--setting", "project=SP"],
            vec!["describe", "--extra"],
            vec!["backends", "extra"],
            vec!["sync", "--full", "yes"],
        ] {
            let argv = ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied());
            assert!(Cli::try_parse_from(argv).is_err(), "accepted {arguments:?}");
        }
    }

    fn descriptors() -> Vec<BackendDescriptor> {
        vec![
            BackendDescriptor {
                kind: "local".into(),
                label: "Local".into(),
                capabilities: BackendCapabilities::default(),
                settings_schema: vec![],
            },
            BackendDescriptor {
                kind: "jira".into(),
                label: "Jira (acli)".into(),
                capabilities: BackendCapabilities {
                    pull: true,
                    push_updates: true,
                    push_create: true,
                    transitions: true,
                    comments: true,
                    custom_properties: true,
                    incremental: true,
                },
                settings_schema: vec![
                    serde_json::from_value(json!({
                        "key": "project", "name": "Project key (required)", "kind": "text"
                    }))
                    .unwrap(),
                    serde_json::from_value(json!({"key": "site", "name": "Site", "kind": "text"}))
                        .unwrap(),
                ],
            },
        ]
    }

    fn schema() -> BackendSchema {
        BackendSchema {
            statuses: vec![
                RemoteStatus {
                    id: "To Do".into(),
                    name: "To Do".into(),
                    category: Some(StatusCategory::Unstarted),
                },
                RemoteStatus {
                    id: "In Progress".into(),
                    name: "In Progress".into(),
                    category: None,
                },
            ],
            labels: vec!["bug".into()],
            properties: vec![
                serde_json::from_value(json!({
                    "key": "jira.issue_type", "name": "Issue type", "kind": "select",
                    "editable": false, "source": "backend"
                }))
                .unwrap(),
            ],
            assignees: vec!["Danny".into(), "Pat".into()],
            key_prefix: Some("SP".into()),
            readonly_fields: vec!["priority".into(), "estimate".into()],
        }
    }

    #[test]
    fn backend_envelopes_have_exact_protocol_one_shapes() {
        let backends = descriptors();
        let schema = schema();
        for (actual, expected) in [
            (
                to_json(&BoardBackendsEnvelope {
                    protocol: PROTOCOL,
                    backends: &backends,
                })
                .unwrap(),
                json!({"protocol": 1, "backends": backends}),
            ),
            (
                to_json(&BoardBackendSchemaEnvelope {
                    protocol: PROTOCOL,
                    schema: &schema,
                })
                .unwrap(),
                json!({"protocol": 1, "schema": schema}),
            ),
        ] {
            assert_eq!(actual.lines().count(), 1);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
                expected
            );
        }
        // The settings keys the CLI documents are the ones `--setting` writes into `BackendRef`.
        assert_eq!(
            json!({"protocol": 1, "backends": backends})["backends"][1]["settingsSchema"][0]["key"],
            "project"
        );
    }

    #[test]
    fn human_backends_table_lists_capabilities_and_setting_keys() {
        let text = human::board_backends(&descriptors());
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "KIND  LABEL  CAPABILITIES  SETTINGS");
        // A backend that can do nothing prints the em dash, not an empty column.
        assert_eq!(lines[1], "local  Local  \u{2014}  \u{2014}");
        assert!(lines[2].starts_with("jira  Jira (acli)  pull, push-updates, push-create"));
        // The key is what `--setting k=v` takes; the name is the only place a backend can say a
        // key is mandatory, and the CLI used to drop it.
        assert!(
            lines[2].ends_with("  project (Project key (required)), site (Site)"),
            "{}",
            lines[2]
        );
        assert_eq!(
            human::board_backends(&[]),
            "KIND  LABEL  CAPABILITIES  SETTINGS"
        );
    }

    #[test]
    fn human_describe_reports_statuses_properties_and_read_only_fields() {
        let text = human::board_backend_schema(&schema());
        for expected in [
            "Key prefix: SP",
            "Read-only fields: priority, estimate",
            "To Do  To Do  unstarted",
            // A status the backend does not categorize is not silently called "backlog".
            "In Progress  In Progress  \u{2014}",
            "jira.issue_type  Issue type  select  read-only",
            "Labels: bug",
            "Assignees: Danny, Pat",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let empty = human::board_backend_schema(&BackendSchema::default());
        assert!(empty.contains("Key prefix: \u{2014}"), "{empty}");
        assert!(empty.contains("Read-only fields: \u{2014}"), "{empty}");
    }

    #[test]
    fn the_board_header_names_the_backend_and_dates_the_last_sync() {
        let mut jira = view();
        jira.board.backend = BackendRef {
            kind: "jira".into(),
            settings: json!({"project": "SP", "site": "example.atlassian.net", "maxConcurrency": 4}),
        };
        jira.board.sync.last_synced_at = Some("2026-09-06T11:57:00Z".into());
        jira.cards[0].dirty = true;
        let now = 1_788_696_000; // 2026-09-06T12:00:00Z
        let jira_backend = descriptors()
            .into_iter()
            .find(|descriptor| descriptor.kind == "jira")
            .unwrap();
        let local_backend = descriptors()
            .into_iter()
            .find(|descriptor| descriptor.kind == "local")
            .unwrap();
        let header = human::board(&jira, Some(&jira_backend), now)
            .lines()
            .nth(1)
            .unwrap()
            .to_owned();
        assert_eq!(
            header,
            "backend: Jira (acli) \u{b7} project SP \u{b7} site example.atlassian.net \u{b7} synced 3m ago \u{b7} 1 dirty \u{b7} 0 conflict"
        );
        // Without a descriptor list the raw registry kind is still an honest answer — and which
        // settings identify a board is the backend's answer, so a daemon that cannot list its
        // backends leaves the header naming none of them rather than guessing at keys.
        assert_eq!(
            human::board(&jira, None, now).lines().nth(1).unwrap(),
            "backend: jira \u{b7} synced 3m ago \u{b7} 1 dirty \u{b7} 0 conflict"
        );
        // A sync that is failing says so here too: `board list` reported it and `board show` did
        // not, so a board broken for days printed a clean header with a stale stamp.
        jira.board.sync.last_error = Some("acli is not authenticated".into());
        let failing = human::board(&jira, Some(&jira_backend), now);
        assert!(
            failing
                .lines()
                .nth(1)
                .unwrap()
                .ends_with("\u{b7} error: acli is not authenticated"),
            "{failing}"
        );
        jira.board.sync.last_error = None;
        // A local board has no remote to be behind: no sync stamp, no dirty or conflict counts.
        let local = view();
        assert_eq!(
            human::board(&local, Some(&local_backend), now)
                .lines()
                .nth(1),
            Some("backend: Local")
        );
    }

    /// A card whose column the board no longer has is counted by `board list` and drawn as an
    /// error row by the app; printing the columns alone made it invisible on the one surface that
    /// was asked to show the board.
    #[test]
    fn a_card_whose_status_is_no_longer_a_column_is_still_printed() {
        let mut orphaned = view();
        orphaned.cards[0].status_id = "archived-column".parse().unwrap();
        let text = human::board(&orphaned, None, 1_788_696_000);
        assert!(text.contains("No column (1)"), "{text}");
        assert!(text.contains("FLT-12"), "{text}");
        assert!(text.contains("[status archived-column]"), "{text}");
        // A card in a real column is printed under it, not twice.
        let normal = human::board(&view(), None, 1_788_696_000);
        assert!(!normal.contains("No column"), "{normal}");
    }

    /// §10: the header names the settings the *backend* declares, so a second backend's identity
    /// settings reach it without a line of client code.
    #[test]
    fn the_board_header_names_whatever_settings_the_backend_declares() {
        let mut board = view();
        board.board.backend = BackendRef {
            kind: "notion".into(),
            settings: json!({"database": "Roadmap", "workspace": "acme"}),
        };
        let descriptor = BackendDescriptor {
            kind: "notion".into(),
            label: "Notion".into(),
            capabilities: BackendCapabilities::default(),
            settings_schema: vec![
                serde_json::from_value(
                    json!({"key": "database", "name": "Database", "kind": "text"}),
                )
                .unwrap(),
                serde_json::from_value(
                    json!({"key": "workspace", "name": "Workspace", "kind": "text"}),
                )
                .unwrap(),
            ],
        };
        let header = human::board(&board, Some(&descriptor), 1_788_696_000)
            .lines()
            .nth(1)
            .unwrap()
            .to_owned();
        assert!(
            header.starts_with("backend: Notion \u{b7} database Roadmap \u{b7} workspace acme"),
            "{header}"
        );
    }

    #[test]
    fn a_board_that_never_synced_says_so_and_an_unreadable_stamp_is_printed_as_is() {
        let now = 1_788_696_000;
        assert_eq!(human::synced_age(None, now), "never synced");
        assert_eq!(
            human::synced_age(Some("2026-09-06T11:57:00Z"), now),
            "synced 3m ago"
        );
        // Offsets are honored, and a clock skew is not reported as a sync from the future.
        assert_eq!(
            human::synced_age(Some("2026-09-06T08:57:00-03:00"), now),
            "synced 3m ago"
        );
        assert_eq!(
            human::synced_age(Some("2026-09-07T00:00:00Z"), now),
            "synced 0s ago"
        );
        for (stamp, expected) in [
            ("2026-09-06T11:00:00Z", "synced 1h ago"),
            ("2026-09-04T12:00:00Z", "synced 2d ago"),
            ("2026-08-16T12:00:00Z", "synced 3w ago"),
        ] {
            assert_eq!(human::synced_age(Some(stamp), now), expected);
        }
        assert_eq!(human::synced_age(Some("now"), now), "synced now");
    }

    #[test]
    fn board_help_lists_the_new_commands_and_their_flags() {
        let help = |arguments: &[&str]| {
            Cli::try_parse_from(
                ["fleet", "board"]
                    .into_iter()
                    .chain(arguments.iter().copied()),
            )
            .unwrap_err()
            .to_string()
        };
        let text = help(&["--help"]);
        assert!(
            text.contains("Manage context and worktree boards and their cards"),
            "{text}"
        );
        for expected in ["backends", "describe", "sync"] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let text = help(&["set", "--help"]);
        for expected in ["--backend", "--setting <KEY=VALUE>"] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let text = help(&["create", "--help"]);
        assert!(text.contains("--setting <KEY=VALUE>"), "{text}");
        assert!(help(&["sync", "--help"]).contains("--full"));

        // The workflow surface: the columns family, its verbs, the run verbs and the flags a
        // chain is built with. Help is the only place a caller with no docs open can find them.
        let text = help(&["--help"]);
        assert!(text.contains("columns"), "{text}");
        let text = help(&["columns", "--help"]);
        for expected in ["add", "edit", "move", "remove", "preset"] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let text = help(&["columns", "edit", "--help"]);
        for expected in [
            "--on-enter <ACTION>",
            "--instructions-file",
            "--expect",
            "--on-success",
            "--no-on-success",
            "--when-unblocked",
            "--no-when-unblocked",
            "--env <KEY=VALUE>",
            "--clear-env",
            "--provider",
            "--model",
            "--effort",
            "--mode",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(
            help(&["columns", "add", "--help"]).contains("--category"),
            "{text}"
        );
        assert!(
            help(&["columns", "remove", "--help"]).contains("--move-cards-to <ID>"),
            "{text}"
        );
        let text = help(&["card", "--help"]);
        for expected in ["run", "cancel", "runs", "attach", "wait"] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let text = help(&["card", "new", "--help"]);
        for expected in [
            "--blocked-by <KEY>",
            "--blocks <KEY>",
            "--desc-file",
            "--provider",
            "--model",
            "--effort",
            "--clear-agent",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        let text = help(&["card", "edit", "--help"]);
        for expected in [
            "--add-blocked-by <KEY>",
            "--remove-blocked-by <KEY>",
            "--clear-blocked-by",
            "--add-blocks <KEY>",
            "--remove-blocks <KEY>",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(help(&["card", "move", "--help"]).contains("--cancel-run"));
        assert!(help(&["card", "wait", "--help"]).contains("--timeout"));
        assert!(help(&["set", "--help"]).contains("--max-live-runs"));
    }
}

mod orchestration {
    use crate::args::{Cli, Command};
    use crate::commands::board::*;
    use clap::Parser;
    use fleet_core::{
        agents::{
            AgentKind, AgentThreadSummary, Attention, PermissionMode, Seq, SessionState, TurnState,
        },
        board::{
            Action, ActionKind, ColumnAgentPrefs, ColumnAutomation, RemoteLink, Status,
            StatusCategory, apply_workflow_preset, new_board, workflow_preset,
        },
        config::Agent,
        model::Context,
        sessions::{Session, SessionKind},
    };
    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        job::JobKind,
        request::{Request, RequestBody},
        response::{
            BOARD_AUTOMATION_CAPABILITY, BOARD_WORKTREE_CAPABILITY, HelloResponse, Response,
            ResponseBody,
        },
        snapshot::Snapshot,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;
    use tokio::{net::UnixListener, time::timeout};
    use tokio_util::codec::Framed;

    fn view() -> BoardView {
        let context = Context {
            id: "work".parse().unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "now".into(),
        };
        let mut board = new_board(&context, "now");
        board.prefix = "FLT".into();
        let card = serde_json::from_value(json!({"id":"Card-12","boardId":"work","number":12,"title":"Fix login","statusId":"todo","createdAt":"now","updatedAt":"now"})).unwrap();
        BoardView {
            board,
            cards: vec![card],
            live_runs: Vec::new(),
        }
    }

    fn args(arguments: &[&str]) -> BoardArgs {
        let Some(Command::Board(args)) = Cli::try_parse_from(
            ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )
        .unwrap()
        .command
        else {
            panic!("expected board")
        };
        args
    }

    fn empty_snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "generatedAt": "now", "contexts": [], "repos": [], "clones": [],
            "worktrees": [], "sessions": [], "statuses": [], "jobs": [],
            "daemon": {"version": "test", "pid": 1, "startedAt": "now", "home": "/tmp"}
        }))
        .unwrap()
    }

    fn get_board() -> RequestBody {
        RequestBody::GetBoard {
            board_id: "work".parse().unwrap(),
        }
    }

    type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

    async fn bind(home: &Path) -> UnixListener {
        UnixListener::bind(home.join("fleetd.sock")).unwrap()
    }

    async fn authenticate(transport: &mut ServerTransport, capabilities: Vec<String>) {
        let hello = next_request(transport).await;
        assert!(matches!(hello.body, RequestBody::Hello { .. }));
        let response = HelloResponse {
            response: Response {
                id: hello.id,
                result: Ok(ResponseBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    server: "test-daemon".to_owned(),
                }),
            },
            snapshot_revision: None,
            capabilities,
            daemon_id: "test-daemon".to_owned(),
            build_commit: None,
        };
        transport
            .send(serde_json::to_value(response).unwrap())
            .await
            .unwrap();
        let subscribe = next_request(transport).await;
        assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
        send_result(transport, subscribe.id, Ok(ResponseBody::Ack)).await;
    }

    async fn next_request(transport: &mut ServerTransport) -> Request {
        transport.next().await.expect("connection closed").unwrap()
    }

    async fn send_result(
        transport: &mut ServerTransport,
        id: u64,
        result: Result<ResponseBody, ProtoError>,
    ) {
        let response = serde_json::to_value(Response { id, result }).unwrap();
        transport.send(response).await.unwrap();
    }

    /// Exercises real request framing without requiring or spawning fleetd.
    async fn run(
        arguments: &[&str],
        steps: Vec<(RequestBody, Result<ResponseBody, ProtoError>)>,
    ) -> Result<CommandOutput, ProtoError> {
        run_with_capabilities(arguments, Vec::new(), steps).await
    }

    async fn run_with_capabilities(
        arguments: &[&str],
        capabilities: Vec<String>,
        steps: Vec<(RequestBody, Result<ResponseBody, ProtoError>)>,
    ) -> Result<CommandOutput, ProtoError> {
        timeout(Duration::from_secs(5), async {
            let home = TempDir::new().unwrap();
            let listener = bind(home.path()).await;
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut transport = Framed::new(socket, FleetCodec::new());
                authenticate(&mut transport, capabilities).await;
                for (expected, result) in steps {
                    let request = next_request(&mut transport).await;
                    assert_eq!(request.body, expected);
                    send_result(&mut transport, request.id, result).await;
                }
            });
            let client = Client::connect(home.path()).await.unwrap();
            let arguments = args(arguments);
            let json = arguments.json;
            let command = Command::Board(arguments);
            assert_eq!(crate::commands::command_requests_json(&command), json);
            let output = crate::commands::execute(&client, command).await;
            drop(client);
            server.await.unwrap();
            output
        })
        .await
        .expect("board CLI socket test timed out")
    }

    fn session(id: &str, kind: SessionKind) -> Session {
        Session {
            id: id.parse().unwrap(),
            host: None,
            kind,
            cwd: "/tmp".into(),
            terminals: Vec::new(),
            active_terminal: None,
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    fn agent_thread(thread: &str, worktree: WorktreeId) -> AgentThreadSummary {
        AgentThreadSummary {
            thread: thread.parse().unwrap(),
            parent: None,
            worktree,
            host: None,
            provider: AgentKind::Claude,
            title: "Fix the board selector".into(),
            attention: Attention::Idle,
            session: SessionState::Ready,
            turn: TurnState::None,
            last_seq: Seq(0),
            last_activity: None,
            last_completed_seq: None,
            last_nonterminal_seq: None,
            exit_code: None,
        }
    }

    fn link(key: &str) -> RemoteLink {
        RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: key.into(),
            url: None,
            version: None,
            synced_at: "now".into(),
            remote_updated_at: None,
        }
    }

    #[test]
    fn resolves_display_local_and_opaque_keys_case_insensitively_and_rejects_ambiguity() {
        let mut view = view();
        // An unlinked card answers to its local key, its id, and — display_key falling back —
        // to the same local key again.
        for key in ["fLt-12", "cARD-12"] {
            assert_eq!(resolve_card(&view, key).unwrap().id, view.cards[0].id);
        }
        view.cards[0].remote = Some(link("PROJ-123"));
        for key in ["pRoJ-123", "cARD-12"] {
            assert_eq!(resolve_card(&view, key).unwrap().id, view.cards[0].id);
        }
        assert_eq!(
            resolve_card(&view, "missing").unwrap_err().kind,
            ErrorKind::NotFound
        );
        let mut collision = view.cards[0].clone();
        collision.id = "other-card".parse().unwrap();
        collision.number = 13;
        view.cards.push(collision);
        assert_eq!(
            resolve_card(&view, "proj-123").unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    /// A board whose prefix is the project key it mirrors puts two namespaces of the same
    /// shape on the same cards. Only the remote key addresses a linked card, or `SP-4` names
    /// both issue SP-4 and the fourth card — and would move whichever one it picked.
    #[test]
    fn a_linked_card_answers_to_its_remote_key_and_never_to_a_local_one() {
        let mut view = view();
        view.board.prefix = "SP".into();
        view.cards[0].number = 3;
        view.cards[0].remote = Some(link("SP-2"));
        let mut second = view.cards[0].clone();
        second.id = "card-99".parse().unwrap();
        second.number = 4;
        second.remote = Some(link("SP-3"));
        view.cards.push(second);
        // "SP-3" is the second card's issue, never the first card's local key.
        assert_eq!(resolve_card(&view, "SP-3").unwrap().id, view.cards[1].id);
        assert_eq!(resolve_card(&view, "SP-2").unwrap().id, view.cards[0].id);
        // A local number no issue carries is not a card at all once the card is linked.
        assert_eq!(
            resolve_card(&view, "SP-4").unwrap_err().kind,
            ErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn rejects_selectors_split_across_subcommand_levels_before_any_request() {
        let error = run(
            &[
                "--board",
                "work",
                "card",
                "show",
                "FLT-12",
                "--context",
                "personal",
            ],
            vec![],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--board and --context"));

        let error = run(
            &[
                "--worktree=acme/api#feature",
                "card",
                "show",
                "FLT-12",
                "--context",
                "personal",
            ],
            vec![],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--context and --worktree"));
    }

    #[test]
    fn resolves_only_worktree_sessions_from_the_snapshot() {
        let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.sessions = vec![
            session("api/feature", SessionKind::Worktree(worktree.clone())),
            session("api/agent", SessionKind::Agent(Agent::Opencode)),
        ];
        assert_eq!(
            worktree_from_session(&snapshot, Some("api/feature")).unwrap(),
            worktree
        );
        for id in [Some("api/agent"), Some("api/missing"), None] {
            let error = worktree_from_session(&snapshot, id).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Validation);
            assert_eq!(
                error.message,
                "no worktree session: pass --worktree=<owner/name#slug> or run inside a worktree terminal"
            );
        }
    }

    #[test]
    fn resolves_a_native_agent_thread_from_the_snapshot_threads() {
        let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
        let thread = "11111111-1111-4111-8111-111111111111";
        let mut snapshot = empty_snapshot();
        snapshot.agent_threads = vec![agent_thread(thread, worktree.clone())];
        let threads = snapshot.agent_threads.as_slice();
        assert_eq!(
            worktree_from_thread(threads, Some(thread)).unwrap(),
            worktree
        );
        for id in [
            Some("22222222-2222-4222-8222-222222222222"),
            Some("api/feature"),
            None,
        ] {
            let error = worktree_from_thread(threads, id).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Validation);
            assert_eq!(
                error.message,
                "no worktree session: pass --worktree=<owner/name#slug> or run inside a worktree terminal"
            );
        }
    }

    #[tokio::test]
    async fn worktree_selector_requires_capability_then_ensures_the_board() {
        let error = run(&["show", "--worktree=acme/api#feature", "--json"], vec![])
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert_eq!(
            error.message,
            "this daemon does not support worktree boards; run `fleet daemon restart`"
        );

        let mut view = view();
        view.board.worktree_id = Some("acme/api#feature".parse().unwrap());
        let output = run_with_capabilities(
            &["show", "--worktree=acme/api#feature", "--json"],
            vec![BOARD_WORKTREE_CAPABILITY.to_owned()],
            vec![(
                RequestBody::EnsureWorktreeBoard {
                    worktree_id: "acme/api#feature".parse().unwrap(),
                },
                Ok(ResponseBody::Board(view)),
            )],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 0);
    }

    #[tokio::test]
    async fn board_create_uses_the_worktree_request() {
        let mut view = view();
        view.board.worktree_id = Some("acme/api#feature".parse().unwrap());
        let output = run_with_capabilities(
            &[
                "create",
                "--worktree=acme/api#feature",
                "--name",
                "Feature",
                "--prefix",
                "FEAT",
                "--backend",
                "local",
                "--json",
            ],
            vec![BOARD_WORKTREE_CAPABILITY.to_owned()],
            vec![(
                RequestBody::CreateWorktreeBoard {
                    worktree_id: "acme/api#feature".parse().unwrap(),
                    name: Some("Feature".into()),
                    prefix: Some("FEAT".into()),
                    backend: Some(BackendRef {
                        kind: "local".into(),
                        settings: serde_json::Value::Null,
                    }),
                },
                Ok(ResponseBody::Board(view)),
            )],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 0);
    }

    #[tokio::test]
    async fn default_and_explicit_context_ensure_board_while_explicit_board_fetches() {
        let view = view();
        let snapshot = Snapshot {
            active_context: Some("work".parse().unwrap()),
            ..empty_snapshot()
        };
        let ensure = RequestBody::EnsureBoard {
            context_id: "work".parse().unwrap(),
        };
        for (arguments, steps) in [
            (
                vec!["show", "--json"],
                vec![
                    (
                        RequestBody::GetSnapshot,
                        Ok(ResponseBody::Snapshot(snapshot)),
                    ),
                    (ensure.clone(), Ok(ResponseBody::Board(view.clone()))),
                ],
            ),
            (
                vec!["card", "show", "flt-12", "--context", "work", "--json"],
                vec![(ensure, Ok(ResponseBody::Board(view.clone())))],
            ),
            (
                vec!["show", "--board", "work", "--json"],
                vec![(get_board(), Ok(ResponseBody::Board(view.clone())))],
            ),
        ] {
            let output = run(&arguments, steps).await.unwrap();
            assert_eq!(output.exit_code, 0);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output.text).unwrap()["protocol"],
                1
            );
        }
        let error = run(
            &["show", "--json"],
            vec![(
                RequestBody::GetSnapshot,
                Ok(ResponseBody::Snapshot(empty_snapshot())),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert_eq!(
            error.message,
            "no active context; select one with --context, --worktree, or --board"
        );
    }

    #[tokio::test]
    async fn board_set_creates_and_removes_the_labels_cards_can_then_carry() {
        let mut view = view();
        view.board.labels.push(Label {
            id: "old".parse().unwrap(),
            name: "Old".into(),
            color: None,
        });
        let patch = BoardPatch {
            labels: Some(vec![Label {
                id: "needs-triage".parse().unwrap(),
                name: "Needs triage".into(),
                color: None,
            }]),
            ..BoardPatch::default()
        };
        run(
            &[
                "set",
                "--board",
                "work",
                "--add-label",
                "Needs triage",
                "--remove-label",
                "Old",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: "work".parse().unwrap(),
                        patch,
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        // Without a label surface the documented `--label` flag can never resolve anything.
        let error = run(
            &["set", "--board", "work", "--remove-label", "ghost"],
            vec![(get_board(), Ok(ResponseBody::Board(view)))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[test]
    fn adding_a_label_that_differs_only_in_case_reuses_the_one_the_board_has() {
        let mut board = view().board;
        board.labels.push(Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        });
        // Every other name lookup here is case-insensitive. A second `bug` would be
        // indistinguishable from `Bug` on a card, and `--label Bug` would then refuse both
        // as ambiguous, so the board could never use either again.
        assert_eq!(
            board_labels(&board, &["bug".into()], &[]).unwrap(),
            Some(board.labels.clone())
        );
        assert_eq!(
            board_labels(&board, &["Triage".into()], &[])
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn listing_an_unknown_board_is_not_an_empty_table() {
        let view = view();
        // An empty table reads as "that board has nothing", which is not what happened.
        let error = run(
            &["list", "--board", "ghost"],
            vec![(
                RequestBody::ListBoards { context_id: None },
                Ok(ResponseBody::Boards(vec![summarize(
                    &view.board,
                    &view.cards,
                    &[],
                    "2026-09-06T12:00:00Z",
                )])),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn board_list_rejects_the_worktree_selector() {
        let error = run(&["list", "--worktree=acme/api#feature"], vec![])
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert_eq!(
            error.message,
            "board list accepts --board to narrow the table, not --worktree"
        );
    }

    #[tokio::test]
    async fn resolving_renders_the_card_against_the_board_the_resolution_left_behind() {
        let view = view();
        let mut card = view.cards[0].clone();
        card.labels = vec!["regression".parse().unwrap()];
        let mut refreshed = view.clone();
        refreshed.board.labels.push(Label {
            id: "regression".parse().unwrap(),
            name: "Regression".into(),
            color: None,
        });
        refreshed.cards = vec![card.clone()];
        // Take-remote materializes the remote's labels on the board itself; the snapshot this
        // command opened with would render the new label as its slug.
        let output = run(
            &[
                "card",
                "resolve",
                "FLT-12",
                "take-remote",
                "--board",
                "work",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ResolveCardConflict {
                        card_id: view.cards[0].id.clone(),
                        resolution: ConflictResolution::TakeRemote,
                    },
                    Ok(ResponseBody::Card(card)),
                ),
                (get_board(), Ok(ResponseBody::Board(refreshed))),
            ],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("Labels: Regression"),
            "{}",
            output.text
        );
    }

    #[tokio::test]
    async fn clear_flags_are_refused_on_create_instead_of_silently_ignored() {
        let error = run(
            &["card", "new", "Title", "--board", "work", "--clear-labels"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--clear-labels"));
    }

    #[tokio::test]
    async fn an_impossible_due_date_is_refused_before_the_daemon_is_asked() {
        for arguments in [
            vec![
                "card",
                "new",
                "Title",
                "--board",
                "work",
                "--due",
                "2026-02-31",
            ],
            vec![
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--due",
                "not-a-date",
            ],
        ] {
            let error = run(
                &arguments,
                vec![(get_board(), Ok(ResponseBody::Board(view())))],
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Validation);
            assert!(error.message.contains("YYYY-MM-DD"), "{}", error.message);
        }
    }

    #[tokio::test]
    async fn a_flagless_board_set_is_refused_like_an_empty_card_edit() {
        let error = run(
            &["set", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(
            error.message.contains("at least one field"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn set_preserves_other_settings_and_edit_does_not_reset_omitted_fields() {
        let mut view = view();
        view.board.settings.push_new_cards = true;
        view.board.settings.branch_template = "task-{key}".into();
        let mut settings = view.board.settings.clone();
        settings.start_on_worktree = false;
        let patch = BoardPatch {
            settings: Some(settings),
            ..BoardPatch::default()
        };
        run(
            &["set", "--board", "work", "--start-on-worktree", "false"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch,
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        // The one board setting with no other surface: without it `push_create` is a
        // capability no client can ever turn on, and `card new` on a remote board makes a
        // card no sync will ever push.
        let mut pushing = view.board.settings.clone();
        pushing.push_new_cards = false;
        run(
            &["set", "--board", "work", "--push-new-cards", "false"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch: BoardPatch {
                            settings: Some(pushing),
                            ..BoardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        let patch = CardPatch {
            title: Some("New title".into()),
            ..CardPatch::default()
        };
        run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--title",
                "New title",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: view.cards[0].id.clone(),
                        patch,
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
        let patch = card_patch(
            &view.board,
            BoardCardFields {
                priority: Some(BoardPriority::None),
                estimate: Some(0),
                ..BoardCardFields::default()
            },
        )
        .unwrap();
        assert_eq!(patch.priority, Some(Priority::None));
        assert_eq!(patch.estimate, Some(Some(0)));
        assert_eq!(patch.labels, None);
        assert_eq!(patch.assignee, None);
    }

    #[tokio::test]
    async fn list_create_board_and_new_card_use_typed_requests() {
        let mut view = view();
        let summary = summarize(&view.board, &view.cards, &[], "2026-09-06T12:00:00Z");
        let output = run(
            &["list", "--json"],
            vec![(
                RequestBody::ListBoards { context_id: None },
                Ok(ResponseBody::Boards(vec![summary.clone()])),
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol":1,"boards":[summary]})
        );
        run(
            &[
                "create",
                "--context",
                "work",
                "--name",
                "Fleet",
                "--prefix",
                "FLT",
                "--backend",
                "local",
                "--json",
            ],
            vec![(
                RequestBody::CreateBoard {
                    context_id: "work".parse().unwrap(),
                    name: Some("Fleet".into()),
                    prefix: Some("FLT".into()),
                    backend: Some(BackendRef::default()),
                },
                Ok(ResponseBody::Board(view.clone())),
            )],
        )
        .await
        .unwrap();
        view.board.labels.push(fleet_core::board::Label {
            id: "bug".parse().unwrap(),
            name: "Bug report".into(),
            color: None,
        });
        run(
            &[
                "card",
                "new",
                "Fix login",
                "--board",
                "work",
                "--desc",
                "Details",
                "--status",
                "In Progress",
                "--priority",
                "urgent",
                "--label",
                "Bug report",
                "--label",
                "BUG",
                "--assignee",
                "Danny",
                "--estimate",
                "3",
                "--due",
                "2026-09-30",
                "--repo",
                "acme/api",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::CreateCard {
                        board_id: view.board.id.clone(),
                        draft: CardDraft {
                            title: "Fix login".into(),
                            description: "Details".into(),
                            status_id: Some("in-progress".parse().unwrap()),
                            priority: Priority::Urgent,
                            labels: vec!["bug".parse().unwrap()],
                            assignee: Some("Danny".into()),
                            estimate: Some(3),
                            due_date: Some("2026-09-30".into()),
                            repo_id: Some("acme/api".parse().unwrap()),
                            ..CardDraft::default()
                        },
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn card_move_comment_delete_and_resolve_send_resolved_ids() {
        let view = view();
        let id = view.cards[0].id.clone();
        for (arguments, request, response) in [
            (
                vec!["move", "fLt-12", "Done", "--index", "0"],
                RequestBody::MoveCard {
                    card_id: id.clone(),
                    status_id: "done".parse().unwrap(),
                    index: Some(0),
                    cancel_run: false,
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["comment", "cARD-12", "Hello"],
                RequestBody::AddCardComment {
                    card_id: id.clone(),
                    body: "Hello".into(),
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["resolve", "FLT-12", "take-remote"],
                RequestBody::ResolveCardConflict {
                    card_id: id.clone(),
                    resolution: ConflictResolution::TakeRemote,
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["delete", "FLT-12"],
                RequestBody::DeleteCard { card_id: id },
                ResponseBody::Ack,
            ),
        ] {
            let arguments: Vec<_> = ["card"]
                .into_iter()
                .chain(arguments)
                .chain(["--board", "work", "--json"])
                .collect();
            // Resolving re-reads the board before rendering: take-remote materializes the
            // remote's labels on the board itself.
            let refreshes = matches!(request, RequestBody::ResolveCardConflict { .. });
            let mut steps = vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (request, Ok(response)),
            ];
            if refreshes {
                steps.push((get_board(), Ok(ResponseBody::Board(view.clone()))));
            }
            let output = run(&arguments, steps).await.unwrap();
            assert_eq!(output.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn worktree_human_output_matches_create() {
        let view = view();
        let worktree: fleet_core::model::Worktree = serde_json::from_value(json!({"id":"acme/api#flt-12","repoId":"acme/api","slug":"flt-12","branch":"flt-12","baseRef":"main","path":"/tmp/flt-12","session":"api/flt-12","createdAt":"now"})).unwrap();
        // `created` is the daemon's answer, not an inference from the card's previous link:
        // adopting an existing worktree must not be reported as a creation.
        for existing in [false, true] {
            let output = run(
                &[
                    "card", "worktree", "FLT-12", "--board", "work", "--repo", "acme/api",
                    "--base", "main", "--host", "local",
                ],
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::CreateWorktreeFromCard {
                            card_id: view.cards[0].id.clone(),
                            repo_id: Some(worktree.repo_id.clone()),
                            base: Some("main".into()),
                            host: None,
                        },
                        Ok(ResponseBody::CardWorktree {
                            card: view.cards[0].clone(),
                            worktree: worktree.clone(),
                            created: !existing,
                        }),
                    ),
                ],
            )
            .await
            .unwrap();
            assert_eq!(
                output.text,
                if existing {
                    "Existing acme/api#flt-12"
                } else {
                    "Created acme/api#flt-12"
                }
            );
        }
    }

    fn job(status: JobStatus) -> JobRecord {
        JobRecord {
            id: "sync-1".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: "work".into(),
            title: "Sync".into(),
            status,
            progress: Some("pulled 3, pushed 1".into()),
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        }
    }

    #[tokio::test]
    async fn sync_wait_polls_until_terminal_and_reports_summary_or_error() {
        for status in [
            JobStatus::Succeeded,
            JobStatus::Failed {
                error: "job failed".into(),
            },
            JobStatus::Cancelled,
        ] {
            let mut view = view();
            if matches!(status, JobStatus::Failed { .. }) {
                view.board.sync.last_error = Some("backend unavailable".into());
            }
            let output = run(
                &["sync", "--board", "work", "--wait", "--json"],
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::SyncBoard {
                            board_id: view.board.id.clone(),
                            full: false,
                        },
                        Ok(ResponseBody::Job(job(JobStatus::Queued))),
                    ),
                    (
                        RequestBody::ListJobs,
                        Ok(ResponseBody::Jobs(vec![job(JobStatus::Running)])),
                    ),
                    (
                        RequestBody::ListJobs,
                        Ok(ResponseBody::Jobs(vec![job(status.clone())])),
                    ),
                    (get_board(), Ok(ResponseBody::Board(view))),
                ],
            )
            .await;
            match status {
                JobStatus::Succeeded => {
                    let output = output.unwrap();
                    let json: serde_json::Value = serde_json::from_str(&output.text).unwrap();
                    assert_eq!(json["job"]["progress"], "pulled 3, pushed 1");
                    assert_eq!(json["summary"]["cardCount"], 1);
                    assert_eq!(json["protocol"], 1);
                }
                JobStatus::Failed { .. } => {
                    assert!(output.unwrap_err().message.contains("backend unavailable"))
                }
                JobStatus::Cancelled => assert_eq!(output.unwrap_err().kind, ErrorKind::Cancelled),
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn sync_without_wait_returns_immediately_and_missing_jobs_report_not_found() {
        let view = view();
        let steps = vec![
            (get_board(), Ok(ResponseBody::Board(view.clone()))),
            (
                RequestBody::SyncBoard {
                    board_id: view.board.id.clone(),
                    full: false,
                },
                Ok(ResponseBody::Job(job(JobStatus::Queued))),
            ),
        ];
        let output = run(&["sync", "--board", "work", "--json"], steps.clone())
            .await
            .unwrap();
        assert_eq!(output.text, r#"{"protocol":1,"jobId":"sync-1"}"#);
        let mut steps = steps;
        steps.push((RequestBody::ListJobs, Ok(ResponseBody::Jobs(vec![]))));
        let error = run(&["sync", "--board", "work", "--wait"], steps)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
        assert!(error.message.contains("board sync job"));
    }

    fn jira_view() -> BoardView {
        let mut view = view();
        view.board.backend = BackendRef {
            kind: "jira".into(),
            settings: json!({"project": "OLD", "jql": "assignee = currentUser()"}),
        };
        view
    }

    fn descriptors() -> Vec<fleet_core::board::BackendDescriptor> {
        vec![fleet_core::board::BackendDescriptor {
            kind: "jira".into(),
            label: "Jira (acli)".into(),
            capabilities: fleet_core::board::BackendCapabilities {
                pull: true,
                ..fleet_core::board::BackendCapabilities::default()
            },
            settings_schema: vec![
                serde_json::from_value(
                    json!({"key": "project", "name": "Project key (required)", "kind": "text"}),
                )
                .unwrap(),
            ],
        }]
    }

    #[tokio::test]
    async fn backends_needs_no_board_and_describe_asks_the_board_it_resolved() {
        let output = run(
            &["backends", "--json"],
            vec![(
                RequestBody::ListBoardBackends {},
                Ok(ResponseBody::BoardBackends(descriptors())),
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol": 1, "backends": descriptors()})
        );
        // Which kinds the daemon registers is not a property of any one board, so no board is
        // resolved first: a context without a board can still ask what it could point at.
        let output = run(
            &["backends"],
            vec![(
                RequestBody::ListBoardBackends {},
                Ok(ResponseBody::BoardBackends(descriptors())),
            )],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("jira  Jira (acli)  pull"),
            "{}",
            output.text
        );

        let schema = fleet_core::board::BackendSchema {
            key_prefix: Some("SP".into()),
            readonly_fields: vec!["priority".into()],
            ..fleet_core::board::BackendSchema::default()
        };
        let output = run(
            &["describe", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(jira_view()))),
                (
                    RequestBody::DescribeBoardBackend {
                        board_id: "work".parse().unwrap(),
                    },
                    Ok(ResponseBody::BoardBackendSchema(schema.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("Read-only fields: priority"),
            "{}",
            output.text
        );
        let output = run(
            &["describe", "--board", "work", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(jira_view()))),
                (
                    RequestBody::DescribeBoardBackend {
                        board_id: "work".parse().unwrap(),
                    },
                    Ok(ResponseBody::BoardBackendSchema(schema.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol": 1, "schema": schema})
        );
    }

    #[tokio::test]
    async fn a_kind_change_starts_from_empty_settings_and_a_setting_alone_merges() {
        for (arguments, expected) in [
            (
                vec![
                    "set",
                    "--backend",
                    "jira",
                    "--setting",
                    "project=SP",
                    "--setting",
                    "maxConcurrency=8",
                ],
                // The previous kind's keys mean nothing to the new one, so `jql` is gone and
                // `8` arrives as a number, not as the string the shell handed us.
                BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "SP", "maxConcurrency": 8}),
                },
            ),
            (
                vec![
                    "set",
                    "--setting",
                    r#"statuses=["To Do","Done"]"#,
                    "--setting",
                    "jql=null",
                ],
                // No `--backend`: the kind stays and the pairs merge into what is stored,
                // where `null` is how a key is removed.
                BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "OLD", "statuses": ["To Do", "Done"]}),
                },
            ),
            (
                vec!["set", "--backend", "local"],
                // `--backend` alone leaves the new kind with nothing configured.
                BackendRef::default(),
            ),
        ] {
            let view = jira_view();
            let arguments: Vec<_> = arguments.into_iter().chain(["--board", "work"]).collect();
            run(
                &arguments,
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::UpdateBoard {
                            board_id: view.board.id.clone(),
                            patch: BoardPatch {
                                backend: Some(expected),
                                ..BoardPatch::default()
                            },
                        },
                        Ok(ResponseBody::Board(view.clone())),
                    ),
                    // `set` prints the same header `show` does, label included.
                    (
                        RequestBody::ListBoardBackends {},
                        Ok(ResponseBody::BoardBackends(descriptors())),
                    ),
                ],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn create_carries_its_settings_and_a_backendless_create_stays_null() {
        for (arguments, backend) in [
            (
                vec!["--backend", "jira", "--setting", "project=SP"],
                Some(BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "SP"}),
                }),
            ),
            (vec!["--backend", "local"], Some(BackendRef::default())),
            (vec![], None),
        ] {
            let arguments: Vec<_> = ["create", "--context", "work"]
                .into_iter()
                .chain(arguments)
                .collect();
            run(
                &arguments,
                vec![
                    (
                        RequestBody::CreateBoard {
                            context_id: "work".parse().unwrap(),
                            name: None,
                            prefix: None,
                            backend,
                        },
                        Ok(ResponseBody::Board(view())),
                    ),
                    (
                        RequestBody::ListBoardBackends {},
                        Ok(ResponseBody::BoardBackends(descriptors())),
                    ),
                ],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn a_full_sync_tells_the_daemon_to_ignore_the_cursor() {
        let view = jira_view();
        let output = run(
            &["sync", "--board", "work", "--full", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::SyncBoard {
                        board_id: view.board.id.clone(),
                        full: true,
                    },
                    Ok(ResponseBody::Job(job(JobStatus::Queued))),
                ),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.text, r#"{"protocol":1,"jobId":"sync-1"}"#);
    }

    #[tokio::test]
    async fn a_read_only_field_fails_with_the_daemons_message_word_for_word() {
        let view = jira_view();
        let message = "priority is read-only on this board's backend";
        let error = run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--priority",
                "high",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: view.cards[0].id.clone(),
                        patch: CardPatch {
                            priority: Some(Priority::High),
                            ..CardPatch::default()
                        },
                    },
                    Err(ProtoError {
                        kind: ErrorKind::Validation,
                        message: message.to_owned(),
                    }),
                ),
            ],
        )
        .await
        .unwrap_err();
        // The CLI is the surface that shows this; a reworded copy would send the user looking
        // for a setting that does not exist.
        assert_eq!(error.message, message);
        assert_eq!(error.kind, ErrorKind::Validation);
    }

    #[tokio::test]
    async fn the_show_header_names_the_backend_and_json_never_pays_for_it() {
        let view = jira_view();
        let output = run(
            &["show", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output
                .text
                .contains("backend: Jira (acli) \u{b7} project OLD \u{b7} never synced"),
            "{}",
            output.text
        );
        // A daemon that cannot list backends still prints the board.
        let output = run(
            &["show", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ListBoardBackends {},
                    Err(ProtoError {
                        kind: ErrorKind::Unknown,
                        message: "unsupported".into(),
                    }),
                ),
            ],
        )
        .await
        .unwrap();
        // Which settings identify a board is the backend's own answer, so without a descriptor
        // the header names the kind and stops there rather than guessing at keys.
        assert!(
            output.text.contains("backend: jira \u{b7} never synced"),
            "{}",
            output.text
        );
        // `--json` returns the board verbatim, so the descriptor round trip is not made.
        let output = run(
            &["show", "--board", "work", "--json"],
            vec![(get_board(), Ok(ResponseBody::Board(view)))],
        )
        .await
        .unwrap();
        assert!(!output.text.contains("Jira (acli)"), "{}", output.text);
    }

    const LIVE_RUN: &str = "00000000-0000-4000-8000-000000000001";
    const DONE_RUN: &str = "00000000-0000-4000-8000-000000000002";
    const LIVE_THREAD: &str = "00000000-0000-4000-8000-0000000000a1";
    const DONE_THREAD: &str = "00000000-0000-4000-8000-0000000000a2";

    /// The view a run verb reads: one card with a finished run and a live one, joined by the
    /// `liveRuns` the daemon answers `GetBoard` with.
    fn view_with_runs() -> BoardView {
        let mut view = view();
        view.cards[0] = serde_json::from_value(json!({
            "id": "Card-12", "boardId": "work", "number": 12, "title": "Fix login",
            "statusId": "in-progress", "createdAt": "now", "updatedAt": "now",
            "runs": [
                {
                    "id": DONE_RUN, "threadId": DONE_THREAD, "statusId": "in-progress",
                    "action": {"kind": "prompt"}, "provider": "codex",
                    "model": "gpt-5.6-sol", "effort": "high",
                    "startedAt": "2026-09-06T11:50:00Z", "endedAt": "2026-09-06T11:52:30Z",
                    "outcome": "succeeded", "tokens": 1200, "costUsd": 0.5
                },
                {
                    "id": LIVE_RUN, "threadId": LIVE_THREAD, "statusId": "in-progress",
                    "action": {"kind": "prompt"}, "provider": "codex",
                    "startedAt": "2026-09-06T11:58:00Z"
                }
            ]
        }))
        .expect("the run fixture is a card");
        view.live_runs = serde_json::from_value(json!([{
            "cardId": "Card-12", "run": LIVE_RUN, "status": "running",
            "started": "2026-09-06T11:58:00Z"
        }]))
        .expect("the run fixture is a join");
        view
    }

    /// A second card, so the link flags have another card to edit.
    fn other_card() -> Card {
        serde_json::from_value(json!({
            "id": "Card-13", "boardId": "work", "number": 13, "title": "Ship login",
            "statusId": "todo", "createdAt": "now", "updatedAt": "now"
        }))
        .expect("the link fixture is a card")
    }

    /// Every `columns` verb is one read-modify-write: the `GetBoard` the dispatcher already
    /// paid, then a single `UpdateBoard` carrying the whole column vector — never a second read
    /// and never a patch of one column.
    #[tokio::test]
    async fn every_columns_verb_sends_one_update_board_carrying_the_whole_vector() {
        let view = view();
        let update = |statuses: Vec<Status>| RequestBody::UpdateBoard {
            board_id: view.board.id.clone(),
            patch: BoardPatch {
                statuses: Some(statuses),
                ..BoardPatch::default()
            },
        };
        let steps = |request: RequestBody| -> Vec<(RequestBody, Result<ResponseBody, ProtoError>)> {
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (request, Ok(ResponseBody::Board(view.clone()))),
            ]
        };

        // add: the column lands where `--after` names, in the category it was given.
        let mut statuses = view.board.statuses.clone();
        statuses.insert(
            3,
            Status {
                id: "in-review".parse().unwrap(),
                name: "In review".into(),
                category: StatusCategory::Started,
                color: None,
                automation: None,
            },
        );
        run(
            &[
                "columns",
                "add",
                "In review",
                "--category",
                "started",
                "--after",
                "in-progress",
                "--board",
                "work",
            ],
            steps(update(statuses)),
        )
        .await
        .unwrap();

        // edit: the action, the agent it runs with, its environment and one route, all on the
        // column the flags named and on no other.
        let mut statuses = view.board.statuses.clone();
        statuses[2].automation = Some(ColumnAutomation {
            on_enter: Some(Action {
                kind: ActionKind::Prompt,
                instructions: "Implement {key}".into(),
                expect: "make test is green".into(),
                agent: ColumnAgentPrefs {
                    provider: Some(AgentKind::Codex),
                    model: None,
                    effort: None,
                    mode: Some(PermissionMode::AcceptEdits),
                },
                env: vec!["RUST_LOG=debug".into()],
            }),
            on_success: Some("done".parse().unwrap()),
            advance_when_unblocked: None,
        });
        run(
            &[
                "columns",
                "edit",
                "in-progress",
                "--on-enter",
                "prompt",
                "--instructions",
                "Implement {key}",
                "--expect",
                "make test is green",
                "--provider",
                "codex",
                "--mode",
                "accept-edits",
                "--env",
                "RUST_LOG=debug",
                "--on-success",
                "done",
                "--board",
                "work",
            ],
            steps(update(statuses)),
        )
        .await
        .unwrap();

        // move: the vector reordered, never a per-column index on the wire.
        let mut statuses = view.board.statuses.clone();
        let done = statuses.remove(3);
        statuses.insert(2, done);
        run(
            &[
                "columns",
                "move",
                "done",
                "--before",
                "in-progress",
                "--board",
                "work",
            ],
            steps(update(statuses)),
        )
        .await
        .unwrap();

        // remove: the vector without the column. The board's one card sits elsewhere, so
        // nothing has to move first.
        let mut statuses = view.board.statuses.clone();
        statuses.remove(4);
        run(
            &["columns", "remove", "canceled", "--board", "work"],
            steps(update(statuses)),
        )
        .await
        .unwrap();

        // preset: the two missing workflow columns, each in its place, and every column the
        // board already had left exactly as it was.
        let mut preset = view.board.clone();
        assert!(apply_workflow_preset(&mut preset));
        assert_eq!(
            preset
                .statuses
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            [
                "backlog",
                "todo",
                "ready",
                "in-progress",
                "in-review",
                "done",
                "canceled"
            ]
        );
        run(
            &["columns", "preset", "workflow", "--board", "work"],
            steps(update(preset.statuses)),
        )
        .await
        .unwrap();
    }

    /// `remove --move-cards-to` empties the column before dropping it: the daemon refuses to
    /// remove a status any card still names, archived cards included.
    #[tokio::test]
    async fn removing_a_column_moves_every_card_out_of_it_first() {
        let mut view = view();
        let mut archived = other_card();
        archived.archived = true;
        view.cards.push(archived);
        // Both cards sit in Todo; the card in another column is not moved.
        view.cards[1].status_id = "todo".parse().unwrap();
        let mut elsewhere = other_card();
        elsewhere.id = "Card-14".parse().unwrap();
        elsewhere.number = 14;
        elsewhere.status_id = "done".parse().unwrap();
        view.cards.push(elsewhere);
        let mut statuses = view.board.statuses.clone();
        statuses.remove(1);
        let moved = |card: &str| RequestBody::MoveCard {
            card_id: card.parse().unwrap(),
            status_id: "backlog".parse().unwrap(),
            index: None,
            cancel_run: false,
        };
        let output = run(
            &[
                "columns",
                "remove",
                "todo",
                "--move-cards-to",
                "backlog",
                "--board",
                "work",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    moved("Card-12"),
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
                (
                    moved("Card-13"),
                    Ok(ResponseBody::Card(view.cards[1].clone())),
                ),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch: BoardPatch {
                            statuses: Some(statuses),
                            ..BoardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("moved 2 cards to Backlog"),
            "{}",
            output.text
        );
    }

    /// A preset that has nothing to add writes nothing at all, and says so: a board already
    /// carrying the workflow columns must not be rewritten just to be told it is fine.
    #[tokio::test]
    async fn a_preset_with_nothing_to_add_sends_no_request_and_says_so() {
        let mut complete = view();
        complete.board.statuses = workflow_preset();
        let output = run(
            &["columns", "preset", "workflow", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(complete)))],
        )
        .await
        .unwrap();
        assert!(
            output
                .text
                .contains("every workflow column is already on this board"),
            "{}",
            output.text
        );
        // The listing is a read as well: one `GetBoard` and the table.
        let output = run(
            &["columns", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap();
        assert!(output.text.contains("ON ENTER"), "{}", output.text);
    }

    /// `--cancel-run` is the one thing that lets a move take a working card: without it the
    /// daemon refuses, and the CLI never decides that on its own.
    #[tokio::test]
    async fn a_move_carries_the_flag_that_cancels_the_cards_run() {
        let view = view_with_runs();
        run(
            &[
                "card",
                "move",
                "FLT-12",
                "done",
                "--cancel-run",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::MoveCard {
                        card_id: "Card-12".parse().unwrap(),
                        status_id: "done".parse().unwrap(),
                        index: None,
                        cancel_run: true,
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
    }

    /// The three run verbs that write send one request each and print what the daemon answered;
    /// `wait` carries its timeout in milliseconds and exits 0 only for a run that has ended.
    #[tokio::test]
    async fn the_writing_run_verbs_send_one_request_each_and_wait_reports_by_exit_code() {
        let view = view_with_runs();
        let card = view.cards[0].clone();
        // The three writing verbs are the daemon's newest requests, so each asks for the
        // capability first and refuses an older daemon rather than sending one.
        let run = |arguments: &'static [&'static str], steps| {
            run_with_capabilities(
                arguments,
                vec![BOARD_AUTOMATION_CAPABILITY.to_owned()],
                steps,
            )
        };
        let output = run(
            &["card", "run", "FLT-12", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::CardRunStart {
                        card_id: "Card-12".parse().unwrap(),
                    },
                    Ok(ResponseBody::Card(card.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output
                .text
                .starts_with(&format!("run {LIVE_RUN} started, thread {LIVE_THREAD}\n")),
            "{}",
            output.text
        );

        run(
            &["card", "cancel", "FLT-12", "--board", "work", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::CardRunCancel {
                        card_id: "Card-12".parse().unwrap(),
                    },
                    Ok(ResponseBody::Card(card.clone())),
                ),
            ],
        )
        .await
        .unwrap();

        // The newest run is still live, so the wait timed out: exit 2, with the card printed.
        let wait = RequestBody::CardRunWait {
            card_id: "Card-12".parse().unwrap(),
            timeout_ms: 30_000,
        };
        let output = run(
            &[
                "card",
                "wait",
                "FLT-12",
                "--timeout",
                "30",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (wait.clone(), Ok(ResponseBody::Card(card.clone()))),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 2);

        // The same wait once the run has ended.
        let mut finished = card.clone();
        finished.runs.pop();
        let output = run(
            &[
                "card",
                "wait",
                "FLT-12",
                "--timeout",
                "30",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (wait.clone(), Ok(ResponseBody::Card(finished))),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 0);

        // A card the board still *owes* a run is unfinished too: its newest row is an older
        // attempt, and reading 0 off that would tell a script the run it is waiting for had
        // finished before anything started.
        let mut owed = card.clone();
        owed.runs.pop();
        owed.pending_run = Some(fleet_core::board::PendingRun {
            status_id: "in-progress".parse().expect("a static status slug"),
            since: "now".into(),
        });
        let output = run(
            &[
                "card",
                "wait",
                "FLT-12",
                "--timeout",
                "30",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (wait.clone(), Ok(ResponseBody::Card(owed))),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 2);

        // A card whose column never started one is as unfinished as a card still running.
        let mut never_ran = card;
        never_ran.runs.clear();
        let output = run(
            &[
                "card",
                "wait",
                "FLT-12",
                "--timeout",
                "30",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (wait, Ok(ResponseBody::Card(never_ran))),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 2);

        // A daemon too old to run cards says so once, and the card is never asked to start.
        let error = run_with_capabilities(
            &["card", "run", "FLT-12", "--board", "work"],
            Vec::new(),
            vec![(get_board(), Ok(ResponseBody::Board(view)))],
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.message,
            "this daemon does not support board automation; run `fleet daemon restart`"
        );
    }

    /// `card runs` and `card attach` answer from the view the dispatcher already read: a second
    /// request would be a second round trip for facts the card carries.
    #[tokio::test]
    async fn the_reading_run_verbs_answer_from_the_board_they_were_given() {
        let with_runs = view_with_runs();
        let output = run(
            &["card", "runs", "FLT-12", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(with_runs.clone())))],
        )
        .await
        .unwrap();
        let lines: Vec<&str> = output.text.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "{DONE_RUN}\tIn Progress\tsucceeded\tcodex\tgpt-5.6-sol\thigh\t2m 30s\t1200\t$0.50\t{DONE_THREAD}"
            )
        );
        // The live run takes its word from the join, and what nobody reported is an em dash.
        let live: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(live.len(), 10);
        assert_eq!(live[0], LIVE_RUN);
        assert_eq!(live[2], "running");
        assert_eq!(&live[4..6], ["\u{2014}", "\u{2014}"]);
        assert_eq!(live[9], LIVE_THREAD);

        let output = run(
            &["card", "attach", "FLT-12", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(with_runs)))],
        )
        .await
        .unwrap();
        assert_eq!(output.text, LIVE_THREAD);

        // A card nothing ever ran has no thread to attach to, and says so before the wire.
        let error = run(
            &["card", "attach", "FLT-12", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.message, "FLT-12 has no run");

        // A card that *did* run, whose runs never reached a thread, gets the other sentence —
        // the app's `A` says the same two about the same two cards (contracts §5.5). The run is
        // on the card and its failure is in the detail, so "has no run" would be a lie.
        let mut never_started = view_with_runs();
        never_started.live_runs.clear();
        never_started.cards[0].runs.truncate(1);
        never_started.cards[0].runs[0].thread_id = None;
        let error = run(
            &["card", "attach", "FLT-12", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(never_started)))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.message, "FLT-12's runs never reached a thread");
    }

    /// `card new` carries the agent block and the blockers in the draft itself, then writes the
    /// other direction of `--blocks` as a patch of the card that flag named.
    #[tokio::test]
    async fn a_new_card_carries_its_agent_and_blockers_then_links_the_other_direction() {
        let mut view = view();
        view.cards.push(other_card());
        let created: Card = serde_json::from_value(json!({
            "id": "Card-14", "boardId": "work", "number": 14, "title": "Ship it",
            "statusId": "todo", "createdAt": "now", "updatedAt": "now",
            "blockedBy": ["Card-12"]
        }))
        .expect("the created card is a card");
        let mut linked = other_card();
        linked.blocked_by = vec!["Card-14".parse().unwrap()];
        let output = run(
            &[
                "card",
                "new",
                "Ship it",
                "--blocked-by",
                "FLT-12",
                "--blocks",
                "FLT-13",
                "--provider",
                "codex",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "high",
                "--board",
                "work",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::CreateCard {
                        board_id: view.board.id.clone(),
                        draft: CardDraft {
                            title: "Ship it".into(),
                            agent: Some(CardAgentPrefs {
                                provider: Some(AgentKind::Codex),
                                model: Some("gpt-5.6-sol".into()),
                                effort: Some("high".into()),
                            }),
                            blocked_by: vec!["Card-12".parse().unwrap()],
                            ..CardDraft::default()
                        },
                    },
                    Ok(ResponseBody::Card(created)),
                ),
                (
                    RequestBody::UpdateCard {
                        card_id: "Card-13".parse().unwrap(),
                        patch: CardPatch {
                            blocked_by: Some(vec!["Card-14".parse().unwrap()]),
                            ..CardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Card(linked)),
                ),
            ],
        )
        .await
        .unwrap();
        // The card the verb was about, then the card its sugar changed.
        assert!(output.text.contains("Ship it"), "{}", output.text);
        assert!(output.text.contains("Ship login"), "{}", output.text);
        // The dependant names its new blocker by key: the view was read before the card
        // existed, and rendered against that list the blocker prints as a raw id and
        // `blocked()` — which calls a blocker it cannot find canceled — marks the dependant
        // amber for a card created a moment ago.
        assert!(
            output.text.contains("Blocked by: FLT-14"),
            "{}",
            output.text
        );
        assert!(
            !output.text.contains("canceled or archived"),
            "{}",
            output.text
        );
    }

    /// `card edit` sends the clear as an explicit null, and `--add-blocks` alone patches only
    /// the card it names — this card's own document has nothing to change.
    #[tokio::test]
    async fn card_edit_clears_the_agent_and_links_without_patching_the_wrong_card() {
        let mut view = view();
        view.cards.push(other_card());
        let card = view.cards[0].clone();
        run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--clear-agent",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: "Card-12".parse().unwrap(),
                        patch: CardPatch {
                            agent: Some(None),
                            ..CardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Card(card.clone())),
                ),
            ],
        )
        .await
        .unwrap();

        // `--add-blocks` names the other card: exactly one `UpdateCard`, and it is not this one.
        let mut linked = other_card();
        linked.blocked_by = vec!["Card-12".parse().unwrap()];
        run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--add-blocks",
                "FLT-13",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: "Card-13".parse().unwrap(),
                        patch: CardPatch {
                            blocked_by: Some(vec!["Card-12".parse().unwrap()]),
                            ..CardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Card(linked)),
                ),
            ],
        )
        .await
        .unwrap();

        // `--add-blocked-by` is the same link from this card's side: the whole vector, computed
        // from the card the CLI already read.
        run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--add-blocked-by",
                "FLT-13",
                "--board",
                "work",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view))),
                (
                    RequestBody::UpdateCard {
                        card_id: "Card-12".parse().unwrap(),
                        patch: CardPatch {
                            blocked_by: Some(vec!["Card-13".parse().unwrap()]),
                            ..CardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Card(card)),
                ),
            ],
        )
        .await
        .unwrap();
    }

    /// The board's run ceiling rides on the settings patch every other `board set` flag uses.
    #[tokio::test]
    async fn board_set_max_live_runs_rides_on_the_settings_patch() {
        let view = view();
        let mut settings = view.board.settings.clone();
        settings.max_live_runs = Some(3);
        run(
            &["set", "--max-live-runs", "3", "--board", "work", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch: BoardPatch {
                            settings: Some(settings),
                            ..BoardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Board(view)),
                ),
            ],
        )
        .await
        .unwrap();
    }

    /// A run may not move the card it is running for: the report moves it when the run ends, so
    /// a move from inside the run races its own outcome.
    ///
    /// The refusal is a pure function of the command and the two variables the daemon injects,
    /// which is what lets this assert it without putting `FLEET_CARD` in the environment of a
    /// test binary running its cases in parallel — no test in this workspace sets one. `board`
    /// calls it above `resolve_board`, so the refused path builds no request at all, and the
    /// error it returns is the exit 1 every refusal exits with.
    #[test]
    fn a_run_is_refused_the_move_of_its_own_card_and_asks_for_nothing() {
        let command = |arguments: &[&str]| args(arguments).command;
        let moved = command(&["card", "move", "flt-12", "done"]);
        let error = refuse_self_move(&moved, Some("delegation-1"), Some("FLT-12"))
            .expect_err("a run may not move its own card");
        assert_eq!(error.message, SELF_MOVE_REFUSAL);
        assert_eq!(
            error.message,
            "a run cannot move its own card; its report moves the card when it finishes"
        );
        let mut stdout = Vec::<u8>::new();
        let mut stderr = Vec::<u8>::new();
        assert_eq!(
            crate::commands::error_result(
                crate::commands::print_error(&error, false, &mut stdout, &mut stderr),
                false,
            ),
            1
        );
        assert!(stdout.is_empty());
        assert_eq!(
            String::from_utf8(stderr).expect("the refusal is printed as text"),
            format!("fleet: {SELF_MOVE_REFUSAL}\n")
        );

        // Everything else passes: another card, another verb, and the same move outside a run —
        // where `FLEET_CARD` is only whatever the caller's shell happens to export.
        assert!(refuse_self_move(&moved, Some("delegation-1"), Some("FLT-13")).is_ok());
        assert!(refuse_self_move(&moved, Some("delegation-1"), None).is_ok());
        assert!(refuse_self_move(&moved, None, Some("FLT-12")).is_ok());
        for arguments in [
            vec!["card", "show", "flt-12"],
            vec!["card", "run", "flt-12"],
            vec!["card", "cancel", "flt-12"],
            vec!["card", "comment", "flt-12", "done"],
        ] {
            assert!(
                refuse_self_move(&command(&arguments), Some("delegation-1"), Some("FLT-12"))
                    .is_ok(),
                "{arguments:?}"
            );
        }
    }

    /// The same move, from a run working on another card, reaches the daemon untouched: the
    /// refusal is advisory and narrow, never a guard on moving cards from inside a run.
    #[tokio::test]
    async fn a_run_may_still_move_every_card_but_its_own() {
        let view = view();
        run(
            &[
                "card", "move", "FLT-12", "done", "--board", "work", "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::MoveCard {
                        card_id: "Card-12".parse().unwrap(),
                        status_id: "done".parse().unwrap(),
                        index: None,
                        cancel_run: false,
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
    }
}
