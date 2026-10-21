//! Export: each clip as a 1080×1920 MP4 with its captions burned in.
//!
//! The picture (crop along the clip's path, or Fit + blur) goes through one FFmpeg filter
//! graph ([`video_graph`]) that ends in constant frame rate, yuv420p and BT.709 tags, with
//! the captions drawn last by libass (`ass` filter, as in KreoKit Captions).
//!
//! **TikTok-ready** (decided 2026-10-21): H.264 High profile (Main or Constrained Baseline
//! accepted where the encoder can't make High, [`accepted_profiles`]), yuv420p, BT.709 tags,
//! `+faststart`, AAC at 48 kHz. Every file is written under a temporary name, checked with
//! ffprobe ([`probe_output`], [`check`]: size, codecs, tags, durations, and over 60.0 s in
//! every stream when the clip must be), and only then put in place ([`place`]), never over
//! an existing file unless the user chose Replace.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::ffmpeg;
use crate::models::CANCELLED;
use crate::reframe::{self, Axis, Keyframe};

/// The exported frame.
pub const OUT: (u32, u32) = (1080, 1920);

/// How the clip's picture fills the 9:16 frame.
#[derive(Debug, Clone, Copy)]
pub enum Picture<'a> {
    /// The 9:16 crop along `keyframes` (Follow face, Center, Manual).
    Crop {
        keyframes: &'a [Keyframe],
        axis: Axis,
    },
    /// The whole frame, over a blurred, darkened copy of itself (the editor's preview:
    /// a 54×96 copy, blurred and at 70% brightness). Two `avgblur` passes, not `boxblur`:
    /// boxblur is GPL-only, so release builds' LGPL FFmpeg doesn't have it.
    FitBlur,
}

/// Frame rates the export snaps to (a source's average rate is rarely exact).
const RATES: &[(u32, u32)] = &[
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// The export's constant frame rate for a source averaging `fps`: the standard rate
/// within 0.5%, else the nearest whole rate, between 24 and 60 (phones record variable
/// rates; high-speed video plays at 60).
pub fn output_rate(fps: f64) -> (u32, u32) {
    if !fps.is_finite() || fps <= 0.0 {
        return (30, 1);
    }
    RATES
        .iter()
        .copied()
        .find(|&(n, d)| (fps / (n as f64 / d as f64) - 1.0).abs() < 0.005)
        .unwrap_or(((fps.round() as u32).clamp(24, 60), 1))
}

/// The `-filter_complex` graph from input `[0:v]` to `[v]`: the picture at 1080×1920
/// (the clip read from `start_ms`, so the crop path's times shift by it), constant
/// `rate`, held for a second past the source's end (so the picture always lasts the whole
/// clip; the output's -t cuts it), yuv420p tagged BT.709 limited range, then `captions`
/// (an `ass=…` filter) if any. The tags come before the captions so libass's colours are
/// converted for BT.709.
pub fn video_graph(
    picture: Picture,
    source: (u32, u32),
    start_ms: u64,
    (rate_n, rate_d): (u32, u32),
    captions: Option<&str>,
) -> String {
    let (w, h) = OUT;
    let picture = match picture {
        Picture::Crop { keyframes, axis } => {
            format!(
                "[0:v]{}",
                reframe::crop_filter(keyframes, axis, source, start_ms, OUT)
            )
        }
        Picture::FitBlur => format!(
            "[0:v]split=2[bg][fg];\
             [bg]scale=54:96:force_original_aspect_ratio=increase:flags=area,crop=54:96,\
             colorchannelmixer=rr=0.7:gg=0.7:bb=0.7,scale=270:480:flags=bicubic,avgblur=sizeX=8,avgblur=sizeX=8,\
             scale={w}:{h}:flags=bicubic[back];\
             [fg]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos[front];\
             [back][front]overlay=x=(W-w)/2:y=(H-h)/2,setsar=1"
        ),
    };
    let captions = captions.map(|c| format!(",{c}")).unwrap_or_default();
    format!(
        "{picture},fps=fps={rate_n}/{rate_d},tpad=stop_mode=clone:stop_duration=1,format=yuv420p,\
         setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709{captions}[v]"
    )
}

/// Video bitrate: 10 Mbit/s for 1080×1920 (also the size estimate before Export all).
pub const VIDEO_BITRATE: u64 = 10_000_000;
pub const AUDIO_BITRATE: u64 = 192_000;
pub const AUDIO_RATE: u32 = 48_000;
/// TikTok's Creator Rewards count only videos over a minute.
pub const TIKTOK_MIN_MS: u64 = 60_000;

/// H.264 encoders we may use, in order of preference (from KreoKit Captions' burn.rs).
/// Platform encoders first (fast, in LGPL FFmpeg builds), then OpenH264. libx264 is GPL
/// (no GPL components), so only development builds may use it, for a GPL FFmpeg a
/// developer may have installed; there it comes before OpenH264, whose older versions
/// (FFmpeg 4.4 on CI) can't make High profile.
fn allowed_encoders(dev_build: bool) -> Vec<&'static str> {
    let mut encoders = Vec::new();
    if cfg!(target_os = "macos") {
        encoders.push("h264_videotoolbox");
    }
    if cfg!(target_os = "windows") {
        encoders.push("h264_mf");
    }
    if dev_build {
        encoders.push("libx264");
    }
    encoders.push("libopenh264");
    encoders
}

/// Which encoders an export may try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderChoice {
    /// This build's order: [`allowed_encoders`] (libx264 only in debug builds).
    Default,
    /// The release builds' order, also in a debug build (no libx264).
    Release,
    /// Exactly this encoder, no fallback.
    Only(&'static str),
}

/// Development aid (debug builds only): `KREOKIT_DEV_ENCODER` picks the encoders, to test
/// the release path with a dev build. `release` = the release order (e.g. VideoToolbox,
/// then OpenH264 on macOS, never libx264); `videotoolbox`, `mf`, `openh264` or `x264` =
/// only that encoder, no fallback. Release builds ignore it.
pub const DEV_ENCODER_VAR: &str = "KREOKIT_DEV_ENCODER";

/// Reads [`DEV_ENCODER_VAR`] (unset or empty: [`EncoderChoice::Default`]).
pub fn parse_choice(value: &str) -> Result<EncoderChoice, String> {
    Ok(match value.trim().to_lowercase().as_str() {
        "" | "default" => EncoderChoice::Default,
        "release" => EncoderChoice::Release,
        "videotoolbox" | "h264_videotoolbox" | "vt" => EncoderChoice::Only("h264_videotoolbox"),
        "mf" | "h264_mf" => EncoderChoice::Only("h264_mf"),
        "openh264" | "libopenh264" => EncoderChoice::Only("libopenh264"),
        "x264" | "libx264" => EncoderChoice::Only("libx264"),
        other => {
            return Err(format!(
                "{DEV_ENCODER_VAR}={other} isn't known: use release, videotoolbox, mf, openh264 or x264."
            ));
        }
    })
}

/// The choice for this run: the dev variable in debug builds, else this build's order.
pub fn current_choice() -> Result<EncoderChoice, String> {
    if !cfg!(debug_assertions) {
        return Ok(EncoderChoice::Default);
    }
    parse_choice(&std::env::var(DEV_ENCODER_VAR).unwrap_or_default())
}

/// The encoders `choice` allows, in order.
fn candidates(choice: EncoderChoice, dev_build: bool) -> Vec<&'static str> {
    match choice {
        EncoderChoice::Default => allowed_encoders(dev_build),
        EncoderChoice::Release => allowed_encoders(false),
        EncoderChoice::Only(encoder) => vec![encoder],
    }
}

/// The encoders `choice` allows that this FFmpeg has, in order of preference.
fn pick_encoders(ffmpeg: &Path, choice: EncoderChoice) -> Result<Vec<&'static str>, String> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
    pick_from(
        &String::from_utf8_lossy(&output.stdout),
        &candidates(choice, cfg!(debug_assertions)),
    )
}

/// The allowed encoders listed in `ffmpeg -encoders` output (" V....D name  description"
/// lines), in `allowed` order; an error if there are none.
fn pick_from(encoder_list: &str, allowed: &[&'static str]) -> Result<Vec<&'static str>, String> {
    let found: Vec<&'static str> = allowed
        .iter()
        .copied()
        .filter(|enc| {
            encoder_list
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some(enc))
        })
        .collect();
    if found.is_empty() {
        return Err(format!(
            "Can't export video: this FFmpeg has none of the allowed H.264 encoders ({}).",
            allowed.join(", ")
        ));
    }
    Ok(found)
}

/// One clip to encode.
#[derive(Debug, Clone, Copy)]
pub struct ClipJob<'a> {
    pub video: &'a Path,
    /// The video's display size and average frame rate (probe.rs).
    pub source: (u32, u32),
    pub fps: f64,
    pub in_ms: u64,
    pub out_ms: u64,
    pub picture: Picture<'a>,
    /// The ASS script of the clip's captions (times on the clip's clock), if any.
    pub ass: Option<&'a str>,
    /// The bundled caption fonts, handed to libass.
    pub fonts: Option<&'a Path>,
}

impl ClipJob<'_> {
    pub fn length_ms(&self) -> u64 {
        self.out_ms.saturating_sub(self.in_ms)
    }
}

/// Encodes `job` into `output` (an MP4 whatever its name) with this run's encoders
/// ([`current_choice`]). `on_progress` gets 0–100. Returns the encoder used. On error or
/// cancel nothing is left at `output`.
pub fn encode(
    job: &ClipJob,
    output: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u32),
) -> Result<&'static str, String> {
    encode_with(job, output, current_choice()?, cancel, on_progress)
}

/// [`encode`] with the encoders `choice` allows.
pub fn encode_with(
    job: &ClipJob,
    output: &Path,
    choice: EncoderChoice,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u32),
) -> Result<&'static str, String> {
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let encoders = pick_encoders(&ffmpeg, choice)?;
    // Each export gets its own folder for the script and a copy of the fonts, and FFmpeg
    // runs inside it, so the filter names only plain relative paths (no escaping of ':'
    // and '\' in Windows paths).
    let work_dir = std::env::temp_dir().join(format!(
        "kreokit-clips-export-{}-{}",
        std::process::id(),
        unique()
    ));
    let result = prepare_work_dir(&work_dir, job.ass, job.fonts).and_then(|captions| {
        let rate = output_rate(job.fps);
        let graph = video_graph(
            job.picture,
            job.source,
            job.in_ms,
            rate,
            captions.as_deref(),
        );
        // An encoder can be listed but unusable (e.g. Media Foundation missing on some
        // Windows editions); if it fails before writing any video, try the next.
        let mut failures = Vec::new();
        for encoder in &encoders {
            match run_ffmpeg(
                &ffmpeg,
                &work_dir,
                job,
                &graph,
                rate,
                encoder,
                output,
                cancel,
                &mut on_progress,
            ) {
                Ok(()) => return Ok(*encoder),
                Err(attempt) if !attempt.started && attempt.message != CANCELLED => {
                    failures.push(format!("{encoder}: {}", attempt.message));
                    let _ = fs::remove_file(output);
                }
                Err(attempt) => return Err(attempt.message),
            }
        }
        Err(format!("export failed:\n{}", failures.join("\n")))
    });
    let _ = fs::remove_dir_all(&work_dir);
    if result.is_err() {
        let _ = fs::remove_file(output);
    }
    result
}

/// A number no other export of this process has used.
fn unique() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Writes `captions.ass` (and copies the `.ttf`/`.otf` files of `fonts_dir` into `fonts/`)
/// under `work_dir`; returns the matching `ass` filter, or `None` without captions.
fn prepare_work_dir(
    work_dir: &Path,
    ass: Option<&str>,
    fonts_dir: Option<&Path>,
) -> Result<Option<String>, String> {
    let fail = |e: std::io::Error| format!("cannot prepare the export: {e}");
    fs::create_dir_all(work_dir).map_err(fail)?;
    let Some(ass) = ass else { return Ok(None) };
    fs::write(work_dir.join("captions.ass"), ass).map_err(fail)?;
    let Some(fonts_dir) = fonts_dir else {
        return Ok(Some("ass=filename=captions.ass".into()));
    };
    let fonts = work_dir.join("fonts");
    fs::create_dir_all(&fonts).map_err(fail)?;
    for entry in fs::read_dir(fonts_dir).map_err(fail)?.flatten() {
        let path = entry.path();
        let is_font = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_lowercase().as_str(), "ttf" | "otf"));
        if is_font {
            fs::copy(&path, fonts.join(entry.file_name())).map_err(fail)?;
        }
    }
    Ok(Some("ass=filename=captions.ass:fontsdir=fonts".into()))
}

/// A failed FFmpeg run, and whether it had started writing video (then retrying with
/// another encoder makes no sense).
struct Attempt {
    message: String,
    started: bool,
}

/// The FFmpeg arguments after the input options, for `encoder`.
fn output_args(encoder: &str, (rate_n, rate_d): (u32, u32), length_ms: u64) -> Vec<String> {
    let gop = (2.0 * rate_n as f64 / rate_d as f64).round() as u32;
    let mut args: Vec<String> = [
        "-map",
        "[v]",
        "-map",
        "0:a:0",
        "-c:v",
        encoder,
        "-profile:v",
        "high",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend([
        "-b:v".into(),
        VIDEO_BITRATE.to_string(),
        "-g".into(),
        gop.to_string(),
    ]);
    // VideoToolbox may fall back to Apple's software encoder (e.g. in VMs).
    if encoder == "h264_videotoolbox" {
        args.extend(["-allow_sw".into(), "1".into()]);
    }
    args.extend(
        [
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "tv",
            // Audio from the clip's first moment (silence where the source's sound starts
            // late, as the transcript's clock has it) to its last.
            "-af",
            "aresample=async=1:first_pts=0,apad",
            "-c:a",
            "aac",
            "-ar",
            "48000",
            "-ac",
            "2",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.extend(["-b:a".into(), AUDIO_BITRATE.to_string()]);
    args.extend([
        "-t".into(),
        secs(length_ms),
        "-movflags".into(),
        "+faststart".into(),
    ]);
    args.extend(["-f".into(), "mp4".into()]);
    args
}

fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[allow(clippy::too_many_arguments)]
fn run_ffmpeg(
    ffmpeg: &Path,
    work_dir: &Path,
    job: &ClipJob,
    graph: &str,
    rate: (u32, u32),
    encoder: &str,
    output: &Path,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u32),
) -> Result<(), Attempt> {
    let length_ms = job.length_ms();
    let mut child = Command::new(ffmpeg)
        .current_dir(work_dir)
        .args([
            "-nostdin",
            "-y",
            "-v",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
        ])
        .args(["-ss", &secs(job.in_ms), "-t", &secs(length_ms + 1000), "-i"])
        .arg(absolute(job.video))
        .args(["-filter_complex", graph])
        .args(output_args(encoder, rate, length_ms))
        .arg(absolute(output))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Attempt {
            message: format!("failed to run {}: {e}", ffmpeg.display()),
            started: false,
        })?;

    // Collect errors on a thread so a chatty stderr can never block FFmpeg.
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut last = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Attempt {
                message: CANCELLED.to_string(),
                started: true,
            });
        }
        let Ok(line) = line else { break };
        if let Some(percent) = progress_percent(&line, length_ms)
            && last != Some(percent)
        {
            last = Some(percent);
            on_progress(percent);
        }
    }

    let started = last.is_some_and(|p| p > 0);
    let status = child.wait().map_err(|e| Attempt {
        message: e.to_string(),
        started,
    })?;
    let errors = errors.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Err(Attempt {
            message: CANCELLED.to_string(),
            started: true,
        });
    }
    if !status.success() {
        return Err(Attempt {
            message: format!("export failed: {}", errors.trim()),
            started,
        });
    }
    on_progress(100);
    Ok(())
}

/// Parses `out_time_us=…` lines from `-progress` into 0–99 (100 is sent on success).
fn progress_percent(line: &str, duration_ms: u64) -> Option<u32> {
    let us: u64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    Some(((us / 1000) * 100 / duration_ms.max(1)).min(99) as u32)
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What ffprobe says about an exported file.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Facts {
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub profile: String,
    pub pix_fmt: String,
    pub color_space: String,
    pub color_primaries: String,
    pub color_transfer: String,
    pub color_range: String,
    pub audio_codec: String,
    pub sample_rate: u32,
    pub container_ms: u64,
    pub video_ms: u64,
    pub audio_ms: u64,
    /// The index (`moov`) comes before the media data, so playback starts at once.
    pub faststart: bool,
}

pub fn probe_output(path: &Path) -> Result<Facts, String> {
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let output = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffprobe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read the exported file: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("ffprobe output: {e}"))?;
    let mut facts = facts_from(&json);
    facts.faststart = moov_first(path)?;
    Ok(facts)
}

fn facts_from(json: &serde_json::Value) -> Facts {
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    let ms = |v: &serde_json::Value| {
        v["duration"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .map_or(0, |s| (s * 1000.0).round() as u64)
    };
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let of = |kind: &str| streams.iter().find(|s| s["codec_type"] == kind).cloned();
    let mut facts = Facts {
        container_ms: ms(&json["format"]),
        ..Facts::default()
    };
    if let Some(v) = of("video") {
        facts.width = v["width"].as_u64().unwrap_or(0) as u32;
        facts.height = v["height"].as_u64().unwrap_or(0) as u32;
        facts.video_codec = text(&v, "codec_name");
        facts.profile = text(&v, "profile");
        facts.pix_fmt = text(&v, "pix_fmt");
        facts.color_space = text(&v, "color_space");
        facts.color_primaries = text(&v, "color_primaries");
        facts.color_transfer = text(&v, "color_transfer");
        facts.color_range = text(&v, "color_range");
        facts.video_ms = ms(&v);
    }
    if let Some(a) = of("audio") {
        facts.audio_codec = text(&a, "codec_name");
        facts.sample_rate = text(&a, "sample_rate").parse().unwrap_or(0);
        facts.audio_ms = ms(&a);
    }
    facts
}

/// Whether the file's top-level `moov` box comes before its `mdat`.
fn moov_first(path: &Path) -> Result<bool, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("cannot read the export: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(at))
            .map_err(|e| e.to_string())?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).map_err(|e| e.to_string())?;
        let mut size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        if size == 1 {
            file.read_exact(&mut head[8..]).map_err(|e| e.to_string())?;
            size = u64::from_be_bytes(head[8..].try_into().unwrap());
        }
        match &head[4..8] {
            b"moov" => return Ok(true),
            b"mdat" => return Ok(false),
            _ => {}
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    Ok(false)
}

/// The H.264 profiles accepted from `encoder` (decided 2026-10-21, user): High from
/// VideoToolbox (and libx264 in dev builds); High or Main from Media Foundation; OpenH264
/// (FFmpeg's wrapper may ignore `-profile high`) also Constrained Baseline. TikTok accepts
/// all three; yuv420p and faststart are required whatever the encoder.
pub fn accepted_profiles(encoder: &str) -> &'static [&'static str] {
    match encoder {
        "h264_mf" => &["High", "Main"],
        "libopenh264" => &["High", "Main", "Constrained Baseline"],
        _ => &["High"],
    }
}

/// What a finished clip from `encoder` must be: TikTok-ready, the clip's length in every
/// stream (within a frame and an audio packet), and over 60.0 s in every stream when
/// `over_a_minute`.
pub fn check(
    facts: &Facts,
    encoder: &str,
    length_ms: u64,
    (rate_n, rate_d): (u32, u32),
    over_a_minute: bool,
) -> Result<(), String> {
    let mut problems = Vec::new();
    let mut expect = |ok: bool, what: String| {
        if !ok {
            problems.push(what);
        }
    };
    expect(
        (facts.width, facts.height) == OUT,
        format!("size {}×{}", facts.width, facts.height),
    );
    expect(
        facts.video_codec == "h264",
        format!("video codec {}", facts.video_codec),
    );
    expect(
        accepted_profiles(encoder).contains(&facts.profile.as_str()),
        format!("H.264 profile {} from {encoder}", facts.profile),
    );
    expect(
        facts.pix_fmt == "yuv420p",
        format!("pixel format {}", facts.pix_fmt),
    );
    for (name, value) in [
        ("colour matrix", &facts.color_space),
        ("colour primaries", &facts.color_primaries),
        ("transfer", &facts.color_transfer),
    ] {
        expect(value == "bt709", format!("{name} {value}"));
    }
    expect(
        facts.color_range == "tv",
        format!("colour range {}", facts.color_range),
    );
    expect(
        facts.audio_codec == "aac",
        format!("audio codec {}", facts.audio_codec),
    );
    expect(
        facts.sample_rate == AUDIO_RATE,
        format!("sample rate {}", facts.sample_rate),
    );
    expect(
        facts.faststart,
        "the index isn't at the start (faststart)".into(),
    );

    let frame_ms = 1000 * rate_d as u64 / rate_n.max(1) as u64;
    let slack = frame_ms.max(1024 * 1000 / AUDIO_RATE as u64) + 10;
    for (name, ms) in [
        ("file", facts.container_ms),
        ("picture", facts.video_ms),
        ("sound", facts.audio_ms),
    ] {
        expect(
            ms.abs_diff(length_ms) <= slack,
            format!("{name} lasts {} instead of {}", secs(ms), secs(length_ms)),
        );
        if over_a_minute {
            expect(
                ms > TIKTOK_MIN_MS,
                format!("{name} lasts {}, not over 60.0 s", secs(ms)),
            );
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "The exported clip isn't right: {}.",
            problems.join(", ")
        ))
    }
}

/// The export part of `--self-test` (milestone 6, decided 2026-10-21): a 3 s synthetic
/// video (picture and 44.1 kHz sound) made with FFmpeg in `work_dir`, a 2 s clip of it
/// with a caption exported with the **release** encoders (never libx264), read back with
/// ffprobe and checked like every export (the encoder's profiles, yuv420p, faststart,
/// BT.709, 48 kHz, lengths). Returns a one-line summary naming the encoder.
pub fn self_check(work_dir: &Path, fonts: Option<&Path>) -> Result<String, String> {
    use crate::style::{Segment, Style, Word};
    fs::create_dir_all(work_dir)
        .map_err(|e| format!("cannot create {}: {e}", work_dir.display()))?;
    let source = work_dir.join("self-test-source.mp4");
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let made = Command::new(&ffmpeg)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=3",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=3",
        ])
        .args(["-c:v", "mpeg4", "-q:v", "5", "-c:a", "aac", "-shortest"])
        .arg(&source)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
    if !made.status.success() {
        return Err(format!(
            "cannot make the test video: {}",
            String::from_utf8_lossy(&made.stderr).trim()
        ));
    }
    let info = crate::probe::probe_video(&source)?;
    let caption = Segment {
        id: 0,
        start_ms: 0,
        end_ms: 1500,
        text: "Self test".into(),
        words: vec![
            Word {
                start_ms: 0,
                end_ms: 600,
                text: "Self".into(),
            },
            Word {
                start_ms: 700,
                end_ms: 1300,
                text: "test".into(),
            },
        ],
    };
    let ass = crate::ass::to_ass(&[caption], &Style::default(), OUT.0, OUT.1);
    let keyframes = [];
    let job = ClipJob {
        video: &source,
        source: (info.width, info.height),
        fps: info.fps,
        in_ms: 500,
        out_ms: 2_500,
        picture: Picture::Crop {
            keyframes: &keyframes,
            axis: Axis::X,
        },
        ass: Some(&ass),
        fonts,
    };
    let out = work_dir.join("self-test-clip.mp4");
    let encoder = encode_with(
        &job,
        &out,
        EncoderChoice::Release,
        &AtomicBool::new(false),
        |_| {},
    )?;
    let facts = probe_output(&out)?;
    check(
        &facts,
        encoder,
        job.length_ms(),
        output_rate(info.fps),
        false,
    )
    .map_err(|e| format!("{encoder}: {e}"))?;
    Ok(format!(
        "export: {encoder}, {}×{} H.264 {} {}, faststart, {} s",
        facts.width,
        facts.height,
        facts.profile,
        facts.pix_fmt,
        secs(facts.container_ms)
    ))
}

/// Moves the finished temporary file `tmp` to `dest`. With `replace` an existing file is
/// replaced; otherwise it is never touched: the clip gets the first free name of
/// `dest`, `dest-2`, `dest-3`… (a file can appear while the export runs). The temporary
/// file is gone afterwards either way.
pub fn place(tmp: &Path, dest: &Path, replace: bool) -> Result<PathBuf, String> {
    let result = if replace {
        replace_with(tmp, dest).map(|()| dest.to_path_buf())
    } else {
        place_new(tmp, dest)
    };
    let _ = fs::remove_file(tmp);
    result
}

fn replace_with(tmp: &Path, dest: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("cannot save {}: {e}", dest.display());
    match fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        // Windows can't rename over an existing file.
        Err(_) if dest.is_file() => {
            fs::remove_file(dest).map_err(fail)?;
            fs::rename(tmp, dest).map_err(fail)
        }
        Err(e) => Err(fail(e)),
    }
}

fn place_new(tmp: &Path, dest: &Path) -> Result<PathBuf, String> {
    let stem = dest
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ext = dest
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for n in 1.. {
        let candidate = match n {
            1 => dest.to_path_buf(),
            n => dest.with_file_name(format!("{stem}-{n}.{ext}")),
        };
        // A hard link fails if the name exists, so nothing is ever replaced; folders
        // without hard links (FAT, some network drives) fall back to a checked rename.
        match fs::hard_link(tmp, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) if candidate.exists() => continue,
            Err(_) => {
                return fs::rename(tmp, &candidate)
                    .map(|()| candidate.clone())
                    .map_err(|e| format!("cannot save {}: {e}", candidate.display()));
            }
        }
    }
    unreachable!()
}

/// Lower-cased file names in `folder` (macOS and Windows compare names without case).
pub fn names_in(folder: &Path) -> HashSet<String> {
    fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// A temporary name for `dest` in its folder (so placing it is a rename on one drive),
/// hidden and not ending in `.mp4`, so it's never taken for a finished clip.
pub fn temp_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    dest.with_file_name(format!(
        ".{name}.{}-{}.kreokit-tmp",
        std::process::id(),
        unique()
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn frame_rates_snap_to_standard_ones() {
        assert_eq!(output_rate(29.97), (30000, 1001));
        assert_eq!(output_rate(29.970_03), (30000, 1001));
        assert_eq!(output_rate(25.0), (25, 1));
        assert_eq!(output_rate(23.976), (24000, 1001));
        assert_eq!(output_rate(59.94), (60000, 1001));
        // A phone's variable rate, slow and fast sources, nonsense.
        assert_eq!(output_rate(29.6), (30, 1));
        assert_eq!(output_rate(15.0), (24, 1));
        assert_eq!(output_rate(120.0), (60, 1));
        assert_eq!(output_rate(0.0), (30, 1));
        assert_eq!(output_rate(f64::NAN), (30, 1));
    }

    /// FFmpeg 9.0.2's GPL-only filters (`grep 'filter_deps="gpl' configure`).
    const GPL_FILTERS: &[&str] = &[
        "blackframe",
        "boxblur",
        "boxblur_opencl",
        "colormatrix",
        "cover_rect",
        "cropdetect",
        "delogo",
        "eq",
        "find_rect",
        "fspp",
        "histeq",
        "hqdn3d",
        "interlace",
        "kerndeint",
        "mcdeint",
        "mpdecimate",
        "mptestsrc",
        "nnedi",
        "owdenoise",
        "perspective",
        "phase",
        "pp7",
        "pullup",
        "repeatfields",
        "sab",
        "signature",
        "smartblur",
        "spp",
        "stereo3d",
        "super2xsai",
        "tinterlace",
        "uspp",
        "vaguedenoiser",
    ];

    #[test]
    fn the_graph_ends_tagged_with_the_captions_last() {
        let keyframes = [Keyframe {
            t_ms: 0,
            pos: 0.5,
            jump: false,
            ease: false,
        }];
        let crop = Picture::Crop {
            keyframes: &keyframes,
            axis: Axis::X,
        };
        let g = video_graph(
            crop,
            (1920, 1080),
            0,
            (30, 1),
            Some("ass=filename=captions.ass"),
        );
        assert!(g.starts_with("[0:v]crop=w=608:h=1080:"), "{g}");
        assert!(g.ends_with(
            ",fps=fps=30/1,tpad=stop_mode=clone:stop_duration=1,format=yuv420p,setparams=range=tv:color_primaries=bt709:\
             color_trc=bt709:colorspace=bt709,ass=filename=captions.ass[v]"
        ));
        let fit = video_graph(Picture::FitBlur, (1920, 1080), 0, (25, 1), None);
        assert!(fit.contains("overlay=x=(W-w)/2:y=(H-h)/2"));
        assert!(fit.ends_with("colorspace=bt709[v]"));
        // Release builds bundle an LGPL FFmpeg: none of FFmpeg 9.0's GPL-only filters
        // (`*_filter_deps="gpl…"` in its configure) may appear.
        for graph in [&g, &fit] {
            for gpl in GPL_FILTERS {
                assert!(
                    !graph
                        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .any(|word| word == *gpl),
                    "{gpl} in {graph}"
                );
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-clips-export-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn good_facts(length_ms: u64) -> Facts {
        Facts {
            width: 1080,
            height: 1920,
            video_codec: "h264".into(),
            profile: "High".into(),
            pix_fmt: "yuv420p".into(),
            color_space: "bt709".into(),
            color_primaries: "bt709".into(),
            color_transfer: "bt709".into(),
            color_range: "tv".into(),
            audio_codec: "aac".into(),
            sample_rate: 48_000,
            container_ms: length_ms,
            video_ms: length_ms,
            audio_ms: length_ms + 10,
            faststart: true,
        }
    }

    #[test]
    fn checks_every_tiktok_requirement() {
        let rate = (30, 1);
        assert_eq!(
            check(&good_facts(61_000), "h264_videotoolbox", 61_000, rate, true),
            Ok(())
        );
        let bad = Facts {
            profile: "Constrained Baseline".into(),
            color_transfer: "unknown".into(),
            sample_rate: 44_100,
            faststart: false,
            ..good_facts(61_000)
        };
        let err = check(&bad, "h264_videotoolbox", 61_000, rate, true).unwrap_err();
        for part in [
            "profile Constrained Baseline",
            "transfer unknown",
            "sample rate 44100",
            "faststart",
        ] {
            assert!(err.contains(part), "{err}");
        }
        // A stream shorter than the clip by more than a frame.
        let short = Facts {
            video_ms: 60_900,
            ..good_facts(61_000)
        };
        assert!(
            check(&short, "h264_videotoolbox", 61_000, rate, false)
                .unwrap_err()
                .contains("picture lasts 60.900")
        );
    }

    /// Decided 2026-10-21 (user): High where the encoder makes it, Main also from Media
    /// Foundation, Main or Constrained Baseline also from OpenH264; never another pixel
    /// format or a file without faststart.
    #[test]
    fn the_accepted_profile_depends_on_the_encoder() {
        let rate = (30, 1);
        let with = |profile: &str| Facts {
            profile: profile.into(),
            ..good_facts(10_000)
        };
        let ok = |encoder: &str, profile: &str| check(&with(profile), encoder, 10_000, rate, false);
        for encoder in ["h264_videotoolbox", "h264_mf", "libopenh264", "libx264"] {
            assert_eq!(ok(encoder, "High"), Ok(()), "{encoder}");
            assert!(ok(encoder, "High 10").is_err(), "{encoder}");
        }
        assert!(
            ok("h264_videotoolbox", "Main")
                .unwrap_err()
                .contains("H.264 profile Main from h264_videotoolbox")
        );
        assert!(ok("libx264", "Main").is_err());
        assert_eq!(ok("h264_mf", "Main"), Ok(()));
        assert!(ok("h264_mf", "Constrained Baseline").is_err());
        assert_eq!(ok("libopenh264", "Main"), Ok(()));
        assert_eq!(ok("libopenh264", "Constrained Baseline"), Ok(()));
        assert!(ok("libopenh264", "Baseline").is_err());
        for encoder in ["h264_mf", "libopenh264"] {
            let bad = Facts {
                pix_fmt: "yuvj420p".into(),
                faststart: false,
                ..with("Main")
            };
            let err = check(&bad, encoder, 10_000, rate, false).unwrap_err();
            assert!(
                err.contains("pixel format yuvj420p") && err.contains("faststart"),
                "{err}"
            );
        }
    }

    #[test]
    fn never_passes_a_clip_of_a_minute_or_less_when_it_must_be_over() {
        let rate = (30, 1);
        // Exactly 60.0 s isn't over a minute; neither is 59.99 in one stream.
        assert!(
            check(&good_facts(60_000), "h264_videotoolbox", 60_000, rate, true)
                .unwrap_err()
                .contains("not over 60.0 s")
        );
        let one_short = Facts {
            audio_ms: 59_990,
            ..good_facts(60_010)
        };
        assert!(
            check(&one_short, "h264_videotoolbox", 60_010, rate, true)
                .unwrap_err()
                .contains("sound lasts 59.990")
        );
        assert_eq!(
            check(&good_facts(60_010), "h264_videotoolbox", 60_010, rate, true),
            Ok(())
        );
        // Shorts don't need it.
        assert_eq!(
            check(
                &good_facts(30_000),
                "h264_videotoolbox",
                30_000,
                rate,
                false
            ),
            Ok(())
        );
    }

    #[test]
    fn reads_ffprobe_json() {
        let json = serde_json::json!({
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "profile": "High", "width": 1080,
                 "height": 1920, "pix_fmt": "yuv420p", "color_range": "tv", "color_space": "bt709",
                 "color_transfer": "bt709", "color_primaries": "bt709", "duration": "61.000000"},
                {"codec_type": "audio", "codec_name": "aac", "sample_rate": "48000", "duration": "61.013333"}
            ],
            "format": {"duration": "61.013333"}
        });
        let facts = facts_from(&json);
        assert_eq!(
            (facts.video_ms, facts.audio_ms, facts.container_ms),
            (61_000, 61_013, 61_013)
        );
        assert_eq!(facts.sample_rate, 48_000);
        assert_eq!(
            check(
                &Facts {
                    faststart: true,
                    ..facts
                },
                "h264_videotoolbox",
                61_000,
                (30, 1),
                true
            ),
            Ok(())
        );
    }

    #[test]
    fn picks_encoders_and_reads_progress() {
        let list = " V....D libx264    libx264 H.264\n V....D libopenh264  OpenH264 H.264\n";
        assert_eq!(
            pick_from(list, &["h264_mf", "libx264", "libopenh264"]),
            Ok(vec!["libx264", "libopenh264"])
        );
        assert!(pick_from("", &["libopenh264"]).is_err());
        assert!(!allowed_encoders(false).contains(&"libx264"));
        assert_eq!(progress_percent("out_time_us=30500000", 61_000), Some(50));
        assert_eq!(progress_percent("out_time_us=N/A", 61_000), None);
    }

    #[test]
    fn the_dev_variable_picks_the_encoders() {
        assert_eq!(parse_choice(""), Ok(EncoderChoice::Default));
        assert_eq!(parse_choice(" Release "), Ok(EncoderChoice::Release));
        assert_eq!(
            parse_choice("videotoolbox"),
            Ok(EncoderChoice::Only("h264_videotoolbox"))
        );
        assert_eq!(
            parse_choice("openh264"),
            Ok(EncoderChoice::Only("libopenh264"))
        );
        assert!(
            parse_choice("nvenc")
                .unwrap_err()
                .contains("KREOKIT_DEV_ENCODER=nvenc")
        );
        // The release order never has libx264, even in a debug build.
        assert!(!candidates(EncoderChoice::Release, true).contains(&"libx264"));
        assert_eq!(
            candidates(EncoderChoice::Release, true),
            allowed_encoders(false)
        );
        assert!(candidates(EncoderChoice::Default, true).contains(&"libx264"));
        assert_eq!(
            candidates(EncoderChoice::Only("h264_videotoolbox"), true),
            ["h264_videotoolbox"]
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                allowed_encoders(false),
                ["h264_videotoolbox", "libopenh264"]
            );
        }
    }

    /// The `--self-test` export with the release encoders. macOS only here: on Linux the
    /// release encoder is OpenH264, and CI's FFmpeg 4.4 can't make High profile with it
    /// (release builds bundle FFmpeg 9, checked by the release workflow's self-test).
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs FFmpeg"]
    fn self_check_exports_with_the_release_encoder() {
        let dir = temp_dir("self-check");
        let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/fonts");
        let summary = self_check(&dir, Some(&fonts)).unwrap();
        println!("{summary}");
        assert!(
            summary
                .starts_with("export: h264_videotoolbox, 1080×1920 H.264 High yuv420p, faststart"),
            "{summary}"
        );
    }

    #[test]
    fn placing_never_replaces_unless_asked() {
        let dir = temp_dir("place");
        let dest = dir.join("talk-clip-01.mp4");
        let put = |bytes: &[u8], replace: bool| {
            let tmp = temp_path(&dest);
            fs::write(&tmp, bytes).unwrap();
            let placed = place(&tmp, &dest, replace).unwrap();
            assert!(!tmp.exists());
            placed
        };
        assert_eq!(put(b"one", false), dest);
        assert_eq!(put(b"two", false), dir.join("talk-clip-01-2.mp4"));
        assert_eq!(fs::read(&dest).unwrap(), b"one");
        assert_eq!(put(b"three", true), dest);
        assert_eq!(fs::read(&dest).unwrap(), b"three");
        let mut names: Vec<String> = names_in(&dir).into_iter().collect();
        names.sort();
        assert_eq!(names, ["talk-clip-01-2.mp4", "talk-clip-01.mp4"]);
        assert!(
            temp_path(&dest)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".talk-clip-01.mp4.")
        );
    }

    fn ffmpeg_ok(args: &[&str]) {
        let output = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A 640×360 test video at `rate` with a 1 kHz beep 3.0–3.2 s into its sound (44.1 kHz,
    /// so the export resamples), the picture `video_s` and the sound `audio_s` long, the
    /// sound starting `audio_delay_s` after the picture.
    pub(crate) fn make_source(
        dir: &Path,
        rate: &str,
        video_s: f64,
        audio_s: f64,
        audio_delay_s: f64,
    ) -> PathBuf {
        let out = dir.join(format!("source-{}.mp4", rate.replace('/', "_")));
        let video = format!("testsrc2=size=640x360:rate={rate}:duration={video_s}");
        let audio =
            format!("aevalsrc='if(between(t,3,3.2),0.5*sin(2*PI*1000*t),0)':s=44100:d={audio_s}");
        let delay = audio_delay_s.to_string();
        ffmpeg_ok(&[
            "-f",
            "lavfi",
            "-i",
            &video,
            "-itsoffset",
            &delay,
            "-f",
            "lavfi",
            "-i",
            &audio,
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "mpeg4",
            "-q:v",
            "5",
            "-c:a",
            "aac",
            out.to_str().unwrap(),
        ]);
        out
    }

    fn fonts() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/fonts")
    }

    /// Exports `in_ms..out_ms` of `video` centre-cropped (with `ass` captions); the facts.
    fn export(
        dir: &Path,
        video: &Path,
        in_ms: u64,
        out_ms: u64,
        ass: Option<&str>,
    ) -> (PathBuf, Facts, (u32, u32), &'static str) {
        let info = crate::probe::probe_video(video).unwrap();
        let keyframes = [];
        let job = ClipJob {
            video,
            source: (info.width, info.height),
            fps: info.fps,
            in_ms,
            out_ms,
            picture: Picture::Crop {
                keyframes: &keyframes,
                axis: Axis::X,
            },
            ass,
            fonts: Some(&fonts()),
        };
        let out = dir.join(format!("clip-{in_ms}-{out_ms}.mp4"));
        let mut progress = Vec::new();
        let encoder = encode(&job, &out, &AtomicBool::new(false), |p| progress.push(p)).unwrap();
        assert_eq!(progress.last(), Some(&100));
        let facts = probe_output(&out).unwrap();
        println!("{} ({encoder}): {facts:?}", out.display());
        (out, facts, output_rate(info.fps), encoder)
    }

    /// When the sound first gets loud in `video`, in ms (48 kHz mono).
    fn beep_at(video: &Path) -> u64 {
        let output = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-i"])
            .arg(video)
            .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "-"])
            .output()
            .unwrap();
        let samples: Vec<f32> = output
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let first = samples.iter().position(|s| s.abs() > 0.1).expect("a beep");
        first as u64 * 1000 / 48_000
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn exports_a_tiktok_ready_clip_with_captions() {
        let dir = temp_dir("ready");
        let video = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en/hello.mp4");
        let words = [("Hello", 0, 400), ("there", 500, 900)].map(|(text, start_ms, end_ms)| {
            crate::style::Word {
                start_ms,
                end_ms,
                text: text.into(),
            }
        });
        let caption = crate::style::Segment {
            id: 0,
            start_ms: 0,
            end_ms: 1500,
            text: "Hello there".into(),
            words: words.to_vec(),
        };
        let ass = crate::ass::to_ass(&[caption], &crate::style::Style::default(), OUT.0, OUT.1);
        let (_, facts, rate, enc) = export(&dir, &video, 1_000, 8_000, Some(&ass));
        assert_eq!(check(&facts, enc, 7_000, rate, false), Ok(()));
        // Seven seconds is not over a minute.
        assert!(check(&facts, enc, 7_000, rate, true).is_err());
    }

    /// TikTok Rewards (decided 2026-10-21): a 61 s clip must come out over 60.0 s in every
    /// stream, whatever the frame rate.
    #[test]
    #[ignore = "needs FFmpeg; encodes three 61 s clips"]
    fn a_61_second_clip_stays_over_a_minute_at_any_frame_rate() {
        let dir = temp_dir("minute");
        for rate in ["30", "30000/1001", "25"] {
            let video = make_source(&dir, rate, 70.0, 70.0, 0.0);
            let (_, facts, out_rate, enc) = export(&dir, &video, 3_000, 64_000, None);
            assert_eq!(check(&facts, enc, 61_000, out_rate, true), Ok(()), "{rate}");
            for ms in [facts.container_ms, facts.video_ms, facts.audio_ms] {
                assert!(ms > 60_900, "{rate}: {facts:?}");
            }
        }
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn a_clip_to_the_very_end_lasts_its_whole_length() {
        let dir = temp_dir("end");
        // The picture ends 300 ms before the sound.
        let video = make_source(&dir, "30", 11.7, 12.0, 0.0);
        let end = crate::probe::probe_video(&video).unwrap().duration_ms;
        let (_, facts, rate, enc) = export(&dir, &video, 500, end, None);
        assert_eq!(check(&facts, enc, end - 500, rate, false), Ok(()));
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn sound_that_starts_late_stays_in_sync() {
        let dir = temp_dir("late");
        // The beep is 3.0 s into the sound, which starts 0.5 s after the picture: 3.5 s on
        // the video's clock (the transcript's and the player's).
        let video = make_source(&dir, "25", 10.0, 9.5, 0.5);
        let (from_one, facts, rate, enc) = export(&dir, &video, 1_000, 9_000, None);
        assert_eq!(check(&facts, enc, 8_000, rate, false), Ok(()));
        assert!(
            beep_at(&from_one).abs_diff(2_500) <= 30,
            "{}",
            beep_at(&from_one)
        );
        // From 0:00 the clip starts with silence where the source has no sound yet.
        let (from_zero, facts, rate, enc) = export(&dir, &video, 0, 8_000, None);
        assert_eq!(check(&facts, enc, 8_000, rate, false), Ok(()));
        assert!(
            beep_at(&from_zero).abs_diff(3_500) <= 30,
            "{}",
            beep_at(&from_zero)
        );
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn cancel_leaves_nothing_behind() {
        let dir = temp_dir("cancel");
        let video = make_source(&dir, "30", 20.0, 20.0, 0.0);
        let keyframes = [];
        let job = ClipJob {
            video: &video,
            source: (640, 360),
            fps: 30.0,
            in_ms: 0,
            out_ms: 20_000,
            picture: Picture::Crop {
                keyframes: &keyframes,
                axis: Axis::X,
            },
            ass: None,
            fonts: None,
        };
        let out = dir.join("cancelled.mp4");
        let cancel = AtomicBool::new(false);
        let result = encode(&job, &out, &cancel, |p| {
            if p > 0 {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(result, Err(CANCELLED.to_string()));
        assert!(!out.exists());
    }

    /// The captions land in the exported 9:16 frame where the layout (and the editor's
    /// preview) puts them: the Bottom spot above the platforms' interface, and a caption
    /// dragged up. From KreoKit Captions' `burned_position_matches_placement`.
    #[test]
    #[ignore = "needs FFmpeg"]
    fn burned_captions_sit_where_the_preview_puts_them() {
        use crate::style::{CaptionMode, Position, Segment, Style, Word};
        let dir = temp_dir("placement");
        let video = dir.join("black.mp4");
        ffmpeg_ok(&[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1920x1080:r=25:d=2",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-t",
            "2",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            video.to_str().unwrap(),
        ]);
        let cases = [
            (Position::Bottom, None, None),
            (Position::Top, Some(0.3), Some(0.2)),
        ];
        for (position, pos_x, pos_y) in cases {
            let style = Style {
                font_size: 80,
                color: "#FFFFFF".into(),
                outline_width: 0,
                mode: CaptionMode::Line,
                uppercase: false,
                position,
                pos_x,
                pos_y,
                ..Style::default()
            };
            let segment = Segment {
                id: 0,
                start_ms: 0,
                end_ms: 2000,
                text: "MARK".into(),
                words: vec![Word {
                    start_ms: 0,
                    end_ms: 2000,
                    text: "MARK".into(),
                }],
            };
            let ass = crate::ass::to_ass(&[segment], &style, OUT.0, OUT.1);
            let (out, facts, rate, enc) = export(&dir, &video, 0, 1_500, Some(&ass));
            assert_eq!(check(&facts, enc, 1_500, rate, false), Ok(()));
            let raw = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
                .args(["-v", "error", "-ss", "0.5", "-i"])
                .arg(&out)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
                .output()
                .unwrap()
                .stdout;
            let (w, h) = OUT;
            assert_eq!(raw.len(), (w * h) as usize);
            let (mut left, mut right, mut top, mut bottom) = (u32::MAX, 0, u32::MAX, 0);
            for (i, &v) in raw.iter().enumerate() {
                if v > 128 {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    (left, right, top, bottom) =
                        (left.min(x), right.max(x), top.min(y), bottom.max(y));
                }
            }
            assert!(right > left, "no caption found");
            let (ax, ay) = style.anchor();
            let (anchor_x, anchor_y) = (ax * w as f64, ay * h as f64);
            let font_px = 80.0 * h as f64 / 1080.0;
            let edge_y = if position == Position::Top {
                top
            } else {
                bottom
            } as f64;
            println!(
                "{position:?}: anchor ({anchor_x:.0}, {anchor_y:.0}), ink x {left}-{right}, y {top}-{bottom}"
            );
            assert!(((left + right) as f64 / 2.0 - anchor_x).abs() <= w as f64 * 0.02);
            // The ink sits inside the font's line box (libass and the preview both size the
            // box as the font size): capitals leave the descender space (~0.3 of the size in
            // Montserrat) below them, so the ink is within 35% of the size of the anchor. A
            // misplaced caption would be hundreds of pixels off.
            assert!(
                (edge_y - anchor_y).abs() <= font_px * 0.35,
                "{position:?}: edge {edge_y}, anchor {anchor_y}"
            );
            if position == Position::Bottom {
                // Above the bottom 22% that TikTok, Reels and Shorts cover.
                assert!((bottom as f64) < h as f64 * (1.0 - 0.22));
            }
        }
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn n_clips_give_n_files_and_nothing_is_overwritten() {
        let dir = temp_dir("n-clips");
        let video = make_source(&dir, "25", 10.0, 10.0, 0.0);
        let out_dir = dir.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("talk-clip-02.mp4"), b"mine").unwrap();
        let info = crate::probe::probe_video(&video).unwrap();
        let keyframes = [];
        let mut placed = Vec::new();
        for (n, (in_ms, out_ms)) in [(0, 2_000), (3_000, 5_000), (6_000, 9_000)]
            .into_iter()
            .enumerate()
        {
            let job = ClipJob {
                video: &video,
                source: (info.width, info.height),
                fps: info.fps,
                in_ms,
                out_ms,
                picture: Picture::Crop {
                    keyframes: &keyframes,
                    axis: Axis::X,
                },
                ass: None,
                fonts: None,
            };
            let dest = out_dir.join(format!("talk-clip-{:02}.mp4", n + 1));
            let tmp = temp_path(&dest);
            let enc = encode(&job, &tmp, &AtomicBool::new(false), |_| {}).unwrap();
            let facts = probe_output(&tmp).unwrap();
            assert_eq!(
                check(&facts, enc, out_ms - in_ms, output_rate(info.fps), false),
                Ok(())
            );
            placed.push(place(&tmp, &dest, false).unwrap());
        }
        let mut names: Vec<String> = names_in(&out_dir).into_iter().collect();
        names.sort();
        assert_eq!(
            names,
            [
                "talk-clip-01.mp4",
                "talk-clip-02-2.mp4",
                "talk-clip-02.mp4",
                "talk-clip-03.mp4"
            ]
        );
        assert_eq!(fs::read(out_dir.join("talk-clip-02.mp4")).unwrap(), b"mine");
        assert_eq!(placed[1], out_dir.join("talk-clip-02-2.mp4"));
    }

    /// Mean brightness (0–255) of each output row of a gray 1080×1920 frame.
    fn row_means(source: &str, graph: &str) -> Vec<f64> {
        let output = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                source,
                "-filter_complex",
                graph,
            ])
            .args([
                "-map",
                "[v]",
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let (w, h) = (OUT.0 as usize, OUT.1 as usize);
        assert_eq!(output.stdout.len(), w * h);
        output
            .stdout
            .chunks(w)
            .map(|row| row.iter().map(|&v| v as f64).sum::<f64>() / w as f64)
            .collect()
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn fit_blur_puts_the_whole_frame_over_a_darker_copy() {
        let graph = video_graph(Picture::FitBlur, (1920, 1080), 0, (30, 1), None);
        let rows = row_means("color=white:size=1920x1080:duration=1", &graph);
        // A 16:9 source fits as 1080×608 in the middle (rows 656–1263).
        let middle: Vec<f64> = rows[700..1220].to_vec();
        let above: Vec<f64> = rows[0..600].to_vec();
        let below: Vec<f64> = rows[1320..].to_vec();
        assert!(
            middle.iter().all(|&v| v > 250.0),
            "front {:?}",
            &middle[..3]
        );
        // The copy behind: 70% brightness, the same above and below.
        for v in above.iter().chain(&below) {
            assert!((v - 0.7 * 255.0).abs() < 8.0, "back {v}");
        }

        // A 9:16 source fills the frame.
        let graph = video_graph(Picture::FitBlur, (1080, 1920), 0, (30, 1), None);
        let rows = row_means("color=white:size=1080x1920:duration=1", &graph);
        assert!(rows.iter().all(|&v| v > 250.0));
    }
}
