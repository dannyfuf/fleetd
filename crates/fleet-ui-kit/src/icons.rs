//! Embedded Lucide icon set.
//!
//! The SVGs in `assets/icons/` are Lucide (ISC), fetched from
//! `https://raw.githubusercontent.com/lucide-icons/lucide/main/icons/<name>.svg`
//! with `stroke-width` rewritten from 2 to 1.5 to match the UX spec.
//!
//! gpui allows exactly **one** [`gpui::AssetSource`] per application, so this crate cannot
//! register its own. It exposes the bytes instead: an app either uses [`KitAssets`] directly,
//! or delegates to [`kit_asset`] from its own source (see `crate::assets`).

use gpui::{
    AnimationExt, App, ElementId, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled,
    Transformation, Window, percentage, px, svg,
};
use std::time::Duration;

use crate::{Tone, theme::ActiveTheme};

macro_rules! lucide_icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// Every Lucide glyph Fleet is allowed to draw.
        ///
        /// The set is closed on purpose: §1 of the UX spec makes each glyph a word, so adding one
        /// is a design decision, not an implementation detail. Adding a variant means downloading
        /// the SVG into `assets/icons/` and documenting it in `docs/DESIGN-SYSTEM.md`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[allow(missing_docs)]
        pub enum Icon {
            $($variant,)*
        }

        impl Icon {
            /// Every variant, in declaration order. Used by the gallery.
            pub const ALL: &'static [Icon] = &[$(Icon::$variant,)*];

            /// The Lucide file name, without extension.
            pub const fn name(self) -> &'static str {
                match self { $(Icon::$variant => $file,)* }
            }

            /// The asset path an [`gpui::AssetSource`] must resolve, e.g. `icons/moon.svg`.
            pub const fn path(self) -> SharedString {
                match self {
                    $(Icon::$variant => SharedString::new_static(concat!("icons/", $file, ".svg")),)*
                }
            }

            /// The embedded SVG bytes.
            pub const fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!("../assets/icons/", $file, ".svg")),)*
                }
            }

            /// Look an icon up by asset path (`icons/moon.svg`) or by bare name (`moon`).
            pub fn from_path(path: &str) -> Option<Icon> {
                let name = path.strip_prefix("icons/").unwrap_or(path);
                let name = name.strip_suffix(".svg").unwrap_or(name);
                match name { $($file => Some(Icon::$variant),)* _ => None }
            }
        }
    };
}

lucide_icons! {
    Activity => "activity",
    ArrowRightLeft => "arrow-right-left",
    Bot => "bot",
    Boxes => "boxes",
    Brain => "brain",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronsUp => "chevrons-up",
    CircleArrowDown => "circle-arrow-down",
    CircleArrowUp => "circle-arrow-up",
    CircleCheck => "circle-check",
    CircleDot => "circle-dot",
    CircleQuestionMark => "circle-question-mark",
    CircleSlash => "circle-slash",
    CircleStop => "circle-stop",
    CircleX => "circle-x",
    Circle => "circle",
    ClipboardCheck => "clipboard-check",
    CopyPlus => "copy-plus",
    Clock => "clock",
    CloudDownload => "cloud-download",
    CloudOff => "cloud-off",
    CloudUpload => "cloud-upload",
    Cloud => "cloud",
    Command => "command",
    Delete => "delete",
    Dot => "dot",
    Ellipsis => "ellipsis",
    Eye => "eye",
    FileDiff => "file-diff",
    FilePen => "file-pen",
    Flag => "flag",
    FolderGit2 => "folder-git-2",
    GitBranchPlus => "git-branch-plus",
    GitBranch => "git-branch",
    GitCommitHorizontal => "git-commit-horizontal",
    GitFork => "git-fork",
    GitMerge => "git-merge",
    GitPullRequestDraft => "git-pull-request-draft",
    GitPullRequest => "git-pull-request",
    Globe => "globe",
    Hourglass => "hourglass",
    Info => "info",
    Import => "import",
    LoaderCircle => "loader-circle",
    Lock => "lock",
    Maximize2 => "maximize-2",
    MessageSquareWarning => "message-square-warning",
    Minimize2 => "minimize-2",
    Minus => "minus",
    Moon => "moon",
    Paperclip => "paperclip",
    Plus => "plus",
    Power => "power",
    RefreshCw => "refresh-cw",
    Sailboat => "sailboat",
    Scissors => "scissors",
    Search => "search",
    SearchCheck => "search-check",
    Server => "server",
    Settings2 => "settings-2",
    Sparkles => "sparkles",
    SquarePen => "square-pen",
    SquareTerminal => "square-terminal",
    Terminal => "terminal",
    Trash => "trash",
    Trash2 => "trash-2",
    TriangleAlert => "triangle-alert",
    Undo2 => "undo-2",
    Unplug => "unplug",
    Wrench => "wrench",
    X => "x",
    Zap => "zap",
}

/// The three icon sizes the spec allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IconSize {
    /// 12 px: status bar, inline marks inside a row, exited-tab cross.
    Small,
    /// 14 px: inside chips, pane headers, filter bar.
    Medium,
    /// 16 px: the default glyph column of every list, dialog headers.
    #[default]
    Large,
}

impl IconSize {
    /// The pixel size.
    pub fn px(self) -> Pixels {
        match self {
            IconSize::Small => px(12.0),
            IconSize::Medium => px(14.0),
            IconSize::Large => px(16.0),
        }
    }
}

impl Icon {
    /// Start a builder for this glyph.
    #[track_caller]
    pub fn el(self) -> IconElement {
        IconElement::new(self)
    }

    /// Shorthand for `self.el().size(size)`.
    #[track_caller]
    pub fn size(self, size: IconSize) -> IconElement {
        self.el().size(size)
    }

    /// Shorthand for `self.el().color(color)`.
    #[track_caller]
    pub fn color(self, color: Hsla) -> IconElement {
        self.el().color(color)
    }
}

/// A rendered glyph: `Icon::Moon.el().size(IconSize::Medium).color(theme.colors.text_secondary)`.
///
/// The color defaults to `text`, never to gpui's black (`svg()` draws nothing without a text
/// color, so `IconElement` always sets one).
#[derive(IntoElement)]
pub struct IconElement {
    icon: Icon,
    size: IconSize,
    color: Option<Hsla>,
    tone: Tone,
    opacity: Option<f32>,
    spinning: bool,
    /// Seeded from the call site so a spinning glyph animates without an explicit id.
    id: ElementId,
}

impl IconElement {
    /// A glyph at the default size and the primary text color.
    #[track_caller]
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            size: IconSize::default(),
            color: None,
            tone: Tone::Default,
            opacity: None,
            spinning: false,
            id: std::panic::Location::caller().into(),
        }
    }

    /// Set the size.
    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    /// Resolve a semantic tint from the active theme at render time. The last of
    /// [`Self::tone`] / [`Self::color`] to be called wins.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self.color = None;
        self
    }

    /// Tint the glyph. Always pass a theme token, never a literal.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Lower the contrast without changing the hue (the `none` dot is `dot` at 30 %).
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    /// Rotate one turn per `Motion::spinner`. Repeated items must supply a scoped [`Self::id`].
    pub fn spinning(mut self, spinning: bool) -> Self {
        self.spinning = spinning;
        self
    }

    /// Override the call-site id. Repeated spinning glyphs need one id per item.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

impl RenderOnce for IconElement {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let mut color = self.color.unwrap_or_else(|| self.tone.color(theme));
        if let Some(opacity) = self.opacity {
            color = color.opacity(opacity);
        }
        let base = svg()
            .flex_none()
            .size(self.size.px())
            .path(self.icon.path())
            .text_color(color);

        if self.spinning {
            let duration = Duration::from_millis(theme.motion.spinner);
            base.with_animation(
                self.id,
                gpui::Animation::new(duration).repeat(),
                |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
            )
            .into_any_element()
        } else {
            base.into_any_element()
        }
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        IconElement::new(self).render(window, cx)
    }
}

impl IntoElement for Icon {
    type Element = <IconElement as IntoElement>::Element;

    fn into_element(self) -> Self::Element {
        IconElement::new(self).into_element()
    }
}
