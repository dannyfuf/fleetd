//! # fleet-ui-kit
//!
//! Fleet's design system on raw gpui: design tokens, a `Theme` global, the embedded Lucide
//! icon set, and one module per component.
//!
//! It is the **component API contract**: `fleet-app` composes only what is exported here and
//! does no ad-hoc styling. `docs/DESIGN-SYSTEM.md` is the prose half of the same contract, and
//! `docs/UX-SPEC.md` §9 is the inventory this crate implements.
//!
//! ## Rules
//!
//! 1. **No domain types.** This crate depends on `gpui` and nothing else. Components take
//!    `SharedString`, scalars and closures; the app converts its domain into those.
//! 2. **No literal colors, sizes or durations in a component.** Everything is read from
//!    [`theme::Theme`] through [`theme::ActiveTheme`], so light/dark and any future density
//!    change is one edit.
//! 3. **One `AssetSource` per app.** gpui allows exactly one, so this crate exposes its icon
//!    bytes ([`assets::KitAssets`], [`assets::kit_asset`]) instead of registering its own.
//!
//! ## Getting started
//!
//! ```no_run
//! use fleet_ui_kit::prelude::*;
//! use gpui::App;
//!
//! fn init(cx: &mut App) {
//!     Theme::init(ThemeMode::Dark, cx);
//! }
//! ```
//!
//! The application must also register the assets, or every icon silently renders nothing:
//!
//! ```ignore
//! gpui_platform::application()
//!     .with_assets(fleet_ui_kit::KitAssets)
//!     .run(|cx| { /* ... */ });
//! ```
//!
//! See `examples/kit_gallery.rs` for every component in every state, in both themes.

#![warn(missing_docs)]

pub mod assets;
pub mod components;
pub mod focus;
pub mod icons;
mod paint_error;
pub mod text;
pub mod theme;
pub mod tone;
pub mod truncate;

pub use assets::{KitAssets, kit_asset, kit_asset_paths};
pub use components::*;
pub use icons::{Icon, IconElement, IconSize};
pub use text::{Text, TextRole, styled_with};
pub use theme::{ActiveTheme, Theme, ThemeMode};
pub use tone::Tone;
pub use truncate::{ELLIPSIS, Truncate, truncate, truncate_shared};

/// Everything a view needs in one `use`.
pub mod prelude {
    pub use crate::components::*;
    pub use crate::icons::{Icon, IconElement, IconSize};
    pub use crate::text::{Text, TextRole};
    pub use crate::theme::{ActiveTheme, Theme, ThemeMode};
    pub use crate::tone::Tone;
    pub use crate::truncate::{Truncate, truncate};
    pub use gpui::prelude::*;
    pub use gpui::{
        AnyElement, App, Context, Div, ElementId, IntoElement, ParentElement, Pixels, Render,
        RenderOnce, SharedString, Styled, Window, div, px,
    };
}
