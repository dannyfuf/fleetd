use super::*;

#[test]
fn failed_snapshot_retry_is_full() {
    let mut engine = GhosttyEngine::new(8, 2, 1024).unwrap();
    engine.feed(b"recover me");
    engine.fail_next = Some(TestFailure::Snapshot);

    assert!(engine.try_take_frame(false).is_err());
    let recovered = engine.try_take_frame(false).unwrap();
    assert!(recovered.full);
    assert_eq!(recovered.rows_changed.len(), usize::from(recovered.rows));
    assert!(
        recovered.rows_changed[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("recover")
    );
}

#[test]
fn encoding_failures_are_retryable() {
    let mut engine = GhosttyEngine::new(80, 24, 1024).unwrap();
    let key = KeyEvent {
        key: Key::Char('x'),
        mods: Modifiers::empty(),
        text: Some("x".to_owned()),
        action: KeyAction::Press,
    };
    engine.fail_next = Some(TestFailure::Key);
    assert!(engine.try_encode_key(&key).is_err());
    assert_eq!(engine.try_encode_key(&key).unwrap(), b"x");

    engine.feed(b"\x1b[?1002h\x1b[?1006h");
    let mouse = MouseEvent {
        button: MouseButton::Left,
        kind: MouseEventKind::Press,
        col: 1,
        row: 1,
        mods: Modifiers::empty(),
    };
    engine.fail_next = Some(TestFailure::Mouse);
    assert!(engine.try_encode_mouse(&mouse).is_err());
    assert!(!engine.mouse_button_down);
    assert!(!engine.try_encode_mouse(&mouse).unwrap().is_empty());
    assert!(engine.mouse_button_down);

    engine.fail_next = Some(TestFailure::Mouse);
    let wheel = WheelEvent {
        steps: -1,
        col: 1,
        row: 1,
        mods: Modifiers::SHIFT,
    };
    assert!(engine.try_wheel(&wheel).is_err());
    assert!(matches!(
        engine.try_wheel(&wheel),
        Ok(WheelAction::Pty(bytes)) if !bytes.is_empty()
    ));

    engine.fail_next = Some(TestFailure::Compression);
    assert!(engine.try_compress_idle().is_err());
    assert!(engine.try_compress_idle().is_ok());
}

#[test]
fn wheel_press_does_not_leave_a_held_button() {
    use fleet_proto::terminal::{Modifiers, MouseButton, MouseEventKind};
    let mut engine = GhosttyEngine::new(80, 24, 1024 * 1024).unwrap();
    engine.feed(b"\x1b[?1002h\x1b[?1006h");
    let mut event = MouseEvent {
        button: MouseButton::WheelUp,
        kind: MouseEventKind::Press,
        col: 2,
        row: 3,
        mods: Modifiers::empty(),
    };
    for button in [
        MouseButton::WheelUp,
        MouseButton::WheelDown,
        MouseButton::Other(4),
        MouseButton::Other(5),
    ] {
        event.button = button;
        assert!(!engine.encode_mouse(&event).is_empty());
        assert!(!engine.mouse_button_down);
    }
    event.button = MouseButton::Left;
    event.kind = MouseEventKind::Move;
    assert!(engine.encode_mouse(&event).is_empty());
    event.kind = MouseEventKind::Press;
    engine.encode_mouse(&event);
    event.button = MouseButton::WheelDown;
    engine.encode_mouse(&event);
    assert!(
        engine.mouse_button_down,
        "wheel must preserve real held buttons"
    );
}

#[test]
fn renders_text_and_sgr_attributes() {
    let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"\x1b[31mhi\x1b[0m\r\nx");
    let frame = engine.take_frame(true);
    let first = &frame.rows_changed[0].cells;
    assert_eq!(first[0].text.as_str(), "h");
    assert_eq!(first[1].text.as_str(), "i");
    assert_eq!(first[0].fg, Color::Palette(1));
    assert!(first[0].attrs.is_empty());
    assert_eq!(frame.rows_changed[1].cells[0].text.as_str(), "x");
    assert_eq!(frame.rows_changed[1].cells[0].fg, Color::Default);
}

#[test]
fn frame_rows_expose_ghostty_soft_wraps() {
    let mut engine = GhosttyEngine::new(4, 3, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"abcdefghij");

    let frame = engine.take_frame(true);
    assert!(frame.rows_changed[0].wrapped);
    assert!(frame.rows_changed[1].wrapped);
    assert!(!frame.rows_changed[2].wrapped);
}

#[test]
fn scrollback_shrink_advances_the_history_epoch() {
    let mut engine = GhosttyEngine::new(8, 2, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"one\r\ntwo\r\nthree\r\nfour");
    let before = engine.take_frame(true).viewport;
    assert!(before.scrollback_len > 0);
    assert_eq!(before.history_epoch, 0);

    engine.feed(b"\x1b[3J");
    let after = engine.take_frame(false).viewport;
    assert!(after.scrollback_len < before.scrollback_len);
    assert_eq!(after.history_epoch, 1);
}

#[test]
fn scroll_only_frames_do_not_advance_the_history_epoch() {
    let mut engine = GhosttyEngine::new(8, 2, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"one\r\ntwo\r\nthree\r\nfour");
    let epoch = engine.take_frame(true).viewport.history_epoch;

    engine.scroll(ScrollCommand::Top);
    assert_eq!(engine.take_frame(true).viewport.history_epoch, epoch);
    engine.scroll(ScrollCommand::Bottom);
    assert_eq!(engine.take_frame(true).viewport.history_epoch, epoch);
}

#[test]
fn narrowing_the_terminal_advances_the_history_epoch() {
    // Reflow rewrites which absolute line holds which text, so every selection anchored
    // in scrollback must be dropped. Losing rows only pushes screen rows *into* history,
    // which keeps every existing line where it was.
    let mut engine = GhosttyEngine::new(16, 4, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"a long wrapped line\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
    let epoch = engine.take_frame(true).viewport.history_epoch;

    engine
        .resize(16, 2)
        .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
    assert_eq!(engine.take_frame(false).viewport.history_epoch, epoch);

    engine
        .resize(8, 2)
        .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
    assert_eq!(engine.take_frame(false).viewport.history_epoch, epoch + 1);
}

#[test]
fn resizes_grid_and_full_frame() {
    let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine
        .resize(12, 5)
        .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
    let frame = engine.take_frame(true);
    assert_eq!((frame.cols, frame.rows), (12, 5));
    assert_eq!(frame.rows_changed.len(), 5);
    assert!(frame.rows_changed.iter().all(|row| row.cells.len() == 12));
}

#[test]
fn incremental_frame_contains_only_dirty_rows() {
    let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    let _ = engine.take_frame(true);
    engine.feed(b"\x1b[2;1Hx");
    let frame = engine.take_frame(false);
    assert!(!frame.full);
    assert!(frame.rows_changed.len() < usize::from(frame.rows));
    assert!(frame.rows_changed.iter().all(|row| row.index != 2));
    let changed = frame
        .rows_changed
        .iter()
        .find(|row| row.index == 1)
        .unwrap_or_else(|| panic!("written row was not dirty"));
    assert_eq!(changed.cells[0].text.as_str(), "x");
}

#[test]
fn application_cursor_and_bracketed_paste_follow_modes() {
    let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"\x1b[?1h\x1b[?2004h");
    let up = KeyEvent {
        key: fleet_proto::terminal::Key::Up,
        mods: fleet_proto::terminal::Modifiers::empty(),
        text: None,
        action: fleet_proto::terminal::KeyAction::Press,
    };
    assert_eq!(engine.encode_key(&up), b"\x1bOA");
    let ctrl_c = KeyEvent {
        key: fleet_proto::terminal::Key::Char('c'),
        mods: fleet_proto::terminal::Modifiers::CTRL,
        text: None,
        action: fleet_proto::terminal::KeyAction::Press,
    };
    assert_eq!(engine.encode_key(&ctrl_c), b"\x03");
    assert_eq!(engine.encode_paste("hello"), b"\x1b[200~hello\x1b[201~");
}

#[test]
fn terminal_queries_are_answered_for_full_screen_apps() {
    let mut engine = GhosttyEngine::new(80, 24, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));

    engine.feed(b"\x1b[c\x1b[>c\x1b[?7$p\x1b[6n\x1b[?u\x1b[18t\x1b[>q");
    let replies = engine
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            EngineEvent::PtyWrite(bytes) => Some(bytes),
            _ => None,
        })
        .collect::<Vec<_>>();

    for expected in [
        b"\x1b[?62;22c".as_slice(),
        b"\x1b[>1;0;0c".as_slice(),
        b"\x1b[?7;1$y".as_slice(),
        b"\x1b[1;1R".as_slice(),
        b"\x1b[?0u".as_slice(),
        b"\x1b[8;24;80t".as_slice(),
        b"\x1bP>|fleet 0.1.0\x1b\\".as_slice(),
    ] {
        assert!(
            replies.iter().any(|reply| reply == expected),
            "missing terminal reply {expected:?}; got {replies:?}"
        );
    }
}

#[test]
fn nvim_mode_keys_encode_in_legacy_and_kitty_modes() {
    let mut engine = GhosttyEngine::new(80, 24, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    let key = |key, mods, text: Option<&str>| KeyEvent {
        key,
        mods,
        text: text.map(str::to_owned),
        action: fleet_proto::terminal::KeyAction::Press,
    };

    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Escape,
            fleet_proto::terminal::Modifiers::empty(),
            None,
        )),
        b"\x1b"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('i'),
            fleet_proto::terminal::Modifiers::empty(),
            Some("i"),
        )),
        b"i"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char(';'),
            fleet_proto::terminal::Modifiers::SHIFT,
            Some(":"),
        )),
        b":"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('c'),
            fleet_proto::terminal::Modifiers::CTRL,
            None,
        )),
        b"\x03"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('['),
            fleet_proto::terminal::Modifiers::CTRL,
            None,
        )),
        b"\x1b[91;5u"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('x'),
            fleet_proto::terminal::Modifiers::ALT,
            Some("x"),
        )),
        b"\x1bx"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Up,
            fleet_proto::terminal::Modifiers::SHIFT,
            None,
        )),
        b"\x1b[1;2A"
    );

    // Nvim enables Kitty's disambiguation flag with CSI > 1 u. The encoder must follow
    // that live terminal mode instead of continuing to emit an incompatible legacy mix.
    engine.feed(b"\x1b[>1u");
    assert_eq!(engine.modes().kitty_keyboard_flags, 1);
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Escape,
            fleet_proto::terminal::Modifiers::empty(),
            None,
        )),
        b"\x1b[27u"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('i'),
            fleet_proto::terminal::Modifiers::empty(),
            Some("i"),
        )),
        b"i"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char(';'),
            fleet_proto::terminal::Modifiers::SHIFT,
            Some(":"),
        )),
        b":"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('c'),
            fleet_proto::terminal::Modifiers::CTRL,
            None,
        )),
        b"\x1b[99;5u"
    );
    assert_eq!(
        engine.encode_key(&key(
            fleet_proto::terminal::Key::Char('['),
            fleet_proto::terminal::Modifiers::CTRL,
            None,
        )),
        b"\x1b[91;5u"
    );
}

#[test]
fn modify_other_keys_encodes_ambiguous_control_keys() {
    let mut engine = GhosttyEngine::new(80, 24, 100)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    let shifted_semicolon = KeyEvent {
        key: fleet_proto::terminal::Key::Char(';'),
        mods: fleet_proto::terminal::Modifiers::SHIFT,
        text: Some(":".to_owned()),
        action: fleet_proto::terminal::KeyAction::Press,
    };
    let ctrl_i = KeyEvent {
        key: fleet_proto::terminal::Key::Char('i'),
        mods: fleet_proto::terminal::Modifiers::CTRL,
        text: None,
        action: fleet_proto::terminal::KeyAction::Press,
    };

    assert_eq!(engine.encode_key(&shifted_semicolon), b":");
    assert_eq!(engine.encode_key(&ctrl_i), b"\x1b[105;5u");
    engine.feed(b"\x1b[>4;2m");
    // Shifted punctuation remains its composed text, while an ambiguous C0 control key is
    // disambiguated exactly as xterm modifyOtherKeys level 2 specifies.
    assert_eq!(engine.encode_key(&shifted_semicolon), b":");
    assert_eq!(engine.encode_key(&ctrl_i), b"\x1b[27;5;105~");
}

#[test]
fn collects_terminal_effects() {
    let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    engine.feed(b"\x07\x1b]2;Fleet title\x07\x1b]7;file:///tmp\x07\x1b]52;c;aGVsbG8=\x07");
    let events = engine.take_events();
    assert!(events.contains(&EngineEvent::Bell));
    assert!(events.contains(&EngineEvent::Title("Fleet title".to_owned())));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::Cwd(_)))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        EngineEvent::ClipboardWrite { data, .. } if data == "hello"
    )));
}

#[test]
fn wheel_routing_uses_live_modes() {
    for (modes, shift, expected) in [
        ("", false, "viewport"),
        ("\x1b[?1000h\x1b[?1006h", false, "viewport"),
        ("", true, "viewport"),
        ("\x1b[?1000h\x1b[?1006h", true, "mouse"),
        ("\x1b[?1049h\x1b[?1000h\x1b[?1006h", false, "mouse"),
        ("\x1b[?1049h\x1b[?1000h\x1b[?1006h", true, "mouse"),
        ("\x1b[?1049h\x1b[?1007h\x1b[?1l", false, "arrow"),
        ("\x1b[?1049h\x1b[?1007h\x1b[?1h", false, "app_arrow"),
        ("\x1b[?1049h\x1b[?1007l", false, "drop"),
    ] {
        for steps in [-2, 2] {
            let mut engine = GhosttyEngine::new(80, 24, 1024 * 1024).unwrap();
            engine.feed(modes.as_bytes());
            let mods = if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::empty()
            };
            let event = WheelEvent {
                steps,
                col: 2,
                row: 3,
                mods,
            };
            let expected = match expected {
                "viewport" => WheelAction::Viewport(steps),
                "mouse" => WheelAction::Pty(match (steps < 0, shift) {
                    (true, false) => b"\x1b[<64;3;4M".repeat(2),
                    (false, false) => b"\x1b[<65;3;4M".repeat(2),
                    (true, true) => b"\x1b[<68;3;4M".repeat(2),
                    (false, true) => b"\x1b[<69;3;4M".repeat(2),
                }),
                "arrow" => WheelAction::Pty(if steps < 0 {
                    b"\x1b[A".repeat(2)
                } else {
                    b"\x1b[B".repeat(2)
                }),
                "app_arrow" => WheelAction::Pty(if steps < 0 {
                    b"\x1bOA".repeat(2)
                } else {
                    b"\x1bOB".repeat(2)
                }),
                _ => WheelAction::Drop,
            };
            assert_eq!(
                engine.wheel(&event),
                expected,
                "modes={modes:?} shift={shift} steps={steps}"
            );
        }
    }
}

#[test]
fn wheel_mouse_coordinates_clamp_at_grid_boundary() {
    let mut engine = GhosttyEngine::new(80, 24, 1024).unwrap();
    engine.feed(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
    for (steps, mods, expected) in [
        (-1, Modifiers::empty(), b"\x1b[<64;80;24M"),
        (1, Modifiers::empty(), b"\x1b[<65;80;24M"),
        (-1, Modifiers::SHIFT, b"\x1b[<68;80;24M"),
        (1, Modifiers::SHIFT, b"\x1b[<69;80;24M"),
    ] {
        assert_eq!(
            engine.wheel(&WheelEvent {
                steps,
                col: u16::MAX,
                row: u16::MAX,
                mods
            }),
            WheelAction::Pty(expected.to_vec())
        );
    }
}

#[test]
fn pty_wheel_expansion_is_bounded_by_rows_and_absolute_limit() {
    for (rows, limit) in [(24, 96), (300, 1024)] {
        for (modes, up, down) in [
            (
                b"\x1b[?1049h\x1b[?1000h\x1b[?1006h".as_slice(),
                b"\x1b[<64;1;1M".as_slice(),
                b"\x1b[<65;1;1M".as_slice(),
            ),
            (
                b"\x1b[?1049h\x1b[?1007h\x1b[?1l".as_slice(),
                b"\x1b[A".as_slice(),
                b"\x1b[B".as_slice(),
            ),
        ] {
            let mut engine = GhosttyEngine::new(80, rows, 1024).unwrap();
            engine.feed(modes);
            for steps in [0, -1, 1, i32::MIN, i32::MAX] {
                let count = (steps.unsigned_abs() as usize).min(limit);
                assert_eq!(
                    engine.wheel(&WheelEvent {
                        steps,
                        col: 0,
                        row: 0,
                        mods: Modifiers::empty()
                    }),
                    WheelAction::Pty(if steps < 0 { up } else { down }.repeat(count))
                );
            }
        }
    }
    let mut engine = GhosttyEngine::new(80, 24, 1024).unwrap();
    assert_eq!(
        engine.wheel(&WheelEvent {
            steps: i32::MIN,
            col: 0,
            row: 0,
            mods: Modifiers::empty()
        }),
        WheelAction::Viewport(i32::MIN)
    );
}

#[test]
fn output_preserves_history_anchor_and_bottom_follows() {
    let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    engine.feed("line\r\n".repeat(100).as_bytes());
    assert_eq!(engine.viewport_offset(), 0);
    engine.scroll(ScrollCommand::Lines(-20));
    let before = engine.take_frame(true);
    engine.feed(b"new output\r\nnew output\r\n");
    let after = engine.take_frame(true);
    assert_eq!(before.rows_changed, after.rows_changed);
    assert_eq!(after.viewport.offset, before.viewport.offset + 2);
}

#[test]
fn row_shift_matches_full_snapshot_and_invalidates_on_output_resize() {
    let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    engine.feed(
        (0..100)
            .map(|i| format!("{i} {}\r\n", "x".repeat(i % 70)))
            .collect::<String>()
            .as_bytes(),
    );
    let mut mirror = engine
        .take_frame(true)
        .rows_changed
        .into_iter()
        .map(|r| (r.cells, r.wrapped))
        .collect::<Vec<_>>();
    for delta in [-3, 1, -4, 2] {
        engine.scroll(ScrollCommand::Lines(delta));
        let frame = engine.take_frame(false);
        assert_eq!(frame.shift, Some(delta));
        assert!(!frame.full);
        assert_eq!(frame.rows_changed.len(), delta.unsigned_abs() as usize);
        if delta > 0 {
            mirror.rotate_left(delta as usize);
        } else {
            mirror.rotate_right(delta.unsigned_abs() as usize);
        }
        for row in frame.rows_changed {
            mirror[usize::from(row.index)] = (row.cells, row.wrapped);
        }
        assert_eq!(
            mirror,
            engine
                .take_frame(true)
                .rows_changed
                .into_iter()
                .map(|r| (r.cells, r.wrapped))
                .collect::<Vec<_>>()
        );
    }
    engine.scroll(ScrollCommand::Lines(-1));
    engine.feed(b"new output");
    assert_eq!(engine.take_frame(false).shift, None);
    engine.scroll(ScrollCommand::Lines(-1));
    engine.resize(41, 10).unwrap();
    assert_eq!(engine.take_frame(false).shift, None);
    engine.scroll(ScrollCommand::Lines(-1));
    assert_eq!(engine.take_frame(true).shift, None);
}

#[test]
fn history_epoch_change_disables_pending_shift_and_replaces_every_row() {
    let mut engine = GhosttyEngine::new(8, 3, 1024 * 1024).unwrap();
    engine.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    let epoch = engine.take_frame(true).viewport.history_epoch;
    engine.scroll(ScrollCommand::Lines(-1));
    assert_ne!(engine.pending_shift, 0);
    // Emulate the backend pruning history without passing through feed's invalidation.
    engine.terminal.vt_write(b"\x1b[3J");
    let frame = engine.take_frame(false);
    assert!(frame.viewport.history_epoch > epoch);
    assert_eq!(frame.shift, None);
    assert!(frame.full);
    assert_eq!(frame.rows_changed.len(), usize::from(frame.rows));
}

#[test]
fn scrollback_budget_is_bytes() {
    let mut engine = GhosttyEngine::new(40, 10, 10 * 1024 * 1024)
        .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
    let output = (1..=5_000)
        .map(|line| format!("{line}\r\n"))
        .collect::<String>();

    engine.feed(output.as_bytes());

    assert_eq!(engine.take_frame(true).viewport.scrollback_len, 4_991);
}

#[test]
fn small_byte_budget_bounds_history_and_zero_disables_it() {
    let output = "1234567890\r\n".repeat(10_000);
    for budget in [0, 10_000] {
        let mut engine = GhosttyEngine::new(40, 10, budget).unwrap();
        engine.feed(output.as_bytes());
        let retained = engine.viewport().scrollback_len;
        if budget == 0 {
            assert_eq!(retained, 0);
        } else {
            let mut raw = Terminal::new(TerminalOptions {
                cols: 40,
                rows: 10,
                max_scrollback: budget,
            })
            .unwrap();
            raw.vt_write(output.as_bytes());
            assert_eq!(retained, raw.scrollback_rows().unwrap());
            assert!(
                retained < 4_991,
                "byte budget must retain less than the 10 MiB case"
            );
        }
    }
}

#[test]
fn converted_cells_preserve_unicode_clusters_and_blank_cells() {
    for (text, width) in [
        (" ".to_owned(), CellWidth::Narrow),
        ("é".to_owned(), CellWidth::Narrow),
        ("e\u{301}".to_owned(), CellWidth::Narrow),
        ("中".to_owned(), CellWidth::Wide),
        ("🙂".to_owned(), CellWidth::Wide),
        (format!("e{}", "\u{301}".repeat(30)), CellWidth::Narrow),
    ] {
        let mut engine = GhosttyEngine::new(20, 2, 1024).unwrap();
        engine.feed(text.as_bytes());
        let frame = engine.take_frame(true);
        assert_eq!(frame.rows_changed[0].cells[0].text.as_str(), text);
        assert_eq!(frame.rows_changed[0].cells[0].width, width);
        assert!(
            frame.rows_changed[1]
                .cells
                .iter()
                .all(|cell| cell.text.is_empty())
        );
        if width == CellWidth::Wide {
            assert_eq!(frame.rows_changed[0].cells[1].width, CellWidth::Spacer);
        }
    }
}

#[test]
fn frame_titles_only_own_changed_or_full_frame_titles() {
    let mut engine = GhosttyEngine::new(20, 2, 1024).unwrap();
    engine.feed(b"\x1b]2;first\x07");
    assert_eq!(engine.take_frame(true).title.as_deref(), Some("first"));
    engine.feed(b"x");
    assert_eq!(engine.take_frame(false).title, None);
    assert_eq!(engine.take_frame(true).title.as_deref(), Some("first"));
    engine.feed(b"\x1b]2;second\x07");
    assert_eq!(engine.take_frame(false).title.as_deref(), Some("second"));
    engine.feed(b"\x1b]2;\x07");
    assert_eq!(engine.take_frame(false).title, None);
    assert_eq!(engine.last_frame_title, None);
}
