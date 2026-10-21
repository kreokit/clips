//! A clip's captions: the transcript's spoken words between its in and out points, with
//! the project's word fixes, on the clip's clock and grouped into caption segments that
//! the line layout (layout.rs) breaks into pages. The editor's preview gets them from
//! here too (`clip_captions`), so preview and export show the same words.

use serde::{Deserialize, Serialize};

use crate::sentences::Text;
use crate::style::{Segment, Word};

/// A word the user corrected, on the video's clock. It applies wherever that word shows,
/// so every clip sharing it gets the fix. An empty `text` hides the word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordFix {
    /// The word's start in the transcript, which identifies it with `was`.
    pub start_ms: u64,
    /// The word as Whisper wrote it; a fix whose word changed (a new transcript) is ignored.
    pub was: String,
    pub text: String,
}

/// A pause between words longer than this starts a new caption.
pub const PAUSE_MS: u64 = 1_000;
/// A caption stays up this long after its last word (less when the next one starts).
pub const HOLD_MS: u64 = 500;

/// The captions of the clip `in_ms`..`out_ms`. A word belongs to the clip when its middle
/// is inside it; its times are clamped into the clip. A new caption starts at every
/// sentence start and after a pause over [`PAUSE_MS`]; it lasts until its last word ends
/// plus [`HOLD_MS`], at most until the next caption or the clip's end.
pub fn clip_captions(text: &Text, fixes: &[WordFix], in_ms: u64, out_ms: u64) -> Vec<Segment> {
    let length = out_ms.saturating_sub(in_ms);
    let mut sentence_starts = vec![false; text.words.len()];
    for s in &text.sentences {
        if let Some(start) = sentence_starts.get_mut(s.first) {
            *start = true;
        }
    }

    // Words as (word on the clip's clock, starts a sentence).
    let mut words: Vec<(Word, bool)> = Vec::new();
    let mut sentence_pending = false;
    for (i, token) in text.words.iter().enumerate() {
        let middle = (token.start_ms + token.end_ms) / 2;
        if middle < in_ms || middle >= out_ms {
            continue;
        }
        sentence_pending |= sentence_starts[i];
        let fixed = fixes
            .iter()
            .find(|f| f.start_ms == token.start_ms && f.was == token.text)
            .map_or(token.text.as_str(), |f| f.text.trim());
        if fixed.is_empty() {
            continue;
        }
        let start_ms = token.start_ms.saturating_sub(in_ms).min(length);
        let end_ms = token.end_ms.saturating_sub(in_ms).clamp(start_ms, length);
        words.push((
            Word {
                start_ms,
                end_ms,
                text: fixed.to_string(),
            },
            std::mem::take(&mut sentence_pending),
        ));
    }

    let mut groups: Vec<Vec<Word>> = Vec::new();
    for (word, sentence) in words {
        let pause = groups
            .last()
            .and_then(|g| g.last())
            .is_some_and(|last| word.start_ms.saturating_sub(last.end_ms) > PAUSE_MS);
        match groups.last_mut() {
            Some(group) if !sentence && !pause => group.push(word),
            _ => groups.push(vec![word]),
        }
    }

    let starts: Vec<u64> = groups.iter().map(|g| g[0].start_ms).collect();
    groups
        .into_iter()
        .enumerate()
        .map(|(i, words)| {
            let last_end = words.last().unwrap().end_ms;
            let limit = starts.get(i + 1).copied().unwrap_or(length);
            Segment {
                id: i as u32,
                start_ms: starts[i],
                end_ms: (last_end + HOLD_MS).min(limit).max(last_end),
                text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
                words,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sentences::{Boundary, Sentence, Token};

    /// Words of `sentences`, each word 300 ms with 100 ms between words and `gap_ms`
    /// between sentences, starting at 0.
    fn text(sentences: &[&str], gap_ms: u64) -> Text {
        let mut out = Text::default();
        let mut t = 0;
        for (n, sentence) in sentences.iter().enumerate() {
            if n > 0 {
                t = t - 100 + gap_ms;
            }
            let first = out.words.len();
            for w in sentence.split(' ') {
                out.words.push(Token {
                    start_ms: t,
                    end_ms: t + 300,
                    text: w.to_string(),
                    norm: w.to_lowercase(),
                    segment: 0,
                });
                t += 400;
            }
            let last = out.words.len() - 1;
            out.sentences.push(Sentence {
                first,
                last,
                start_ms: out.words[first].start_ms,
                end_ms: out.words[last].end_ms,
                starts: Boundary::Punctuation,
                ends: Boundary::Punctuation,
                question: false,
            });
        }
        out
    }

    fn texts(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn every_word_once_on_the_clip_clock() {
        let t = text(&["One two three.", "Four five.", "Six seven eight nine."], 300);
        let all = clip_captions(&t, &[], 0, 100_000);
        let words: Vec<&str> = all.iter().flat_map(|s| s.words.iter().map(|w| w.text.as_str())).collect();
        assert_eq!(words.join(" "), "One two three. Four five. Six seven eight nine.");
        // Each sentence starts a caption.
        assert_eq!(texts(&all), ["One two three.", "Four five.", "Six seven eight nine."]);

        // In at 1000 (the middle of "three." is 950, so it stays out).
        let clip = clip_captions(&t, &[], 1_000, 3_000);
        assert_eq!(texts(&clip), ["Four five.", "Six seven"]);
        let first = &clip[0].words[0];
        assert_eq!((first.start_ms, first.end_ms), (400, 700));
        for s in &clip {
            assert!(s.start_ms <= s.end_ms && s.end_ms <= 2_000);
            for w in &s.words {
                assert!(s.start_ms <= w.start_ms && w.end_ms <= s.end_ms);
            }
        }
    }

    #[test]
    fn edge_words_are_clamped_and_captions_hold_briefly() {
        let t = text(&["One two three four."], 0);
        // "two" runs 400–700 (middle 550): in, clamped to start at 0 from in = 500;
        // "four." (middle 1350) is out.
        let clip = clip_captions(&t, &[], 500, 1_300);
        assert_eq!(texts(&clip), ["two three"]);
        assert_eq!(clip[0].words[0].start_ms, 0);
        // "three" ends at 1100 → 600 on the clip; held 500 ms but not past the end (800).
        assert_eq!(clip[0].end_ms, 800);

        let whole = clip_captions(&t, &[], 0, 10_000);
        assert_eq!(whole[0].end_ms, 1_500 + HOLD_MS);
    }

    #[test]
    fn long_pauses_split_and_the_hold_stops_at_the_next_caption() {
        let mut t = text(&["One two three four."], 0);
        // A pause of 1.2 s after "two" (inside the sentence).
        for w in &mut t.words[2..] {
            w.start_ms += 1_200;
            w.end_ms += 1_200;
        }
        let clip = clip_captions(&t, &[], 0, 10_000);
        assert_eq!(texts(&clip), ["One two", "three four."]);
        assert_eq!(clip[0].end_ms, 700 + HOLD_MS);

        let near = text(&["One.", "Two."], 200);
        let clip = clip_captions(&near, &[], 0, 10_000);
        assert_eq!(clip[0].end_ms, clip[1].start_ms);
    }

    #[test]
    fn fixes_change_or_hide_words_wherever_they_show() {
        let t = text(&["Hello wrold again."], 0);
        let fixes = [
            WordFix { start_ms: 400, was: "wrold".into(), text: "world".into() },
            WordFix { start_ms: 800, was: "again.".into(), text: " ".into() },
            // A fix for a word that's no longer there (another transcript): ignored.
            WordFix { start_ms: 0, was: "Hi".into(), text: "Bye".into() },
        ];
        assert_eq!(texts(&clip_captions(&t, &fixes, 0, 5_000)), ["Hello world"]);
        assert_eq!(texts(&clip_captions(&t, &fixes, 300, 5_000)), ["world"]);
    }

    #[test]
    fn a_hidden_sentence_start_still_starts_the_next_caption() {
        let t = text(&["One two.", "Um three four."], 0);
        let fixes = [WordFix { start_ms: 700, was: "Um".into(), text: String::new() }];
        assert_eq!(texts(&clip_captions(&t, &fixes, 0, 5_000)), ["One two.", "three four."]);
    }

    #[test]
    fn no_words_no_captions() {
        assert!(clip_captions(&Text::default(), &[], 0, 5_000).is_empty());
        let t = text(&["One two."], 0);
        assert!(clip_captions(&t, &[], 3_000, 5_000).is_empty());
        assert!(clip_captions(&t, &[], 500, 500).is_empty());
    }
}
