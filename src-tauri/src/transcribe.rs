//! Speech-to-text with whisper.cpp (via whisper-rs), producing segments with word timings.
//! Ported from KreoKit Captions (word timings, edge refinement, sound-tag filter, custom
//! words). Clips loads the model once into an [`Engine`] and transcribes a long video
//! piece by piece (see `transcription.rs`); times here are relative to the piece.
//!
//! Runs on the CPU. ggml's Metal backend needs simdgroup support (Apple7 GPU family or
//! Metal 3) for its fast paths, which Intel Macs on macOS 12 lack, so Metal would mostly fall
//! back to the CPU anyway.
//!
//! TODO(metal): CPU-only on every platform, as in Captions. Enable whisper-rs's `metal`
//! feature (and `use_gpu`) for aarch64-apple-darwin once it can be tested on an Apple
//! Silicon Mac.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters,
};

use crate::models::{CANCELLED, WhisperModel};
use crate::vad;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Detected (or requested) language code, e.g. "en".
    pub language: String,
    pub segments: Vec<Segment>,
}

/// What to transcribe and how; see the fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// A Whisper language code, or `None` to auto-detect.
    pub language: Option<&'a str>,
    /// Drop sound tags like "[MUSIC]" (see [`strip_non_speech`]).
    pub hide_non_speech: bool,
    /// Initial prompt for Whisper, e.g. from [`prompt_from_custom_words`].
    pub prompt: Option<&'a str>,
}

/// A loaded Whisper model, reused for every piece of a video.
pub struct Engine {
    ctx: WhisperContext,
}

impl Engine {
    pub fn load(model: WhisperModel, model_path: &Path) -> Result<Self, String> {
        // Routes whisper.cpp's console logging into the (silent) `log` facade.
        whisper_rs::install_logging_hooks();

        let mut ctx_params = WhisperContextParameters::default();
        ctx_params.use_gpu(false);
        ctx_params.dtw_parameters(DtwParameters {
            mode: DtwMode::ModelPreset {
                model_preset: dtw_preset(model),
            },
            ..DtwParameters::default()
        });
        let ctx = WhisperContext::new_with_params(model_path, ctx_params)
            .map_err(|e| format!("cannot load model {}: {e}", model_path.display()))?;
        Ok(Self { ctx })
    }

    /// Transcribes `samples` (16 kHz mono); times are relative to their start.
    /// `on_progress` receives 0–100. Returns [`CANCELLED`] if `cancel` is set.
    pub fn transcribe(
        &self,
        samples: &[f32],
        options: Options,
        cancel: &Arc<AtomicBool>,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<Transcript, String> {
        let ctx = &self.ctx;
        let mut state = ctx.create_state().map_err(|e| e.to_string())?;

        // Same as whisper.cpp's default greedy params; beam search is ~3x slower on CPU.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_n_threads(thread_count());
        params.set_language(Some(options.language.unwrap_or("auto")));
        params.set_translate(false);
        params.set_token_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(prompt) = options.prompt {
            params.set_initial_prompt(prompt);
        }
        params.set_progress_callback_safe(on_progress);
        let abort = cancel.clone();
        // whisper-rs 0.16 calls the closure as `F` but stores it as `Box<dyn FnMut>`, so `F`
        // must itself be that box type for the call to be sound.
        params.set_abort_callback_safe::<_, Box<dyn FnMut() -> bool>>(Box::new(move || {
            abort.load(Ordering::Relaxed)
        })
            as Box<dyn FnMut() -> bool>);

        let result = state.full(params, samples);
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        result.map_err(|e| format!("transcription failed: {e}"))?;

        let eot = ctx.token_eot();
        let mut segments = Vec::new();
        let mut prev_end_ms = 0;
        for seg in state.as_iter() {
            let mut text = seg
                .to_bytes()
                .map(|b| String::from_utf8_lossy(b).trim().to_string())
                .unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            let tokens: Vec<RawToken> = (0..seg.n_tokens())
                .filter_map(|t| seg.get_token(t))
                .filter(|tok| tok.token_id() < eot) // drop special and timestamp tokens
                .map(|tok| {
                    let data = tok.token_data();
                    RawToken {
                        bytes: tok.to_bytes().unwrap_or_default().to_vec(),
                        start_ms: centis_to_ms(data.t0),
                        end_ms: centis_to_ms(data.t1),
                        dtw_ms: (data.t_dtw >= 0).then(|| centis_to_ms(data.t_dtw)),
                    }
                })
                .collect();
            let mut words = align_words(group_tokens(&tokens), &mut prev_end_ms);
            if options.hide_non_speech && !words.is_empty() {
                let kept = strip_non_speech(words.clone());
                if kept.is_empty() {
                    continue;
                }
                if kept != words {
                    text = kept
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    words = kept;
                }
            }
            let (start_ms, end_ms) = match (words.first(), words.last()) {
                (Some(first), Some(last)) => (first.start_ms, last.end_ms),
                _ => (
                    centis_to_ms(seg.start_timestamp()),
                    centis_to_ms(seg.end_timestamp()),
                ),
            };
            segments.push(Segment {
                id: segments.len() as u32,
                start_ms,
                end_ms,
                text,
                words,
            });
        }

        refine_with_audio(&mut segments, samples);

        let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
            .map(str::to_string)
            .or_else(|| options.language.map(str::to_string))
            .unwrap_or_else(|| "auto".into());
        Ok(Transcript { language, segments })
    }
}

/// Turns the user's "custom words" (names, brands, jargon; separated by commas or new
/// lines) into a Whisper initial prompt. Whisper continues the prompt's style and
/// spellings, so listing the words nudges it towards them. Returns None if there are none.
pub fn prompt_from_custom_words(text: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let words: Vec<String> = text
        .split([',', '\n', ';'])
        .map(|w| w.replace('\0', "").trim().to_string())
        .filter(|w| !w.is_empty() && seen.insert(w.to_lowercase()))
        .collect();
    (!words.is_empty()).then(|| words.join(", ") + ".")
}

fn thread_count() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4)
}

/// Whisper timestamps are in 10 ms units.
fn centis_to_ms(t: i64) -> u64 {
    (t.max(0) as u64) * 10
}

#[derive(Debug, Clone)]
struct RawToken {
    bytes: Vec<u8>,
    start_ms: u64,
    end_ms: u64,
    /// DTW-aligned time of the token, when available.
    dtw_ms: Option<u64>,
}

/// Attention heads used for DTW alignment differ per model.
fn dtw_preset(model: WhisperModel) -> DtwModelPreset {
    match model {
        WhisperModel::Base => DtwModelPreset::Base,
        WhisperModel::Small => DtwModelPreset::Small,
        WhisperModel::LargeV3Turbo => DtwModelPreset::LargeV3Turbo,
    }
}

/// A word with whisper's classic timestamps and the DTW time of its last token.
#[derive(Debug, Clone, PartialEq)]
struct WordDraft {
    text: String,
    start_ms: u64,
    end_ms: u64,
    dtw_end_ms: Option<u64>,
}

/// Groups subword tokens into words. A token starting with a space begins a new word;
/// others (word pieces, punctuation) attach to the current one. Bytes are joined before
/// decoding because a token can end in the middle of a multi-byte character (ë, Cyrillic).
fn group_tokens(tokens: &[RawToken]) -> Vec<WordDraft> {
    let mut groups: Vec<(Vec<u8>, u64, u64, Option<u64>)> = Vec::new();
    for tok in tokens {
        match groups.last_mut() {
            Some((bytes, _, end, dtw)) if !tok.bytes.starts_with(b" ") => {
                bytes.extend_from_slice(&tok.bytes);
                *end = tok.end_ms;
                *dtw = tok.dtw_ms.or(*dtw);
            }
            _ => groups.push((tok.bytes.clone(), tok.start_ms, tok.end_ms, tok.dtw_ms)),
        }
    }
    groups
        .into_iter()
        .filter_map(|(bytes, start_ms, end_ms, dtw_end_ms)| {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            (!text.is_empty()).then_some(WordDraft {
                text,
                start_ms,
                end_ms,
                dtw_end_ms,
            })
        })
        .collect()
}

/// Shortest duration we give a word, so word highlights never flash.
const MIN_WORD_MS: u64 = 100;

/// Word timings from DTW. Whisper's classic token timestamps drift (on our fixture they
/// were up to 1 s early), while the DTW time of a word's last token tracks where the word
/// ends to within ~0.3 s. So: end = DTW time; start = the end minus the word's classic
/// duration, but not before the previous word's end.
///
/// Consecutive words whose DTW end doesn't advance (e.g. "the caption" both at 2060 ms)
/// share one span, split in proportion to their classic durations. Every word gets at least
/// [`MIN_WORD_MS`]. Without DTW the classic times are kept. Times never go backwards;
/// `prev_end_ms` carries across segments.
fn align_words(drafts: Vec<WordDraft>, prev_end_ms: &mut u64) -> Vec<Word> {
    let mut words = Vec::with_capacity(drafts.len());
    let mut i = 0;
    while i < drafts.len() {
        let mut j = i + 1;
        if let Some(run_end) = drafts[i].dtw_end_ms {
            while j < drafts.len() && drafts[j].dtw_end_ms.is_some_and(|e| e <= run_end) {
                j += 1;
            }
        }
        let run = &drafts[i..j];
        let weights: Vec<u64> = run
            .iter()
            .map(|d| d.end_ms.saturating_sub(d.start_ms).max(MIN_WORD_MS))
            .collect();
        let total: u64 = weights.iter().sum();

        let (start, end) = match run[0].dtw_end_ms {
            Some(dtw_end) => {
                let end = dtw_end.max(*prev_end_ms);
                (end.saturating_sub(total).max(*prev_end_ms), end)
            }
            None => {
                let start = run[0].start_ms.max(*prev_end_ms);
                (start, run[0].end_ms.max(start))
            }
        };
        let end = end.max(start + MIN_WORD_MS * run.len() as u64);

        // Split [start, end] by weight.
        let mut t = start;
        let mut acc = 0;
        for (d, w) in run.iter().zip(&weights) {
            acc += w;
            let word_end = start + (end - start) * acc / total;
            words.push(Word {
                start_ms: t,
                end_ms: word_end,
                text: d.text.clone(),
            });
            t = word_end;
        }
        *prev_end_ms = end;
        i = j;
    }
    words
}

/// Snaps word edges to pauses in `samples` and updates segment bounds to match.
fn refine_with_audio(segments: &mut [Segment], samples: &[f32]) {
    let voiced = vad::voiced_frames(samples);
    let mut words: Vec<Word> = segments.iter().flat_map(|s| s.words.clone()).collect();
    snap_to_speech(&mut words, &vad::onsets(&voiced), &vad::offsets(&voiced));

    let mut words = words.into_iter();
    for seg in segments {
        seg.words = words.by_ref().take(seg.words.len()).collect();
        if let (Some(first), Some(last)) = (seg.words.first(), seg.words.last()) {
            seg.start_ms = first.start_ms;
            seg.end_ms = last.end_ms;
        }
    }
}

/// How far a word edge may move to meet a pause in the audio.
const SNAP_WINDOW_MS: u64 = 400;

/// Moves word edges next to pauses onto the audio's actual silence↔speech transitions:
/// each speech onset becomes the start of the word starting nearest to it, each offset
/// the end of the word ending nearest to it (within [`SNAP_WINDOW_MS`]). With continuous
/// sound (music) there are no transitions and nothing moves. Words stay ordered and at
/// least [`MIN_WORD_MS`] long where there is room.
fn snap_to_speech(words: &mut [Word], onsets: &[u64], offsets: &[u64]) {
    fn nearest(times: impl Iterator<Item = u64>, target: u64) -> Option<(usize, u64)> {
        times
            .enumerate()
            .map(|(i, t)| (i, t.abs_diff(target)))
            .filter(|&(_, d)| d <= SNAP_WINDOW_MS)
            .min_by_key(|&(_, d)| d)
    }
    let starts: Vec<u64> = words.iter().map(|w| w.start_ms).collect();
    let ends: Vec<u64> = words.iter().map(|w| w.end_ms).collect();

    // Each transition goes to the nearest word edge, and only if that edge has no closer
    // transition of its own.
    for &onset in onsets {
        if let Some((i, d)) = nearest(starts.iter().copied(), onset)
            && nearest(onsets.iter().copied(), starts[i]).is_some_and(|(_, best)| best == d)
        {
            words[i].start_ms = onset;
        }
    }
    for &offset in offsets {
        if let Some((i, d)) = nearest(ends.iter().copied(), offset)
            && nearest(offsets.iter().copied(), ends[i]).is_some_and(|(_, best)| best == d)
        {
            words[i].end_ms = offset;
        }
    }

    // Repair order: a moved start may now overlap the previous word, or a moved end the
    // next one. Earlier edges win; later ones are pushed forward.
    for i in 0..words.len() {
        if i > 0 && words[i].start_ms < words[i - 1].end_ms {
            let prev = &words[i - 1];
            let room = words[i].start_ms.max(prev.start_ms + MIN_WORD_MS);
            if room < prev.end_ms {
                words[i - 1].end_ms = room;
            }
            words[i].start_ms = words[i].start_ms.max(words[i - 1].end_ms);
        }
        let min_end = words[i].start_ms + MIN_WORD_MS;
        let next_start = words.get(i + 1).map_or(u64::MAX, |w| w.start_ms);
        if words[i].end_ms < min_end {
            words[i].end_ms = min_end.min(next_start.max(words[i].start_ms + 1));
        }
    }
}

/// Music symbols Whisper uses around sung lyrics.
const MUSIC_NOTES: &[char] = &['♪', '♫', '♬', '♩'];

/// Longest sound tag we look for, in words ("[upbeat music playing softly]").
const MAX_TAG_WORDS: usize = 6;

/// Removes the non-speech tags Whisper writes for sounds, e.g. "[MUSIC]", "[Musik]",
/// "[BLANK_AUDIO]", "(applause)", "*laughs*", and bare "♪" symbols. Lyrics between ♪ marks
/// are kept. An opening bracket without a nearby closing one is kept as ordinary text.
fn strip_non_speech(words: Vec<Word>) -> Vec<Word> {
    let mut kept = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let text = words[i].text.as_str();
        let closer = match text.chars().next() {
            Some('[') => Some(']'),
            Some('(') => Some(')'),
            Some('*') => Some('*'),
            _ => None,
        };
        if let Some(close) = closer {
            let closes = |w: &Word, skip_first_char: bool| {
                let t = w
                    .text
                    .trim_end_matches(|c: char| c.is_ascii_punctuation() && c != close);
                let t = if skip_first_char {
                    t.get(1..).unwrap_or("")
                } else {
                    t
                };
                t.ends_with(close)
            };
            let end = (i..words.len().min(i + MAX_TAG_WORDS)).find(|&j| closes(&words[j], j == i));
            if let Some(end) = end {
                i = end + 1;
                continue;
            }
        }

        let trimmed = text.trim_matches(|c| MUSIC_NOTES.contains(&c)).trim();
        if !trimmed.is_empty() && trimmed.chars().any(char::is_alphanumeric) {
            let mut word = words[i].clone();
            if trimmed.len() != text.len() {
                word.text = trimmed.to_string();
            }
            kept.push(word);
        }
        i += 1;
    }
    kept
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The base model, from Clips' models folder or Captions' (`models::tests::dev_dirs`).
    pub(crate) fn base_engine() -> Engine {
        let dirs = crate::models::tests::dev_dirs();
        let path = crate::models::resolve(WhisperModel::Base, &dirs, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "base model missing: put ggml-base.bin into {} (or set KREOKIT_APP_DATA)",
                    dirs.ours.display()
                )
            });
        Engine::load(WhisperModel::Base, &path).unwrap()
    }

    fn tok(bytes: &[u8], start_ms: u64, end_ms: u64) -> RawToken {
        RawToken {
            bytes: bytes.to_vec(),
            start_ms,
            end_ms,
            dtw_ms: None,
        }
    }

    fn draft(text: &str, start_ms: u64, end_ms: u64, dtw_end_ms: Option<u64>) -> WordDraft {
        WordDraft {
            text: text.into(),
            start_ms,
            end_ms,
            dtw_end_ms,
        }
    }

    fn word(text: &str, start_ms: u64, end_ms: u64) -> Word {
        Word {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    #[test]
    fn groups_subword_tokens_and_punctuation() {
        let mut tokens = [
            tok(b" Hel", 0, 200),
            tok(b"lo", 200, 400),
            tok(b",", 400, 420),
            tok(b" world", 500, 900),
            tok(b".", 900, 950),
        ];
        tokens[1].dtw_ms = Some(380);
        assert_eq!(
            group_tokens(&tokens),
            [
                draft("Hello,", 0, 420, Some(380)),
                draft("world.", 500, 950, None)
            ]
        );
    }

    #[test]
    fn joins_multibyte_characters_split_across_tokens() {
        // "Здраво" (Macedonian), with "З" (0xD0 0x97) split between two tokens.
        let tokens = [
            tok(b" \xD0", 0, 100),
            tok(b"\x97\xD0\xB4\xD1\x80\xD0\xB0\xD0\xB2\xD0\xBE", 100, 600),
        ];
        assert_eq!(group_tokens(&tokens), [draft("Здраво", 0, 600, None)]);
    }

    #[test]
    fn first_token_without_space_starts_a_word() {
        let tokens = [tok(b"Faleminderit", 0, 800), tok(b"!", 800, 850)];
        assert_eq!(
            group_tokens(&tokens),
            [draft("Faleminderit!", 0, 850, None)]
        );
    }

    #[test]
    fn skips_blank_tokens() {
        assert_eq!(group_tokens(&[tok(b" ", 0, 10)]), []);
    }

    #[test]
    fn aligns_words_to_dtw_ends() {
        // Times from the English fixture: classic timestamps start at 0, speech at 1040 ms.
        let drafts = vec![
            draft("Welcome", 0, 550, Some(1340)),
            draft("to", 550, 710, Some(1560)),
            draft("the", 950, 950, Some(2060)),
        ];
        let mut prev_end = 0;
        assert_eq!(
            align_words(drafts, &mut prev_end),
            [
                word("Welcome", 790, 1340),
                word("to", 1400, 1560),
                word("the", 1960, 2060)
            ]
        );
        assert_eq!(prev_end, 2060);
    }

    #[test]
    fn word_after_a_pause_starts_after_the_gap() {
        // "every" follows a pause: its start comes from its own duration, not "dog,".
        let drafts = vec![draft("every", 5220, 5400, Some(5820))];
        let mut prev_end = 5340;
        assert_eq!(
            align_words(drafts, &mut prev_end),
            [word("every", 5640, 5820)]
        );
    }

    #[test]
    fn timings_never_go_backwards() {
        let drafts = vec![
            draft("a", 0, 300, Some(900)),
            draft("b", 300, 500, Some(700)), // DTW earlier: shares a's span
            draft("c", 400, 600, None),      // no DTW, classic time too early: pushed, min length
        ];
        let mut prev_end = 0;
        assert_eq!(
            align_words(drafts, &mut prev_end),
            [
                word("a", 400, 700),
                word("b", 700, 900),
                word("c", 900, 1000)
            ]
        );
    }

    #[test]
    fn words_sharing_a_dtw_end_split_the_span() {
        // From the fixture: "the" and "caption" both had DTW end 2060 and "the" got 0 ms.
        let drafts = vec![
            draft("the", 950, 950, Some(2060)),
            draft("caption", 1050, 1500, Some(2060)),
        ];
        let mut prev_end = 1560;
        assert_eq!(
            align_words(drafts, &mut prev_end),
            [word("the", 1560, 1650), word("caption", 1650, 2060)]
        );
    }

    #[test]
    fn snaps_edges_next_to_pauses() {
        let mut ws = vec![
            word("Welcome", 790, 1340),
            word("test,", 2300, 2800),
            word("the", 2810, 3020),
            word("captions.", 7560, 7980),
        ];
        snap_to_speech(&mut ws, &[1041, 2898], &[2647, 8254]);
        assert_eq!(
            ws,
            [
                word("Welcome", 1041, 1340),
                word("test,", 2300, 2647),
                word("the", 2898, 3020),
                word("captions.", 7560, 8254)
            ]
        );
    }

    #[test]
    fn a_transition_moves_only_the_nearest_edge() {
        let mut ws = vec![word("a", 1000, 1200), word("b", 1200, 1400)];
        snap_to_speech(&mut ws, &[1150], &[]);
        assert_eq!(ws, [word("a", 1000, 1150), word("b", 1150, 1400)]);
    }

    #[test]
    fn far_or_missing_transitions_change_nothing() {
        let original = vec![word("a", 1000, 1200), word("b", 1200, 1400)];
        let mut ws = original.clone();
        snap_to_speech(&mut ws, &[], &[]);
        assert_eq!(ws, original);
        snap_to_speech(&mut ws, &[2000], &[100]);
        assert_eq!(ws, original);
    }

    #[test]
    fn snapping_keeps_words_ordered_and_long_enough() {
        // An offset pulls "a" to end before it could be 100 ms long; it keeps the minimum.
        let mut ws = vec![word("a", 1000, 1300), word("b", 1300, 1600)];
        snap_to_speech(&mut ws, &[], &[1030]);
        assert_eq!(ws, [word("a", 1000, 1100), word("b", 1300, 1600)]);
    }

    fn words(texts: &[&str]) -> Vec<Word> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| word(t, i as u64 * 100, i as u64 * 100 + 90))
            .collect()
    }

    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn strips_sound_tags() {
        for tag in [
            "[MUSIC]",
            "[Musik]",
            "[BLANK_AUDIO]",
            "(applause)",
            "*laughs*",
            "♪",
            "[MUSIC].",
        ] {
            assert_eq!(strip_non_speech(words(&[tag])), [], "{tag}");
        }
    }

    #[test]
    fn strips_multi_word_tags_and_keeps_speech_around_them() {
        let kept = strip_non_speech(words(&[
            "Hi", "[Music", "playing]", "there", "(laughs)", "friends.",
        ]));
        assert_eq!(texts(&kept), ["Hi", "there", "friends."]);
        // Timings of the kept words are untouched.
        assert_eq!(kept[1], word("there", 300, 390));
    }

    #[test]
    fn keeps_lyrics_between_music_notes() {
        let kept = strip_non_speech(words(&["♪", "Let", "it", "be", "♪"]));
        assert_eq!(texts(&kept), ["Let", "it", "be"]);
        assert_eq!(texts(&strip_non_speech(words(&["♪Hello♪"]))), ["Hello"]);
    }

    #[test]
    fn strips_tags_split_into_separate_bracket_words() {
        let kept = strip_non_speech(words(&["[", "MUSIC", "]", "Hello", "("]));
        assert_eq!(texts(&kept), ["Hello"]);
    }

    #[test]
    fn keeps_unclosed_brackets_and_normal_words() {
        let input = words(&["Use", "(or", "skip", "it", "entirely", "if", "you", "want"]);
        assert_eq!(strip_non_speech(input.clone()), input);
    }

    #[test]
    fn builds_a_prompt_from_custom_words() {
        assert_eq!(
            prompt_from_custom_words(" KreoKit, Zylvana\nMorvelle; kreokit ,, "),
            Some("KreoKit, Zylvana, Morvelle.".into())
        );
        assert_eq!(prompt_from_custom_words(" , \n "), None);
        assert_eq!(prompt_from_custom_words("a\0b"), Some("ab.".into()));
    }

    /// Transcribes fixtures/en/brands.mp4 (made-up names) with and without the names as
    /// custom words. Needs ffmpeg and the base model:
    /// `cargo test custom_words_fix_names -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn custom_words_fix_names() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en");
        let reference = std::fs::read_to_string(fixtures.join("brands.txt")).unwrap();
        let engine = base_engine();
        let samples = crate::audio::tests::read_all(&fixtures.join("brands.mp4"));
        let run = |prompt: Option<&str>| {
            let t = engine
                .transcribe(
                &samples,
                Options {
                    language: Some("en"),
                    hide_non_speech: true,
                    prompt,
                },
                &Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .unwrap();
            t.segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };

        let names = ["KreoKit", "Zylvana", "Morvelle", "Quillhaven"];
        let prompt = prompt_from_custom_words(&names.join(", ")).unwrap();
        let without = run(None);
        let with = run(Some(&prompt));
        let wer = |h: &str| crate::wer::word_error_rate(&reference, h);
        println!("reference: {}", reference.trim());
        println!("without:   {without}  (WER {:.3})", wer(&without));
        println!("with:      {with}  (WER {:.3})", wer(&with));

        // Base model, synthetic voice: "Today Zylvana" runs together and still comes out
        // as "today's Ilevana", so require most names, not all, plus a clear WER gain.
        let fixed = names.iter().filter(|n| with.contains(*n)).count();
        assert!(
            fixed >= 3,
            "only {fixed} of {} names right: {with}",
            names.len()
        );
        assert!(
            wer(&with) < wer(&without) / 2.0,
            "custom words should clearly help"
        );
    }

    #[test]
    fn converts_centiseconds() {
        assert_eq!(centis_to_ms(123), 1_230);
        assert_eq!(centis_to_ms(-1), 0);
    }

    /// Transcribes fixtures/en/hello.mp4 with the base model and checks the word error
    /// rate against the reference transcript. Needs ffmpeg and the base model
    /// ([`base_engine`]): `cargo test transcribes_english_fixture -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn transcribes_english_fixture() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en");
        let reference = std::fs::read_to_string(fixtures.join("hello.txt")).unwrap();
        let engine = base_engine();

        let samples = crate::audio::tests::read_all(&fixtures.join("hello.mp4"));
        let started = std::time::Instant::now();
        let transcript = engine.transcribe(
            &samples,
            Options {
                hide_non_speech: true,
                ..Options::default()
            },
            &Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let elapsed = started.elapsed();

        let hypothesis: Vec<&str> = transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        let hypothesis = hypothesis.join(" ");
        let wer = crate::wer::word_error_rate(&reference, &hypothesis);
        println!("language:   {}", transcript.language);
        println!("reference:  {}", reference.trim());
        println!("hypothesis: {hypothesis}");
        println!(
            "WER (base): {wer:.3}, took {elapsed:.1?} for {:.1}s of audio",
            samples.len() as f64 / crate::audio::SAMPLE_RATE as f64
        );

        for w in transcript.segments.iter().flat_map(|s| &s.words) {
            println!("  {:>5}–{:>5} ms  {}", w.start_ms, w.end_ms, w.text);
        }

        assert_eq!(transcript.language, "en");
        assert!(wer < 0.3, "WER {wer:.3} is too high");

        // Speech intervals of hello.mp4 measured with ffmpeg's silencedetect (see
        // scripts/make-fixtures.sh); captions should start and end within 0.15 s of them.
        let speech_ms = [(1_041, 2_647), (2_898, 5_347), (5_588, 8_254)];
        let words: Vec<&Word> = transcript.segments.iter().flat_map(|s| &s.words).collect();
        let sentence_ranges = [("welcome", "test"), ("the", "dog"), ("every", "captions")];
        let mut from = 0;
        for ((first, last), (speech_start, speech_end)) in
            sentence_ranges.into_iter().zip(speech_ms)
        {
            let starts = |w: &&Word, prefix: &str| w.text.to_lowercase().starts_with(prefix);
            let i = from + words[from..].iter().position(|w| starts(w, first)).unwrap();
            let j = i + words[i..].iter().position(|w| starts(w, last)).unwrap();
            from = j + 1;
            let (start, end) = (words[i].start_ms, words[j].end_ms);
            println!(
                "sentence {first}…{last}: {start}–{end} ms, speech {speech_start}–{speech_end} ms"
            );
            assert!(
                start.abs_diff(speech_start) <= 150,
                "{first} starts at {start} ms, speech at {speech_start} ms"
            );
            assert!(
                end.abs_diff(speech_end) <= 150,
                "{last} ends at {end} ms, speech at {speech_end} ms"
            );
        }

        for w in &words {
            assert!(
                w.end_ms - w.start_ms >= 80,
                "{w:?} is too short to highlight"
            );
        }

        // Word timings are ordered and lie within their segment.
        for seg in &transcript.segments {
            assert!(!seg.words.is_empty(), "segment without words: {seg:?}");
            for w in &seg.words {
                assert!(w.start_ms <= w.end_ms, "{w:?}");
                assert!(
                    w.start_ms >= seg.start_ms && w.end_ms <= seg.end_ms,
                    "{w:?} outside {seg:?}"
                );
            }
        }
    }
}
