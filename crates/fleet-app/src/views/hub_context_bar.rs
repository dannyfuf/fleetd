//! The Hub's half of the context bar (UX-SPEC §3.1).
//!
//! # Who draws the bar
//!
//! The 36 px context bar is **persistent chrome**: `docs/APP-CONTRACTS.md` §2 rule 4 gives the
//! shell sole ownership of it, and [`crate::shell::chrome::context_bar`] already implements
//! §3.1 in full — the numbered tabs, the 2 px blue active underline, the `+n` overflow chip,
//! the zero-suppressed status chips of §2.3 in their fixed order, and the daemon dot. A screen
//! that drew a second bar would double the row.
//!
//! What is left of §3.1, and what lives here, is the **keyboard half**: `1`–`9`, `gt` / `gT`
//! and the `flag` chip's count. Those are Hub actions (`Fleet > Hub` key context), the shell
//! does not listen for them, and every one of them is a pure function of the context list, so
//! they are unit-tested here and merely dispatched from [`crate::screens::hub`].
//!
//! Zero-suppression is not re-implemented either: [`fleet_ui_kit::Chip`] suppresses a zero
//! count itself, which is why the chips never need a caller-side `if count > 0`.

use fleet_core::{ids::ContextId, model::Context as FleetContext};

/// How many context tabs the bar can address by digit (`1`–`9`, §2.2).
pub const ADDRESSABLE_TABS: usize = 9;

/// The index of the active context, or `0` when nothing is active.
///
/// The bar underlines this tab, and `gt` / `gT` step from it.
#[must_use]
pub fn active_index(contexts: &[FleetContext], active: Option<&ContextId>) -> usize {
    active
        .and_then(|active| contexts.iter().position(|context| &context.id == active))
        .unwrap_or(0)
}

/// The context a digit key selects, one-based as it is printed on the tab.
///
/// Digits past the end of the list select nothing rather than clamping: pressing `7` with four
/// contexts must not silently land on the fourth (§3.1 — contexts past 9 are `gt` / palette
/// reachable, and a digit always means *that* tab).
#[must_use]
pub fn context_for_digit(contexts: &[FleetContext], digit: usize) -> Option<&ContextId> {
    if digit == 0 || digit > ADDRESSABLE_TABS {
        return None;
    }
    contexts.get(digit - 1).map(|context| &context.id)
}

/// The context `gt` (`delta = 1`) or `gT` (`delta = -1`) moves to, wrapping at both ends.
///
/// Wrapping is what makes `gt` usable with two contexts, and it reaches contexts past tab 9,
/// which no digit can.
#[must_use]
pub fn cycle<'a>(
    contexts: &'a [FleetContext],
    active: Option<&ContextId>,
    delta: isize,
) -> Option<&'a ContextId> {
    if contexts.is_empty() {
        return None;
    }
    let len = contexts.len() as isize;
    let current = active_index(contexts, active) as isize;
    let next = (current + delta).rem_euclid(len) as usize;
    contexts.get(next).map(|context| &context.id)
}

/// How many contexts the `+n` overflow chip stands for.
#[must_use]
pub fn overflow(contexts: &[FleetContext]) -> usize {
    contexts.len().saturating_sub(ADDRESSABLE_TABS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contexts(names: &[&str]) -> Vec<FleetContext> {
        names
            .iter()
            .map(|name| FleetContext {
                id: ContextId::try_from(*name).unwrap_or_else(|error| panic!("{error}")),
                name: (*name).to_owned(),
                owners: Vec::new(),
                created_at: "2026-09-04T12:00:00Z".to_owned(),
            })
            .collect()
    }

    #[test]
    fn digits_are_one_based_and_never_clamp() {
        let contexts = contexts(&["buk", "personal", "oss"]);
        assert_eq!(
            context_for_digit(&contexts, 1).map(ContextId::as_str),
            Some("buk")
        );
        assert_eq!(
            context_for_digit(&contexts, 3).map(ContextId::as_str),
            Some("oss")
        );
        assert_eq!(context_for_digit(&contexts, 4), None);
        assert_eq!(context_for_digit(&contexts, 0), None);
    }

    #[test]
    fn only_the_first_nine_tabs_are_addressable_by_digit() {
        let names: Vec<String> = (1..=11).map(|index| format!("c{index}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let contexts = contexts(&refs);
        assert!(context_for_digit(&contexts, 9).is_some());
        assert_eq!(context_for_digit(&contexts, 10), None);
        assert_eq!(overflow(&contexts), 2);
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        let contexts = contexts(&["buk", "personal", "oss"]);
        let active = contexts[2].id.clone();
        assert_eq!(
            cycle(&contexts, Some(&active), 1).map(ContextId::as_str),
            Some("buk")
        );
        let active = contexts[0].id.clone();
        assert_eq!(
            cycle(&contexts, Some(&active), -1).map(ContextId::as_str),
            Some("oss")
        );
    }

    #[test]
    fn an_empty_context_list_has_nothing_to_cycle_to() {
        assert_eq!(cycle(&[], None, 1), None);
        assert_eq!(active_index(&[], None), 0);
        assert_eq!(overflow(&[]), 0);
    }
}
