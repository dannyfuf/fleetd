//! The slice of the GUI corpus that rides `make test`.
//!
//! `make harness` drives the whole corpus on a real compositor, which no CI machine has. This
//! wrapper runs the scenarios the headless lane can honour as part of `cargo test --workspace`,
//! so a corpus regression is caught by the ordinary test command rather than only by a developer
//! who remembered to run the harness. `Makefile`'s `harness-headless` target selects the same
//! scenarios by the same rule, one runner invocation per file.
//!
//! Selection is textual and mechanical, because the corpus grows without this file: every
//! `*.scenario` and `*.txt` under `scenarios/` is considered, and one is excluded only when it
//! names a directive the headless lane cannot answer. Nothing here is a list of scenario names.
//!
//! Each scenario costs one private `fleetd`, one `fleet` and one hermetic `FLEET_HOME`, which is
//! about a third of a second for a plain scenario and a few seconds for one that kills and
//! restarts the daemon. `make test` builds `fleet-daemon`, `fleet-app` and `fleet-harness` before
//! the suite so those binaries exist; `cargo test -p fleet-app` on its own finds them beside the
//! test executable in `target/debug/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fleet_harness::{
    lane::Lane,
    scenario::{self, RunOptions},
};

/// Extensions the runner treats as scenarios (`fleet_harness::scenario::SCENARIO_EXTENSIONS`).
const SCENARIO_EXTENSIONS: [&str; 2] = ["scenario", "txt"];

/// Directives no headless run can answer, whatever else the scenario does.
///
/// `shot` has no compositor surface to photograph and `clipboard` cannot read its own write back
/// (`docs/TESTING-HARNESS.md`, sections 1 and 4). A scenario naming either is pixel-bound by
/// construction and belongs to `make harness`. `Makefile`'s `HARNESS_PIXEL_BOUND` is this list.
const PIXEL_BOUND_DIRECTIVES: [&str; 2] = ["shot", "clipboard"];

/// What this subset may cost `make test` before it should move back to `make harness-headless`.
const BUDGET: Duration = Duration::from_secs(60);

#[tokio::test]
async fn every_scenario_the_headless_lane_can_honour_passes() {
    let corpus = corpus_root();
    assert!(
        corpus.is_dir(),
        "the scenario corpus is missing at {}",
        corpus.display()
    );

    let mut runnable = Vec::new();
    let mut excluded = Vec::new();
    for scenario in scenarios_under(&corpus) {
        let source = std::fs::read_to_string(&scenario)
            .unwrap_or_else(|error| panic!("read {}: {error}", scenario.display()));
        match unrunnable_directive(&source) {
            Some(directive) => excluded.push((scenario, directive)),
            None => runnable.push(scenario),
        }
    }

    for (scenario, directive) in &excluded {
        eprintln!(
            "skip {} ({directive} needs the virtual lane)",
            show(&corpus, scenario)
        );
    }

    assert!(
        !runnable.is_empty(),
        "no scenario under {} runs without pixels, so this test proves nothing; \
         docs/TESTING-HARNESS.md section 7 requires a pixel-free slice of the corpus",
        corpus.display()
    );

    let started = Instant::now();
    let mut failures = Vec::new();
    for scenario in &runnable {
        let began = Instant::now();
        let outcome = scenario::run_path(
            scenario,
            RunOptions {
                lane: Lane::Headless,
                keep: false,
                run_dir: None,
                continue_on_failure: false,
                update_baselines: false,
            },
        )
        .await;
        eprintln!(
            "ran  {} in {:.1}s",
            show(&corpus, scenario),
            began.elapsed().as_secs_f64()
        );
        if let Err(error) = outcome {
            failures.push(format!("{}: {error:#}", show(&corpus, scenario)));
        }
    }

    let elapsed = started.elapsed();
    eprintln!(
        "headless subset: {} scenario(s) in {:.1}s, {} skipped",
        runnable.len(),
        elapsed.as_secs_f64(),
        excluded.len()
    );
    if elapsed > BUDGET {
        eprintln!(
            "warning: the headless subset now costs make test {:.0}s, over its {:.0}s budget; \
             move scenarios back to `make harness-headless` (docs/TESTING-HARNESS.md)",
            elapsed.as_secs_f64(),
            BUDGET.as_secs_f64()
        );
    }

    assert!(
        failures.is_empty(),
        "{} headless scenario(s) failed; each run directory holds the evidence:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The corpus directory, which sits beside `crates/` at the workspace root.
fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/fleet-app is two directories below the workspace root")
        .join("scenarios")
}

/// Every scenario under `directory`, recursing in the lexical order a directory run uses.
///
/// This mirrors `fleet_harness::scenario::collect_scenarios`, which is private; the two agree on
/// the extension list above.
fn scenarios_under(directory: &Path) -> Vec<PathBuf> {
    let mut entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut found = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            found.extend(scenarios_under(&path));
        } else if path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| SCENARIO_EXTENSIONS.contains(&extension))
        {
            found.push(path);
        }
    }
    found
}

/// The first directive that keeps this scenario out of the headless lane, if there is one.
///
/// Only the leading word of a directive line counts, so a comment mentioning `shot` and a
/// predicate whose value happens to read `shot` are both ignored, exactly as the runner's own
/// parser ignores them.
fn unrunnable_directive(source: &str) -> Option<&'static str> {
    source.lines().find_map(|line| {
        let line = line.trim_start();
        if line.starts_with('#') {
            return None;
        }
        let word = line.split_whitespace().next()?;
        PIXEL_BOUND_DIRECTIVES
            .into_iter()
            .find(|directive| *directive == word)
    })
}

/// A scenario path as the corpus names it, so failures read like the files on disk.
fn show(corpus: &Path, scenario: &Path) -> String {
    scenario
        .strip_prefix(corpus)
        .unwrap_or(scenario)
        .display()
        .to_string()
}

#[test]
fn a_scenario_is_excluded_by_its_directives_and_never_by_its_prose() {
    assert_eq!(
        unrunnable_directive("fixture: busy\nawait idle\nshot hub\nquit\n"),
        Some("shot")
    );
    assert_eq!(
        unrunnable_directive("fixture: busy\n  clipboard get\nquit\n"),
        Some("clipboard")
    );
    // The keyboard reaches the focused view in the headless lane, so a key scenario belongs to
    // this subset rather than to `make harness`. It was quarantined while the app's stale-key
    // queue waited on a frame callback the headless platform never delivers.
    assert_eq!(
        unrunnable_directive("fixture: busy\n  key ctrl-s A\ntype hello\nquit\n"),
        None
    );
    assert_eq!(
        unrunnable_directive("# shot: taken once the key lands\nfixture: busy\nquit\n"),
        None
    );
    assert_eq!(
        unrunnable_directive("fixture: busy\nawait toasts[0].text ~= shot\nquit\n"),
        None
    );
    assert_eq!(
        unrunnable_directive("fixture: one-repo\ndaemon kill\nclick hub.tab[1]\nquit\n"),
        None
    );
}
