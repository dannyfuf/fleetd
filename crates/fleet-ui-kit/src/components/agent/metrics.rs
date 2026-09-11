//! Fixed native-agent geometry the canvas pins down but [`crate::theme::Metrics`] does not
//! carry.
//!
//! `docs/DESIGN-SYSTEM.md` §2.8 keeps these out of the token set on purpose: `Metrics` is the
//! density ladder a theme may restate, while these are fixed product decisions from
//! `docs/NATIVE-AGENTS.md` §5 that no theme may move. They live in one module so no agent
//! component carries a bare literal, and they are exported from the crate root so no app-side
//! copy of `760` exists. Colours, type roles, radii, durations and the 4 px spacing scale are
//! **not** here — those always come from the theme.

use gpui::{Pixels, px};

/// 760 px: the agent content measure (`NATIVE-AGENTS.md` §5). Wide windows stay empty on the
/// right on purpose.
pub const AGENT_CONTENT_W: Pixels = px(760.0);

/// 60 px: the fixed tool-row kind column, wide enough for `search` at the small mono size.
pub const AGENT_TOOL_KIND_W: Pixels = px(60.0);

/// 17 px: the streaming caret block, one [`crate::theme::Metrics::cell_w`] wide.
pub const AGENT_CARET_H: Pixels = px(17.0);

/// 240 px: how tall an expanded tool body may grow before it scrolls inside its own row.
pub const AGENT_BODY_MAX_H: Pixels = px(240.0);

/// 66 px: the approval payload well — about three data lines. §6.2: the invocation is never
/// truncated and never line-clamped, so the well scrolls in **both** axes instead.
pub const AGENT_WELL_MAX_H: Pixels = px(66.0);

/// 256 px: how far above and below the viewport the transcript measures rows, so a fast
/// stream never pops rows in at the fold.
pub const AGENT_LIST_OVERDRAW: Pixels = px(256.0);

/// 3 px: how far the transcript scrollbar thumb sits from the right edge.
pub const AGENT_SCROLLBAR_INSET: Pixels = px(3.0);

/// 40 px: the band at the live edge inside which follow re-arms (`spec-B` §B7.3).
///
/// Strict on purpose. A list's own "near end" heuristic fires within half a viewport, which
/// re-arms follow while the user is reading history and yanks them back down on the next chunk.
/// A small pixel band — rather than a 1 px epsilon — still re-arms reliably while streaming
/// content is growing under the viewport.
pub const AGENT_FOLLOW_REARM_PX: Pixels = px(40.0);

/// 456 px: 60 % of the measure, which is how wide a user bubble may grow before it wraps.
pub const AGENT_USER_MAX_W: Pixels = px(456.0);

/// 144 px: eight data lines — the preview a collapsed user message shows before `[⏎] full
/// message` (`spec-B` §B1.2 collapses at > 600 chars or > 8 lines).
pub const AGENT_PREVIEW_MAX_H: Pixels = px(144.0);

/// 180 px: ten data lines — the preview a collapsed plan card shows (it collapses at > 900
/// chars or > 20 lines).
pub const AGENT_PLAN_PREVIEW_H: Pixels = px(180.0);
