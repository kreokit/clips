//! The moment finder: proposes clips from a transcript, with a transparent score.
//!
//! **Windows.** Every run of whole sentences ([`crate::sentences`]) lasting between the
//! minimum and maximum length is a candidate; it never starts or ends mid-sentence.
//! Leading fillers ("um", "uh", "you know", "like", "so um") are skipped: the in point
//! moves to the first real word, as long as the clip stays long enough.
//!
//! **Score** (0–100), the weighted sum of five features, each 0–1 with its reasons:
//! - **Hook** (first ~3 s): a question, a number, a strong opener ("the secret is", "most
//!   people"), a bold word ("never", "worst"), "you"; fillers there count against it.
//!   English phrases only: for other languages the hook isn't judged and the other
//!   weights scale up.
//! - **Start**: starting at punctuation beats a pause; a dangling first word ("and",
//!   "because") or an unexplained "he / they / it" costs.
//! - **Ending**: punctuation or a strong pause is a clean ending; a weak pause, a question
//!   left hanging or a next sentence that carries on ("…because") is mid-thought.
//! - **Energy**: speech rate and loudness against the video's own average, laughter or
//!   applause in or right after the clip, minus long dead air.
//! - **Length**: closeness to the target length.
//!
//! **Picking.** Best first; a window overlapping an already picked one by more than
//! [`MAX_OVERLAP`] of the shorter of the two is dropped. Ties go to the earlier clip, so
//! the result is deterministic. Pure functions, no I/O.

use serde::{Deserialize, Serialize};

use crate::sentences::{self, Boundary, Sentence, Text, Token, is_number};

/// What the finder looks for; stored in the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FinderSettings {
    pub min_ms: u64,
    pub max_ms: u64,
    pub target_ms: u64,
    /// How many clips to propose; `None`: about one per [`AUTO_MS_PER_CLIP`] of video.
    pub count: Option<usize>,
}

impl Default for FinderSettings {
    fn default() -> Self {
        Self {
            min_ms: 20_000,
            max_ms: 60_000,
            target_ms: 35_000,
            count: None,
        }
    }
}

/// One proposed clip per this much video, when no count is set…
pub const AUTO_MS_PER_CLIP: u64 = 120_000;
/// …but at least / at most this many.
pub const AUTO_MIN: usize = 5;
pub const AUTO_MAX: usize = 30;
/// Picked clips may share at most this fraction of the shorter one.
pub const MAX_OVERLAP: f64 = 0.2;
/// The hook is judged on the words starting within this long after the in point…
const HOOK_MS: u64 = 3_000;
/// …and at least this many words.
const HOOK_MIN_WORDS: usize = 6;
/// Lead-in before the first word and tail after the last, at most half the gap to the
/// neighbouring word.
const LEAD_MS: u64 = 150;
const TAIL_MS: u64 = 300;
/// Silence inside a clip longer than this counts against its energy.
const DEAD_AIR_MS: u64 = 2_000;
/// A laugh or applause this soon after the out point still counts for the clip.
const SOUND_AFTER_MS: u64 = 2_000;
/// A next sentence starting within this long after the clip's end may carry on its thought.
const CARRY_ON_MS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    Hook,
    Start,
    Ending,
    Energy,
    Length,
}

/// Weights out of 100.
const WEIGHTS: [(FeatureKind, u32); 5] = [
    (FeatureKind::Hook, 25),
    (FeatureKind::Start, 15),
    (FeatureKind::Ending, 15),
    (FeatureKind::Energy, 25),
    (FeatureKind::Length, 20),
];

/// One line of "why this clip".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub text: String,
    /// Raises the score (else lowers it, or just informs).
    pub good: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub kind: FeatureKind,
    /// Out of 100 for all features; 0 when the feature isn't judged.
    pub weight: u32,
    /// 0–1.
    pub value: f64,
    pub reasons: Vec<Reason>,
}

/// A scored stretch of the video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    pub in_ms: u64,
    pub out_ms: u64,
    /// 0–100.
    pub score: u32,
    pub features: Vec<Feature>,
    /// The words of the clip; filled in only for the clips picked (see [`with_text`]).
    pub text: String,
    /// First and last word (indices into [`Text::words`]).
    #[serde(skip)]
    words: (usize, usize),
}

/// What the scores compare against, worked out once per video.
pub struct Context<'a> {
    pub text: &'a Text,
    /// Loudness per second, dBFS (`loudness.json`); may be empty.
    pub loudness: &'a [f32],
    pub language: &'a str,
    pub duration_ms: u64,
    /// Words per second of speech over the whole video.
    rate: f64,
    /// Average loudness of the seconds with speech.
    level: Option<f64>,
    /// Per second of `loudness`: whether a word is spoken in it.
    speech: Vec<bool>,
    /// Per word: the index of its sentence.
    sentence_of: Vec<usize>,
}

impl<'a> Context<'a> {
    pub fn new(text: &'a Text, loudness: &'a [f32], language: &'a str, duration_ms: u64) -> Self {
        let mut speech = vec![false; loudness.len()];
        for w in &text.words {
            let a = (w.start_ms / 1000) as usize;
            let b = (w.end_ms.saturating_sub(1) / 1000) as usize;
            for s in speech.iter_mut().take(b + 1).skip(a) {
                *s = true;
            }
        }
        let mut sentence_of = vec![0; text.words.len()];
        for (k, s) in text.sentences.iter().enumerate() {
            sentence_of[s.first..=s.last].fill(k);
        }
        let mut ctx = Self {
            text,
            loudness,
            language,
            duration_ms,
            rate: speech_rate(text, 0, text.words.len().saturating_sub(1)),
            level: None,
            speech,
            sentence_of,
        };
        ctx.level = ctx.mean_level(0, duration_ms);
        ctx
    }

    /// Average level of the seconds in `from..to` that have speech.
    fn mean_level(&self, from_ms: u64, to_ms: u64) -> Option<f64> {
        let a = (from_ms / 1000) as usize;
        let b = (to_ms.saturating_sub(1) / 1000) as usize + 1;
        let (sum, n) = self
            .loudness
            .iter()
            .zip(&self.speech)
            .take(b)
            .skip(a)
            .filter(|&(_, &s)| s)
            .fold((0.0, 0), |(sum, n), (&l, _)| (sum + l as f64, n + 1));
        (n > 0).then(|| sum / n as f64)
    }

    /// The sentence word `i` is in.
    fn sentence(&self, i: usize) -> &Sentence {
        &self.text.sentences[self.sentence_of[i]]
    }

    fn english(&self) -> bool {
        self.language == "en" || self.language.starts_with("en-")
    }
}

/// Words per second while speaking (pauses over 1 s between words left out).
fn speech_rate(text: &Text, first: usize, last: usize) -> f64 {
    if text.words.is_empty() || last < first {
        return 0.0;
    }
    let mut ms = 0;
    for i in first..=last {
        let w = &text.words[i];
        ms += w.end_ms.saturating_sub(w.start_ms);
        if i < last {
            ms += text.words[i + 1]
                .start_ms
                .saturating_sub(w.end_ms)
                .min(1_000);
        }
    }
    (last + 1 - first) as f64 * 1000.0 / ms.max(1) as f64
}

// ---- Words ----------------------------------------------------------------------------

const FILLERS: &[&str] = &["um", "uh", "uhm", "umm", "erm", "er", "ah", "hmm", "mm"];
/// First words that leave the clip hanging off something said before.
const DANGLING: &[&str] = &[
    "and", "but", "because", "that", "which", "then", "also", "or", "plus",
];
/// Pronouns that need something said before to make sense.
const REFERENCES: &[&str] = &[
    "he", "she", "they", "it", "him", "her", "them", "his", "their", "its",
];
/// Words after which the previous thought carries on.
const CARRIES_ON: &[&str] = &[
    "and", "but", "because", "so", "which", "or", "cause", "'cause",
];
/// Strong openers, as normalized word sequences.
const HOOK_PHRASES: &[&str] = &[
    "the secret",
    "the truth",
    "the mistake",
    "the biggest",
    "the problem",
    "the reason",
    "the key",
    "the one thing",
    "the first thing",
    "the hardest",
    "here's why",
    "here's the thing",
    "here's what",
    "most people",
    "nobody tells you",
    "no one tells you",
    "what nobody",
    "i was wrong",
    "let me tell you",
    "i'll tell you",
    "you need to",
    "you have to",
    "stop doing",
    "changed my life",
    "i learned",
    "the best",
    "the worst",
    "never again",
];
/// Bold single words.
const BOLD_WORDS: &[&str] = &[
    "never",
    "always",
    "nobody",
    "everyone",
    "everybody",
    "nothing",
    "everything",
    "worst",
    "best",
    "biggest",
    "secret",
    "mistake",
    "truth",
    "crazy",
    "insane",
    "impossible",
];
const YOU: &[&str] = &["you", "your", "you're", "you've", "you'll", "yourself"];

/// How many leading words of `words[from..]` are fillers ("um", "you know", "like",
/// "so um").
fn leading_fillers(words: &[Token], from: usize, until: usize) -> usize {
    let w = |i: usize| (i <= until).then(|| words[i].norm.as_str());
    let mut i = from;
    loop {
        match w(i) {
            Some(f) if FILLERS.contains(&f) || f == "like" => i += 1,
            Some("you") if w(i + 1) == Some("know") => i += 2,
            Some("so") if w(i + 1).is_some_and(|n| FILLERS.contains(&n)) => i += 2,
            _ => return i - from,
        }
    }
}

/// `text` quoted for a reason line ("“and”").
fn quote(text: &str) -> String {
    format!("“{text}”")
}

fn join(words: &[Token]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn seconds(ms: u64) -> String {
    format!("{} s", (ms + 500) / 1000)
}

// ---- Scoring --------------------------------------------------------------------------

/// A span to score: words `first..=last`, how it starts and ends, and whether its
/// leading fillers were skipped.
struct Span {
    first: usize,
    last: usize,
    in_ms: u64,
    out_ms: u64,
    starts: Boundary,
    ends: Boundary,
    /// Whether `first` begins a sentence / `last` ends one (manual clips may not).
    whole_start: bool,
    whole_end: bool,
    /// Fillers skipped before `first` (shown as a reason).
    skipped: Option<String>,
}

fn reason(text: impl Into<String>, good: bool) -> Reason {
    Reason {
        text: text.into(),
        good,
    }
}

fn hook(ctx: &Context, span: &Span) -> Feature {
    let words = &ctx.text.words;
    let mut reasons = Vec::new();
    if !ctx.english() {
        reasons.push(reason("Hook not judged for this language", false));
        return Feature {
            kind: FeatureKind::Hook,
            weight: 0,
            value: 0.0,
            reasons,
        };
    }
    let mut end = span.first;
    while end < span.last
        && (end + 1 - span.first < HOOK_MIN_WORDS || words[end + 1].start_ms < span.in_ms + HOOK_MS)
    {
        end += 1;
    }
    let hook = &words[span.first..=end];
    let norms: Vec<&str> = hook.iter().map(|w| w.norm.as_str()).collect();
    let mut value: f64 = 0.3;

    let question = ctx.sentence(span.first).question;
    if question {
        value += 0.3;
        reasons.push(reason("Opens with a question", true));
    }
    let joined = format!(" {} ", norms.join(" "));
    if let Some(phrase) = HOOK_PHRASES
        .iter()
        .find(|p| joined.contains(&format!(" {p} ")))
    {
        value += 0.3;
        reasons.push(reason(format!("Strong opener: {}", quote(phrase)), true));
    } else if let Some(bold) = norms.iter().find(|w| BOLD_WORDS.contains(w)) {
        value += 0.15;
        reasons.push(reason(format!("Bold claim: {}", quote(bold)), true));
    }
    if let Some(i) = norms.iter().position(|w| is_number(w)) {
        value += 0.15;
        reasons.push(reason(
            format!("A number early: {}", quote(&hook[i].text)),
            true,
        ));
    }
    if norms.iter().any(|w| YOU.contains(w)) {
        value += 0.15;
        reasons.push(reason("Speaks to “you”", true));
    }

    if let Some(skipped) = &span.skipped {
        reasons.push(reason(
            format!("Skipped {} at the start", quote(skipped)),
            true,
        ));
    }
    let leading = leading_fillers(words, span.first, end);
    if leading > 0 {
        value -= 0.3;
        let f = join(&words[span.first..span.first + leading]);
        reasons.push(reason(format!("Opens with a filler: {}", quote(&f)), false));
    }
    let inner = norms[leading..]
        .iter()
        .filter(|w| FILLERS.contains(w))
        .count();
    if inner > 0 {
        value -= (0.1 * inner as f64).min(0.3);
        reasons.push(reason(
            if inner == 1 {
                "A filler in the first seconds".to_string()
            } else {
                format!("{inner} fillers in the first seconds")
            },
            false,
        ));
    }
    if reasons.iter().all(|r| !r.good) && leading == 0 && inner == 0 {
        reasons.push(reason("No clear hook in the first seconds", false));
    }
    Feature {
        kind: FeatureKind::Hook,
        weight: 0,
        value: value.clamp(0.0, 1.0),
        reasons,
    }
}

fn start(ctx: &Context, span: &Span) -> Feature {
    let mut value: f64 = 1.0;
    let mut reasons = Vec::new();
    if !span.whole_start {
        value -= 0.6;
        reasons.push(reason("Starts mid-sentence", false));
    } else {
        match span.starts {
            Boundary::Edge | Boundary::Punctuation => {
                reasons.push(reason("Starts at the beginning of a sentence", true))
            }
            Boundary::Pause => {
                value -= 0.1;
                reasons.push(reason("Starts after a pause (no punctuation here)", false));
            }
            Boundary::WeakPause => {
                value -= 0.4;
                reasons.push(reason(
                    "Starts after a short pause, may be mid-sentence",
                    false,
                ));
            }
        }
    }
    let first = &ctx.text.words[span.first];
    if DANGLING.contains(&first.norm.as_str()) {
        value -= 0.4;
        reasons.push(reason(format!("Starts with {}", quote(&first.norm)), false));
    } else if first.norm == "so" {
        value -= 0.2;
        reasons.push(reason("Starts with “so”", false));
    } else if REFERENCES.contains(&first.norm.as_str()) {
        value -= 0.2;
        reasons.push(reason(
            format!("Starts with {}: said before?", quote(&first.norm)),
            false,
        ));
    }
    Feature {
        kind: FeatureKind::Start,
        weight: 0,
        value: value.clamp(0.0, 1.0),
        reasons,
    }
}

fn ending(ctx: &Context, span: &Span) -> Feature {
    let words = &ctx.text.words;
    let mut value: f64 = 1.0;
    let mut reasons = Vec::new();
    if !span.whole_end {
        value -= 0.7;
        reasons.push(reason("Ends mid-sentence", false));
    } else {
        match span.ends {
            Boundary::Edge | Boundary::Punctuation => {
                reasons.push(reason("Ends cleanly at the end of a sentence", true))
            }
            Boundary::Pause => reasons.push(reason("Ends at a long pause", true)),
            Boundary::WeakPause => {
                value -= 0.5;
                reasons.push(reason("Ends at a short pause, may be mid-thought", false));
            }
        }
        let sentence = ctx.sentence(span.last);
        let last = sentence.last == span.last && sentence.question;
        if last {
            value -= 0.3;
            reasons.push(reason("Ends on a question", false));
        }
    }
    if let Some(next) = words.get(span.last + 1) {
        let soon = next.start_ms.saturating_sub(words[span.last].end_ms) <= CARRY_ON_MS;
        if soon && CARRIES_ON.contains(&next.norm.as_str()) {
            value -= 0.3;
            reasons.push(reason(
                format!("The thought carries on: {}…", quote(&next.norm)),
                false,
            ));
        }
    }
    Feature {
        kind: FeatureKind::Ending,
        weight: 0,
        value: value.clamp(0.0, 1.0),
        reasons,
    }
}

/// Signed percentage ("18% faster").
fn percent(ratio: f64) -> u32 {
    ((ratio - 1.0).abs() * 100.0).round() as u32
}

fn energy(ctx: &Context, span: &Span) -> Feature {
    let words = &ctx.text.words;
    let mut reasons = Vec::new();
    let mut parts = Vec::new();

    let rate = speech_rate(ctx.text, span.first, span.last);
    if ctx.rate > 0.0 {
        let ratio = rate / ctx.rate;
        parts.push((0.5 + (ratio - 1.0) * 1.25).clamp(0.0, 1.0));
        if percent(ratio) >= 10 {
            let faster = ratio > 1.0;
            reasons.push(reason(
                format!(
                    "Speech {}% {} than usual",
                    percent(ratio),
                    if faster { "faster" } else { "slower" }
                ),
                faster,
            ));
        }
    }
    if let (Some(avg), Some(level)) = (ctx.level, ctx.mean_level(span.in_ms, span.out_ms)) {
        let diff = level - avg;
        parts.push((0.5 + diff / 6.0).clamp(0.0, 1.0));
        if diff.abs() >= 1.0 {
            reasons.push(reason(
                format!(
                    "{:.1} dB {} than usual",
                    diff.abs(),
                    if diff > 0.0 { "louder" } else { "quieter" }
                ),
                diff > 0.0,
            ));
        }
    }
    let mut value = if parts.is_empty() {
        0.5
    } else {
        parts.iter().sum::<f64>() / parts.len() as f64
    };

    let mut kinds: Vec<&str> = Vec::new();
    for s in &ctx.text.sounds {
        if s.start_ms + 1 < span.in_ms || s.start_ms > span.out_ms + SOUND_AFTER_MS {
            continue;
        }
        let t = s.text.to_lowercase();
        let kind = if t.contains("laugh") || t.contains("chuckl") {
            "Laughter"
        } else if t.contains("applause") || t.contains("clap") || t.contains("cheer") {
            "Applause"
        } else {
            continue;
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    for kind in kinds {
        value += 0.25;
        reasons.push(reason(kind, true));
    }

    let longest_gap = (span.first..span.last)
        .map(|i| words[i + 1].start_ms.saturating_sub(words[i].end_ms))
        .max()
        .unwrap_or(0);
    if longest_gap > DEAD_AIR_MS {
        value -= 0.2;
        reasons.push(reason(
            format!("{:.1} s of silence inside", longest_gap as f64 / 1000.0),
            false,
        ));
    }
    Feature {
        kind: FeatureKind::Energy,
        weight: 0,
        value: value.clamp(0.0, 1.0),
        reasons,
    }
}

fn length(settings: &FinderSettings, span: &Span) -> Feature {
    let len = span.out_ms - span.in_ms;
    let target = settings.target_ms;
    let (side, off) = if len >= target {
        (settings.max_ms.saturating_sub(target), len - target)
    } else {
        (target.saturating_sub(settings.min_ms), target - len)
    };
    // 1 at the target, 0.4 at the min/max, 0 at twice that distance.
    let value = (1.0 - 0.6 * off as f64 / side.max(1) as f64).clamp(0.0, 1.0);
    let text = if value >= 0.8 {
        format!("{}, close to the {} target", seconds(len), seconds(target))
    } else if len > target {
        format!(
            "{}, longer than the {} target",
            seconds(len),
            seconds(target)
        )
    } else {
        format!(
            "{}, shorter than the {} target",
            seconds(len),
            seconds(target)
        )
    };
    let mut reasons = vec![reason(text, value >= 0.8)];
    if len < settings.min_ms || len > settings.max_ms {
        reasons.push(reason(
            format!(
                "Outside {}–{}",
                seconds(settings.min_ms),
                seconds(settings.max_ms)
            ),
            false,
        ));
    }
    Feature {
        kind: FeatureKind::Length,
        weight: 0,
        value,
        reasons,
    }
}

fn moment(ctx: &Context, settings: &FinderSettings, span: Span) -> Moment {
    let mut features = vec![
        hook(ctx, &span),
        start(ctx, &span),
        ending(ctx, &span),
        energy(ctx, &span),
        length(settings, &span),
    ];
    let judged = |kind: FeatureKind| kind != FeatureKind::Hook || ctx.english();
    let total: u32 = WEIGHTS
        .iter()
        .filter(|(k, _)| judged(*k))
        .map(|(_, w)| w)
        .sum();
    let mut score = 0.0;
    for f in &mut features {
        let w = WEIGHTS
            .iter()
            .find(|(k, _)| *k == f.kind)
            .map_or(0, |(_, w)| *w);
        f.weight = if judged(f.kind) { w * 100 / total } else { 0 };
        score += f.weight as f64 * f.value;
    }
    Moment {
        in_ms: span.in_ms,
        out_ms: span.out_ms,
        score: score.round().clamp(0.0, 100.0) as u32,
        features,
        text: String::new(),
        words: (span.first, span.last),
    }
}

/// The moment with its words filled in.
pub fn with_text(ctx: &Context, mut m: Moment) -> Moment {
    m.text = join(&ctx.text.words[m.words.0..=m.words.1]);
    m
}

/// In point before word `first`: a short lead-in, at most half the gap to the word before.
fn in_point(words: &[Token], first: usize) -> u64 {
    let start = words[first].start_ms;
    let limit = match first {
        0 => 0,
        _ => start - start.saturating_sub(words[first - 1].end_ms) / 2,
    };
    start.saturating_sub(LEAD_MS).max(limit)
}

/// Out point after word `last`: a short tail, at most half the gap to the next word.
fn out_point(words: &[Token], last: usize, duration_ms: u64) -> u64 {
    let end = words[last].end_ms;
    let limit = match words.get(last + 1) {
        Some(next) => end + next.start_ms.saturating_sub(end) / 2,
        None => duration_ms.max(end),
    };
    (end + TAIL_MS).min(limit)
}

// ---- Finding ----------------------------------------------------------------------------

/// Every whole-sentence window between the min and max length, scored.
pub fn candidates(ctx: &Context, settings: &FinderSettings) -> Vec<Moment> {
    let words = &ctx.text.words;
    let sentences = &ctx.text.sentences;
    let mut out = Vec::new();
    for (i, first_sentence) in sentences.iter().enumerate() {
        // Skip leading fillers, unless the sentence is nothing but fillers.
        let mut first = first_sentence.first;
        let skip = leading_fillers(words, first, first_sentence.last);
        let skippable = skip > 0 && first + skip <= first_sentence.last;
        for last_sentence in &sentences[i..] {
            let last = last_sentence.last;
            let out_ms = out_point(words, last, ctx.duration_ms);
            let full_in = in_point(words, first_sentence.first);
            if out_ms - full_in > settings.max_ms {
                break;
            }
            let trimmed_in = in_point(words, first + if skippable { skip } else { 0 });
            let (in_ms, skipped) = if skippable && out_ms - trimmed_in >= settings.min_ms {
                (
                    trimmed_in,
                    Some(join(
                        &words[first_sentence.first..first_sentence.first + skip],
                    )),
                )
            } else {
                (full_in, None)
            };
            if out_ms - in_ms < settings.min_ms {
                continue;
            }
            if skipped.is_some() {
                first = first_sentence.first + skip;
            } else {
                first = first_sentence.first;
            }
            out.push(moment(
                ctx,
                settings,
                Span {
                    first,
                    last,
                    in_ms,
                    out_ms,
                    starts: first_sentence.starts,
                    ends: last_sentence.ends,
                    whole_start: true,
                    whole_end: true,
                    skipped,
                },
            ));
        }
    }
    out
}

/// How many clips to propose.
pub fn wanted(settings: &FinderSettings, duration_ms: u64) -> usize {
    settings.count.unwrap_or_else(|| {
        (((duration_ms + AUTO_MS_PER_CLIP / 2) / AUTO_MS_PER_CLIP) as usize)
            .clamp(AUTO_MIN, AUTO_MAX)
    })
}

/// Whether two clips share more than [`MAX_OVERLAP`] of the shorter one.
pub fn overlaps(a: (u64, u64), b: (u64, u64)) -> bool {
    let shared = a.1.min(b.1).saturating_sub(a.0.max(b.0));
    let shorter = (a.1 - a.0).min(b.1 - b.0).max(1);
    shared as f64 > MAX_OVERLAP * shorter as f64
}

/// The best `count` clips, best first, no two overlapping (see [`overlaps`]) and none
/// overlapping `avoid` (clips already on the board, kept or discarded).
pub fn pick(mut all: Vec<Moment>, count: usize, avoid: &[(u64, u64)]) -> Vec<Moment> {
    all.sort_by(|a, b| b.score.cmp(&a.score).then(a.in_ms.cmp(&b.in_ms)));
    let mut picked: Vec<Moment> = Vec::new();
    for m in all {
        if picked.len() >= count {
            break;
        }
        let range = (m.in_ms, m.out_ms);
        let clash = avoid.iter().any(|&t| overlaps(t, range))
            || picked.iter().any(|p| overlaps((p.in_ms, p.out_ms), range));
        if !clash {
            picked.push(m);
        }
    }
    picked
}

/// Finds the best moments: as many as [`wanted`] minus the `have` clips the board already
/// counts, none overlapping `avoid`.
pub fn find(
    ctx: &Context,
    settings: &FinderSettings,
    avoid: &[(u64, u64)],
    have: usize,
) -> Vec<Moment> {
    let count = wanted(settings, ctx.duration_ms).saturating_sub(have);
    pick(candidates(ctx, settings), count, avoid)
        .into_iter()
        .map(|m| with_text(ctx, m))
        .collect()
}

/// Scores any stretch (an edited or manual clip): the words whose middle falls inside it.
/// `None` when it has no words.
pub fn score_range(
    ctx: &Context,
    settings: &FinderSettings,
    in_ms: u64,
    out_ms: u64,
) -> Option<Moment> {
    let words = &ctx.text.words;
    let inside = |w: &Token| {
        let mid = (w.start_ms + w.end_ms) / 2;
        mid >= in_ms && mid < out_ms
    };
    let first = words.iter().position(inside)?;
    let last = words.iter().rposition(inside)?;
    let starting = Some(ctx.sentence(first)).filter(|s| s.first == first);
    let ending = Some(ctx.sentence(last)).filter(|s| s.last == last);
    Some(with_text(
        ctx,
        moment(
            ctx,
            settings,
            Span {
                first,
                last,
                in_ms,
                out_ms: out_ms.max(in_ms + 1),
                starts: starting.map_or(Boundary::WeakPause, |s| s.starts),
                ends: ending.map_or(Boundary::WeakPause, |s| s.ends),
                whole_start: starting.is_some(),
                whole_end: ending.is_some(),
                skipped: None,
            },
        ),
    ))
}

/// Splits a transcript and finds its moments (what the command runs).
pub fn find_in(
    transcript: &crate::transcribe::Transcript,
    loudness: &[f32],
    duration_ms: u64,
    settings: &FinderSettings,
    avoid: &[(u64, u64)],
    have: usize,
) -> Vec<Moment> {
    let text = sentences::split(transcript);
    let ctx = Context::new(&text, loudness, &transcript.language, duration_ms);
    find(&ctx, settings, avoid, have)
}

/// Scores stretches of a transcript (edited or manual clips); `None` where there are no
/// words.
pub fn score_in(
    transcript: &crate::transcribe::Transcript,
    loudness: &[f32],
    duration_ms: u64,
    settings: &FinderSettings,
    ranges: &[(u64, u64)],
) -> Vec<Option<Moment>> {
    let text = sentences::split(transcript);
    let ctx = Context::new(&text, loudness, &transcript.language, duration_ms);
    ranges
        .iter()
        .map(|&(a, b)| score_range(&ctx, settings, a, b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sentences::tests::{realistic, transcript};
    use crate::transcribe::{Transcript, Word};

    /// A transcript of these sentences, one segment each; each word 250 ms, 50 ms between
    /// words, `pause` ms between sentences.
    fn talk(sentences: &[&str], pause: u64) -> Transcript {
        let ends: Vec<usize> = sentences
            .iter()
            .scan(0, |n, s| {
                *n += s.split_whitespace().count();
                Some(*n - 1)
            })
            .collect();
        transcript(sentences, |i, _| if ends.contains(&i) { pause } else { 50 })
    }

    /// A ~5 s sentence (16 words) of plain filler text.
    const PLAIN: &str =
        "We moved the workbench to the other side of the barn during the winter months.";

    fn settings(min: u64, max: u64, target: u64) -> FinderSettings {
        FinderSettings {
            min_ms: min,
            max_ms: max,
            target_ms: target,
            count: None,
        }
    }

    /// Every candidate, with its words.
    fn candidates(ctx: &Context, s: &FinderSettings) -> Vec<Moment> {
        super::candidates(ctx, s)
            .into_iter()
            .map(|m| with_text(ctx, m))
            .collect()
    }

    fn with_ctx<R>(t: &Transcript, loudness: &[f32], f: impl FnOnce(&Context) -> R) -> R {
        let text = sentences::split(t);
        let end = t.segments.last().map_or(0, |s| s.end_ms);
        let ctx = Context::new(&text, loudness, &t.language, end + 1000);
        f(&ctx)
    }

    fn feature(m: &Moment, kind: FeatureKind) -> &Feature {
        m.features.iter().find(|f| f.kind == kind).unwrap()
    }

    fn reasons(m: &Moment) -> Vec<String> {
        m.features
            .iter()
            .flat_map(|f| f.reasons.iter().map(|r| r.text.clone()))
            .collect()
    }

    #[test]
    fn windows_are_whole_sentences_within_the_limits() {
        let (t, _) = realistic(3, false);
        let s = settings(20_000, 60_000, 35_000);
        with_ctx(&t, &[], |ctx| {
            let all = candidates(ctx, &s);
            assert!(all.len() > 50, "{}", all.len());
            let words = &ctx.text.words;
            for m in &all {
                assert!(m.out_ms - m.in_ms >= s.min_ms && m.out_ms - m.in_ms <= s.max_ms);
                // The first word (after any skipped fillers) and last word of a clip are a
                // sentence's first (or after fillers) and a sentence's last.
                let first = words.iter().position(|w| w.start_ms >= m.in_ms).unwrap();
                let last = words.iter().rposition(|w| w.end_ms <= m.out_ms).unwrap();
                let starts = ctx.text.sentences.iter().any(|x| {
                    x.first == first
                        || (x.first < first
                            && leading_fillers(words, x.first, x.last) == first - x.first)
                });
                assert!(starts, "starts mid-sentence: {}", m.text);
                assert!(
                    ctx.text.sentences.iter().any(|x| x.last == last),
                    "{}",
                    m.text
                );
                // Nothing of a neighbouring word is inside.
                assert!(first == 0 || words[first - 1].end_ms <= m.in_ms);
                assert!(words.get(last + 1).is_none_or(|w| w.start_ms >= m.out_ms));
            }
        });
    }

    #[test]
    fn picks_the_best_without_overlaps() {
        let (t, _) = realistic(3, false);
        let s = settings(20_000, 60_000, 35_000);
        with_ctx(&t, &[], |ctx| {
            let picked = find(ctx, &s, &[], 0);
            assert_eq!(picked.len(), wanted(&s, ctx.duration_ms));
            for (i, a) in picked.iter().enumerate() {
                for b in &picked[i + 1..] {
                    assert!(!overlaps((a.in_ms, a.out_ms), (b.in_ms, b.out_ms)));
                    assert!(a.score >= b.score);
                }
            }
            // Deterministic.
            assert_eq!(find(ctx, &s, &[], 0), picked);
            // Clips already on the board are avoided and count towards the total.
            let taken = [(picked[0].in_ms, picked[0].out_ms)];
            let again = find(ctx, &s, &taken, 1);
            assert_eq!(again.len(), picked.len() - 1);
            assert!(
                again
                    .iter()
                    .all(|m| !overlaps(taken[0], (m.in_ms, m.out_ms)))
            );
        });
    }

    #[test]
    fn automatic_count_is_one_per_two_minutes_within_limits() {
        let s = FinderSettings::default();
        assert_eq!(wanted(&s, 3 * 60_000), 5);
        assert_eq!(wanted(&s, 20 * 60_000), 10);
        assert_eq!(wanted(&s, 3 * 3_600_000), 30);
        assert_eq!(
            wanted(
                &FinderSettings {
                    count: Some(3),
                    ..s
                },
                3_600_000
            ),
            3
        );
    }

    #[test]
    fn overlap_is_judged_on_the_shorter_clip() {
        assert!(!overlaps((0, 30_000), (30_000, 60_000)));
        assert!(
            !overlaps((0, 30_000), (25_000, 60_000)),
            "5 s of 30 s: under 20%"
        );
        assert!(overlaps((0, 30_000), (20_000, 60_000)));
        assert!(overlaps((0, 60_000), (10_000, 30_000)), "contained");
    }

    /// Scores of the single window covering `first` + 3 plain sentences (~20 s).
    fn score_of(first: &str, loudness: &[f32]) -> Moment {
        let t = talk(&[first, PLAIN, PLAIN, PLAIN], 400);
        let s = settings(1_000, 60_000, 20_000);
        with_ctx(&t, loudness, |ctx| {
            candidates(ctx, &s)
                .into_iter()
                .find(|m| m.in_ms < 1_000 && m.out_ms + 1_000 > ctx.duration_ms)
                .unwrap()
        })
    }

    #[test]
    fn hooks_raise_the_score() {
        let plain = score_of(PLAIN, &[]);
        let question = score_of(
            "Why do most furniture makers quit in their first year?",
            &[],
        );
        let secret = score_of("The secret is that the wood decides, not you.", &[]);
        let number = score_of("I sold eleven pieces before I quit my job.", &[]);
        let hook = |m: &Moment| feature(m, FeatureKind::Hook).value;
        assert!(hook(&question) > hook(&plain), "{:?}", reasons(&question));
        assert!(hook(&secret) > hook(&plain));
        assert!(hook(&number) > hook(&plain));
        assert!(question.score > plain.score && secret.score > plain.score);
        assert!(reasons(&question).contains(&"Opens with a question".to_string()));
        assert!(reasons(&secret).iter().any(|r| r.contains("the secret")));
        assert!(reasons(&secret).iter().any(|r| r == "Speaks to “you”"));
        assert!(reasons(&plain).contains(&"No clear hook in the first seconds".to_string()));
    }

    #[test]
    fn dangling_starts_cost() {
        let plain = score_of(
            "My grandfather built a chair that lasted seventy years.",
            &[],
        );
        let dangling = score_of(
            "And my grandfather built a chair that lasted seventy years.",
            &[],
        );
        let pronoun = score_of(
            "He built a chair that lasted seventy years in that house.",
            &[],
        );
        let start = |m: &Moment| feature(m, FeatureKind::Start).value;
        assert!(start(&dangling) < start(&plain));
        assert!(start(&pronoun) < start(&plain));
        assert!(dangling.score < plain.score);
        assert!(reasons(&dangling).contains(&"Starts with “and”".to_string()));
    }

    #[test]
    fn leading_fillers_are_skipped_when_possible() {
        let um = "Um, you know, the secret is that the wood decides.";
        let t = talk(&[um, PLAIN, PLAIN, PLAIN], 400);
        let words = |t: &Transcript| t.segments[0].words.clone();
        let the = words(&t)[3].clone();
        assert_eq!(the.text, "the");

        // Room to skip: the in point moves to "the" and the hook is judged from there.
        let s = settings(1_000, 60_000, 20_000);
        let m = with_ctx(&t, &[], |ctx| {
            candidates(ctx, &s)
                .into_iter()
                .find(|m| m.in_ms < 2_000)
                .unwrap()
        });
        assert_eq!(
            m.in_ms,
            the.start_ms - LEAD_MS.min((the.start_ms - words(&t)[2].end_ms) / 2)
        );
        assert!(m.text.starts_with("the secret"));
        assert!(reasons(&m).contains(&"Skipped “Um, you know,” at the start".to_string()));
        assert!(
            !reasons(&m)
                .iter()
                .any(|r| r.starts_with("Opens with a filler"))
        );

        // No room (the clip would get too short): the fillers stay and cost hook points.
        let whole = with_ctx(&t, &[], |ctx| ctx.duration_ms - 1000);
        let tight = settings(whole - 200, 60_000, 20_000);
        let kept = with_ctx(&t, &[], |ctx| {
            candidates(ctx, &tight)
                .into_iter()
                .find(|m| m.in_ms < 2_000)
                .unwrap()
        });
        assert!(kept.text.starts_with("Um, you know,"));
        assert!(
            reasons(&kept)
                .iter()
                .any(|r| r.starts_with("Opens with a filler"))
        );
        let hook = |m: &Moment| feature(m, FeatureKind::Hook).value;
        assert!(hook(&kept) < hook(&m));

        // "so um" and "like" are fillers too; a plain "so" isn't.
        let t = talk(&["So um like we built it."], 0);
        let text = sentences::split(&t);
        assert_eq!(leading_fillers(&text.words, 0, 5), 3);
        let t = talk(&["So we built it."], 0);
        assert_eq!(leading_fillers(&sentences::split(&t).words, 0, 3), 0);
    }

    #[test]
    fn fillers_inside_the_hook_cost() {
        let clean = score_of("The barn was cold and dark when we first moved in.", &[]);
        let um = score_of(
            "The barn was um cold and uh dark when we first moved in.",
            &[],
        );
        let hook = |m: &Moment| feature(m, FeatureKind::Hook).value;
        assert!(hook(&um) < hook(&clean));
        assert!(reasons(&um).contains(&"2 fillers in the first seconds".to_string()));
    }

    #[test]
    fn clean_endings_beat_mid_thought_ones() {
        let s = settings(1_000, 60_000, 20_000);
        let last_window = |t: &Transcript| {
            with_ctx(t, &[], |ctx| {
                candidates(ctx, &s)
                    .into_iter()
                    .filter(|m| m.in_ms < 1_000 && m.text.ends_with("winter months."))
                    .max_by_key(|m| m.out_ms)
                    .unwrap()
            })
        };
        let ending = |m: &Moment| feature(m, FeatureKind::Ending).value;
        // Clean: punctuation, then a pause before an unrelated sentence.
        let clean = last_window(&talk(&[PLAIN, PLAIN, PLAIN, "Then spring came."], 600));
        assert_eq!(ending(&clean), 1.0);
        assert!(reasons(&clean).contains(&"Ends cleanly at the end of a sentence".to_string()));
        // The next sentence follows at once with "because": the thought carries on.
        let carried = last_window(&talk(
            &[PLAIN, PLAIN, PLAIN, "Because the roof leaked."],
            100,
        ));
        assert!(ending(&carried) < ending(&clean));
        assert!(reasons(&carried).contains(&"The thought carries on: “because”…".to_string()));
        // Ending on a question.
        let q = "Did we regret moving the workbench during the cold winter months?";
        let question = with_ctx(&talk(&[PLAIN, PLAIN, PLAIN, q], 600), &[], |ctx| {
            candidates(ctx, &s)
                .into_iter()
                .find(|m| m.text.ends_with("months?"))
                .unwrap()
        });
        assert!(ending(&question) < 1.0);
    }

    #[test]
    fn weak_endings_and_manual_cuts_cost() {
        // A ~36 s unpunctuated sentence gets split at its best pause, a weak boundary.
        let words: Vec<String> = (0..40).map(|i| format!("word{i}")).collect();
        let mut text = words.join(" ");
        text.push('.');
        let t = transcript(&[&text], |i, _| if i == 25 { 800 } else { 600 });
        let s = settings(1_000, 60_000, 20_000);
        with_ctx(&t, &[], |ctx| {
            assert_eq!(ctx.text.sentences[0].ends, Boundary::WeakPause);
            let m = candidates(ctx, &s)
                .into_iter()
                .find(|m| m.text.ends_with("word25"))
                .unwrap();
            assert_eq!(feature(&m, FeatureKind::Ending).value, 0.5);

            // A manual range cutting through sentences.
            let w = &ctx.text.words;
            let manual = score_range(ctx, &s, w[3].start_ms, w[10].end_ms).unwrap();
            assert!(reasons(&manual).contains(&"Starts mid-sentence".to_string()));
            assert!(reasons(&manual).contains(&"Ends mid-sentence".to_string()));
            assert!(manual.text.starts_with("word3") && manual.text.ends_with("word10"));
            assert_eq!(score_range(ctx, &s, 10_000_000, 10_000_100), None);
        });
    }

    #[test]
    fn energy_follows_rate_loudness_and_laughter() {
        // Six sentences of eight words; the one at `fast_at` is spoken without gaps
        // between its words, the others with 400 ms gaps.
        let sentence = "we built tables and chairs all year long.";
        let make = |fast_at: usize| {
            let sentences = vec![sentence; 6];
            transcript(&sentences, |i, _| match (i / 8 == fast_at, i % 8 == 7) {
                (_, true) => 900,
                (true, false) => 20,
                (false, false) => 400,
            })
        };
        let s = settings(1_000, 60_000, 3_000);
        let rate_of = |t: &Transcript, loud: &[f32], i: usize| {
            with_ctx(t, loud, |ctx| {
                let sentence = &ctx.text.sentences[i];
                let m = score_range(ctx, &s, sentence.start_ms, sentence.end_ms + 1).unwrap();
                (feature(&m, FeatureKind::Energy).value, reasons(&m))
            })
        };
        let t = make(2);
        let (fast_e, fast_r) = rate_of(&t, &[], 2);
        let (slow_e, _) = rate_of(&t, &[], 1);
        assert!(fast_e > slow_e, "{fast_e} vs {slow_e}");
        assert!(
            fast_r.iter().any(|r| r.contains("faster than usual")),
            "{fast_r:?}"
        );

        // Louder seconds raise it.
        let quiet: Vec<f32> = vec![-30.0; 60];
        let mut loud = quiet.clone();
        let third = with_ctx(&t, &[], |ctx| ctx.text.sentences[3].clone());
        for l in loud
            .iter_mut()
            .take((third.end_ms / 1000 + 1) as usize)
            .skip((third.start_ms / 1000) as usize)
        {
            *l = -22.0;
        }
        let (louder, r) = rate_of(&t, &loud, 3);
        let (same, _) = rate_of(&t, &quiet, 3);
        assert!(louder > same, "{r:?}");
        assert!(r.iter().any(|r| r.contains("dB louder")));

        // Laughter right after the clip counts.
        let mut t = make(9);
        let after = t.segments[4].words[0].end_ms + 200;
        t.segments[4].sounds.push(Word {
            start_ms: after,
            end_ms: after + 800,
            text: "[Laughter]".into(),
        });
        let (plain, _) = rate_of(&make(9), &[], 4);
        let (laugh, r) = rate_of(&t, &[], 4);
        assert!(laugh > plain);
        assert!(r.contains(&"Laughter".to_string()));
    }

    #[test]
    fn length_fit_peaks_at_the_target() {
        let s = settings(20_000, 60_000, 35_000);
        let span = |len: u64| Span {
            first: 0,
            last: 0,
            in_ms: 0,
            out_ms: len,
            starts: Boundary::Edge,
            ends: Boundary::Edge,
            whole_start: true,
            whole_end: true,
            skipped: None,
        };
        let v = |len| length(&s, &span(len)).value;
        assert_eq!(v(35_000), 1.0);
        assert!((v(20_000) - 0.4).abs() < 1e-9 && (v(60_000) - 0.4).abs() < 1e-9);
        assert!(v(30_000) > v(25_000) && v(40_000) > v(50_000));
        assert_eq!(
            length(&s, &span(35_000)).reasons[0].text,
            "35 s, close to the 35 s target"
        );
        assert!(
            length(&s, &span(70_000))
                .reasons
                .iter()
                .any(|r| r.text == "Outside 20 s–60 s")
        );
    }

    #[test]
    fn other_languages_skip_the_hook() {
        let mut t = talk(&[PLAIN, PLAIN, PLAIN, PLAIN], 400);
        t.language = "de".into();
        let s = settings(1_000, 60_000, 20_000);
        let m = with_ctx(&t, &[], |ctx| candidates(ctx, &s).remove(0));
        assert_eq!(feature(&m, FeatureKind::Hook).weight, 0);
        let total: u32 = m.features.iter().map(|f| f.weight).sum();
        assert!((98..=100).contains(&total), "{total}");
        assert!(reasons(&m).contains(&"Hook not judged for this language".to_string()));
    }

    #[test]
    fn unpunctuated_text_still_gives_clips_at_clean_pauses() {
        let (t, truth) = realistic(1, true);
        let s = FinderSettings::default();
        with_ctx(&t, &[], |ctx| {
            let picked = find(ctx, &s, &[], 0);
            assert!(!picked.is_empty());
            // Picked clips prefer clean ends: most end at a real sentence end.
            let words = &ctx.text.words;
            let real_ends = picked
                .iter()
                .filter(|m| {
                    let last = words.iter().rposition(|w| w.end_ms <= m.out_ms).unwrap();
                    truth[last]
                })
                .count();
            assert!(
                real_ends * 4 >= picked.len() * 3,
                "{real_ends} of {}",
                picked.len()
            );
        });
    }

    #[test]
    fn two_hours_are_quick() {
        // ~0.4 s on the dev Mac in a test build (opt-level 1); the limit leaves room for
        // slow CI runners. Release builds are faster still.
        // The script (~7.5 minutes here) over and over, for two hours.
        let (one, _) = realistic(5, false);
        let span = one.segments.last().unwrap().end_ms + 2_000;
        let mut t = one.clone();
        t.segments.clear();
        let copies = 7_200_000 / span + 1;
        for k in 0..copies {
            for seg in &one.segments {
                let mut seg = seg.clone();
                for w in &mut seg.words {
                    w.start_ms += k * span;
                    w.end_ms += k * span;
                }
                seg.start_ms += k * span;
                seg.end_ms += k * span;
                t.segments.push(seg);
            }
        }
        let duration = copies * span;
        let started = std::time::Instant::now();
        let found = find_in(&t, &[], duration, &FinderSettings::default(), &[], 0);
        let took = started.elapsed();
        assert_eq!(
            found.len(),
            AUTO_MAX.min(wanted(&FinderSettings::default(), duration))
        );
        assert!(took.as_millis() < 3_000, "{took:?}");
    }

    /// Prints the moments of a real transcript (not in the repo):
    /// `KREOKIT_TRANSCRIPT=<…/transcript/transcript.json> cargo test real_transcript_moments
    /// -- --ignored --nocapture`; `loudness.json` next to it is used when present. Pass
    /// `KREOKIT_STRIP=1` to strip the punctuation first.
    #[test]
    #[ignore]
    fn real_transcript_moments() {
        let path = std::path::PathBuf::from(std::env::var("KREOKIT_TRANSCRIPT").unwrap());
        let mut t: Transcript = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if std::env::var("KREOKIT_STRIP").is_ok() {
            for w in t.segments.iter_mut().flat_map(|s| s.words.iter_mut()) {
                w.text = w.text.replace(['.', '?', '!', ','], "").to_lowercase();
            }
        }
        let loudness: Vec<f32> = std::fs::read(path.with_file_name("loudness.json"))
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap_or_default();
        let end = t.segments.last().map_or(0, |s| s.end_ms);
        let found = find_in(&t, &loudness, end, &FinderSettings::default(), &[], 0);
        for m in found {
            eprintln!(
                "\n{} {}–{} ({} s)",
                m.score,
                m.in_ms / 1000,
                m.out_ms / 1000,
                (m.out_ms - m.in_ms) / 1000
            );
            for f in &m.features {
                let r: Vec<String> = f
                    .reasons
                    .iter()
                    .map(|r| format!("{}{}", if r.good { "+" } else { "-" }, r.text))
                    .collect();
                eprintln!(
                    "  {:?} {:.2}×{}: {}",
                    f.kind,
                    f.value,
                    f.weight,
                    r.join("; ")
                );
            }
            eprintln!("  {}", m.text);
        }
    }
}
