//! Sentences from a transcript's words, so clips never start or end mid-sentence.
//!
//! **Punctuation first.** A sentence ends at a word ending in ".", "?", "!" or "…" (a
//! closing quote or bracket may follow). Titles ("Mr.", "Dr.") never end one; other
//! abbreviations ("etc.", "U.S.") and an ellipsis only when the next word is capitalised.
//!
//! **Pauses where punctuation is missing.** Whisper sometimes writes a whole part in
//! lowercase without full stops, and some speakers run sentences together. Any stretch
//! between sentence ends that is longer than [`FALLBACK_MS`] or [`FALLBACK_WORDS`] is cut
//! at pauses instead, judged from the word timings (whose edges `transcribe.rs` already
//! snapped to the audio's silences) and the words on either side:
//! - a pause scores 1–3 points by length (400 / 700 / 2000 ms, scaled up for speakers who
//!   pause a lot between words), +1 when the next word usually starts a sentence ("so",
//!   "I", "what"…), −1 when it usually continues one ("and", "because"…), +2 at a Whisper
//!   segment edge (on real speech, 3 in 4 segment edges after a pause are sentence ends);
//! - never after a word that can't end a sentence ("the", "to", "my", "is"…) or before
//!   one that can't start it ("of", "is", "than"…), nor between two numbers;
//! - 3 points or more is a [`Boundary::Pause`].
//!
//! Real speech pauses mid-sentence a lot and often runs sentences together, so this finds
//! only some sentence ends: on a real talk stripped of its punctuation, one in seven, and
//! nearly nine in ten of those right (2-point pauses would be right only half the time).
//! A missed end costs a candidate clip, a wrong one cuts a thought, so it stays strict.
//!
//! Finally any sentence longer than [`MAX_SENTENCE_MS`] is split at its best pause of at
//! least 400 ms (a [`Boundary::WeakPause`] below 3 points), if it has one. The moment
//! finder scores clips by how clean their boundaries are.

use serde::Serialize;

use crate::transcribe::{Transcript, Word};

/// Stretches without a sentence end longer than this are cut at pauses.
pub const FALLBACK_MS: u64 = 20_000;
/// …or with more words than this.
pub const FALLBACK_WORDS: usize = 50;
/// Sentences longer than this are split at their best pause.
pub const MAX_SENTENCE_MS: u64 = 30_000;
/// Shorter gaps between words are never a boundary.
const MIN_PAUSE_MS: u64 = 250;
/// Pause lengths worth 1, 2 and 3 points, for a speaker with typical gaps.
const PAUSE_POINTS_MS: [u64; 3] = [400, 700, 2000];
/// Points for a Whisper segment edge at the pause.
const SEGMENT_EDGE_POINTS: i32 = 2;
/// Points a pause needs to end a sentence.
const STRONG_POINTS: i32 = 3;
/// The 75th percentile of gaps between words, for a typical speaker (measured on real
/// speech); speakers with longer gaps get proportionally longer thresholds (up to 2×).
const TYPICAL_GAP_P75_MS: u64 = 110;

/// What ends (or starts) a sentence, from cleanest to least sure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    /// Start or end of the transcript.
    Edge,
    Punctuation,
    /// A long pause with words around it that fit a sentence end (see the module docs).
    Pause,
    /// The best pause in an overlong sentence; may well be mid-thought.
    WeakPause,
}

impl Boundary {
    /// Punctuation, the transcript's edges or a strong pause: safe to cut at.
    pub fn is_clean(self) -> bool {
        !matches!(self, Boundary::WeakPause)
    }
}

/// A spoken word (sound tags left out).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub start_ms: u64,
    pub end_ms: u64,
    /// As Whisper wrote it.
    pub text: String,
    /// Lowercase letters, digits and apostrophes only ("That's," → "that's").
    pub norm: String,
    /// Index of the Whisper segment it came from.
    pub segment: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    /// First and last word (indices into [`Text::words`], inclusive).
    pub first: usize,
    pub last: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub starts: Boundary,
    pub ends: Boundary,
    /// Ends with "?", or (without punctuation) opens like a question ("why", "do you").
    pub question: bool,
}

/// A transcript as words and sentences, plus its sound tags.
#[derive(Debug, Clone, Default)]
pub struct Text {
    pub words: Vec<Token>,
    pub sentences: Vec<Sentence>,
    pub sounds: Vec<Word>,
}

/// Lowercase letters, digits and apostrophes only (curly apostrophes made straight).
pub fn normalize(word: &str) -> String {
    word.chars()
        .map(|c| if c == '’' { '\'' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .trim_matches('\'')
        .to_string()
}

/// The word without closing quotes and brackets ("end.”" → "end.").
fn without_closers(word: &str) -> &str {
    word.trim_end_matches(['"', '\'', ')', ']', '”', '’', '»'])
}

fn capitalised(word: &str) -> bool {
    word.trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
}

/// Titles that never end a sentence.
const TITLES: &[&str] = &["mr.", "mrs.", "ms.", "dr.", "prof.", "st.", "sr.", "mt."];
/// Abbreviations that end a sentence only when the next word is capitalised.
const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "approx.", "no.", "u.s.", "u.k.", "a.m.", "p.m.", "jr.", "inc.",
    "ltd.", "co.",
];

/// Whether `word` ends a sentence by its punctuation, given the word after it.
pub fn ends_sentence(word: &str, next: Option<&str>) -> bool {
    let bare = without_closers(word);
    if bare.ends_with(['?', '!']) {
        return true;
    }
    let next_capital = next.is_none_or(capitalised);
    if bare.ends_with('…') || bare.ends_with("...") {
        return next_capital;
    }
    if !bare.ends_with('.') {
        return false;
    }
    let lower = bare
        .trim_start_matches(['"', '\'', '(', '[', '“', '‘', '«'])
        .to_lowercase();
    if TITLES.contains(&lower.as_str()) {
        return false;
    }
    // "J." (an initial)
    let letters: Vec<char> = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    if letters.len() == 1 && letters[0].is_alphabetic() {
        return false;
    }
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return next_capital;
    }
    true
}

/// Words that can't end a sentence: articles, prepositions, conjunctions, auxiliaries,
/// subject pronouns, possessives, intensifiers, fillers.
const NEVER_END: &[&str] = &[
    "the", "a", "an", "to", "of", "and", "or", "but", "nor", "with", "my", "your", "our", "their",
    "his", "her", "its", "in", "on", "at", "for", "from", "by", "as", "that", "this", "these",
    "those", "than", "if", "because", "so", "very", "really", "just", "i", "i'm", "i've", "i'll",
    "i'd", "we", "we're", "we've", "you're", "you've", "they", "they're", "he", "she", "he's",
    "she's", "is", "are", "was", "were", "be", "been", "am", "have", "has", "had", "do", "does",
    "did", "can", "could", "would", "should", "will", "won't", "can't", "not", "don't", "didn't",
    "doesn't", "it's", "there's", "what's", "into", "about", "like", "um", "uh", "how", "what",
    "when", "where", "who", "which", "whose", "most", "more", "less", "every", "each", "some",
    "any", "no", "until", "unless", "whether", "although", "while", "since", "became", "become",
    "becomes", "gonna", "wanna", "gotta", "let's", "kind", "sort", "lot",
];

/// Words that can't start a sentence.
const NEVER_START: &[&str] = &[
    "of", "is", "are", "was", "were", "am", "has", "had", "than", "to", "for", "with", "from",
    "by", "into", "onto", "at", "down", "off", "isn't", "wasn't", "aren't", "'s",
];

/// Words that usually continue a sentence rather than start one.
const CONTINUES: &[&str] = &[
    "and", "or", "but", "because", "so", "then", "which", "that", "who", "whom", "whose", "where",
    "when", "if", "as",
];

/// Words that often start a spoken sentence.
const STARTERS: &[&str] = &[
    "i",
    "you",
    "we",
    "they",
    "he",
    "she",
    "it's",
    "that's",
    "this",
    "these",
    "what",
    "why",
    "how",
    "now",
    "okay",
    "ok",
    "well",
    "look",
    "listen",
    "there's",
    "here's",
    "my",
    "people",
    "everybody",
    "nobody",
    "yeah",
    "no",
    "yes",
    "today",
    "imagine",
    "let's",
];

/// Question words, and auxiliaries that open a question before a pronoun ("do you").
const WH_WORDS: &[&str] = &["what", "why", "how", "who", "where", "when", "which"];
const AUX: &[&str] = &[
    "do", "does", "did", "is", "are", "was", "were", "can", "could", "would", "should", "will",
    "have", "has", "am", "isn't", "don't", "doesn't", "didn't", "aren't", "won't", "can't",
];
const PRONOUNS: &[&str] = &[
    "you", "i", "we", "they", "he", "she", "it", "that", "this", "there", "anyone", "anybody",
    "everyone",
];

/// The transcript's spoken words, without sound tags (whether hidden or shown).
fn tokens(transcript: &Transcript) -> (Vec<Token>, Vec<Word>) {
    let mut words = Vec::new();
    let mut sounds = Vec::new();
    for (index, seg) in transcript.segments.iter().enumerate() {
        sounds.extend(seg.sounds.iter().cloned());
        for w in &seg.words {
            let in_tag = seg.sounds.iter().any(|s| {
                w.start_ms >= s.start_ms && w.end_ms <= s.end_ms && s.text.contains(&w.text)
            });
            let norm = normalize(&w.text);
            if in_tag || norm.is_empty() {
                continue;
            }
            words.push(Token {
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                text: w.text.clone(),
                norm,
                segment: index,
            });
        }
    }
    (words, sounds)
}

/// Number words and digits ("nineteen", "fifty", "1952").
const NUMBER_WORDS: &[&str] = &[
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "percent",
];

pub fn is_number(norm: &str) -> bool {
    NUMBER_WORDS.contains(&norm) || norm.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// The pause after word `i` (to the next word), in ms.
fn gap_after(words: &[Token], i: usize) -> u64 {
    words[i + 1].start_ms.saturating_sub(words[i].end_ms)
}

/// Points for a pause boundary after word `i` (see the module docs); `None` when the words
/// around it rule it out or the pause is too short. `scale_pct` stretches the thresholds.
fn pause_points(words: &[Token], i: usize, scale_pct: u64) -> Option<i32> {
    let gap = gap_after(words, i);
    let (before, after) = (words[i].norm.as_str(), words[i + 1].norm.as_str());
    if gap < MIN_PAUSE_MS
        || NEVER_END.contains(&before)
        || NEVER_START.contains(&after)
        || (is_number(before) && is_number(after))
    {
        return None;
    }
    let length = PAUSE_POINTS_MS
        .iter()
        .filter(|&&t| gap >= t * scale_pct / 100)
        .count() as i32;
    Some(
        length + i32::from(STARTERS.contains(&after)) - i32::from(CONTINUES.contains(&after))
            + SEGMENT_EDGE_POINTS * i32::from(words[i].segment != words[i + 1].segment),
    )
}

/// How much longer than typical this speaker's gaps are, in percent (100–200).
fn gap_scale_pct(words: &[Token]) -> u64 {
    if words.len() < 20 {
        return 100;
    }
    let mut gaps: Vec<u64> = (0..words.len() - 1).map(|i| gap_after(words, i)).collect();
    gaps.sort_unstable();
    let p75 = gaps[gaps.len() * 3 / 4];
    (p75 * 100 / TYPICAL_GAP_P75_MS).clamp(100, 200)
}

/// Whether the words `first..=last` open like a question.
fn opens_like_question(words: &[Token], first: usize, last: usize) -> bool {
    let w = |i: usize| (i <= last).then(|| words[i].norm.as_str());
    let Some(one) = w(first) else { return false };
    let one = if ["so", "and", "but", "okay", "well"].contains(&one) {
        match w(first + 1) {
            Some(next) => next,
            None => return false,
        }
    } else {
        one
    };
    let skip = usize::from(one != words[first].norm);
    WH_WORDS.contains(&one)
        || (AUX.contains(&one) && w(first + 1 + skip).is_some_and(|p| PRONOUNS.contains(&p)))
}

/// Splits the transcript into sentences (see the module docs).
pub fn split(transcript: &Transcript) -> Text {
    let (words, sounds) = tokens(transcript);
    if words.is_empty() {
        return Text {
            words,
            sentences: Vec::new(),
            sounds,
        };
    }
    let n = words.len();
    let scale = gap_scale_pct(&words);

    // ends[i]: the boundary after word i, if any.
    let mut ends: Vec<Option<Boundary>> = (0..n)
        .map(|i| {
            let next = words.get(i + 1).map(|w| w.text.as_str());
            ends_sentence(&words[i].text, next).then_some(Boundary::Punctuation)
        })
        .collect();
    ends[n - 1] = Some(Boundary::Edge);

    // Pauses inside stretches without punctuation.
    let mut start = 0;
    for end in 0..n {
        if ends[end].is_none() {
            continue;
        }
        let long = words[end].end_ms - words[start].start_ms > FALLBACK_MS
            || end + 1 - start > FALLBACK_WORDS;
        if long {
            for i in start..end {
                ends[i] = pause_points(&words, i, scale)
                    .is_some_and(|p| p >= STRONG_POINTS)
                    .then_some(Boundary::Pause);
            }
        }
        start = end + 1;
    }

    // Overlong sentences: split at their best pause, again and again.
    let mut start = 0;
    let mut end = 0;
    while end < n {
        if ends[end].is_none() {
            end += 1;
            continue;
        }
        if words[end].end_ms - words[start].start_ms > MAX_SENTENCE_MS {
            let best = (start..end)
                .filter_map(|i| {
                    let points = pause_points(&words, i, scale)?;
                    (gap_after(&words, i) >= PAUSE_POINTS_MS[0]).then_some((points, i))
                })
                .max_by_key(|&(points, i)| (points, gap_after(&words, i)));
            if let Some((points, i)) = best {
                ends[i] = Some(if points >= STRONG_POINTS {
                    Boundary::Pause
                } else {
                    Boundary::WeakPause
                });
                end = start; // look at the first half again
                continue;
            }
        }
        start = end + 1;
        end += 1;
    }

    let mut sentences = Vec::new();
    let mut first = 0;
    let mut starts = Boundary::Edge;
    for (last, kind) in ends.iter().enumerate() {
        let Some(kind) = *kind else { continue };
        let bare = without_closers(&words[last].text);
        let question = bare.ends_with('?')
            || (kind != Boundary::Punctuation
                && !bare.ends_with(['.', '!'])
                && opens_like_question(&words, first, last));
        sentences.push(Sentence {
            first,
            last,
            start_ms: words[first].start_ms,
            end_ms: words[last].end_ms,
            starts,
            ends: kind,
            question,
        });
        first = last + 1;
        starts = kind;
    }
    Text {
        words,
        sentences,
        sounds,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::transcribe::Segment;

    /// One segment per entry; each word lasts 250 ms, `gaps_ms[i]` follows word i
    /// (default 50 ms).
    pub(crate) fn transcript(segments: &[&str], gap: impl Fn(usize, &str) -> u64) -> Transcript {
        let mut t = 0;
        let mut i = 0;
        let segments = segments
            .iter()
            .enumerate()
            .map(|(id, text)| {
                let words: Vec<Word> = text
                    .split_whitespace()
                    .map(|w| {
                        let word = Word {
                            start_ms: t,
                            end_ms: t + 250,
                            text: w.to_string(),
                        };
                        t += 250 + gap(i, w);
                        i += 1;
                        word
                    })
                    .collect();
                Segment {
                    id: id as u32,
                    start_ms: words.first().map_or(t, |w| w.start_ms),
                    end_ms: words.last().map_or(t, |w| w.end_ms),
                    text: text.to_string(),
                    words,
                    sounds: Vec::new(),
                }
            })
            .collect();
        Transcript {
            language: "en".into(),
            segments,
        }
    }

    fn sentence_texts(text: &Text) -> Vec<String> {
        text.sentences
            .iter()
            .map(|s| {
                text.words[s.first..=s.last]
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn normalizes_words() {
        assert_eq!(normalize("That’s,"), "that's");
        assert_eq!(normalize("\"Why?\""), "why");
        assert_eq!(normalize("$7"), "7");
        assert_eq!(normalize("..."), "");
    }

    #[test]
    fn punctuation_ends_sentences() {
        assert!(ends_sentence("there.", Some("Then")));
        assert!(
            ends_sentence("there.", Some("then")),
            "a full stop is enough"
        );
        assert!(ends_sentence("why?", Some("because")));
        assert!(ends_sentence("wow!\"", None));
        assert!(ends_sentence("end.)", Some("Next")));
        assert!(!ends_sentence("Dr.", Some("Smith")));
        assert!(!ends_sentence("J.", Some("Smith")));
        assert!(!ends_sentence("e.g.", Some("tables")));
        assert!(ends_sentence("etc.", Some("Then")));
        assert!(!ends_sentence("so...", Some("maybe")));
        assert!(ends_sentence("so...", Some("Maybe")));
        assert!(!ends_sentence("3.5", Some("Then")));
        assert!(!ends_sentence("well,", Some("Then")));
    }

    #[test]
    fn splits_at_punctuation() {
        let t = transcript(
            &[
                "Hello there. How are you? I am fine, thanks.",
                "Dr. Smith agrees.",
            ],
            |_, _| 50,
        );
        let text = split(&t);
        assert_eq!(
            sentence_texts(&text),
            [
                "Hello there.",
                "How are you?",
                "I am fine, thanks.",
                "Dr. Smith agrees."
            ]
        );
        let kinds: Vec<_> = text.sentences.iter().map(|s| (s.starts, s.ends)).collect();
        assert_eq!(kinds[0], (Boundary::Edge, Boundary::Punctuation));
        assert_eq!(kinds[3], (Boundary::Punctuation, Boundary::Edge));
        assert!(text.sentences[1].question);
        assert!(!text.sentences[0].question);
    }

    #[test]
    fn short_unpunctuated_stretches_stay_whole() {
        // Under 20 s and 50 words: a long sentence, not a run of them.
        let t = transcript(
            &["so we went out and then it rained the whole day"],
            |i, _| {
                if i == 4 { 1500 } else { 50 }
            },
        );
        assert_eq!(split(&t).sentences.len(), 1);
    }

    /// 70 lowercase words in sentences of 10, with `pause` ms after each sentence's last
    /// word and 60 ms elsewhere.
    fn lowercase_run(pause: u64) -> Transcript {
        let sentence = "people always ask me how the barn started years ago";
        let text = vec![sentence; 7].join(" ");
        transcript(&[&text], |i, _| if i % 10 == 9 { pause } else { 60 })
    }

    #[test]
    fn long_pauses_end_sentences_without_punctuation() {
        let text = split(&lowercase_run(900));
        assert_eq!(text.sentences.len(), 7, "{:?}", sentence_texts(&text));
        for s in &text.sentences {
            assert_eq!(s.last - s.first, 9);
        }
        // 900 ms (2 points) + "people" starts a sentence (+1).
        assert_eq!(text.sentences[0].ends, Boundary::Pause);
        assert_eq!(text.sentences[0].starts, Boundary::Edge);
        assert_eq!(text.sentences[1].starts, Boundary::Pause);
        assert!(text.sentences[0].ends.is_clean());
    }

    #[test]
    fn short_pauses_are_not_enough() {
        // 450 ms: 1 point + starter = 2, not a sentence end; the run (~23 s) stays one
        // sentence, being under the 30 s limit.
        assert_eq!(split(&lowercase_run(450)).sentences.len(), 1);
        assert_eq!(split(&lowercase_run(200)).sentences.len(), 1);
    }

    #[test]
    fn never_ends_on_a_word_that_cant_end_a_sentence() {
        // Long pauses after "the" and before "of" don't count.
        let words = "we built the barn out of oak and pine ".repeat(8);
        let t = transcript(
            &[&words],
            |_, w| {
                if w == "the" || w == "out" { 1500 } else { 60 }
            },
        );
        let text = split(&t);
        for s in &text.sentences[..text.sentences.len() - 1] {
            let last = &text.words[s.last].norm;
            let next = &text.words[s.last + 1].norm;
            assert!(!NEVER_END.contains(&last.as_str()), "ends on {last}");
            assert!(!NEVER_START.contains(&next.as_str()), "next is {next}");
        }
    }

    #[test]
    fn overlong_sentences_split_at_their_best_pause() {
        // A 40-word, ~36 s sentence with commas and two pauses: split at the longer one.
        let words: Vec<String> = (0..40).map(|i| format!("word{i}")).collect();
        let mut text = words.join(" ");
        text.push('.');
        let t = transcript(&[&text], |i, _| match i {
            12 => 500,
            25 => 800,
            _ => 600,
        });
        let text = split(&t);
        // Over 20 s → pause fallback first, but no pause scores 3 points; over 30 s → split
        // at the best pause, 800 ms (2 points, weak); the halves are then under 30 s.
        assert_eq!(text.sentences.len(), 2, "{:?}", sentence_texts(&text));
        assert_eq!(text.sentences[0].last, 25);
        assert_eq!(text.sentences[0].ends, Boundary::WeakPause);
        assert!(!text.sentences[0].ends.is_clean());
        assert_eq!(text.sentences[1].ends, Boundary::Edge);
    }

    #[test]
    fn questions_without_punctuation_are_recognised() {
        let t = transcript(
            &["so why did you leave the office job back then people asked me that a lot"],
            |i, _| if i == 8 { 1300 } else { 60 },
        );
        // Under the fallback limits → one sentence; force the fallback with a long run.
        assert_eq!(split(&t).sentences.len(), 1);
        let long = "so do you think it works well ".repeat(10);
        let t = transcript(&[&long], |i, _| if i % 7 == 6 { 1300 } else { 60 });
        let text = split(&t);
        assert!(text.sentences.len() > 1);
        assert!(
            text.sentences.iter().all(|s| s.question),
            "{:?}",
            text.sentences
        );
        assert!(
            !opens_like_question(&text.words, 2, 2),
            "\"you\" alone isn't one"
        );
    }

    #[test]
    fn sound_tags_are_not_words() {
        let mut t = transcript(&["That was funny. [Laughter] Anyway."], |_, _| 50);
        let tag = t.segments[0].words[3].clone();
        t.segments[0].sounds = vec![tag.clone()];
        let text = split(&t);
        assert_eq!(text.words.len(), 4);
        assert_eq!(text.sounds, [tag]);
        assert_eq!(sentence_texts(&text), ["That was funny.", "Anyway."]);
    }

    #[test]
    fn empty_transcript() {
        let text = split(&transcript(&[], |_, _| 0));
        assert!(text.words.is_empty() && text.sentences.is_empty());
    }

    // ---- The fallback against real sentence ends --------------------------------------

    /// A tiny deterministic random generator (no dependency needed for tests).
    pub(crate) struct Lcg(u64);
    impl Lcg {
        pub(crate) fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        /// A value from a distribution given as (probability, value) quantiles, linear in
        /// between; the first is (0, min), the last (1, max).
        fn quantile(&mut self, q: &[(f64, u64)]) -> u64 {
            let u = (self.next() % 10_000) as f64 / 10_000.0;
            let i = q
                .iter()
                .rposition(|&(p, _)| p <= u)
                .unwrap_or(0)
                .min(q.len() - 2);
            let ((p0, v0), (p1, v1)) = (q[i], q[i + 1]);
            (v0 as f64 + (v1 as f64 - v0 as f64) * (u - p0) / (p1 - p0)) as u64
        }
    }

    /// The long fixture's script (our own text), one sentence per entry.
    pub(crate) fn script_sentences() -> Vec<String> {
        let script = include_str!("../../fixtures/en/long-script.txt");
        let mut sentences = Vec::new();
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("PAUSE") {
                continue;
            }
            let body = line.split_once(": ").map_or(line, |(_, b)| b);
            let mut current = String::new();
            for word in body.split_whitespace() {
                current.push_str(word);
                current.push(' ');
                if ends_sentence(word, None) {
                    sentences.push(current.trim().to_string());
                    current.clear();
                }
            }
            if !current.trim().is_empty() {
                sentences.push(current.trim().to_string());
            }
        }
        sentences
    }

    /// The script as one long transcript with pauses and Whisper segment edges drawn from
    /// what real speech shows (a 10-minute talk, Whisper Small): gap quantiles after a
    /// sentence end and inside sentences (min and max assumed); a segment edge after 63 %
    /// of sentence ends and 4.7 % of other words. With `strip`, lowercase without
    /// punctuation (Whisper's bad mode). Returns the transcript and, per word, whether a
    /// sentence really ends there.
    pub(crate) fn realistic(seed: u64, strip: bool) -> (Transcript, Vec<bool>) {
        const P: [f64; 8] = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 0.97, 1.0];
        const END_MS: [u64; 8] = [0, 0, 40, 170, 370, 710, 1070, 2500];
        const INNER_MS: [u64; 8] = [0, 0, 0, 0, 110, 240, 620, 2000];
        let end_gaps: Vec<(f64, u64)> = P.into_iter().zip(END_MS).collect();
        let inner_gaps: Vec<(f64, u64)> = P.into_iter().zip(INNER_MS).collect();
        let mut rng = Lcg(seed);
        let mut truth = Vec::new();
        let mut gaps = Vec::new();
        let mut segments: Vec<String> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for sentence in script_sentences() {
            let words: Vec<&str> = sentence.split_whitespace().collect();
            for (j, w) in words.iter().enumerate() {
                let last = j + 1 == words.len();
                truth.push(last);
                gaps.push(rng.quantile(if last { &end_gaps } else { &inner_gaps }));
                current.push(if strip {
                    w.chars()
                        .filter(|c| !matches!(c, '.' | '?' | '!' | ','))
                        .collect::<String>()
                        .to_lowercase()
                } else {
                    w.to_string()
                });
                let edge_per_10k = if last { 6300 } else { 470 };
                if rng.next() % 10_000 < edge_per_10k {
                    segments.push(current.join(" "));
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            segments.push(current.join(" "));
        }
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        (transcript(&refs, |i, _| gaps[i]), truth)
    }

    /// The transcript with its punctuation stripped and lowercased, and the real ends.
    fn stripped(t: &Transcript) -> (Transcript, Vec<bool>) {
        let text = split(t);
        let truth = (0..text.words.len())
            .map(|i| {
                text.sentences
                    .iter()
                    .any(|s| s.last == i && s.ends != Boundary::WeakPause)
            })
            .collect();
        let mut t = t.clone();
        for seg in &mut t.segments {
            for w in &mut seg.words {
                w.text = w
                    .text
                    .chars()
                    .filter(|c| !matches!(c, '.' | '?' | '!' | ','))
                    .collect::<String>()
                    .to_lowercase();
            }
        }
        (t, truth)
    }

    /// Precision and recall of the boundaries of `kinds` against the real ends.
    pub(crate) fn score(text: &Text, truth: &[bool], kinds: &[Boundary]) -> (f64, f64, usize) {
        let found: Vec<usize> = text
            .sentences
            .iter()
            .filter(|s| kinds.contains(&s.ends))
            .map(|s| s.last)
            .collect();
        let right = found.iter().filter(|&&i| truth[i]).count();
        let precision = right as f64 / found.len().max(1) as f64;
        let recall = right as f64 / truth.iter().filter(|&&t| t).count() as f64;
        (precision, recall, found.len())
    }

    #[test]
    fn punctuated_text_keeps_every_sentence_end() {
        let (t, truth) = realistic(7, false);
        let text = split(&t);
        let (p, r, _) = score(&text, &truth, &[Boundary::Punctuation, Boundary::Edge]);
        assert_eq!((p, r), (1.0, 1.0));
    }

    #[test]
    fn the_pause_fallback_is_precise_on_stripped_text() {
        for seed in [1, 2, 3, 4, 5] {
            let (t, truth) = realistic(seed, true);
            let text = split(&t);
            let (p, r, n) = score(&text, &truth, &[Boundary::Pause]);
            let (pw, rw, nw) = score(&text, &truth, &[Boundary::Pause, Boundary::WeakPause]);
            eprintln!(
                "seed {seed}: strong {n} found, precision {p:.2} recall {r:.2}; \
                 with weak {nw}, {pw:.2} / {rw:.2}"
            );
            assert!(p >= 0.75, "strong pauses precision {p:.2}");
            assert!(r >= 0.08, "recall {r:.2}");
            // Weak splits of overlong sentences are often wrong; that's why they're not
            // clean and score lower. They only add to the strong ones.
            assert!(nw >= n && rw >= r && pw <= 1.0);
        }
    }

    /// A real transcript (not in the repo): `KREOKIT_TRANSCRIPT=<transcript.json> cargo
    /// test real_transcript -- --ignored --nocapture`. Strips its punctuation and reports
    /// how well the pauses find its sentence ends again.
    #[test]
    #[ignore]
    fn real_transcript_without_punctuation() {
        let path = std::env::var("KREOKIT_TRANSCRIPT").expect("set KREOKIT_TRANSCRIPT");
        let t: Transcript = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let (t, truth) = stripped(&t);
        let text = split(&t);
        let (p, r, n) = score(&text, &truth, &[Boundary::Pause]);
        eprintln!("strong {n} found, precision {p:.2} recall {r:.2}");
        for s in text.sentences.iter().filter(|s| s.ends == Boundary::Pause) {
            let w = |i: usize| text.words.get(i).map_or("", |w| w.text.as_str());
            eprintln!(
                "{} {} {} {} | {} {} gap {}",
                if truth[s.last] { "  " } else { "✗ " },
                w(s.last.saturating_sub(1)),
                w(s.last),
                "",
                w(s.last + 1),
                w(s.last + 2),
                gap_after(&text.words, s.last.min(text.words.len() - 2))
            );
        }
        assert!(p >= 0.6, "precision {p:.2}");
    }
}
