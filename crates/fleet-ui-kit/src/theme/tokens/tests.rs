//! The token inventory and `docs/DESIGN-SYSTEM.md` §2 must move together.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The token source, read back as text: Rust has no reflection, so the field inventory the doc
/// must mirror is parsed out of the source that declares it.
const SOURCE: &str = include_str!("../tokens.rs");
/// The half of the contract written in prose (`docs/README.md:9`).
const DOC: &str = include_str!("../../../../../docs/DESIGN-SYSTEM.md");
/// The sentence §2.8 introduces the opacity ladder with.
const LADDER: &str = "The same token set owns the opacity ladder";

/// The lines of `SOURCE` after the first line that starts (ignoring indent) with `start`, up to
/// the closing brace of that item or of a `Self {` body.
fn block(start: &str) -> impl Iterator<Item = &'static str> {
    SOURCE
        .lines()
        .skip_while(move |line| !line.trim_start().starts_with(start))
        .skip(1)
        .take_while(|line| *line != "}" && *line != "        }")
}

/// `name: value,` pairs from a `Default` body, keyed by field name.
fn defaults(start: &str) -> BTreeMap<&'static str, &'static str> {
    block(start)
        .filter_map(|line| line.trim().strip_suffix(',')?.split_once(": "))
        .collect()
}

/// Every backticked `name value` pair in `section`.
fn ladder(section: &str) -> BTreeMap<&str, &str> {
    section
        .split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|span| span.split_once(' '))
        .collect()
}

/// The paragraph of §2.8 that lists the opacity ladder.
fn documented_ladder() -> BTreeMap<&'static str, &'static str> {
    let start = DOC
        .find(LADDER)
        .unwrap_or_else(|| panic!("§2.8 still introduces the ladder with {LADDER:?}"));
    let paragraph = &DOC[start..];
    let end = paragraph
        .find("\n\n")
        .unwrap_or_else(|| panic!("the ladder paragraph is terminated by a blank line"));
    ladder(&paragraph[..end])
}

/// The `| `token` | ms | what |` rows of §2.7.
fn documented_motion() -> BTreeMap<&'static str, &'static str> {
    let start = DOC
        .find("### 2.7 Motion")
        .unwrap_or_else(|| panic!("§2.7 is still the motion section"));
    let section = &DOC[start..];
    let end = section
        .find("### 2.8")
        .unwrap_or_else(|| panic!("§2.7 is followed by §2.8"));
    section[..end]
        .lines()
        .filter(|line| line.starts_with("| `"))
        .filter_map(|line| {
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            Some((fields.get(1)?.trim_matches('`'), *fields.get(2)?))
        })
        .collect()
}

/// The text of `DOC` from `start` up to (not including) `end`.
fn doc_between(start: &str, end: &str) -> &'static str {
    let from = DOC
        .find(start)
        .unwrap_or_else(|| panic!("DESIGN-SYSTEM.md still has {start:?}"));
    let rest = &DOC[from..];
    let to = rest
        .find(end)
        .unwrap_or_else(|| panic!("{start:?} is still followed by {end:?}"));
    &rest[..to]
}

/// A token's declared value as the doc spells it: `px(560.0)` is `560`, `ch(4.0)` is `4ch`,
/// `px(CH)` is the cell width.
fn doc_value(declared: &str) -> String {
    let number = |inner: &str| match inner {
        "CH" => super::CH.to_string(),
        other => other
            .parse::<f32>()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| panic!("{declared} is a literal")),
    };
    if let Some(inner) = declared
        .strip_prefix("px(")
        .and_then(|v| v.strip_suffix(')'))
    {
        number(inner)
    } else if let Some(inner) = declared
        .strip_prefix("ch(")
        .and_then(|v| v.strip_suffix(')'))
    {
        format!("{}ch", number(inner))
    } else {
        panic!("{declared} is neither px(..) nor ch(..)")
    }
}

/// The field names of `pub struct <name>`.
fn fields(start: &str) -> BTreeSet<&'static str> {
    block(start)
        .filter_map(|line| line.trim().strip_prefix("pub ")?.split_once(": "))
        .map(|(name, _)| name)
        .collect()
}

/// The first backticked cell of every table row in `section`.
fn table_roles(section: &str) -> BTreeMap<&str, Vec<&str>> {
    section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .filter_map(|line| {
            let cells: Vec<_> = line.split('|').map(str::trim).collect();
            Some((cells.get(1)?.trim_matches('`'), cells))
        })
        .collect()
}

#[test]
fn the_documented_metrics_match_the_metrics_tokens() {
    let declared: BTreeMap<_, _> = defaults("impl Default for Metrics")
        .into_iter()
        .filter(|(name, _)| !name.ends_with("_opacity"))
        .map(|(name, value)| (name, doc_value(value)))
        .collect();
    assert!(!declared.is_empty(), "Metrics still declares geometry");
    let documented: BTreeMap<_, _> = ladder(doc_between("### 2.8 Metrics", LADDER))
        .into_iter()
        .map(|(name, value)| (name, value.to_owned()))
        .collect();
    assert_eq!(
        declared, documented,
        "§2.8's metric inventory and Metrics must move together"
    );
}

#[test]
fn the_documented_radii_match_the_radii_tokens() {
    let declared: BTreeMap<_, _> = defaults("impl Default for Radii")
        .into_iter()
        .map(|(name, value)| (name, doc_value(value)))
        .collect();
    let documented: BTreeMap<_, _> = ladder(doc_between("### 2.5 Radii", "### 2.6"))
        .into_iter()
        .map(|(name, value)| (name, value.to_owned()))
        .collect();
    assert_eq!(
        declared, documented,
        "§2.5's radii and Radii must move together"
    );
}

#[test]
fn every_color_role_is_documented_with_its_opaque_values() {
    let roles = table_roles(doc_between("### 2.1 Color roles", "### 2.2"));
    let declared = fields("pub struct ColorTokens");
    assert!(!declared.is_empty(), "ColorTokens still declares roles");
    let documented: BTreeSet<_> = roles.keys().copied().collect();
    assert_eq!(
        declared, documented,
        "§2.1's role table and ColorTokens must name the same roles"
    );
    for (column, mode, body) in [
        (2, "dark", "pub fn dark() -> Self {"),
        (3, "light", "pub fn light() -> Self {"),
    ] {
        for (name, value) in defaults(body)
            .into_iter()
            .take_while(|(name, _)| declared.contains(name))
        {
            let Some(hex) = value.strip_prefix("c(0x").and_then(|v| v.strip_suffix(')')) else {
                continue;
            };
            assert_eq!(
                roles[name][column],
                format!("`#{hex}`"),
                "§2.1 documents `{name}` with a different {mode} value"
            );
        }
    }
}

#[test]
fn every_type_role_is_documented() {
    let roles = table_roles(doc_between("### 2.3 Type scale", "### 2.4"));
    let documented: BTreeSet<_> = roles.keys().copied().collect();
    assert_eq!(
        fields("pub struct TypeScale"),
        documented,
        "§2.3's type table and TypeScale must name the same roles"
    );
}

#[test]
fn the_documented_opacity_ladder_matches_the_metrics_tokens() {
    let declared: BTreeMap<_, _> = defaults("impl Default for Metrics")
        .into_iter()
        .filter_map(|(name, value)| Some((name.strip_suffix("_opacity")?, value)))
        .collect();
    assert!(!declared.is_empty(), "Metrics still names its opacities");
    assert_eq!(
        declared,
        documented_ladder(),
        "§2.8's opacity ladder and Metrics must move together"
    );
}

#[test]
fn the_documented_motion_table_matches_the_motion_tokens() {
    let declared = defaults("impl Default for Motion");
    assert!(!declared.is_empty(), "Motion still names its durations");
    assert_eq!(
        declared,
        documented_motion(),
        "§2.7's motion table and Motion must move together"
    );
}

#[test]
fn every_motion_token_has_a_consumer() {
    let fields = fields("pub struct Motion");
    assert!(!fields.is_empty(), "Motion still declares fields");

    let mut sources = String::new();
    let mut stack = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                // The token module declares the durations; a consumer lives outside it.
                if path.file_name().is_some_and(|name| name != "theme") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }

    let unread: Vec<_> = fields
        .iter()
        .filter(|field| {
            !sources
                .match_indices(&format!("motion.{field}"))
                .any(|(at, _)| {
                    let next = sources[at + field.len() + 7..].chars().next();
                    !next.is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
        })
        .collect();
    assert!(
        unread.is_empty(),
        "these motion tokens have no consumer, so wire them or delete them: {unread:?}"
    );
}
