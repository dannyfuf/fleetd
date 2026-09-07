use super::*;

use fleet_core::github::InspectionPrState;

/// Everything the worktree variant needs.
pub struct WorktreeProps<'a> {
    /// The worktree under the cursor.
    pub worktree: &'a Worktree,
    /// Its runtime status, for the SESSION block.
    pub status: Option<&'a WorktreeStatus>,
    /// Whether its session is slept rather than merely detached.
    pub slept: bool,
    /// Whether its host's last probe failed.
    pub host_unreachable: bool,
    /// The inspection cache entry, which may be missing, loading or errored.
    pub inspected: Option<&'a Inspected>,
    /// `$HOME`, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// §3.4's worktree panel: head, `path`, SESSION, SAFETY, times, freshness footer.
#[must_use]
pub fn worktree(props: WorktreeProps<'_>, cx: &App) -> AnyElement {
    let WorktreeProps {
        worktree,
        status,
        slept,
        host_unreachable,
        inspected,
        home,
        now,
    } = props;

    let host = worktree
        .host
        .as_ref()
        .map_or_else(|| "local".to_owned(), |host| format!("@{host}"));
    let subtitle = format!("{} · {} · {host}", worktree.repo_id, worktree.base_ref);

    let session_state = status.map_or(SessionState::None, |status| status.session);
    let glyph = row_glyph(
        session_state,
        slept,
        status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
        worktree.degraded.is_some(),
        host_unreachable,
        false,
    );

    let mut children = vec![
        head(
            Icon::GitBranch,
            SharedString::from(worktree.branch.clone()),
            SharedString::from(subtitle),
            cx,
        ),
        block(
            vec![
                FactRow::new("path", path_value(&worktree.path, home, cx))
                    .mono(true)
                    .into_any_element(),
            ],
            cx,
        ),
        session_block(glyph, status, cx),
    ];
    children.push(safety_block(inspected, now, cx));
    children.push(times_block(worktree, now, cx));
    if let Some(footer) = freshness_footer(inspected, now, cx) {
        children.push(footer);
    }

    variant(children)
}

/// `SESSION  ◉ attached` plus one row per terminal (§3.4) — the only place window names exist.
fn session_block(glyph: StatusKind, status: Option<&WorktreeStatus>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let header = SectionHeader::new("Session").trailing(
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .child(
                StatusGlyph::new(glyph)
                    .size(IconSize::Medium)
                    .id("detail-session-glyph"),
            )
            .child(Text::ui(glyph.detail_word()).muted()),
    );
    let windows: Vec<AnyElement> = status
        .map(|status| {
            status
                .windows
                .iter()
                .map(|window| {
                    let label = if window.keep_alive.is_empty() {
                        FactValue::Null
                    } else {
                        FactValue::known(window.keep_alive.join(", "))
                    };
                    FactRow::new(format!("{} {}", window.index + 1, window.name), label)
                        .into_any_element()
                })
                .collect()
        })
        .unwrap_or_default();

    let mut children = vec![header.into_any_element()];
    children.extend(windows);
    block(children, cx)
}

/// The SAFETY block: exactly the facts the delete confirm will quote, plus their age.
fn safety_block(inspected: Option<&Inspected>, now: i64, cx: &App) -> AnyElement {
    let Some(inspected) = inspected else {
        return block(
            vec![
                SectionHeader::new("Safety").into_any_element(),
                Text::ui("not checked · I to check")
                    .faint()
                    .into_any_element(),
            ],
            cx,
        );
    };
    if let Some(error) = inspected.failure() {
        return block(
            vec![
                SectionHeader::new("Safety").into_any_element(),
                Text::ui(format!("error: {error}"))
                    .tone(Tone::Danger)
                    .into_any_element(),
                Text::hint("I  retry").into_any_element(),
            ],
            cx,
        );
    }
    let Some(data) = inspected.data.as_ref() else {
        return block(
            vec![
                SectionHeader::new("Safety")
                    .trailing(Text::ui("\u{27F3} checking\u{2026}").muted())
                    .into_any_element(),
            ],
            cx,
        );
    };

    let age = age_secs(&data.inspected_at, now).unwrap_or_default();
    let mut list = KeyValueList::titled("Safety")
        .trailing(FreshnessStamp::new("checked", age))
        .row("dirty", dirty_value(data))
        .row(
            "ahead / behind",
            match (data.ahead, data.behind) {
                (Some(ahead), Some(behind)) => {
                    FactValue::known(format!("\u{21E1}{ahead} \u{21E3}{behind}"))
                }
                _ => FactValue::Null,
            },
        )
        .row(
            "unique commits",
            FactValue::from_option(data.unique_commits.map(|count| count.to_string())),
        )
        .row("published", FactValue::known(yes_no(data.published)))
        .row("merged", FactValue::known(yes_no(data.merged)));
    if let Some(pr) = data.pr.as_ref() {
        list = list.row(
            "PR",
            FactValue::known(format!("#{} {}", pr.number, pr_state_word(pr.state))),
        );
    }

    let mut children = vec![list.into_any_element()];
    children.extend(
        data.warnings
            .iter()
            .map(|warning| FactRow::warning(warning.clone()).into_any_element()),
    );

    let body = block(children, cx);
    if inspected.loading {
        // §3.4: a running inspection dims the previous values, it never blanks them.
        return div()
            .opacity(cx.theme().metrics.refreshing_opacity)
            .child(body)
            .into_any_element();
    }
    body
}

/// `dirty  12 files` — a file count when the status collection succeeded.
fn dirty_value(data: &WorktreeInspection) -> FactValue {
    match (data.dirty, data.dirty_files) {
        (true, Some(count)) => FactValue::known(format!("{count} files")),
        (true, None) => FactValue::known("yes"),
        (false, _) => FactValue::known("clean"),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn pr_state_word(state: InspectionPrState) -> &'static str {
    match state {
        InspectionPrState::Open => "open",
        InspectionPrState::Merged => "merged",
        InspectionPrState::Closed => "closed",
    }
}

/// `opened 2h ago · created 5d ago` — low priority by definition, so it sits last.
fn times_block(worktree: &Worktree, now: i64, cx: &App) -> AnyElement {
    let opened = worktree
        .last_opened_at
        .as_deref()
        .and_then(|iso| age_secs(iso, now))
        .map(|age| format!("opened {} ago", format_age(age)));
    let created =
        age_secs(&worktree.created_at, now).map(|age| format!("created {} ago", format_age(age)));
    let line = [opened, created]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    block(vec![Text::ui(line).muted().into_any_element()], cx)
}

/// `inspected <age> · I refresh`, shown only past 60 s (§2.6) so fresh facts stay quiet.
fn freshness_footer(inspected: Option<&Inspected>, now: i64, cx: &App) -> Option<AnyElement> {
    let data = inspected?.data.as_ref()?;
    let age = age_secs(&data.inspected_at, now)?;
    if age <= 60 {
        return None;
    }
    Some(
        div()
            .px(cx.theme().space.md)
            .pt(cx.theme().space.sm)
            .child(FreshnessStamp::new("inspected", age).action("I", "refresh"))
            .into_any_element(),
    )
}
