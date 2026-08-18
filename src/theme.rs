//! Built-in read-only color themes.
//!
//! Every role is purely additive: the underlying text and symbols carry meaning
//! without color, and the `Monochrome` theme maps every role to the terminal
//! default to demonstrate that. Layout never branches on the active theme, so a
//! theme cannot cause out-of-bounds rendering.
use ratatui::style::Color;

/// Semantic roles the terminal view colors. The terminal view reads these
/// through `App::theme`; the app never stores per-screen color logic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Market summary and the table header emphasis.
    pub summary: Color,
    /// Help overlay, resize message, and no-results notices.
    pub notice: Color,
    /// Positive values and rising charts.
    pub gain: Color,
    /// Negative values and falling charts.
    pub loss: Color,
    /// Chart line when no trend data is available.
    pub neutral: Color,
}

pub const DEFAULT_THEME: Theme = Theme {
    name: "Default",
    summary: Color::Cyan,
    notice: Color::Yellow,
    gain: Color::Green,
    loss: Color::Red,
    neutral: Color::Cyan,
};

/// Nord-inspired accent: frost-blue emphasis and aurora gain/loss.
pub const NORD_THEME: Theme = Theme {
    name: "Nord",
    summary: Color::Rgb(136, 192, 208),
    notice: Color::Rgb(235, 203, 139),
    gain: Color::Rgb(163, 190, 140),
    loss: Color::Rgb(191, 97, 106),
    neutral: Color::Rgb(129, 161, 193),
};

/// Every role maps to the terminal default; readable with `NO_COLOR=1`.
pub const MONO_THEME: Theme = Theme {
    name: "Monochrome",
    summary: Color::Reset,
    notice: Color::Reset,
    gain: Color::Reset,
    loss: Color::Reset,
    neutral: Color::Reset,
};

/// Built-in themes in cycle order. The first entry is the startup default.
pub const THEMES: &[Theme] = &[DEFAULT_THEME, NORD_THEME, MONO_THEME];
