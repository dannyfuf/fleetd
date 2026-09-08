//! Fixed native-agent geometry the canvas pins down but [`crate::theme::Metrics`] does not
//! carry yet.
//!
//! Every value here is a candidate for the token set; they live in one module so the promotion
//! is a single edit and no agent component ever carries a bare literal. Colours, type roles,
//! radii, durations and the 4 px spacing scale always come from the theme — nothing in here is
//! one of those.

use gpui::{Pixels, px};

/// 760 px: the agent content measure (`NATIVE-AGENTS.md` §2). Wide windows stay empty on the
/// right on purpose.
pub const AGENT_CONTENT_W: Pixels = px(760.0);

/// 60 px: the fixed tool-row kind column, wide enough for `search` at the small mono size.
pub const AGENT_TOOL_KIND_W: Pixels = px(60.0);

/// 17 px: the streaming caret block, one [`crate::theme::Metrics::cell_w`] wide.
pub const AGENT_CARET_H: Pixels = px(17.0);

/// 240 px: how tall an expanded tool body may grow before it scrolls inside its own row.
pub const AGENT_BODY_MAX_H: Pixels = px(240.0);

/// 256 px: how far above and below the viewport the transcript measures rows, so a fast
/// stream never pops rows in at the fold.
pub const AGENT_LIST_OVERDRAW: Pixels = px(256.0);

/// 3 px: how far the transcript scrollbar thumb sits from the right edge.
pub const AGENT_SCROLLBAR_INSET: Pixels = px(3.0);
