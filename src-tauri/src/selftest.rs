//! `clips --self-test <video> [--reference <txt>] [--models <dir>] [--fonts <dir>]`: a hidden
//! mode for release CI (milestone 6). It runs the app's own pipeline inside the shipped
//! binary (the bundled FFmpeg, whisper.cpp's and ONNX Runtime's release builds) on a short
//! video with a face and speech (`fixtures/selftest/speaker.mp4`) and exits non-zero on
//! any failure. It runs before Tauri starts, so it needs no window or display.
//!
//! 1. CPU check; FFmpeg must be the bundled one (next to the executable) in release builds.
//! 2. Transcribe with Whisper Base (`transcription::run`, as the app), word error rate
//!    against `--reference` at most [`MAX_WER`].
//! 3. Find moments with Custom lengths that fit the video: at least one.
//! 4. Find faces with BlazeFace (`analysis::run`) and follow the speaker (`auto_path`):
//!    faces on most samples, and the crop keeps the subject's face inside on every one.
//! 5. Export the best moment twice through Export all's own code (`batch::export_one`:
//!    captions, the release encoder, the ffprobe check with the encoder's accepted
//!    profiles, safe placing, SRT): following the face, and Fit + blur.
//! 6. `export::self_check`: a synthetic clip with a caption, checked the same way.
//!
//! The models are downloaded into `--models` (default: the system temp dir) from our
//! `models-v1` / `models-v2` mirrors if they aren't there yet, which also exercises the
//! download and checksum code. The fonts default to the app's bundled `fonts` folder.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::models::{self, WhisperModel};
use crate::moments::{FinderSettings, Platform};
use crate::{
    analysis, audio, batch, chunks, cpu, export, faces, ffmpeg, frames, moments, probe, projects,
    reframe, sentences, style, tracks, transcribe, transcription, wer,
};

/// The highest word error rate (against `--reference`) that still passes.
const MAX_WER: f64 = 0.3;
/// Of the samples, at least this share must have a face (the fixture has none for 0.5 s
/// of 12 s).
const MIN_FACE_SHARE: f64 = 0.8;
/// The finder's lengths for the 12 s fixture (Custom): whole sentences of 4–11 s.
const FINDER: (u64, u64, u64) = (4_000, 11_000, 6_000);

/// Runs the self-test if `args` (without the program name) start with `--self-test`.
pub fn run_if_requested(args: &[String]) -> Option<ExitCode> {
    if args.first().map(String::as_str) != Some("--self-test") {
        return None;
    }
    Some(match parse(&args[1..]).and_then(|options| run(&options)) {
        Ok(()) => {
            println!("self-test: ok");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("self-test: FAILED: {e}");
            ExitCode::FAILURE
        }
    })
}

#[derive(Debug, PartialEq)]
struct SelfTestOptions {
    video: PathBuf,
    reference: Option<PathBuf>,
    models_dir: PathBuf,
    fonts_dir: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<SelfTestOptions, String> {
    let usage =
        "usage: clips --self-test <video> [--reference <txt>] [--models <dir>] [--fonts <dir>]";
    let mut video = None;
    let mut reference = None;
    let mut fonts_dir = None;
    let mut models_dir = std::env::temp_dir().join("kreokit-clips-self-test-models");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reference" => reference = Some(PathBuf::from(args.next().ok_or(usage)?)),
            "--models" => models_dir = PathBuf::from(args.next().ok_or(usage)?),
            "--fonts" => fonts_dir = Some(PathBuf::from(args.next().ok_or(usage)?)),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}; {usage}")),
            _ if video.is_none() => video = Some(PathBuf::from(arg)),
            _ => return Err(usage.into()),
        }
    }
    Ok(SelfTestOptions {
        video: video.ok_or(usage)?,
        reference,
        models_dir,
        fonts_dir,
    })
}

fn run(options: &SelfTestOptions) -> Result<(), String> {
    let started = Instant::now();
    let support = cpu::check();
    if !support.supported {
        return Err(cpu::unsupported_message(&support.missing));
    }
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let ffprobe = ffmpeg::locate("ffprobe")?;
    println!("ffmpeg:   {}", ffmpeg.display());
    println!("ffprobe:  {}", ffprobe.display());
    if !cfg!(debug_assertions) {
        bundled(&ffmpeg)?;
        bundled(&ffprobe)?;
    }
    let fonts = match &options.fonts_dir {
        Some(dir) => dir.clone(),
        None => bundled_fonts()?,
    };
    if !fonts.join("Montserrat-Bold.ttf").is_file() {
        return Err(format!("no caption fonts in {}", fonts.display()));
    }
    println!("fonts:    {}", fonts.display());

    let work = std::env::temp_dir().join(format!("kreokit-clips-self-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let result = pipeline(options, &fonts, &work);
    let _ = std::fs::remove_dir_all(&work);
    println!("took {:.1?}", started.elapsed());
    result
}

/// Release builds must run the FFmpeg they bundle, not one found on the system.
fn bundled(tool: &Path) -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let overridden = ["KREOKIT_FFMPEG", "KREOKIT_FFPROBE"]
        .iter()
        .any(|var| std::env::var_os(var).is_some());
    if overridden || tool.parent() == exe_dir.as_deref() {
        Ok(())
    } else {
        Err(format!(
            "{} isn't the bundled one (next to the executable)",
            tool.display()
        ))
    }
}

/// The bundle's `fonts` folder: `Contents/Resources/fonts` in a macOS app, next to the
/// executable elsewhere (Tauri's resource folder).
fn bundled_fonts() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no executable folder")?;
    [dir.join("../Resources/fonts"), dir.join("fonts")]
        .into_iter()
        .find(|p| p.is_dir())
        .ok_or_else(|| format!("no fonts folder near {}; pass --fonts", exe.display()))
}

fn pipeline(options: &SelfTestOptions, fonts: &Path, work: &Path) -> Result<(), String> {
    let video_path = &options.video;
    let info = probe::probe_video(video_path)?;
    let (size, mtime_ms) = projects::fingerprint(video_path)?;
    println!(
        "video:    {} ({}×{}, {} s, {:.2} fps)",
        video_path.display(),
        info.width,
        info.height,
        secs(info.duration_ms),
        info.fps
    );

    // 2. Transcribe, as the Transcribe screen does.
    let step = Instant::now();
    let settings = transcription::Settings {
        model: WhisperModel::Base,
        ..transcription::Settings::default()
    };
    let model_path = download(settings.model.file(), &options.models_dir)?;
    let video = transcription::Video {
        size,
        mtime_ms,
        duration_ms: info.duration_ms,
    };
    let output = transcription::run(
        &work.join("transcript"),
        &settings,
        &video,
        &chunks::LIMITS,
        |start_ms| audio::AudioStream::open_at(video_path, start_ms),
        || transcribe::Engine::load(settings.model, &model_path),
        &Arc::new(AtomicBool::new(false)),
        |_| {},
    )?;
    let transcript = &output.transcript;
    let words: Vec<&str> = transcript.segments.iter().map(|s| s.text.trim()).collect();
    let text = words.join(" ");
    println!("language: {}", transcript.language);
    println!("text:     {text}");
    if text.trim().is_empty() {
        return Err("the transcript is empty".into());
    }
    if let Some(reference) = &options.reference {
        let reference = std::fs::read_to_string(reference)
            .map_err(|e| format!("cannot read {}: {e}", reference.display()))?;
        let wer = wer::word_error_rate(&reference, &text);
        println!("WER:      {wer:.3} (max {MAX_WER})");
        if wer > MAX_WER {
            return Err(format!("WER {wer:.3} is above {MAX_WER}"));
        }
    }
    println!("          transcribed in {:.1?}", step.elapsed());

    // 3. Moments.
    let (min_ms, max_ms, target_ms) = FINDER;
    let finder = FinderSettings {
        platform: Some(Platform::Custom),
        min_ms,
        max_ms,
        target_ms,
        count: Some(3),
    };
    let found = moments::find_in(
        transcript,
        &output.loudness,
        info.duration_ms,
        &finder,
        &[],
        0,
    );
    for m in &found {
        println!(
            "moment:   {}–{} s, score {}: {}",
            secs(m.in_ms),
            secs(m.out_ms),
            m.score,
            m.text
        );
    }
    let best = found
        .iter()
        .max_by_key(|m| m.score)
        .ok_or("the finder found no moment")?;
    if let Some(m) = found
        .iter()
        .find(|m| !(min_ms..=max_ms + 500).contains(&(m.out_ms - m.in_ms)))
    {
        return Err(format!(
            "a moment of {} s is outside {}–{} s",
            secs(m.out_ms - m.in_ms),
            secs(min_ms),
            secs(max_ms)
        ));
    }

    // 4. Faces and the crop path, as the clip editor gets them.
    let step = Instant::now();
    let face_model = download(models::FACE_MODEL, &options.models_dir)?;
    let faces_video = analysis::Video {
        path: video_path.clone(),
        width: info.width,
        height: info.height,
        duration_ms: info.duration_ms,
        size,
        mtime_ms,
    };
    let faces_dir = work.join("faces");
    analysis::run(
        &faces_dir,
        &faces_video,
        &[(0, info.duration_ms)],
        || faces::Detector::load(&face_model),
        &AtomicBool::new(false),
        |_| {},
    )?;
    let all = analysis::samples(&faces_dir, &faces_video, 0, info.duration_ms)
        .ok_or("the face analysis left no samples")?;
    let with_face = all.iter().filter(|s| !s.faces.is_empty()).count();
    println!(
        "faces:    on {with_face} of {} samples, in {:.1?}",
        all.len(),
        step.elapsed()
    );
    if all.is_empty() || (with_face as f64) < MIN_FACE_SHARE * all.len() as f64 {
        return Err(format!(
            "faces found on only {with_face} of {} samples",
            all.len()
        ));
    }
    let path = crop_path(&faces_video, &faces_dir, best.in_ms, best.out_ms)?;

    // 5. Export all, with this build's encoders.
    let text = sentences::split(transcript);
    let out_dir = work.join("out");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let batch = batch::Batch {
        video: video_path,
        info: &info,
        video_name: "self-test",
        text: &text,
        fixes: &[],
        folder: &out_dir,
        largest_number: 2,
        srt: true,
        replace: false,
        tiktok: false,
        fonts: Some(fonts),
    };
    let pictures = [
        (
            "follow face",
            batch::PictureSpec::Crop {
                axis: path.axis,
                keyframes: path.keyframes.clone(),
            },
        ),
        ("fit + blur", batch::PictureSpec::Fit),
    ];
    for (n, (name, picture)) in pictures.into_iter().enumerate() {
        let item = batch::Item {
            clip_id: n as u32 + 1,
            number: n as u32 + 1,
            in_ms: best.in_ms,
            out_ms: best.out_ms,
            picture,
            style: style::Style::default(),
        };
        let step = Instant::now();
        let (file, srt) = batch::export_one(&batch, &item, &AtomicBool::new(false), |_| {})?;
        let facts = export::probe_output(&file)?;
        if srt.is_none_or(|p| !p.is_file()) {
            return Err(format!("{name}: no SRT next to {}", file.display()));
        }
        println!(
            "export:   {name}: {} {}×{} H.264 {} {}, {} s, {} KB, SRT, in {:.1?}",
            file.file_name().unwrap_or_default().to_string_lossy(),
            facts.width,
            facts.height,
            facts.profile,
            facts.pix_fmt,
            secs(facts.container_ms),
            std::fs::metadata(&file).map_or(0, |m| m.len() / 1000),
            step.elapsed()
        );
    }

    // 6. The synthetic export check.
    println!(
        "{}",
        export::self_check(&work.join("export-check"), Some(fonts))?
    );
    Ok(())
}

/// The clip's automatic crop path (`reframe_path` in lib.rs), checked: it follows a face,
/// and every sample's subject face (the biggest) is inside the crop.
fn crop_path(
    video: &analysis::Video,
    dir: &Path,
    in_ms: u64,
    out_ms: u64,
) -> Result<reframe::Path, String> {
    let samples =
        analysis::samples(dir, video, in_ms, out_ms).ok_or("the clip's faces aren't analysed")?;
    let (w, h) = frames::sample_size(video.width, video.height);
    let frame = (w as f32, h as f32);
    let found = tracks::track(&samples);
    let subjects = tracks::subjects(&samples, &found, frame);
    let path = reframe::auto_path(&samples, &subjects, (video.width, video.height), frame);
    if path.no_face {
        return Err("the crop path found no face to follow".into());
    }
    let (crop_w, crop_h) = path.crop;
    let mut checked = 0;
    for sample in &samples {
        let Some(face) = sample
            .faces
            .iter()
            .max_by(|a, b| a.height().total_cmp(&b.height()))
        else {
            continue;
        };
        let pos = reframe::position_at(&path.keyframes, sample.t_ms);
        let (along, size, crop) = match path.axis {
            reframe::Axis::X => ((face.x0, face.x1), frame.0, crop_w),
            reframe::Axis::Y => ((face.y0, face.y1), frame.1, crop_h),
        };
        let start = (pos - crop / 2.0).clamp(0.0, 1.0 - crop);
        let (a, b) = (along.0 / size, along.1 / size);
        // A face may reach a few percent past the crop while the camera catches up.
        let slack = 0.02;
        if a < start - slack || b > start + crop + slack {
            return Err(format!(
                "at {} s the face ({a:.2}–{b:.2}) is outside the crop ({start:.2}–{:.2})",
                secs(sample.t_ms),
                start + crop
            ));
        }
        checked += 1;
    }
    println!(
        "reframe:  {} keyframes, the face inside the crop on {checked} of {} samples",
        path.keyframes.len(),
        samples.len()
    );
    Ok(path)
}

fn download(file: models::MirrorFile, models_dir: &Path) -> Result<PathBuf, String> {
    if !file.is_downloaded(models_dir) {
        println!("download: {} from {} …", file.name, file.release);
    }
    let path = models::download(file, models_dir, &AtomicBool::new(false), |_| {})?;
    println!("model:    {}", path.display());
    Ok(path)
}

fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_runs_with_the_flag() {
        assert_eq!(run_if_requested(&args(&[])), None);
        assert_eq!(run_if_requested(&args(&["clip.mp4"])), None);
    }

    #[test]
    fn parses_video_reference_models_and_fonts() {
        let parsed = parse(&args(&[
            "a.mp4",
            "--reference",
            "a.txt",
            "--models",
            "m",
            "--fonts",
            "f",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            SelfTestOptions {
                video: "a.mp4".into(),
                reference: Some("a.txt".into()),
                models_dir: "m".into(),
                fonts_dir: Some("f".into()),
            }
        );
        assert_eq!(parse(&args(&["a.mp4"])).unwrap().fonts_dir, None);
    }

    #[test]
    fn rejects_missing_video_and_unknown_flags() {
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["a.mp4", "--fast"])).is_err());
        assert!(parse(&args(&["a.mp4", "--models"])).is_err());
        assert!(parse(&args(&["a.mp4", "b.mp4"])).is_err());
    }

    /// The whole self-test on the fixture with a dev build (system FFmpeg, the source
    /// fonts). Downloads Whisper Base and BlazeFace on first run.
    /// macOS only here, like `export::self_check`'s test: CI's Linux FFmpeg 4.4 may lack
    /// OpenH264; the release workflow runs the real self-test on every platform.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs FFmpeg and the models (downloads them)"]
    fn passes_on_the_fixture() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let options = SelfTestOptions {
            video: root.join("../fixtures/selftest/speaker.mp4"),
            reference: Some(root.join("../fixtures/selftest/speaker.txt")),
            models_dir: std::env::temp_dir().join("kreokit-clips-self-test-models"),
            fonts_dir: Some(root.join("resources/fonts")),
        };
        run(&options).unwrap();
    }
}
