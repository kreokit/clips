//! Transcribing a whole video, however long: audio streamed from ffmpeg, cut into ~5-minute
//! parts (`chunks.rs`), each part sent to Whisper, finished parts cached, and the parts
//! merged into one transcript with times on the video's clock.
//!
//! **Cache** (`<project>/transcript/`): `meta.json` records what the transcript is made
//! from (settings, video size and time, part limits); any difference discards the folder.
//! Each finished part is saved as `part-NNN.json` the moment it is done, so a cancelled
//! (or crashed) run resumes after its last finished part. The finished transcript is
//! `transcript.json`, with `loudness.json` (one level per second) next to it; the part
//! files are removed then.
//!
//! **Continuity across parts.** The language is detected on the first part with sound and
//! then fixed, so it can't change halfway. Each part's prompt is the custom words plus the
//! end of the previous part's text, so sentences carry on across a cut.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::audio::{AudioStream, Loudness, samples_to_ms};
use crate::chunks::{self, Limits};
use crate::models::{CANCELLED, WhisperModel};
use crate::projects::write_atomic;
use crate::transcribe::{self, Engine, Options, Segment, Transcript};

/// Bump when the cache's contents change meaning; older caches are discarded.
const CACHE_VERSION: u32 = 1;
const META_FILE: &str = "meta.json";
const TRANSCRIPT_FILE: &str = "transcript.json";
const LOUDNESS_FILE: &str = "loudness.json";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
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
    /// The part being worked on (1-based) and about how many there are.
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
    fn transcribe(
        &self,
        samples: &[f32],
        options: Options,
        cancel: &Arc<AtomicBool>,
        on_progress: Box<dyn FnMut(i32)>,
    ) -> Result<Transcript, String>;
}

impl Transcriber for Engine {
    fn transcribe(
        &self,
        samples: &[f32],
        options: Options,
        cancel: &Arc<AtomicBool>,
        on_progress: Box<dyn FnMut(i32)>,
    ) -> Result<Transcript, String> {
        Engine::transcribe(self, samples, options, cancel, on_progress)
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

/// How many parts a cancelled run has finished (0 if none, or if the settings differ).
pub fn finished_parts(dir: &Path, settings: &Settings, video: &Video, limits: &Limits) -> usize {
    if !cache_matches(dir, &Meta::new(settings, video, limits)) {
        return 0;
    }
    (0..)
        .take_while(|&i| read_json::<Part>(&part_file(dir, i)).is_some())
        .count()
}

/// Estimated time left: the remaining audio at the speed measured so far.
fn eta_ms(remaining_audio_ms: u64, spent_ms: u64, done_audio_ms: u64) -> Option<u64> {
    // Too little measured to say (the first seconds include warming up).
    if done_audio_ms < 5_000 || spent_ms == 0 {
        return None;
    }
    Some((remaining_audio_ms as u128 * spent_ms as u128 / done_audio_ms as u128) as u64)
}

/// The prompt for the next part: custom words, then the end of the previous part's text.
fn part_prompt(custom: Option<&str>, previous: &[Segment]) -> Option<String> {
    let words: Vec<&str> = previous
        .iter()
        .flat_map(|s| s.text.split_whitespace())
        .collect();
    let tail = words[words.len().saturating_sub(PROMPT_TAIL_WORDS)..].join(" ");
    let prompt = [custom.unwrap_or(""), &tail]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (!prompt.is_empty()).then_some(prompt)
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
            for w in &mut s.words {
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

/// Transcribes the whole video into `dir`, resuming from finished parts. `load` is called
/// only if some part still needs Whisper. Returns [`CANCELLED`] if `cancel` is set; the
/// finished parts stay cached.
#[allow(clippy::too_many_arguments)]
pub fn run<T: Transcriber>(
    dir: &Path,
    settings: &Settings,
    video: &Video,
    limits: &Limits,
    audio: &mut impl AudioSource,
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
    let parts_estimate = video.duration_ms.div_ceil(limits.target_ms).max(1) as usize;
    let custom = transcribe::prompt_from_custom_words(&settings.custom_words);

    let mut engine: Option<T> = None;
    let mut load = Some(load);
    let mut language = settings.language.clone();
    let mut parts: Vec<Part> = Vec::new();
    let mut loudness = Loudness::default();
    let mut buffer: Vec<f32> = Vec::new();
    let mut at_end = false;
    let mut offset = 0usize; // samples before buffer[0]
    // Measured speed, over the parts Whisper actually worked on.
    let mut spent_ms = 0u64;
    let mut worked_audio_ms = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let Some(cut) = chunks::next_cut(&buffer, at_end, limits) else {
            let before = buffer.len();
            let want = limits.max_samples() + 1 - before;
            if audio.read(&mut buffer, want)? == 0 {
                at_end = true;
            }
            loudness.push(&buffer[before..]);
            continue;
        };
        if cut == 0 {
            break;
        }

        let index = parts.len();
        let start_ms = samples_to_ms(offset);
        let end_ms = samples_to_ms(offset + cut);
        let file = part_file(dir, index);
        let part = match read_json::<Part>(&file) {
            Some(p) if p.start_ms == start_ms && p.end_ms == end_ms => p,
            _ => {
                let samples = &buffer[..cut];
                let mut part = Part {
                    start_ms,
                    end_ms,
                    language: None,
                    segments: Vec::new(),
                };
                if chunks::has_sound(samples) {
                    if engine.is_none() {
                        on_progress(Progress {
                            stage: Stage::LoadingModel,
                            position_ms: start_ms,
                            duration_ms: video.duration_ms,
                            part: index + 1,
                            parts: parts_estimate.max(index + 1),
                            eta_ms: None,
                        });
                        engine = Some((load.take().expect("loaded once"))()?);
                    }
                    let prompt = part_prompt(
                        custom.as_deref(),
                        parts.last().map_or(&[][..], |p| p.segments.as_slice()),
                    );
                    let options = Options {
                        language: language.as_deref(),
                        hide_non_speech: settings.hide_non_speech,
                        prompt: prompt.as_deref(),
                    };

                    let started = Instant::now();
                    let report = {
                        let on_progress = on_progress.clone();
                        let (duration_ms, part_no) = (video.duration_ms, index + 1);
                        let parts = parts_estimate.max(index + 1);
                        let (spent_before, worked_before) = (spent_ms, worked_audio_ms);
                        let len_ms = end_ms - start_ms;
                        move |percent: i32| {
                            let done_ms = len_ms * percent.clamp(0, 100) as u64 / 100;
                            let position_ms = (start_ms + done_ms).min(duration_ms);
                            on_progress(Progress {
                                stage: Stage::Transcribing,
                                position_ms,
                                duration_ms,
                                part: part_no,
                                parts,
                                eta_ms: eta_ms(
                                    duration_ms.saturating_sub(position_ms),
                                    spent_before + started.elapsed().as_millis() as u64,
                                    worked_before + done_ms,
                                ),
                            });
                        }
                    };
                    report(0);
                    let t = engine.as_ref().unwrap().transcribe(
                        samples,
                        options,
                        cancel,
                        Box::new(report),
                    )?;
                    spent_ms += started.elapsed().as_millis() as u64;
                    worked_audio_ms += end_ms - start_ms;
                    part.language = Some(t.language);
                    part.segments = place(t.segments, start_ms, end_ms);
                }
                write_json(&file, &part)?;
                part
            }
        };
        if language.is_none() {
            language = part.language.clone();
        }
        parts.push(part);
        buffer.drain(..cut);
        offset += cut;
    }

    on_progress(Progress {
        stage: Stage::Finishing,
        position_ms: video.duration_ms,
        duration_ms: video.duration_ms,
        part: parts.len().max(1),
        parts: parts.len().max(1),
        eta_ms: Some(0),
    });
    let part_count = parts.len();
    let output = Output {
        transcript: merge(parts, settings),
        loudness: loudness.finish(),
    };
    write_json(&dir.join(LOUDNESS_FILE), &output.loudness)?;
    write_json(&dir.join(TRANSCRIPT_FILE), &output.transcript)?;
    for i in 0..part_count {
        let _ = fs::remove_file(part_file(dir, i));
    }
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
    /// number `cancel_on` (1-based).
    #[derive(Default)]
    struct FakeWhisper {
        calls: RefCell<Vec<Call>>,
        cancel_on: Option<usize>,
    }

    impl Transcriber for &FakeWhisper {
        fn transcribe(
            &self,
            samples: &[f32],
            options: Options,
            cancel: &Arc<AtomicBool>,
            mut on_progress: Box<dyn FnMut(i32)>,
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
            &mut FakeAudio::new(audio),
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

        // Progress: loading once, then parts 1..3 forward, then finishing.
        assert_eq!(progress[0].stage, Stage::LoadingModel);
        assert_eq!(
            progress
                .iter()
                .filter(|p| p.stage == Stage::LoadingModel)
                .count(),
            1
        );
        assert!(
            progress
                .windows(2)
                .all(|w| w[0].position_ms <= w[1].position_ms)
        );
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
        assert_eq!(finished_parts(&dir, &settings(), &VIDEO, &SMALL), 1);
        assert_eq!(cached(&dir, &settings(), &VIDEO, &SMALL), None);

        // The fake numbers its parts by call, so count from 2 like an uninterrupted run.
        let resumed = FakeWhisper::default();
        resumed.calls.borrow_mut().push(Call {
            len_ms: 0,
            language: None,
            prompt: None,
        });
        let (result, _) = go(&dir, &settings(), &VIDEO, speech(), &resumed);
        assert_eq!(
            resumed.calls.borrow().len(),
            3,
            "only parts 2 and 3 were transcribed"
        );
        assert_eq!(result.unwrap(), uninterrupted);
    }

    #[test]
    fn changed_settings_or_video_start_over() {
        let dir = temp_dir("invalidate");
        let stopping = FakeWhisper {
            cancel_on: Some(2),
            ..FakeWhisper::default()
        };
        let _ = go(&dir, &settings(), &VIDEO, speech(), &stopping);
        assert_eq!(finished_parts(&dir, &settings(), &VIDEO, &SMALL), 1);

        let small = Settings {
            model: WhisperModel::Small,
            ..settings()
        };
        assert_eq!(finished_parts(&dir, &small, &VIDEO, &SMALL), 0);
        let moved = Video {
            mtime_ms: 43,
            ..VIDEO
        };
        assert_eq!(finished_parts(&dir, &settings(), &moved, &SMALL), 0);

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
                .map(|i| format!("w{i}"))
                .collect::<Vec<_>>()
                .join(" "),
            words: Vec::new(),
        }];
        let tail = part_prompt(None, &previous).unwrap();
        assert_eq!(tail.split(' ').count(), PROMPT_TAIL_WORDS);
        assert!(tail.starts_with("w11 ") && tail.ends_with(" w40"));
        assert_eq!(part_prompt(None, &[]), None);
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
        assert_eq!(finished_parts(&dir, &settings(), &VIDEO, &SMALL), 2);
        // Parts 1–2 cached; part 3 still needs the model.
        let cancel = Arc::new(AtomicBool::new(false));
        let whisper = FakeWhisper::default();
        let mut loads = 0;
        run(
            &dir,
            &settings(),
            &VIDEO,
            &SMALL,
            &mut FakeAudio::new(speech()),
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
            &mut FakeAudio::new(silent),
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
            &mut FakeAudio::new(speech()),
            || Ok(&whisper),
            &cancel,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert!(whisper.calls.borrow().is_empty());
    }

    #[test]
    fn estimates_time_left_from_the_measured_speed() {
        // 60 s of audio took 30 s: 120 s left take 60 s.
        assert_eq!(eta_ms(120_000, 30_000, 60_000), Some(60_000));
        assert_eq!(eta_ms(120_000, 30_000, 4_000), None, "too little measured");
        assert_eq!(eta_ms(120_000, 0, 60_000), None);
    }
}
