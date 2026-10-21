//! Caption line layout: how a segment's words are broken into lines and on-screen pages.
//!
//! The same algorithm lives in src/lib/captions/layout.ts for the preview; both are tested against
//! fixtures/layout-cases.json so preview and exports always break lines identically.
//! Keep the two in sync.

use crate::style::{Segment, Word};

/// One caption screen: up to `max_lines` lines of word indices into the layout's words.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub start_ms: u64,
    pub end_ms: u64,
    pub lines: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The segment's words, or one word spanning the segment if it has no word timings.
    pub words: Vec<Word>,
    pub pages: Vec<Page>,
}

/// Line breaking: words fill a line while it stays within `max_chars` characters (words
/// plus single spaces); a longer word gets a line of its own. When the next word doesn't
/// fit, the break may move back a few words to a better spot (see [`choose_break`]).
/// Lines are grouped into pages of `max_lines`, so the same rules decide where captions
/// end. The first page starts with the segment, later pages with their first word, and
/// each page lasts until the next one starts (the last until the segment ends).
pub fn layout(segment: &Segment, max_chars: u32, max_lines: u32) -> Layout {
    let words = if !segment.words.is_empty() {
        segment.words.clone()
    } else if !segment.text.trim().is_empty() {
        vec![Word {
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            text: segment.text.trim().to_string(),
        }]
    } else {
        Vec::new()
    };
    let max_chars = max_chars.max(1) as usize;
    let max_lines = max_lines.max(1) as usize;

    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for i in 0..words.len() {
        if current.is_empty() || line_len(&texts, &current) + 1 + char_len(texts[i]) <= max_chars {
            current.push(i);
            continue;
        }
        let moved = choose_break(&texts, &current, i, max_chars);
        let next: Vec<usize> = current.drain(current.len() - moved..).chain([i]).collect();
        lines.push(std::mem::replace(&mut current, next));
    }
    if !current.is_empty() {
        lines.push(current);
    }

    let chunks: Vec<Vec<Vec<usize>>> = lines.chunks(max_lines).map(<[_]>::to_vec).collect();
    let starts: Vec<u64> = chunks
        .iter()
        .enumerate()
        .map(|(p, page)| {
            if p == 0 {
                segment.start_ms
            } else {
                words[page[0][0]].start_ms
            }
        })
        .collect();
    let pages = chunks
        .into_iter()
        .enumerate()
        .map(|(p, lines)| Page {
            start_ms: starts[p],
            end_ms: starts.get(p + 1).copied().unwrap_or(segment.end_ms),
            lines,
        })
        .collect();
    Layout { words, pages }
}

/// Function words a line or caption shouldn't end on.
const SHORT_WORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "and", "or", "but", "in", "on", "at", "for", "with", "from",
    "by", "as", "i", "my", "your",
];

/// How many of `line`'s last words to move to the next line (with word `next`) when
/// `next` doesn't fit. Each possible break, after word w, scores:
///   +3 if w ends a sentence (. ! ? …), +2 if it ends a clause (, ; :)
///   -3 if w starts a new sentence (dangling "RICH. I / DON'T")
///   -2 if w is a short function word ("the", "to", …)
/// Twice the score minus the words moved wins; ties keep more on the line. A break may not
/// leave the line under half of `max_chars`, and the moved words plus `next` must fit.
fn choose_break(texts: &[&str], line: &[usize], next: usize, max_chars: usize) -> usize {
    let mut best = (i32::MIN, 0);
    for moved in 0..line.len() {
        let keep = &line[..line.len() - moved];
        if moved > 0 {
            let moved_len = line_len(texts, &line[keep.len()..]) + 1 + char_len(texts[next]);
            if moved_len > max_chars || line_len(texts, keep) * 2 < max_chars {
                break;
            }
        }
        let last = texts[*keep.last().unwrap()];
        let mut score = 0;
        if ends_sentence(last) {
            score += 3;
        } else if ends_clause(last) {
            score += 2;
        }
        if keep.len() >= 2 && ends_sentence(texts[keep[keep.len() - 2]]) {
            score -= 3;
        }
        if SHORT_WORDS.contains(&last.to_lowercase().as_str()) {
            score -= 2;
        }
        let total = score * 2 - moved as i32;
        if total > best.0 {
            best = (total, moved);
        }
    }
    best.1
}

/// The word without closing quotes or brackets after its punctuation.
fn core(word: &str) -> &str {
    word.trim_end_matches(['"', '\'', '”', '’', ')', ']', '»'])
}

fn ends_sentence(word: &str) -> bool {
    core(word).ends_with(['.', '!', '?', '…'])
}

fn ends_clause(word: &str) -> bool {
    core(word).ends_with([',', ';', ':'])
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// Characters of the words joined by single spaces.
fn line_len(texts: &[&str], line: &[usize]) -> usize {
    line.iter().map(|&i| char_len(texts[i])).sum::<usize>() + line.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Cases {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        max_chars: u32,
        max_lines: u32,
        segment: CaseSegment,
        pages: Vec<CasePage>,
    }

    #[derive(Deserialize)]
    struct CaseSegment {
        start_ms: u64,
        end_ms: u64,
        text: String,
        words: Vec<Word>,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct CasePage {
        start_ms: u64,
        end_ms: u64,
        lines: Vec<Vec<String>>,
    }

    #[test]
    fn matches_shared_cases() {
        let cases: Cases =
            serde_json::from_str(include_str!("../../fixtures/layout-cases.json")).unwrap();
        for case in cases.cases {
            let segment = Segment {
                id: 0,
                start_ms: case.segment.start_ms,
                end_ms: case.segment.end_ms,
                text: case.segment.text,
                words: case.segment.words,
            };
            let result = layout(&segment, case.max_chars, case.max_lines);
            let pages: Vec<CasePage> = result
                .pages
                .iter()
                .map(|p| CasePage {
                    start_ms: p.start_ms,
                    end_ms: p.end_ms,
                    lines: p
                        .lines
                        .iter()
                        .map(|l| l.iter().map(|&i| result.words[i].text.clone()).collect())
                        .collect(),
                })
                .collect();
            assert_eq!(pages, case.pages, "case: {}", case.name);
        }
    }
}
