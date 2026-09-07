//! Board CLI parsing and orchestration tests.

mod parsing {
    use crate::{
        args::{
            BoardArgs, BoardCardCommand, BoardCommand, BoardConflictPolicy, BoardPriority, Cli,
            Command,
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
        let BoardCardCommand::New { title, fields } = command else {
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
            matches!(card_command(&["card", "move", "FLT-12", "done", "--index", "0"]), BoardCardCommand::Move { key, status, index: Some(0) } if key == "FLT-12" && status == "done")
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
        }
    }

    #[test]
    fn board_and_card_envelopes_have_exact_protocol_one_shapes() {
        let view = view();
        let summaries = vec![summarize(&view.board, &view.cards)];
        for (actual, expected) in [
            (
                to_json(&BoardEnvelope {
                    protocol: PROTOCOL,
                    board: &view.board,
                    cards: &view.cards,
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
        let summary = summarize(&view.board, &view.cards);
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
        let mut summary = summarize(&view.board, &view.cards);
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
    }
}

mod orchestration {
    use crate::args::{Cli, Command};
    use crate::commands::board::*;
    use clap::Parser;
    use fleet_core::{
        board::{RemoteLink, new_board},
        model::Context,
    };
    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        job::JobKind,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
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

    async fn authenticate(transport: &mut ServerTransport) {
        let hello = next_request(transport).await;
        assert!(matches!(hello.body, RequestBody::Hello { .. }));
        send_result(
            transport,
            hello.id,
            Ok(ResponseBody::Hello {
                protocol: PROTOCOL_VERSION,
                server: "test-daemon".to_owned(),
            }),
        )
        .await;
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
        timeout(Duration::from_secs(5), async {
            let home = TempDir::new().unwrap();
            let listener = bind(home.path()).await;
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut transport = Framed::new(socket, FleetCodec::new());
                authenticate(&mut transport).await;
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
        assert!(error.message.contains("no active context"));
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
                )])),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
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
        let summary = summarize(&view.board, &view.cards);
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
}
