//! Transcribing a whole video, however long: audio streamed from ffmpeg, cut into ~5-minute
//! parts (`chunks.rs`), each part sent to Whisper, finished parts cached, and the parts
//! merged into one transcript with times on the video's clock.
//!
//! **Plan.** The first run reads the whole audio once to cut it into parts and measure its
//! loudness (`plan.json`), so the number of parts is known; later runs seek straight to the
//! first unfinished part.
//!
//! **Cache** (`<project>/transcript/`): `meta.json` records what the transcript is made
//! from (settings, video size and time, part limits); any difference discards the folder.
//! Each finished part is saved as `part-NNN.json` the moment it is done, so a cancelled
//! (or crashed) run resumes after its last finished part. Inside a part, everything Whisper
//! has finished is saved as `part-NNN.partial.json` after each of its 30-second windows, and
//! a resume continues from there, so a cancel loses at most about half a minute. The finished transcript is
//! `transcript.json`, with `loudness.json` (one level per second) next to it; the part
//! files and the plan are removed then.
//!
//! **Continuity across parts.** The language is detected on the first part with sound and
//! then fixed, so it can't change halfway. Each part's prompt is the custom words plus the
//! end of the previous part's text, so sentences carry on across a cut; an unpunctuated
//! end is left out, so Whisper's lowercase mode doesn't spread to the next part.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::audio::{AudioStream, Loudness, ms_to_samples, samples_to_ms};
use crate::chunks::{self, Limits};
use crate::models::{CANCELLED, WhisperModel};
use crate::projects::write_atomic;
use crate::transcribe::{self, Checkpoint, Engine, Options, Segment, Transcript};

/// Bump when the cache's contents change meaning; older caches are discarded.
const CACHE_VERSION: u32 = 1;
const META_FILE: &str = "meta.json";
const TRANSCRIPT_FILE: &str = "transcript.json";
const LOUDNESS_FILE: &str = "loudness.json";
const PLAN_FILE: &str = "plan.json";

/// How many words of the previous part go into the next part's prompt.
const PROMPT_TAIL_WORDS: usize = 30;

/// The user's choices on the Transcribe screen (also stored in the project).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub model: WhisperModel,
    /// A Whisper language code, or `None` to detect it.
    pub language: Option<String>,
    /// Names, brands and jargon, separated by commas or new lines.
    pub custom_words: String,
    /// Drop sound tags like "[MUSIC]".
    pub hide_non_speech: bool,
}

impl Default for Settings {
    /// Base keeps development fast; releases default to Small (as in Captions).
    fn default() -> Self {
        Self {
            model: if cfg!(debug_assertions) {
                WhisperModel::Base
            } else {
                WhisperModel::Small
            },
            language: None,
            custom_words: String::new(),
            hide_non_speech: true,
        }
    }
}

/// The video being transcribed: its fingerprint (as in the project) and probed length.
#[derive(Debug, Clone, Copy)]
pub struct Video {
    pub size: u64,
    pub mtime_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Meta {
    version: u32,
    settings: Settings,
    video_size: u64,
    video_mtime_ms: u64,
    /// [min, target, max] part length; changing the limits moves the cuts.
    limits_ms: [u64; 3],
}

impl Meta {
    fn new(settings: &Settings, video: &Video, limits: &Limits) -> Self {
        Self {
            version: CACHE_VERSION,
            settings: settings.clone(),
            video_size: video.size,
            video_mtime_ms: video.mtime_ms,
            limits_ms: [limits.min_ms, limits.target_ms, limits.max_ms],
        }
    }
}

/// One finished part, with times on the video's clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Part {
    start_ms: u64,
    end_ms: u64,
    /// Detected language; `None` for a silent part (not sent to Whisper).
    language: Option<String>,
    segments: Vec<Segment>,
}

/// The finished start of a part, saved while Whisper works on the rest (after each of
/// its 30-second windows), so a cancel loses at most about that much.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Partial {
    start_ms: u64,
    end_ms: u64,
    /// The segments cover the part up to here; a resume continues from this time.
    done_ms: u64,
    language: Option<String>,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Reading the whole audio once to plan the parts (the first run only).
    ReadingAudio,
    LoadingModel,
    Transcribing,
    Finishing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub stage: Stage,
    /// How far through the video, 0–`duration_ms`.
    pub position_ms: u64,
    pub duration_ms: u64,
    /// The part being worked on (1-based) and how many there are (0 while reading).
    pub part: usize,
    pub parts: usize,
    /// Time left, once there is a measured speed.
    pub eta_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub transcript: Transcript,
    /// Loudness per second of audio (dBFS), for the moment finder.
    pub loudness: Vec<f32>,
}

/// Where the audio comes from: ffmpeg in the app, a sample buffer in tests.
pub trait AudioSource {
    /// Appends up to `max` samples to `out`; 0 means the audio ended.
    fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, String>;
}

impl AudioSource for AudioStream {
    fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, String> {
        AudioStream::read(self, out, max)
    }
}

/// Speech to text for one part: Whisper in the app, a fake in tests.
pub trait Transcriber {
    fn transcribe<'a>(
        &self,
        samples: &'a [f32],
        options: Options,
        cancel: &Arc<AtomicBool>,
        on_progress: Box<dyn FnMut(i32) + 'a>,
        on_checkpoint: Box<dyn FnMut(Checkpoint) + 'a>,
    ) -> Result<Transcript, String>;
}

impl Transcriber for Engine {
    fn transcribe<'a>(
        &self,
        samples: &'a [f32],
        options: Options,
        cancel: &Arc<AtomicBool>,
        on_progress: Box<dyn FnMut(i32) + 'a>,
        on_checkpoint: Box<dyn FnMut(Checkpoint) + 'a>,
    ) -> Result<Transcript, String> {
        Engine::transcribe(self, samples, options, cancel, on_progress, on_checkpoint)
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    write_atomic(path, &json)
}

fn part_file(dir: &Path, index: usize) -> std::path::PathBuf {
    dir.join(format!("part-{index:03}.json"))
}

fn partial_file(dir: &Path, index: usize) -> std::path::PathBuf {
    dir.join(format!("part-{index:03}.partial.json"))
}

fn cache_matches(dir: &Path, meta: &Meta) -> bool {
    read_json::<Meta>(&dir.join(META_FILE)).as_ref() == Some(meta)
}

/// The finished transcript for these settings and this video, if there is one.
pub fn cached(dir: &Path, settings: &Settings, video: &Video, limits: &Limits) -> Option<Output> {
    if !cache_matches(dir, &Meta::new(settings, video, limits)) {
        return None;
    }
    Some(Output {
        transcript: read_json(&dir.join(TRANSCRIPT_FILE))?,
        loudness: read_json(&dir.join(LOUDNESS_FILE))?,
    })
}

/// How far a cancelled run got: the end of its saved work in ms (finished parts, then the
/// checkpoint inside the next part), and how many parts there are once the audio has been
/// planned. (0, None) if nothing is cached for these settings.
pub fn saved_until(
    dir: &Path,
    settings: &Settings,
    video: &Video,
    limits: &Limits,
) -> (u64, Option<usize>) {
    if !cache_matches(dir, &Meta::new(settings, video, limits)) {
        return (0, None);
    }
    let Some(plan) = read_json::<Plan>(&dir.join(PLAN_FILE)) else {
        return (0, None);
    };
    let parts = finished(dir, &plan);
    let saved = match plan
        .parts
        .get(parts.len())
        .and_then(|b| partial(dir, parts.len(), b))
    {
        Some(p) => p.done_ms,
        None => parts.last().map_or(0, |p| p.end_ms),
    };
    (saved, Some(plan.parts.len()))
}

/// Estimated time left: the remaining audio at the speed measured so far.
fn eta_ms(remaining_audio_ms: u64, spent_ms: u64, done_audio_ms: u64) -> Option<u64> {
    // Too little measured to say (the first seconds include warming up).
    if done_audio_ms < 5_000 || spent_ms == 0 {
        return None;
    }
    Some((remaining_audio_ms as u128 * spent_ms as u128 / done_audio_ms as u128) as u64)
}

/// The prompt for the next part: custom words, then the end of the previous part's text,
/// unless that text [looks unpunctuated](looks_unpunctuated): Whisper copies the prompt's
/// style, so a lowercase run without full stops would carry over into the next part.
fn part_prompt(custom: Option<&str>, previous: &[Segment]) -> Option<String> {
    let words: Vec<&str> = previous
        .iter()
        .flat_map(|s| s.text.split_whitespace())
        .collect();
    let tail = &words[words.len().saturating_sub(PROMPT_TAIL_WORDS)..];
    let tail = if looks_unpunctuated(tail) {
        String::new()
    } else {
        tail.join(" ")
    };
    let prompt = [custom.unwrap_or(""), &tail]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (!prompt.is_empty()).then_some(prompt)
}

/// Fewer words than this are too few to judge the style of.
const MIN_STYLE_WORDS: usize = 12;

/// Whether these words read like Whisper's lowercase, unpunctuated mode: enough of them,
/// and no sentence end (".", "?", "!") or no capital letter other than "I".
fn looks_unpunctuated(words: &[&str]) -> bool {
    let ends = |w: &&str| {
        w.trim_end_matches(['"', '\'', ')', ']', '”', '’'])
            .ends_with(['.', '?', '!', '…'])
    };
    let capital = |w: &&str| {
        let bare = w.trim_start_matches(|c: char| !c.is_alphanumeric());
        bare.chars().next().is_some_and(char::is_uppercase)
            && bare != "I"
            && !bare.starts_with("I'")
            && !bare.starts_with("I’")
    };
    words.len() >= MIN_STYLE_WORDS && !(words.iter().any(ends) && words.iter().any(capital))
}

/// Moves a part's times onto the video's clock and keeps every time inside the part
/// (Whisper can overshoot the end of its audio a little).
fn place(segments: Vec<Segment>, start_ms: u64, end_ms: u64) -> Vec<Segment> {
    let clamp = |t: u64| (start_ms + t).min(end_ms);
    segments
        .into_iter()
        .map(|mut s| {
            s.start_ms = clamp(s.start_ms);
            s.end_ms = clamp(s.end_ms);
            for w in s.words.iter_mut().chain(&mut s.sounds) {
                w.start_ms = clamp(w.start_ms);
                w.end_ms = clamp(w.end_ms);
            }
            s
        })
        .collect()
}

/// Joins the parts: ids renumbered, times never going backwards.
fn merge(parts: Vec<Part>, settings: &Settings) -> Transcript {
    let language = settings
        .language
        .clone()
        .or_else(|| parts.iter().find_map(|p| p.language.clone()))
        .unwrap_or_else(|| "auto".into());
    let mut segments: Vec<Segment> = Vec::new();
    let mut last_ms = 0;
    for mut seg in parts.into_iter().flat_map(|p| p.segments) {
        for w in &mut seg.words {
            w.start_ms = w.start_ms.max(last_ms);
            w.end_ms = w.end_ms.max(w.start_ms);
            last_ms = w.end_ms;
        }
        seg.start_ms = seg.words.first().map_or(seg.start_ms, |w| w.start_ms);
        seg.end_ms = seg.words.last().map_or(seg.end_ms, |w| w.end_ms);
        seg.id = segments.len() as u32;
        segments.push(seg);
    }
    Transcript { language, segments }
}

/// Where each part starts and ends (in samples) and the loudness per second, worked out
/// once by reading the whole audio ([`make_plan`]) and cached as `plan.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Plan {
    parts: Vec<(u64, u64)>,
    loudness: Vec<f32>,
}

/// Reads the whole audio once: the cut plan (`chunks::next_cut`, so a video is always cut
/// the same way) and the loudness. Reports how far it got in ms.
fn make_plan(
    audio: &mut impl AudioSource,
    limits: &Limits,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> Result<Plan, String> {
    let mut parts = Vec::new();
    let mut loudness = Loudness::default();
    let mut buffer: Vec<f32> = Vec::new();
    let mut at_end = false;
    let mut offset = 0u64; // samples before buffer[0]
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let Some(cut) = chunks::next_cut(&buffer, at_end, limits) else {
            let before = buffer.len();
            if audio.read(&mut buffer, limits.max_samples() + 1 - before)? == 0 {
                at_end = true;
            }
            loudness.push(&buffer[before..]);
            on_progress(samples_to_ms(offset as usize + buffer.len()));
            continue;
        };
        if cut == 0 {
            break;
        }
        parts.push((offset, offset + cut as u64));
        buffer.drain(..cut);
        offset += cut as u64;
    }
    Ok(Plan {
        parts,
        loudness: loudness.finish(),
    })
}

/// Reads exactly `n` samples (fewer only if the audio ends).
fn read_exact(audio: &mut impl AudioSource, n: usize) -> Result<Vec<f32>, String> {
    let mut samples = Vec::with_capacity(n);
    while samples.len() < n {
        let want = n - samples.len();
        if audio.read(&mut samples, want)? == 0 {
            break;
        }
    }
    Ok(samples)
}

fn part_ms(&(start, end): &(u64, u64)) -> (u64, u64) {
    (samples_to_ms(start as usize), samples_to_ms(end as usize))
}

/// The finished parts at the start of the plan (a part file whose bounds don't match the
/// plan doesn't count).
fn finished(dir: &Path, plan: &Plan) -> Vec<Part> {
    plan.parts
        .iter()
        .enumerate()
        .map_while(|(i, bounds)| {
            let (start_ms, end_ms) = part_ms(bounds);
            read_json::<Part>(&part_file(dir, i))
                .filter(|p| p.start_ms == start_ms && p.end_ms == end_ms)
        })
        .collect()
}

/// The checkpoint inside part `index`, if it belongs to these bounds.
fn partial(dir: &Path, index: usize, bounds: &(u64, u64)) -> Option<Partial> {
    let (start_ms, end_ms) = part_ms(bounds);
    read_json::<Partial>(&partial_file(dir, index))
        .filter(|p| p.start_ms == start_ms && p.end_ms == end_ms && p.done_ms >= start_ms)
}

/// Transcribes the whole video into `dir`, resuming from finished parts and checkpoints. `open_audio`
/// starts the audio at a time in ms; `load` is called only if some part still needs
/// Whisper. Returns [`CANCELLED`] if `cancel` is set; finished work stays cached.
#[allow(clippy::too_many_arguments)]
pub fn run<T: Transcriber, A: AudioSource>(
    dir: &Path,
    settings: &Settings,
    video: &Video,
    limits: &Limits,
    mut open_audio: impl FnMut(u64) -> Result<A, String>,
    load: impl FnOnce() -> Result<T, String>,
    cancel: &Arc<AtomicBool>,
    on_progress: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<Output, String> {
    let meta = Meta::new(settings, video, limits);
    if !cache_matches(dir, &meta) {
        let _ = fs::remove_dir_all(dir);
        fs::create_dir_all(dir).map_err(|e| format!("cannot create the cache folder: {e}"))?;
        write_json(&dir.join(META_FILE), &meta)?;
    }
    let on_progress = Arc::new(on_progress);
    let duration_ms = video.duration_ms;

    let plan = match read_json::<Plan>(&dir.join(PLAN_FILE)) {
        Some(plan) => plan,
        None => {
            let plan = make_plan(&mut open_audio(0)?, limits, cancel, |position_ms| {
                on_progress(Progress {
                    stage: Stage::ReadingAudio,
                    position_ms: position_ms.min(duration_ms),
                    duration_ms,
                    part: 0,
                    parts: 0,
                    eta_ms: None,
                })
            })?;
            write_json(&dir.join(PLAN_FILE), &plan)?;
            plan
        }
    };
    let total = plan.parts.len();
    let custom = transcribe::prompt_from_custom_words(&settings.custom_words);

    let mut parts = finished(dir, &plan);
    let mut language = settings.language.clone();
    if language.is_none() {
        language = parts.iter().find_map(|p| p.language.clone());
    }
    let mut engine: Option<T> = None;
    let mut load = Some(load);
    // Measured speed, over the parts Whisper actually worked on.
    let mut spent_ms = 0u64;
    let mut worked_audio_ms = 0u64;

    if parts.len() < total {
        let first = parts.len();
        // A checkpoint inside the first unfinished part: continue from there.
        let mut resume = partial(dir, first, &plan.parts[first]);
        if language.is_none() {
            language = resume.as_ref().and_then(|p| p.language.clone());
        }
        let first_ms = resume
            .as_ref()
            .map_or(part_ms(&plan.parts[first]).0, |p| p.done_ms);
        let mut audio = open_audio(first_ms)?;
        for index in first..total {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            let (start_ms, end_ms) = part_ms(&plan.parts[index]);
            // Earlier work in this part (from a checkpoint) and where the rest starts.
            let (from_ms, prior, prior_language) = match resume.take() {
                Some(p) => (p.done_ms.min(end_ms), p.segments, p.language),
                None => (start_ms, Vec::new(), None),
            };
            let end = plan.parts[index].1 as usize;
            let samples = read_exact(&mut audio, end.saturating_sub(ms_to_samples(from_ms)))?;
            let mut part = Part {
                start_ms,
                end_ms,
                language: prior_language.clone(),
                segments: prior.clone(),
            };
            if chunks::has_sound(&samples) {
                if engine.is_none() {
                    on_progress(Progress {
                        stage: Stage::LoadingModel,
                        position_ms: start_ms,
                        duration_ms,
                        part: index + 1,
                        parts: total,
                        eta_ms: None,
                    });
                    engine = Some((load.take().expect("loaded once"))()?);
                }
                let previous = if prior.is_empty() {
                    parts.last().map_or(&[][..], |p| p.segments.as_slice())
                } else {
                    prior.as_slice()
                };
                let prompt = part_prompt(custom.as_deref(), previous);
                let options = Options {
                    language: language.as_deref(),
                    hide_non_speech: settings.hide_non_speech,
                    prompt: prompt.as_deref(),
                };

                let started = Instant::now();
                let report = {
                    let on_progress = on_progress.clone();
                    let (spent_before, worked_before) = (spent_ms, worked_audio_ms);
                    let len_ms = end_ms - from_ms;
                    move |percent: i32| {
                        let done_ms = len_ms * percent.clamp(0, 100) as u64 / 100;
                        let position_ms = (from_ms + done_ms).min(duration_ms);
                        on_progress(Progress {
                            stage: Stage::Transcribing,
                            position_ms,
                            duration_ms,
                            part: index + 1,
                            parts: total,
                            eta_ms: eta_ms(
                                duration_ms.saturating_sub(position_ms),
                                spent_before + started.elapsed().as_millis() as u64,
                                worked_before + done_ms,
                            ),
                        });
                    }
                };
                // Saving a checkpoint is best-effort: if it fails, a cancel just loses more.
                let checkpoint = |c: Checkpoint| {
                    let mut segments = prior.clone();
                    segments.extend(place(c.segments, from_ms, end_ms));
                    let _ = write_json(
                        &partial_file(dir, index),
                        &Partial {
                            start_ms,
                            end_ms,
                            done_ms: (from_ms + c.done_ms).min(end_ms),
                            language: c.language.or_else(|| prior_language.clone()),
                            segments,
                        },
                    );
                };
                report(0);
                let t = engine.as_ref().unwrap().transcribe(
                    &samples,
                    options,
                    cancel,
                    Box::new(report),
                    Box::new(checkpoint),
                )?;
                spent_ms += started.elapsed().as_millis() as u64;
                worked_audio_ms += end_ms - from_ms;
                part.language = Some(t.language);
                part.segments.extend(place(t.segments, from_ms, end_ms));
            }
            write_json(&part_file(dir, index), &part)?;
            let _ = fs::remove_file(partial_file(dir, index));
            if language.is_none() {
                language = part.language.clone();
            }
            parts.push(part);
        }
    }

    on_progress(Progress {
        stage: Stage::Finishing,
        position_ms: duration_ms,
        duration_ms,
        part: total.max(1),
        parts: total.max(1),
        eta_ms: Some(0),
    });
    let output = Output {
        transcript: merge(parts, settings),
        loudness: plan.loudness,
    };
    write_json(&dir.join(LOUDNESS_FILE), &output.loudness)?;
    write_json(&dir.join(TRANSCRIPT_FILE), &output.transcript)?;
    for i in 0..total {
        let _ = fs::remove_file(part_file(dir, i));
        let _ = fs::remove_file(partial_file(dir, i));
    }
    let _ = fs::remove_file(dir.join(PLAN_FILE));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{SAMPLE_RATE, ms_to_samples};
    use crate::transcribe::Word;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Parts between 4.5 and 5.5 s, else 5 s, so test audio stays short.
    const SMALL: Limits = Limits {
        min_ms: 4_500,
        target_ms: 5_000,
        max_ms: 5_500,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-clips-transcription-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// (milliseconds, amplitude) pieces of a 440 Hz tone; amplitude 0 is silence.
    fn tone(parts: &[(u64, f32)]) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(ms, amp)| {
                (0..ms_to_samples(ms)).map(move |i| {
                    amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32).sin()
                })
            })
            .collect()
    }

    /// 13 s of "speech": 1.1 s sounds with 0.2 s pauses (three parts with SMALL).
    fn speech() -> Vec<f32> {
        let mut parts = vec![(700, 0.0)];
        for _ in 0..10 {
            parts.push((1_100, 0.5));
            parts.push((200, 0.0));
        }
        tone(&parts)
    }

    /// Opens `samples` at a time, like ffmpeg's seek, and remembers where it was opened.
    fn opener(
        samples: Vec<f32>,
        opened: Arc<Mutex<Vec<u64>>>,
    ) -> impl FnMut(u64) -> Result<FakeAudio, String> {
        move |ms| {
            opened.lock().unwrap().push(ms);
            Ok(FakeAudio::new(
                samples[ms_to_samples(ms).min(samples.len())..].to_vec(),
            ))
        }
    }

    /// Hands out its samples in uneven pieces.
    struct FakeAudio {
        samples: Vec<f32>,
        pos: usize,
    }

    impl FakeAudio {
        fn new(samples: Vec<f32>) -> Self {
            Self { samples, pos: 0 }
        }
    }

    impl AudioSource for FakeAudio {
        fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, String> {
            let n = max.min(7_777).min(self.samples.len() - self.pos);
            out.extend_from_slice(&self.samples[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Call {
        len_ms: u64,
        language: Option<String>,
        prompt: Option<String>,
    }

    /// Writes one segment per part, "part<N> lasts <ms>", covering the whole part (and a
    /// little past its end, like Whisper sometimes does). Detects "en". Cancels on call
    /// number `cancel_on` (1-based); on call `checkpoint_then_cancel`, first reports a
    /// checkpoint "part<N> first" covering the first half.
    #[derive(Default)]
    struct FakeWhisper {
        calls: RefCell<Vec<Call>>,
        cancel_on: Option<usize>,
        checkpoint_then_cancel: Option<usize>,
    }

    impl Transcriber for &FakeWhisper {
        fn transcribe<'a>(
            &self,
            samples: &'a [f32],
            options: Options,
            cancel: &Arc<AtomicBool>,
            mut on_progress: Box<dyn FnMut(i32) + 'a>,
            mut on_checkpoint: Box<dyn FnMut(Checkpoint) + 'a>,
        ) -> Result<Transcript, String> {
            let len_ms = samples_to_ms(samples.len());
            let mut calls = self.calls.borrow_mut();
            calls.push(Call {
                len_ms,
                language: options.language.map(str::to_string),
                prompt: options.prompt.map(str::to_string),
            });
            if self.cancel_on == Some(calls.len()) {
                cancel.store(true, Ordering::Relaxed);
                return Err(CANCELLED.into());
            }
            if self.checkpoint_then_cancel == Some(calls.len()) {
                let half = len_ms / 2;
                let text = format!("part{} first", calls.len());
                on_checkpoint(Checkpoint {
                    done_ms: half,
                    language: Some("en".into()),
                    segments: vec![Segment {
                        id: 0,
                        start_ms: 0,
                        end_ms: half,
                        text: text.clone(),
                        words: vec![Word {
                            start_ms: 0,
                            end_ms: half,
                            text,
                        }],
                        sounds: Vec::new(),
                    }],
                });
                cancel.store(true, Ordering::Relaxed);
                return Err(CANCELLED.into());
            }
            on_progress(50);
            on_progress(100);
            let n = calls.len();
            let words = vec![
                Word {
                    start_ms: 0,
                    end_ms: len_ms / 2,
                    text: format!("part{n}"),
                },
                Word {
                    start_ms: len_ms / 2,
                    end_ms: len_ms + 300,
                    text: format!("lasts {len_ms}"),
                },
            ];
            Ok(Transcript {
                language: options.language.unwrap_or("en").to_string(),
                segments: vec![Segment {
                    id: 0,
                    start_ms: 0,
                    end_ms: len_ms + 300,
                    text: format!("part{n} lasts {len_ms}"),
                    words,
                    sounds: Vec::new(),
                }],
            })
        }
    }

    fn settings() -> Settings {
        Settings {
            model: WhisperModel::Base,
            language: None,
            custom_words: String::new(),
            hide_non_speech: true,
        }
    }

    const VIDEO: Video = Video {
        size: 1000,
        mtime_ms: 42,
        duration_ms: 13_700,
    };

    fn go(
        dir: &Path,
        settings: &Settings,
        video: &Video,
        audio: Vec<f32>,
        whisper: &FakeWhisper,
    ) -> (Result<Output, String>, Vec<Progress>) {
        let progress = Arc::new(Mutex::new(Vec::new()));
        let log = progress.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = run(
            dir,
            settings,
            video,
            &SMALL,
            opener(audio, Arc::default()),
            || Ok(whisper),
            &cancel,
            move |p| log.lock().unwrap().push(p),
        );
        let progress = progress.lock().unwrap().clone();
        (result, progress)
    }

    #[test]
    fn transcribes_in_parts_and_merges_them() {
        let dir = temp_dir("merge");
        let whisper = FakeWhisper::default();
        let (out, progress) = go(&dir, &settings(), &VIDEO, speech(), &whisper);
        let out = out.unwrap();

        let lens: Vec<u64> = whisper.calls.borrow().iter().map(|c| c.len_ms).collect();
        assert_eq!(lens.len(), 3, "{lens:?}");
        assert_eq!(lens.iter().sum::<u64>(), 13_700);
        let t = &out.transcript;
        assert_eq!(t.language, "en");
        assert_eq!(
            t.segments.iter().map(|s| s.id).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        // On the video's clock, inside each part, never backwards.
        let mut part_start = 0;
        let mut last = 0;
        for (seg, len) in t.segments.iter().zip(&lens) {
            assert_eq!(seg.start_ms, part_start);
            assert_eq!(seg.end_ms, part_start + len, "clamped to the part's end");
            for w in &seg.words {
                assert!(w.start_ms >= last && w.end_ms >= w.start_ms, "{w:?}");
                last = w.end_ms;
            }
            part_start += len;
        }
        assert_eq!(out.loudness.len(), 14); // 13.7 s
        assert_eq!(cached(&dir, &settings(), &VIDEO, &SMALL), Some(out));
        let files: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|f| f.starts_with("part-"))
            .collect();
        assert!(files.is_empty(), "part files are removed: {files:?}");

        // Progress: reading the audio, loading once, parts 1..3 of exactly 3, finishing.
        assert_eq!(progress[0].stage, Stage::ReadingAudio);
        let transcribing: Vec<_> = progress
            .iter()
            .filter(|p| p.stage == Stage::Transcribing)
            .collect();
        assert!(transcribing.iter().all(|p| p.parts == 3));
        assert_eq!(transcribing.last().unwrap().part, 3);
        assert_eq!(
            progress
                .iter()
                .filter(|p| p.stage == Stage::LoadingModel)
                .count(),
            1
        );
        // Position moves forward within each stage (reading, then transcribing from 0).
        for stage in [Stage::ReadingAudio, Stage::Transcribing] {
            let positions: Vec<u64> = progress
                .iter()
                .filter(|p| p.stage == stage)
                .map(|p| p.position_ms)
                .collect();
            assert!(positions.windows(2).all(|w| w[0] <= w[1]), "{stage:?}");
        }
        let last = progress.last().unwrap();
        assert_eq!(
            (last.stage, last.position_ms, last.part, last.parts),
            (Stage::Finishing, 13_700, 3, 3)
        );
    }

    #[test]
    fn a_cancelled_run_resumes_after_its_finished_parts() {
        let dir = temp_dir("resume");
        let uninterrupted = {
            let other = temp_dir("resume-reference");
            go(
                &other,
                &settings(),
                &VIDEO,
                speech(),
                &FakeWhisper::default(),
            )
            .0
            .unwrap()
        };

        let stopping = FakeWhisper {
            cancel_on: Some(2),
            ..FakeWhisper::default()
        };
        let (result, _) = go(&dir, &settings(), &VIDEO, speech(), &stopping);
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert_eq!(
            saved_until(&dir, &settings(), &VIDEO, &SMALL),
            (4_500, Some(3))
        );
        assert_eq!(cached(&dir, &settings(), &VIDEO, &SMALL), None);

        // The fake numbers its parts by call, so count from 2 like an uninterrupted run.
        let resumed = FakeWhisper::default();
        resumed.calls.borrow_mut().push(Call {
            len_ms: 0,
            language: None,
            prompt: None,
        });
        let opened = Arc::new(Mutex::new(Vec::new()));
        let stages = Arc::new(Mutex::new(Vec::new()));
        let log = stages.clone();
        let result = run(
            &dir,
            &settings(),
            &VIDEO,
            &SMALL,
            opener(speech(), opened.clone()),
            || Ok(&resumed),
            &Arc::new(AtomicBool::new(false)),
            move |p| log.lock().unwrap().push(p.stage),
        );
        assert_eq!(
            resumed.calls.borrow().len(),
            3,
            "only parts 2 and 3 were transcribed"
        );
        assert_eq!(result.unwrap(), uninterrupted);
        // The plan was cached: no second read of the audio, which starts at part 2 (4.5 s).
        assert!(!stages.lock().unwrap().contains(&Stage::ReadingAudio));
        assert_eq!(*opened.lock().unwrap(), [4_500]);
    }

    #[test]
    fn a_resume_continues_from_the_checkpoint_inside_a_part() {
        let dir = temp_dir("checkpoint");
        let stopping = FakeWhisper {
            checkpoint_then_cancel: Some(2),
            ..FakeWhisper::default()
        };
        let (result, _) = go(&dir, &settings(), &VIDEO, speech(), &stopping);
        assert_eq!(result.unwrap_err(), CANCELLED);
        // Part 2 runs 4.5–9.7 s; its first half was saved.
        assert_eq!(
            saved_until(&dir, &settings(), &VIDEO, &SMALL),
            (7_100, Some(3))
        );

        let opened = Arc::new(Mutex::new(Vec::new()));
        let resumed = FakeWhisper::default();
        resumed.calls.borrow_mut().push(Call {
            len_ms: 0,
            language: None,
            prompt: None,
        });
        let out = run(
            &dir,
            &settings(),
            &VIDEO,
            &SMALL,
            opener(speech(), opened.clone()),
            || Ok(&resumed),
            &Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            *opened.lock().unwrap(),
            [7_100],
            "the audio starts at the checkpoint"
        );
        let calls = resumed.calls.borrow();
        assert_eq!(calls[1].len_ms, 2_600, "only the rest of part 2");
        assert_eq!(calls[1].prompt.as_deref(), Some("part2 first"));
        assert_eq!(calls[1].language.as_deref(), Some("en"));

        let texts: Vec<&str> = out
            .transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(
            &texts[..3],
            ["part1 lasts 4500", "part2 first", "part2 lasts 2600"]
        );
        let starts: Vec<u64> = out.transcript.segments.iter().map(|s| s.start_ms).collect();
        assert_eq!(&starts[..3], [0, 4_500, 7_100]);
        assert!(!dir.join("part-001.partial.json").exists());
    }

    #[test]
    fn changed_settings_or_video_start_over() {
        let dir = temp_dir("invalidate");
        let stopping = FakeWhisper {
            cancel_on: Some(2),
            ..FakeWhisper::default()
        };
        let _ = go(&dir, &settings(), &VIDEO, speech(), &stopping);
        assert_eq!(
            saved_until(&dir, &settings(), &VIDEO, &SMALL),
            (4_500, Some(3))
        );

        let small = Settings {
            model: WhisperModel::Small,
            ..settings()
        };
        assert_eq!(saved_until(&dir, &small, &VIDEO, &SMALL), (0, None));
        let moved = Video {
            mtime_ms: 43,
            ..VIDEO
        };
        assert_eq!(saved_until(&dir, &settings(), &moved, &SMALL), (0, None));

        let whisper = FakeWhisper::default();
        go(&dir, &small, &VIDEO, speech(), &whisper).0.unwrap();
        assert_eq!(whisper.calls.borrow().len(), 3);
        assert!(cached(&dir, &settings(), &VIDEO, &SMALL).is_none());
        assert!(cached(&dir, &small, &VIDEO, &SMALL).is_some());
    }

    #[test]
    fn the_language_is_detected_once_then_fixed() {
        let whisper = FakeWhisper::default();
        go(
            &temp_dir("lang-auto"),
            &settings(),
            &VIDEO,
            speech(),
            &whisper,
        )
        .0
        .unwrap();
        let langs: Vec<_> = whisper
            .calls
            .borrow()
            .iter()
            .map(|c| c.language.clone())
            .collect();
        assert_eq!(langs, [None, Some("en".into()), Some("en".into())]);

        let german = Settings {
            language: Some("de".into()),
            ..settings()
        };
        let whisper = FakeWhisper::default();
        let out = go(&temp_dir("lang-de"), &german, &VIDEO, speech(), &whisper)
            .0
            .unwrap();
        assert!(
            whisper
                .calls
                .borrow()
                .iter()
                .all(|c| c.language.as_deref() == Some("de"))
        );
        assert_eq!(out.transcript.language, "de");
    }

    #[test]
    fn prompts_carry_custom_words_and_the_previous_text() {
        let words = Settings {
            custom_words: "KreoKit, Zylvana".into(),
            ..settings()
        };
        let whisper = FakeWhisper::default();
        go(&temp_dir("prompt"), &words, &VIDEO, speech(), &whisper)
            .0
            .unwrap();
        let calls = whisper.calls.borrow();
        assert_eq!(calls[0].prompt.as_deref(), Some("KreoKit, Zylvana."));
        let first = format!("part1 lasts {}", calls[0].len_ms);
        assert_eq!(calls[1].prompt, Some(format!("KreoKit, Zylvana. {first}")));

        let previous = vec![Segment {
            id: 0,
            start_ms: 0,
            end_ms: 1,
            text: (1..=40)
                .map(|i| format!("W{i}."))
                .collect::<Vec<_>>()
                .join(" "),
            words: Vec::new(),
            sounds: Vec::new(),
        }];
        let tail = part_prompt(None, &previous).unwrap();
        assert_eq!(tail.split(' ').count(), PROMPT_TAIL_WORDS);
        assert!(tail.starts_with("W11. ") && tail.ends_with(" W40."));
        assert_eq!(part_prompt(None, &[]), None);
    }

    #[test]
    fn an_unpunctuated_tail_is_left_out_of_the_prompt() {
        let previous = |text: &str| {
            vec![Segment {
                id: 0,
                start_ms: 0,
                end_ms: 1,
                text: text.into(),
                words: Vec::new(),
                sounds: Vec::new(),
            }]
        };
        let lowercase = "and then we went back to the studio and i said you know we \
                         should try it again with the other microphone";
        assert_eq!(part_prompt(None, &previous(lowercase)), None);
        assert_eq!(
            part_prompt(Some("KreoKit."), &previous(lowercase)).as_deref(),
            Some("KreoKit."),
            "custom words are still sent"
        );
        // Commas and capitals but no sentence end: still left out.
        let no_ends = "And then we went back to the studio, and I said, you know, \
                       we should try it again with the other microphone";
        assert_eq!(part_prompt(None, &previous(no_ends)), None);
        // Punctuated text is kept, and so is a short tail (too little to judge).
        let fine = "And then we went back to the studio. I said we should try it \
                    again with the other microphone. She agreed.";
        assert_eq!(part_prompt(None, &previous(fine)).as_deref(), Some(fine));
        assert_eq!(
            part_prompt(None, &previous("thank you so much")).as_deref(),
            Some("thank you so much")
        );
    }

    #[test]
    fn silent_parts_skip_whisper() {
        // Speech, then 6 s of silence (a whole part), then speech.
        let mut audio = tone(&[(700, 0.0), (3_900, 0.5), (400, 0.0)]);
        audio.extend(tone(&[(6_000, 0.0)]));
        audio.extend(tone(&[(3_000, 0.5)]));
        let video = Video {
            duration_ms: samples_to_ms(audio.len()),
            ..VIDEO
        };
        let whisper = FakeWhisper::default();
        let out = go(&temp_dir("silent"), &settings(), &video, audio, &whisper)
            .0
            .unwrap();
        let calls = whisper.calls.borrow().len();
        assert_eq!(out.transcript.segments.len(), calls);
        assert!(
            calls < 3,
            "the silent part was sent to Whisper ({calls} calls)"
        );
    }

    #[test]
    fn nothing_to_load_when_every_part_is_cached() {
        let dir = temp_dir("all-cached");
        let stopping = FakeWhisper {
            cancel_on: Some(3),
            ..FakeWhisper::default()
        };
        let _ = go(&dir, &settings(), &VIDEO, speech(), &stopping);
        assert_eq!(
            saved_until(&dir, &settings(), &VIDEO, &SMALL),
            (9_700, Some(3))
        );
        // Parts 1–2 cached; part 3 still needs the model.
        let cancel = Arc::new(AtomicBool::new(false));
        let whisper = FakeWhisper::default();
        let mut loads = 0;
        run(
            &dir,
            &settings(),
            &VIDEO,
            &SMALL,
            opener(speech(), Arc::default()),
            || {
                loads += 1;
                Ok(&whisper)
            },
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(loads, 1);
        // Silent audio never loads the model at all.
        let silent = tone(&[(3_000, 0.0)]);
        run(
            &temp_dir("all-silent"),
            &settings(),
            &VIDEO,
            &SMALL,
            opener(silent, Arc::default()),
            never_loaded,
            &cancel,
            |_| {},
        )
        .unwrap();
    }

    fn never_loaded() -> Result<&'static FakeWhisper, String> {
        panic!("loaded the model")
    }

    #[test]
    fn cancelling_while_reading_stops() {
        let cancel = Arc::new(AtomicBool::new(true));
        let whisper = FakeWhisper::default();
        let err = run(
            &temp_dir("cancel-read"),
            &settings(),
            &VIDEO,
            &SMALL,
            opener(speech(), Arc::default()),
            || Ok(&whisper),
            &cancel,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert!(whisper.calls.borrow().is_empty());
    }

    /// The whole pipeline on fixtures/en/long.mp4 (10:32, two voices): real ffmpeg and
    /// Whisper Base, cancelled in part 2 and resumed, then compared with an uninterrupted
    /// run. Needs ffmpeg and the base model; takes minutes:
    /// `cargo test transcribes_the_long_fixture -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn transcribes_the_long_fixture_in_parts() {
        use crate::chunks::LIMITS;
        use crate::transcribe::tests::base_engine;

        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en");
        let path = fixtures.join("long.mp4");
        let reference = fs::read_to_string(fixtures.join("long.txt")).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let video = Video {
            size: meta.len(),
            mtime_ms: 1,
            duration_ms: crate::probe::probe_video(&path).unwrap().duration_ms,
        };
        let audio = |ms| AudioStream::open_at(&path, ms);
        let text = |t: &Transcript| {
            t.segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };

        // Cancelled about two minutes into part 1: its checkpoints are kept.
        let dir = temp_dir("long");
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let started = Instant::now();
        let err = run(
            &dir,
            &settings(),
            &video,
            &LIMITS,
            audio,
            || Ok(base_engine()),
            &cancel,
            move |p| {
                if p.stage == Stage::Transcribing && p.position_ms >= 120_000 {
                    stop.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        let saved = saved_until(&dir, &settings(), &video, &LIMITS).0;
        assert!(
            (60_000..=150_000).contains(&saved),
            "saved inside part 1, at most ~30 s behind: {saved}"
        );
        println!(
            "2 minutes and the cancel took {:.0?}; saved until {saved} ms",
            started.elapsed()
        );

        // Resumed.
        let cancel = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let progress = log.clone();
        let started = Instant::now();
        let resumed = run(
            &dir,
            &settings(),
            &video,
            &LIMITS,
            audio,
            || Ok(base_engine()),
            &cancel,
            move |p| progress.lock().unwrap().push(p),
        )
        .unwrap();
        println!("the resumed parts took {:.0?}", started.elapsed());
        let log = log.lock().unwrap();
        let parts = log.last().unwrap().parts;
        assert!(parts >= 2, "{parts} parts");
        assert!(
            log.iter()
                .filter(|p| p.stage == Stage::Transcribing)
                .all(|p| p.position_ms >= saved),
            "the saved start of part 1 came from the cache"
        );
        assert!(
            log.iter().any(|p| p.eta_ms.is_some()),
            "a time left was shown"
        );

        let t = &resumed.transcript;
        let wer = crate::wer::word_error_rate(&reference, &text(t));
        println!(
            "{parts} parts, language {}, WER (base) {wer:.3}",
            t.language
        );
        assert_eq!(t.language, "en");
        assert!(wer < 0.15, "WER {wer:.3} is too high");
        let mut last = 0;
        for w in t.segments.iter().flat_map(|s| &s.words) {
            assert!(w.start_ms >= last && w.end_ms >= w.start_ms, "{w:?}");
            last = w.end_ms;
        }
        assert!(last <= video.duration_ms);
        assert!(
            resumed
                .loudness
                .len()
                .abs_diff(video.duration_ms as usize / 1000)
                <= 1
        );

        // An uninterrupted run says the same words. (Not the identical text: the resume
        // seeks with ffmpeg, so its samples differ in the last bits, and Whisper's casing
        // and punctuation are sensitive to that.)
        let whole = run(
            &temp_dir("long-whole"),
            &settings(),
            &video,
            &LIMITS,
            audio,
            || Ok(base_engine()),
            &cancel,
            |_| {},
        )
        .unwrap();
        let between = crate::wer::word_error_rate(&text(&whole.transcript), &text(t));
        println!("resumed vs uninterrupted: WER {between:.3}");
        assert!(
            between < 0.05,
            "resumed and uninterrupted differ: WER {between:.3}"
        );

        // The moment finder on both: whole-sentence clips (judged against the script, so
        // a recognition slip in a clip's first or last word may count against it).
        for (name, output) in [("resumed", &resumed), ("uninterrupted", &whole)] {
            moments_follow_the_script(name, output, video.duration_ms);
        }
    }

    /// Finds moments in a transcript of the long fixture and checks they start and end at
    /// sentences of its script; prints how much of the transcript has punctuation.
    fn moments_follow_the_script(name: &str, output: &Output, duration_ms: u64) {
        use crate::sentences::{self, Boundary, normalize, tests::script_sentences};
        let script: Vec<Vec<String>> = script_sentences()
            .iter()
            .map(|s| {
                s.split_whitespace()
                    .map(normalize)
                    .filter(|w| !w.is_empty())
                    .collect()
            })
            .collect();
        let text = sentences::split(&output.transcript);
        let by_punctuation = text
            .sentences
            .iter()
            .filter(|s| s.ends == Boundary::Punctuation)
            .count();
        println!(
            "{name}: {} sentences, {by_punctuation} ending at punctuation",
            text.sentences.len()
        );
        let settings = crate::moments::FinderSettings::default();
        let found = crate::moments::find_in(
            &output.transcript,
            &output.loudness,
            duration_ms,
            &settings,
            &[],
            0,
        );
        assert_eq!(found.len(), crate::moments::wanted(&settings, duration_ms));
        let mut whole = 0;
        for m in &found {
            let words: Vec<String> = m
                .text
                .split_whitespace()
                .map(normalize)
                .filter(|w| !w.is_empty())
                .collect();
            let starts = script
                .iter()
                .any(|s| s.len() >= 2 && words.starts_with(&s[..2]));
            let ends = script
                .iter()
                .any(|s| s.len() >= 2 && words.ends_with(&s[s.len() - 2..]));
            println!(
                "  {} {}–{} s: {}…{}",
                m.score,
                m.in_ms / 1000,
                m.out_ms / 1000,
                words[..4.min(words.len())].join(" "),
                words[words.len().saturating_sub(4)..].join(" ")
            );
            whole += usize::from(starts && ends);
        }
        assert!(
            whole + 1 >= found.len(),
            "{name}: only {whole} of {} clips are whole script sentences",
            found.len()
        );
    }

    #[test]
    fn estimates_time_left_from_the_measured_speed() {
        // 60 s of audio took 30 s: 120 s left take 60 s.
        assert_eq!(eta_ms(120_000, 30_000, 60_000), Some(60_000));
        assert_eq!(eta_ms(120_000, 30_000, 4_000), None, "too little measured");
        assert_eq!(eta_ms(120_000, 0, 60_000), None);
    }
}
