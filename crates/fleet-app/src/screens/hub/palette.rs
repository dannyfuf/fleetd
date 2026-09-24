//! What the Hub lends the command palette: the pull requests its PR screen has loaded.

use fleet_core::github::PrTab;
use gpui::App;

use super::{HubScreen, cache::PrCacheKey};
use crate::{
    dialogs::PalettePr,
    presentation::SnapshotIndex,
    state::AppState,
    views::prs_screen::{self, PrInputs},
};

impl HubScreen {
    /// Both PR tabs as the PR screen lists them for the current scope, for the palette's
    /// `Pull requests` rows. Only what is already cached: opening the palette fetches nothing.
    pub(crate) fn palette_prs(&self, state: &AppState, cx: &App) -> Vec<PalettePr> {
        let Some(snapshot) = state.snapshot.as_ref() else {
            return Vec::new();
        };
        let hub = self.hub.read(cx);
        let index = SnapshotIndex::new(snapshot);
        let key = PrCacheKey::from_state(state);
        let now = super::now_unix();
        [PrTab::Mine, PrTab::Review]
            .into_iter()
            .flat_map(|tab| {
                prs_screen::build_rows(
                    &PrInputs {
                        slice: hub.prs.slice_for(tab, &key),
                        worktrees: &snapshot.worktrees,
                        creating: &hub.creating,
                        now,
                    },
                    &index,
                )
                .into_iter()
                .map(move |row| PalettePr {
                    tab,
                    repo: row.repo,
                    number: row.number,
                    title: row.title.to_string(),
                    local: row.local,
                })
            })
            .collect()
    }
}
