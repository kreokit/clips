//! SRT (and WebVTT) export, plus parsers for round-trip tests. From KreoKit Captions; Clips
//! writes an SRT per clip on request.
//!
//! Cues are the layout pages (see layout.rs), so subtitle files break lines exactly like
//! the preview.

use serde::Deserialize;

use crate::layout::layout;
use crate::style::{Segment, Style};

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Lines separated by "\n".
    pub text: String,
}

#[cfg_attr(not(test), allow(dead_code))] // Clips writes SRT only
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleFormat {
    Srt,
    Vtt,
    /// Styled; written by ass.rs, not from cues.
    Ass,
}

/// One cue per layout page; pages with no duration are skipped.
pub fn cues(segments: &[Segment], style: &Style) -> Vec<Cue> {
    segments
        .iter()
        .flat_map(|segment| {
            let layout = layout(segment, style.max_chars_per_line, style.max_lines);
            layout
                .pages
                .into_iter()
                .filter(|page| page.end_ms > page.start_ms)
                .map(|page| Cue {
                    start_ms: page.start_ms,
                    end_ms: page.end_ms,
                    text: page
                        .lines
                        .iter()
                        .map(|line| {
                            line.iter()
                                .map(|&i| layout.words[i].text.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn to_srt(cues: &[Cue]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, cue)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                timestamp(cue.start_ms, ','),
                timestamp(cue.end_ms, ','),
                cue.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg_attr(not(test), allow(dead_code))] // Clips writes SRT only
pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n");
    for cue in cues {
        out.push_str(&format!(
            "\n{} --> {}\n{}\n",
            timestamp(cue.start_ms, '.'),
            timestamp(cue.end_ms, '.'),
            // A blank line would end the cue early; "-->" would start a new one.
            cue.text.replace("-->", "→")
        ));
    }
    out
}

/// "HH:MM:SS,mmm" (SRT) or "HH:MM:SS.mmm" (VTT). Hours keep growing past 99.
fn timestamp(ms: u64, separator: char) -> String {
    let (h, rest) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rest) = (rest / 60_000, rest % 60_000);
    let (s, millis) = (rest / 1000, rest % 1000);
    format!("{h:02}:{m:02}:{s:02}{separator}{millis:03}")
}

/// Parses "HH:MM:SS,mmm", "HH:MM:SS.mmm" or (VTT) "MM:SS.mmm".
#[cfg(test)]
fn parse_timestamp(text: &str) -> Option<u64> {
    let (clock, millis) = text.trim().split_once([',', '.'])?;
    if millis.len() != 3 {
        return None;
    }
    let parts: Vec<u64> = clock
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, s) = match parts[..] {
        [h, m, s] => (h, m, s),
        [m, s] => (0, m, s),
        _ => return None,
    };
    if m >= 60 || s >= 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + s) * 1000 + millis.parse::<u64>().ok()?)
}

/// Parses SRT or WebVTT text into cues. Handles CRLF, a BOM, cue numbers / identifiers
/// and VTT cue settings after the timestamps. Malformed blocks are skipped.
#[cfg(test)]
pub fn parse(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    text.split("\n\n")
        .filter_map(|block| {
            let lines: Vec<&str> = block.lines().collect();
            let timing = lines.iter().position(|l| l.contains("-->"))?;
            let (start, rest) = lines[timing].split_once("-->")?;
            let end = rest.split_whitespace().next()?;
            Some(Cue {
                start_ms: parse_timestamp(start)?,
                end_ms: parse_timestamp(end)?,
                text: lines[timing + 1..].join("\n"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Word;

    fn cue(start_ms: u64, end_ms: u64, text: &str) -> Cue {
        Cue {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    fn sample() -> Vec<Cue> {
        vec![
            cue(0, 1_000, "Starts at zero"),
            cue(1_040, 2_650, "Two lines,\nthe second one"),
            cue(3_723_456, 3_725_000, "Past one hour"),
            cue(360_000_000, 360_000_001, "Hundred hours"),
        ]
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(timestamp(0, ','), "00:00:00,000");
        assert_eq!(timestamp(3_723_456, ','), "01:02:03,456");
        assert_eq!(timestamp(59_999, '.'), "00:00:59.999");
        assert_eq!(timestamp(360_000_001, ','), "100:00:00,001");
    }

    #[test]
    fn writes_srt() {
        assert_eq!(
            to_srt(&sample()[..2]),
            "1\n00:00:00,000 --> 00:00:01,000\nStarts at zero\n\n\
             2\n00:00:01,040 --> 00:00:02,650\nTwo lines,\nthe second one\n"
        );
    }

    #[test]
    fn writes_vtt() {
        assert_eq!(
            to_vtt(&sample()[..1]),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nStarts at zero\n"
        );
    }

    #[test]
    fn srt_round_trips() {
        assert_eq!(parse(&to_srt(&sample())), sample());
    }

    #[test]
    fn vtt_round_trips() {
        assert_eq!(parse(&to_vtt(&sample())), sample());
    }

    #[test]
    fn parses_real_world_variations() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\n\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nWorld\r\n";
        assert_eq!(
            parse(srt),
            [cue(1_000, 2_500, "Hello"), cue(3_000, 4_000, "World")]
        );

        let vtt = "WEBVTT - made elsewhere\n\nNOTE a comment\n\nintro\n01:02.500 --> 01:04.000 align:start line:90%\nShort timestamps\n";
        assert_eq!(parse(vtt), [cue(62_500, 64_000, "Short timestamps")]);
    }

    #[test]
    fn rejects_bad_timestamps() {
        for bad in [
            "00:00:60,000",
            "00:61:00,000",
            "1:2",
            "00:00:01,5",
            "aa:bb:cc,ddd",
        ] {
            assert_eq!(parse_timestamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn cues_follow_the_layout() {
        let words: Vec<Word> = ["The", "quick", "brown", "fox", "jumps"]
            .iter()
            .enumerate()
            .map(|(i, t)| Word {
                start_ms: i as u64 * 500,
                end_ms: i as u64 * 500 + 400,
                text: t.to_string(),
            })
            .collect();
        let segment = Segment {
            id: 0,
            start_ms: 0,
            end_ms: 2_500,
            text: "The quick brown fox jumps".into(),
            words,
        };
        let style = Style {
            max_chars_per_line: 10,
            max_lines: 1,
            ..Style::default()
        };
        assert_eq!(
            cues(&[segment], &style),
            [
                cue(0, 1_000, "The quick"),
                cue(1_000, 2_000, "brown fox"),
                cue(2_000, 2_500, "jumps")
            ]
        );
    }
}
