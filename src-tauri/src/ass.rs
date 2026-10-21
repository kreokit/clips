//! Style + captions → ASS subtitle script, rendered onto the video by FFmpeg (libass).
//!
//! The editor's overlay (src/components/CaptionOverlay.tsx) shows the same thing: both use
//! the shared line layout, the same timing rules per mode, a 1080 px reference frame and
//! the same margins. When changing what appears on screen here, change the overlay too.

use std::fmt::Write;

use crate::layout::{Page, layout};
use crate::style::{Animation, CaptionMode, Position, Segment, Style, Word};

/// Style sizes are for a frame this tall.
pub const REFERENCE_HEIGHT: u32 = 1080;
/// Distance of bottom/top captions from the frame edge, in reference pixels (8%).
pub const MARGIN_V: u32 = 86;
/// Pop animation: captions start at this scale (%) and grow to 100% over POP_MS.
const POP_SCALE: u32 = 80;
const POP_MS: u32 = 120;

/// Builds the ASS script for a `width`×`height` video.
pub fn to_ass(segments: &[Segment], style: &Style, width: u32, height: u32) -> String {
    let play_res_x = play_res_x(width, height);
    let mut out = header(style, play_res_x);
    let (an, x, y) = placement(style, play_res_x);
    // Every event pins the caption's anchor to the style's position.
    let place = format!("{{\\an{an}\\pos({x},{y})}}");
    for segment in segments {
        for event in events(segment, style) {
            writeln!(
                out,
                "Dialogue: 0,{},{},Default,,0,0,0,,{place}{}",
                ass_time(event.start_ms),
                ass_time(event.end_ms),
                event.text
            )
            .unwrap();
        }
    }
    out
}

/// The script width: 1080 high, as wide as the video's shape.
pub fn play_res_x(width: u32, height: u32) -> u32 {
    (REFERENCE_HEIGHT as f64 * width as f64 / height.max(1) as f64)
        .round()
        .max(1.0) as u32
}

/// ASS alignment (8 top / 5 middle / 2 bottom, horizontally centred) and the anchor in
/// script coordinates, rounded to whole pixels. The preview places captions with the
/// same numbers (src/lib/captions/position.ts; shared cases in fixtures/position-cases.json).
pub fn placement(style: &Style, play_res_x: u32) -> (u8, i64, i64) {
    let an = match style.position {
        Position::Top => 8,
        Position::Middle => 5,
        Position::Bottom => 2,
    };
    let (x, y) = style.anchor();
    (
        an,
        (x * play_res_x as f64).round() as i64,
        (y * REFERENCE_HEIGHT as f64).round() as i64,
    )
}

fn header(style: &Style, play_res_x: u32) -> String {
    let alignment = match style.position {
        Position::Bottom => 2,
        Position::Middle => 5,
        Position::Top => 8,
    };
    let margin_h = play_res_x / 20;
    format!(
        "[Script Info]\n\
         ScriptType: v4.00+\n\
         PlayResX: {play_res_x}\n\
         PlayResY: {REFERENCE_HEIGHT}\n\
         WrapStyle: 2\n\
         ScaledBorderAndShadow: yes\n\
         YCbCr Matrix: None\n\
         \n\
         [V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,{font},{size},{primary},{primary},{outline},&H00000000,{bold},0,0,0,100,100,0,0,1,{outline_width},0,{alignment},{margin_h},{margin_h},{MARGIN_V},1\n\
         \n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        font = style.font_family.replace(',', " "),
        size = style.font_size,
        primary = ass_color(&style.color),
        outline = ass_color(&style.outline_color),
        bold = if style.bold { -1 } else { 0 },
        outline_width = style.outline_width,
    )
}

#[derive(Debug, PartialEq)]
struct Event {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// The caption events for one segment, following the overlay's rules:
/// - line: one event per page
/// - word_highlight: per page, a new event whenever the spoken word changes; a word is
///   spoken from its start (inclusive) to its end (exclusive); between words none is
/// - one_word: each word from its start until the next word starts (the last until the
///   segment ends), in the highlight colour
fn events(segment: &Segment, style: &Style) -> Vec<Event> {
    let mut segment = segment.clone();
    if style.uppercase {
        for w in &mut segment.words {
            w.text = w.text.to_uppercase();
        }
        segment.text = segment.text.to_uppercase();
    }
    let layout = layout(&segment, style.max_chars_per_line, style.max_lines);
    let pop = style.animation == Animation::Pop;
    let highlight = ass_color(&style.highlight_color);
    let primary = ass_color(&style.color);

    let mut events = Vec::new();
    match style.mode {
        CaptionMode::Line => {
            for page in &layout.pages {
                events.push(Event {
                    start_ms: page.start_ms,
                    end_ms: page.end_ms,
                    text: pop_tag(pop)
                        + &page_text(page, &layout.words, None, &highlight, &primary),
                });
            }
        }
        CaptionMode::WordHighlight => {
            for page in &layout.pages {
                let mut first = true;
                for (start_ms, end_ms, spoken) in highlight_spans(page, &layout.words) {
                    let tag = if first { pop_tag(pop) } else { String::new() };
                    first = false;
                    events.push(Event {
                        start_ms,
                        end_ms,
                        text: tag + &page_text(page, &layout.words, spoken, &highlight, &primary),
                    });
                }
            }
        }
        CaptionMode::OneWord => {
            let words = &layout.words;
            for (i, word) in words.iter().enumerate() {
                let end_ms = words
                    .get(i + 1)
                    .map_or(segment.end_ms, |next| next.start_ms);
                if end_ms > word.start_ms {
                    events.push(Event {
                        start_ms: word.start_ms,
                        end_ms,
                        text: format!("{}{{\\c{highlight}}}{}", pop_tag(pop), escape(&word.text)),
                    });
                }
            }
        }
    }
    // Drop events that vanish at ASS's centisecond resolution.
    events.retain(|e| centis(e.end_ms) > centis(e.start_ms));
    events
}

/// Splits a page's time into spans with a constant spoken word (or none).
fn highlight_spans(page: &Page, words: &[Word]) -> Vec<(u64, u64, Option<usize>)> {
    let indices: Vec<usize> = page.lines.iter().flatten().copied().collect();
    let mut cuts = vec![page.start_ms, page.end_ms];
    for &i in &indices {
        cuts.extend([words[i].start_ms, words[i].end_ms]);
    }
    cuts.retain(|&t| t >= page.start_ms && t <= page.end_ms);
    cuts.sort_unstable();
    cuts.dedup();

    let mut spans: Vec<(u64, u64, Option<usize>)> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let spoken = indices
            .iter()
            .copied()
            .find(|&i| words[i].start_ms <= start && start < words[i].end_ms);
        match spans.last_mut() {
            Some(last) if last.2 == spoken => last.1 = end,
            _ => spans.push((start, end, spoken)),
        }
    }
    spans
}

/// The page's lines joined with \N; the spoken word, if any, in the highlight colour.
fn page_text(
    page: &Page,
    words: &[Word],
    spoken: Option<usize>,
    highlight: &str,
    primary: &str,
) -> String {
    page.lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|&i| {
                    let text = escape(&words[i].text);
                    if Some(i) == spoken {
                        format!("{{\\c{highlight}}}{text}{{\\c{primary}}}")
                    } else {
                        text
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\\N")
}

fn pop_tag(pop: bool) -> String {
    if pop {
        format!("{{\\fscx{POP_SCALE}\\fscy{POP_SCALE}\\t(0,{POP_MS},\\fscx100\\fscy100)}}")
    } else {
        String::new()
    }
}

/// Characters ASS would treat as commands are swapped for full-width look-alikes. The
/// overlay applies the same mapping (src/lib/captions/overlay.ts `displayText`).
pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '{' => '｛',
            '}' => '｝',
            '\\' => '＼',
            c => c,
        })
        .collect()
}

/// "#RRGGBB" → "&H00BBGGRR" (ASS stores colours as alpha, blue, green, red).
fn ass_color(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return "&H00FFFFFF".into();
    }
    let (r, g, b) = (&hex[0..2], &hex[2..4], &hex[4..6]);
    format!("&H00{b}{g}{r}").to_uppercase()
}

/// ASS times have centisecond resolution; every time is rounded the same way, so an
/// event ending when the next starts never leaves a gap or overlaps.
fn centis(ms: u64) -> u64 {
    (ms + 5) / 10
}

fn ass_time(ms: u64) -> String {
    let cs = centis(ms);
    let (h, rest) = (cs / 360_000, cs % 360_000);
    let (m, rest) = (rest / 6_000, rest % 6_000);
    let (s, cs) = (rest / 100, rest % 100);
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(words: &[(&str, u64, u64)], start_ms: u64, end_ms: u64) -> Segment {
        Segment {
            id: 0,
            start_ms,
            end_ms,
            text: words.iter().map(|w| w.0).collect::<Vec<_>>().join(" "),
            words: words
                .iter()
                .map(|&(text, start_ms, end_ms)| Word {
                    start_ms,
                    end_ms,
                    text: text.into(),
                })
                .collect(),
        }
    }

    fn hello() -> Segment {
        segment(
            &[
                ("Hello", 1000, 1400),
                ("there", 1500, 1800),
                ("friend", 1800, 2300),
            ],
            1000,
            2500,
        )
    }

    fn style(mode: CaptionMode) -> Style {
        Style {
            mode,
            uppercase: false,
            max_chars_per_line: 11,
            max_lines: 2,
            ..Style::default()
        }
    }

    /// Dialogue lines without the leading placement block (tested separately).
    fn dialogue(ass: &str) -> Vec<String> {
        ass.lines()
            .filter(|l| l.starts_with("Dialogue:"))
            .map(|l| {
                let start = l.find("{\\an").expect("placement block");
                let end = start + l[start..].find('}').unwrap() + 1;
                format!("{}{}", &l[..start], &l[end..])
            })
            .collect()
    }

    #[test]
    fn every_event_is_placed() {
        let ass = to_ass(&[hello()], &style(CaptionMode::WordHighlight), 1080, 1920);
        let lines: Vec<&str> = ass.lines().filter(|l| l.starts_with("Dialogue:")).collect();
        assert_eq!(lines.len(), 5);
        assert!(
            lines.iter().all(|l| l.contains(",,{\\an2\\pos(304,799)}")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_vertical_clip_is_a_608_by_1080_script_above_the_platform_interface() {
        // A clip's captions start at its in point (0 on its clock).
        let seg = segment(&[("Hi", 0, 300)], 0, 800);
        let ass = to_ass(&[seg], &Style::default(), 1080, 1920);
        assert!(ass.contains("PlayResX: 608\nPlayResY: 1080\n"));
        assert!(ass.contains("Dialogue: 0,0:00:00.00,0:00:00.30,Default,,0,0,0,,{\\an2\\pos(304,799)}"));
        // The bottom edge sits above the bottom 22% that TikTok, Reels and Shorts cover.
        assert!(799.0 < 1080.0 * (1.0 - 0.22));
    }

    #[test]
    fn matches_shared_position_cases() {
        use serde::Deserialize;
        #[derive(Deserialize)]
        struct Cases {
            cases: Vec<Case>,
        }
        #[derive(Deserialize)]
        struct Case {
            name: String,
            style: serde_json::Value,
            frame: Frame,
            play_res_x: u32,
            an: u8,
            pos: (i64, i64),
        }
        #[derive(Deserialize)]
        struct Frame {
            width: u32,
            height: u32,
        }
        let cases: Cases =
            serde_json::from_str(include_str!("../../fixtures/position-cases.json")).unwrap();
        for case in cases.cases {
            let style: Style = serde_json::from_value(case.style).unwrap();
            let res_x = play_res_x(case.frame.width, case.frame.height);
            assert_eq!(res_x, case.play_res_x, "{}", case.name);
            assert_eq!(
                placement(&style, res_x),
                (case.an, case.pos.0, case.pos.1),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn converts_colours_and_times() {
        assert_eq!(ass_color("#FFD400"), "&H0000D4FF");
        assert_eq!(ass_color("#C8F135"), "&H0035F1C8");
        assert_eq!(ass_color("#0a0B0c"), "&H000C0B0A");
        assert_eq!(ass_color("red"), "&H00FFFFFF");
        assert_eq!(ass_time(0), "0:00:00.00");
        assert_eq!(ass_time(3_723_456), "1:02:03.46");
        assert_eq!(ass_time(1_004), "0:00:01.00");
    }

    #[test]
    fn header_matches_the_video_shape_and_style() {
        let ass = to_ass(&[], &Style::default(), 1080, 1920);
        assert!(ass.contains("PlayResX: 608\nPlayResY: 1080\nWrapStyle: 2\n"));
        assert!(ass.contains(
            "Style: Default,Montserrat,56,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,5,0,2,30,30,86,1\n"
        ));
        let wide = to_ass(
            &[],
            &Style {
                position: Position::Middle,
                ..Style::default()
            },
            1920,
            1080,
        );
        assert!(wide.contains("PlayResX: 1920\n"));
        assert!(wide.contains(",1,5,0,5,96,96,86,1\n"));
    }

    #[test]
    fn line_mode_snapshot() {
        let ass = to_ass(&[hello()], &style(CaptionMode::Line), 1920, 1080);
        assert_eq!(
            dialogue(&ass),
            ["Dialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,Hello there\\Nfriend"]
        );
    }

    #[test]
    fn word_highlight_snapshot() {
        let ass = to_ass(&[hello()], &style(CaptionMode::WordHighlight), 1920, 1080);
        let on = |w: &str| format!("{{\\c&H0035F1C8}}{w}{{\\c&H00FFFFFF}}");
        assert_eq!(
            dialogue(&ass),
            [
                format!(
                    "Dialogue: 0,0:00:01.00,0:00:01.40,Default,,0,0,0,,{} there\\Nfriend",
                    on("Hello")
                ),
                "Dialogue: 0,0:00:01.40,0:00:01.50,Default,,0,0,0,,Hello there\\Nfriend"
                    .to_string(),
                format!(
                    "Dialogue: 0,0:00:01.50,0:00:01.80,Default,,0,0,0,,Hello {}\\Nfriend",
                    on("there")
                ),
                format!(
                    "Dialogue: 0,0:00:01.80,0:00:02.30,Default,,0,0,0,,Hello there\\N{}",
                    on("friend")
                ),
                "Dialogue: 0,0:00:02.30,0:00:02.50,Default,,0,0,0,,Hello there\\Nfriend"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn one_word_pop_snapshot() {
        let style = Style {
            animation: Animation::Pop,
            uppercase: true,
            ..style(CaptionMode::OneWord)
        };
        let ass = to_ass(&[hello()], &style, 1920, 1080);
        let pop = "{\\fscx80\\fscy80\\t(0,120,\\fscx100\\fscy100)}{\\c&H0035F1C8}";
        assert_eq!(
            dialogue(&ass),
            [
                format!("Dialogue: 0,0:00:01.00,0:00:01.50,Default,,0,0,0,,{pop}HELLO"),
                format!("Dialogue: 0,0:00:01.50,0:00:01.80,Default,,0,0,0,,{pop}THERE"),
                format!("Dialogue: 0,0:00:01.80,0:00:02.50,Default,,0,0,0,,{pop}FRIEND"),
            ]
        );
    }

    #[test]
    fn pages_follow_each_other_without_gaps() {
        let long = segment(
            &[
                ("one", 0, 400),
                ("two", 400, 800),
                ("three", 800, 1200),
                ("four", 1200, 1600),
            ],
            0,
            1600,
        );
        let style = Style {
            max_chars_per_line: 9,
            max_lines: 1,
            ..style(CaptionMode::Line)
        };
        assert_eq!(
            dialogue(&to_ass(&[long], &style, 1920, 1080)),
            [
                "Dialogue: 0,0:00:00.00,0:00:00.80,Default,,0,0,0,,one two",
                "Dialogue: 0,0:00:00.80,0:00:01.20,Default,,0,0,0,,three",
                "Dialogue: 0,0:00:01.20,0:00:01.60,Default,,0,0,0,,four",
            ]
        );
    }

    #[test]
    fn escapes_override_characters() {
        let s = segment(&[("{\\b1}hi", 0, 500)], 0, 500);
        let ass = to_ass(&[s], &style(CaptionMode::Line), 1920, 1080);
        assert_eq!(
            dialogue(&ass),
            ["Dialogue: 0,0:00:00.00,0:00:00.50,Default,,0,0,0,,｛＼b1｝hi"]
        );
    }
}
