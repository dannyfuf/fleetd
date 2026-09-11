//! Tests for the native agent tab.
//!
//! Split by what they pin, per `rust-gpui-testing`: the pure functions carry the weight — the
//! row projection, the fold table, the decision routing, the composer's rules — and the
//! `#[gpui::test]` half carries the wiring that only an entity graph can show.

mod composer;
mod decisions;
mod fixtures;
mod rows;
mod view;
