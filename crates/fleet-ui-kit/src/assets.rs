//! The asset source for the kit's embedded icons.
//!
//! gpui builds exactly one [`AssetSource`] per application (`zed#8713`), so a library cannot
//! register its own. Two supported shapes:
//!
//! * an app with no other assets registers [`KitAssets`] directly:
//!   `application().with_assets(KitAssets)`;
//! * an app with its own assets keeps its own source and delegates the `icons/` prefix:
//!   `if let Some(bytes) = kit_asset(path) { return Ok(Some(Cow::Borrowed(bytes))) }`.
//!
//! A missing asset must return `Ok(None)`, never `Err` — gpui logs nothing for `svg()` and an
//! erroring source turns an invisible icon into an invisible crash.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

use crate::icons::Icon;

/// Resolve an asset path (`icons/moon.svg`) to embedded bytes.
pub fn kit_asset(path: &str) -> Option<&'static [u8]> {
    if !path.starts_with("icons/") {
        return None;
    }
    Icon::from_path(path).map(Icon::bytes)
}

/// Every asset path this crate can serve.
pub fn kit_asset_paths() -> Vec<SharedString> {
    Icon::ALL.iter().map(|icon| icon.path()).collect()
}

/// An [`AssetSource`] serving only the kit's icons.
#[derive(Clone, Copy, Debug, Default)]
pub struct KitAssets;

impl AssetSource for KitAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(kit_asset(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        if prefix.is_empty() || prefix == "icons" {
            Ok(kit_asset_paths())
        } else {
            Ok(Vec::new())
        }
    }
}
