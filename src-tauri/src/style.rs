//! Caption look and caption segments, copied from KreoKit Captions (`model.rs`), mirrored
//! in src/lib/captions/types.ts. Sizes are for a 1080 px tall frame, so a 9:16 clip is a
//! 608 × 1080 script (see ass.rs).

use serde::{Deserialize, Serialize};

pub use crate::transcribe::Word;

/// One caption segment of a clip, on the clip's clock (0 = the clip's in point).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Bottom,
    Middle,
    Top,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionMode {
    Line,
    WordHighlight,
    OneWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Animation {
    None,
    /// Each caption (or word, in one_word mode) scales up briefly as it appears.
    Pop,
}

/// Caption look. Sizes are in pixels for a 1080 px tall frame and scale with the video.
/// The default is the "Bold highlight" preset (src/lib/captions/presets.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub font_family: String,
    pub font_size: u32,
    pub color: String,
    pub highlight_color: String,
    pub outline_color: String,
    pub outline_width: u32,
    pub position: Position,
    pub max_chars_per_line: u32,
    pub max_lines: u32,
    pub mode: CaptionMode,
    pub bold: bool,
    pub uppercase: bool,
    pub animation: Animation,
    /// Free position: the caption's anchor as a fraction of the frame width (0–1).
    /// `None` (older projects, presets) means the preset spot for `position`.
    pub pos_x: Option<f64>,
    /// Free position, fraction of the frame height (0–1). Which edge of the caption
    /// sits there depends on `position` (top edge, centre or bottom edge).
    pub pos_y: Option<f64>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font_family: "Montserrat".into(),
            font_size: 56,
            color: "#FFFFFF".into(),
            highlight_color: "#C8F135".into(),
            outline_color: "#000000".into(),
            outline_width: 5,
            position: Position::Bottom,
            max_chars_per_line: 14,
            max_lines: 2,
            mode: CaptionMode::WordHighlight,
            bold: true,
            uppercase: true,
            animation: Animation::None,
            pos_x: None,
            pos_y: None,
        }
    }
}

/// The Top / Bottom preset spots (fractions of the frame height), just clear of the parts
/// TikTok, Reels and Shorts cover with their own interface (top 10%, bottom 22%).
/// Mirrors `PRESET_Y` in src/lib/captions/position.ts.
pub const PRESET_TOP: f64 = 0.12;
pub const PRESET_BOTTOM: f64 = 0.74;

impl Style {
    /// The caption's anchor point as fractions of the frame (0–1 each). Horizontally it is
    /// the centre of the text; vertically the top edge, centre or bottom edge for
    /// `position` top / middle / bottom.
    pub fn anchor(&self) -> (f64, f64) {
        let preset_y = match self.position {
            Position::Top => PRESET_TOP,
            Position::Middle => 0.5,
            Position::Bottom => PRESET_BOTTOM,
        };
        (
            self.pos_x.unwrap_or(0.5).clamp(0.0, 1.0),
            self.pos_y.unwrap_or(preset_y).clamp(0.0, 1.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_style_is_the_bold_highlight_preset() {
        let presets: serde_json::Value =
            serde_json::from_str(include_str!("../../src/lib/captions/presets.json")).unwrap();
        let preset = presets
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "bold-highlight")
            .unwrap();
        let style: Style = serde_json::from_value(preset["style"].clone()).unwrap();
        assert_eq!(style, Style::default());
    }

    #[test]
    fn preset_spots_clear_the_platform_interface() {
        let style = |position| Style {
            position,
            ..Style::default()
        };
        assert_eq!(style(Position::Top).anchor(), (0.5, PRESET_TOP));
        assert_eq!(style(Position::Bottom).anchor(), (0.5, PRESET_BOTTOM));
        assert!(PRESET_TOP > 0.10 && PRESET_BOTTOM < 1.0 - 0.22);
    }
}
