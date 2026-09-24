use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::*;

#[test]
fn every_bound_action_has_a_hand_written_entry() {
    let missing: Vec<&str> = keymap::table()
        .iter()
        .map(|spec| spec.action)
        .filter(|action| info(action).is_none())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    assert!(
        missing.is_empty(),
        "bound actions with no catalogue entry (add them to action_catalogue/entries.rs): \
         {missing:?}"
    );
}

#[test]
fn every_catalogued_action_is_bound_somewhere() {
    // A typo in an entry's action name would otherwise leave a label nothing ever reads.
    let bound: HashSet<&str> = keymap::table().iter().map(|spec| spec.action).collect();
    for entry in entries() {
        for action in entry.actions {
            assert!(
                bound.contains(action),
                "`{action}` is catalogued but bound nowhere"
            );
        }
    }
}

#[test]
fn no_action_is_catalogued_twice() {
    let mut seen = HashSet::new();
    for entry in entries() {
        for action in entry.actions {
            assert!(seen.insert(*action), "`{action}` has two entries");
        }
    }
}

#[test]
fn labels_are_hand_written_sentences() {
    for entry in entries() {
        let ActionInfo {
            label,
            short_label,
            description,
            ..
        } = entry.info;
        for text in [label, short_label] {
            assert!(!text.is_empty(), "{:?} has an empty label", entry.actions);
            assert!(
                text.chars().next().is_some_and(char::is_uppercase) || text.starts_with("fleetd"),
                "`{text}` is not sentence case"
            );
            assert!(!text.contains("::"), "`{text}` names a type");
            assert!(!text.ends_with('.'), "`{text}` is a label, not a sentence");
        }
        assert!(
            description.ends_with('.'),
            "the description of `{label}` is one full sentence line: `{description}`"
        );
        assert!(
            !description.contains('\n'),
            "`{label}`'s description is one line"
        );
    }
}

#[test]
fn a_label_is_unique_in_its_place() {
    // Help's search and the palette tell entries apart by label; two identical labels in one
    // place would be two rows a reader cannot choose between.
    let mut seen: HashMap<(Place, &str), &[&str]> = HashMap::new();
    for entry in entries() {
        if let Some(other) = seen.insert((entry.info.place, entry.info.label), entry.actions) {
            panic!(
                "`{}` is the label of both {other:?} and {:?} in {:?}",
                entry.info.label, entry.actions, entry.info.place
            );
        }
    }
}

#[test]
fn ranges_are_one_entry_with_a_range_label() {
    for (member, label) in [
        ("prefix::SelectTab5", "Go to tab 1–9"),
        ("native_agent::SelectTab9", "Go to tab 1–9"),
        ("hub::SelectContext3", "Switch to context 1–9"),
        ("native_agent::Choose2", "Choose answer 1–5"),
    ] {
        let entry = entry(member).unwrap_or_else(|| panic!("`{member}` is not catalogued"));
        assert!(entry.is_range(), "`{member}` belongs to a range");
        assert_eq!(entry.info.label, label);
    }
    for entry in entries().iter().filter(|entry| entry.is_range()) {
        assert!(
            entry.info.label.contains('–'),
            "the range {:?} needs a range label",
            entry.actions
        );
    }
}

#[test]
fn every_key_context_has_a_place_and_a_heading() {
    for spec in keymap::table() {
        assert!(
            Place::of_context(spec.context).is_some(),
            "`{}` has no place in action_catalogue/contexts.rs",
            spec.context
        );
        assert!(context_title(spec.context).is_some());
    }
}

#[test]
fn an_entry_is_filed_where_its_keys_work() {
    // `place` is Help's "Where" column: an entry filed under a place none of its keys reach
    // would send the reader to the wrong screen.
    for placed in all() {
        let info = placed.info();
        if info.place == Place::Everywhere {
            continue;
        }
        assert!(
            placed
                .bindings
                .iter()
                .any(|spec| Place::of_context(spec.context) == Some(info.place)),
            "`{}` is filed under {:?} but bound only in {:?}",
            info.label,
            info.place,
            placed
                .bindings
                .iter()
                .map(|spec| spec.context)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn for_place_lists_the_keys_that_work_there() {
    let terminal: Vec<&Placed> = for_place(Place::Terminal).collect();
    let help = terminal
        .iter()
        .find(|placed| placed.entry.action() == "fleet::OpenHelp")
        .unwrap_or_else(|| panic!("help is reachable from a terminal"));
    assert_eq!(
        help.keys,
        vec!["?"],
        "only the terminal's key, not the Hub's"
    );

    let tabs = terminal
        .iter()
        .find(|placed| placed.info().label == "Go to tab 1–9")
        .unwrap_or_else(|| panic!("no tab range in the terminal"));
    assert_eq!(tabs.key_range(), Some(("1", "9")));

    let thread_tabs = for_place(Place::AgentThread)
        .find(|placed| placed.info().label == "Go to tab 1–9")
        .unwrap_or_else(|| panic!("no tab range in an agent thread"));
    assert_eq!(thread_tabs.key_range(), Some(("ctrl-s 1", "ctrl-s 9")));

    assert!(
        for_place(Place::Hub).all(|placed| placed.entry.action() != "prefix::NewTerminal"),
        "a terminal-only command is not offered in the Hub"
    );
}

#[test]
fn the_order_is_stable() {
    let first: Vec<&str> = for_place(Place::Worktrees)
        .map(|placed| placed.info().label)
        .collect();
    let second: Vec<&str> = for_place(Place::Worktrees)
        .map(|placed| placed.info().label)
        .collect();
    assert_eq!(first, second);
    assert_eq!(first.first(), Some(&"Open the selected worktree"));
    assert_eq!(all().count(), entries().len());
}

#[test]
fn nothing_user_visible_spells_an_action_name() {
    // `humanize` turns `hub::MoveDown` into "move down". It may stay as a debugging aid, but a
    // label a person reads comes from this catalogue.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = [
        src.join("presentation/keys.rs"),
        src.join("action_catalogue/tests.rs"),
    ];
    let mut offenders = Vec::new();
    visit(&src, &mut |path, text| {
        if !allowed.iter().any(|allowed| allowed == path) && text.contains("humanize(") {
            offenders.push(path.display().to_string());
        }
    });
    assert!(
        offenders.is_empty(),
        "user-visible code calls `humanize`; read `action_catalogue::info` instead: {offenders:?}"
    );
}

fn visit(dir: &Path, found: &mut impl FnMut(&Path, &str)) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{dir:?}: {error}"));
    for entry in entries {
        let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
        if path.is_dir() {
            visit(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
            found(&path, &text);
        }
    }
}

#[test]
fn an_action_bound_in_one_place_is_catalogued_there() {
    // Help lists a key under its entry's place: the Hub's `X` catalogued as `Everywhere` told a
    // reader it dismisses the error from a terminal too, where it types an `X`.
    let table = keymap::table();
    for entry in entries() {
        let places: HashSet<Place> = table
            .iter()
            .filter(|spec| entry.actions.contains(&spec.action))
            .filter_map(|spec| Place::of_context(spec.context))
            .collect();
        if let [place] = places.into_iter().collect::<Vec<_>>()[..] {
            assert_eq!(
                entry.info.place, place,
                "{:?} is bound only in {place:?}",
                entry.actions
            );
        }
    }
}
