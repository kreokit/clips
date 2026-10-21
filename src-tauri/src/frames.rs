//! Sampled video frames for face detection: small RGB frames at a fixed rate, streamed
//! from FFmpeg, plus a cheap camera-cut score between consecutive samples.
//!
//! **Clock:** the same as the audio's and the player's: 0:00 is the container's start
//! (FFmpeg's `-ss` counts from there). A run from `from_ms` delivers the picture shown at
//! `from_ms + k × 250 ms` for k = 0, 1, … (`fps=4:start_time=0:round=up` after the seek:
//! the last frame at or before each sample time, i.e. the one on screen then; the first
//! one repeated if the picture starts a little late). The default `round=near` puts the
//! last frame *rounding* to a sample there, up to half a step (125 ms) late.
//!
//! **Size:** the display size (rotation applied, as FFmpeg does by default) scaled so the
//! long side is at most [`MAX_SIDE`]: enough for the face model's 192 px input and for
//! its two-tile pass (a 640×360 frame gives two 360 px squares).

// Used by the face analysis job from M4 step 8 on.
#![allow(dead_code)]

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::faces::Rgb;
use crate::ffmpeg;
use crate::models::CANCELLED;

/// Samples per second.
pub const RATE: u64 = 4;
/// Milliseconds between samples.
pub const STEP_MS: u64 = 1000 / RATE;
/// Long side of a sampled frame, at most.
pub const MAX_SIDE: u32 = 640;

/// The sampled frame size for a video displayed at `width`×`height`: long side at most
/// [`MAX_SIDE`] (never upscaled), both sides even, at least 2.
pub fn sample_size(width: u32, height: u32) -> (usize, usize) {
    let long = width.max(height).max(1);
    let scale = (MAX_SIDE as f64 / long as f64).min(1.0);
    let even = |side: u32| (((side as f64 * scale / 2.0).round() as usize) * 2).max(2);
    (even(width), even(height))
}

/// Streams the frames from `from_ms` up to (not including) `to_ms` of `video` (displayed
/// at `width`×`height`), calling `on_frame(time_ms, frame)` for each. Stops early with
/// [`CANCELLED`] when `cancel` is set, or with `on_frame`'s error.
pub fn sample(
    video: &Path,
    (width, height): (u32, u32),
    from_ms: u64,
    to_ms: u64,
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(u64, Rgb) -> Result<(), String>,
) -> Result<(), String> {
    if to_ms <= from_ms {
        return Ok(());
    }
    let (w, h) = sample_size(width, height);
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let seconds = |ms: u64| format!("{}.{:03}", ms / 1000, ms % 1000);
    let mut child = Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(["-ss", &seconds(from_ms)])
        .arg("-i")
        .arg(video)
        .args(["-t", &seconds(to_ms - from_ms)])
        .args(["-map", "0:v:0", "-an", "-sn", "-dn"])
        .args([
            "-vf",
            &format!("fps={RATE}:start_time=0:round=up,scale={w}:{h}:flags=area,format=rgb24"),
        ])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
    // Read stderr on the side so a chatty ffmpeg can never block on a full pipe.
    let mut stderr = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut buffer = vec![0u8; w * h * 3];
    let mut result = Ok(());
    for k in 0.. {
        let time_ms = from_ms + k * STEP_MS;
        if cancel.load(Ordering::Relaxed) {
            result = Err(CANCELLED.to_string());
            break;
        }
        match read_full(&mut stdout, &mut buffer) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                result = Err(format!("cannot read the video frames: {e}"));
                break;
            }
        }
        if time_ms >= to_ms {
            break;
        }
        let frame = Rgb {
            data: &buffer,
            width: w,
            height: h,
        };
        if let Err(e) = on_frame(time_ms, frame) {
            result = Err(e);
            break;
        }
    }
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = stderr.join().unwrap_or_default();
    result?;
    if !status.success() {
        return Err(format!(
            "ffmpeg could not read the video: {}",
            stderr.trim()
        ));
    }
    Ok(())
}

/// Fills `buffer` completely: `false` at a clean end of the stream (nothing read).
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 if filled == 0 => return Ok(false),
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => filled += n,
        }
    }
    Ok(true)
}

/// Bins per colour channel in [`Histogram`].
const BINS: usize = 16;

/// A frame's colour distribution: per channel, the share of pixels in each of 16 bins.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram([f32; 3 * BINS]);

impl Histogram {
    pub fn of(frame: Rgb) -> Self {
        let mut counts = [0u32; 3 * BINS];
        for px in frame.data.chunks_exact(3) {
            for c in 0..3 {
                counts[c * BINS + px[c] as usize * BINS / 256] += 1;
            }
        }
        let pixels = (frame.data.len() / 3).max(1) as f32;
        Self(counts.map(|n| n as f32 / pixels))
    }

    /// How different two frames' colours are, 0 (same) to 1 (nothing in common): half the
    /// summed bin differences, averaged over the channels. A camera cut scores high; a
    /// person moving in the same shot scores low.
    pub fn distance(&self, other: &Histogram) -> f32 {
        let sum: f32 = self
            .0
            .iter()
            .zip(&other.0)
            .map(|(a, b)| (a - b).abs())
            .sum();
        sum / 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sample_sizes_keep_the_aspect_and_never_upscale() {
        assert_eq!(sample_size(1920, 1080), (640, 360));
        assert_eq!(sample_size(1080, 1920), (360, 640));
        assert_eq!(sample_size(3840, 2160), (640, 360));
        assert_eq!(sample_size(1440, 1080), (640, 480));
        assert_eq!(sample_size(320, 180), (320, 180));
        assert_eq!(sample_size(321, 181), (322, 182));
        assert_eq!(sample_size(1, 1), (2, 2));
    }

    fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
        [r, g, b].repeat(64 * 36)
    }

    fn rgb(data: &[u8]) -> Rgb<'_> {
        Rgb {
            data,
            width: 64,
            height: 36,
        }
    }

    #[test]
    fn histogram_distance_is_zero_to_one() {
        let red = solid(250, 10, 10);
        let blue = solid(10, 10, 250);
        let h = |d: &[u8]| Histogram::of(rgb(d));
        assert_eq!(h(&red).distance(&h(&red)), 0.0);
        // Red and blue differ in two channels of three.
        assert!((h(&red).distance(&h(&blue)) - 2.0 / 3.0).abs() < 1e-6);
        let mut moved = red.clone();
        moved[..64 * 3 * 4].copy_from_slice(&[10, 10, 250].repeat(64 * 4)); // 4 of 36 rows
        assert!(h(&red).distance(&h(&moved)) < 0.1);
    }

    #[test]
    fn reads_whole_frames_only() {
        let mut buffer = [0u8; 4];
        assert!(read_full(&mut &[1u8, 2, 3, 4, 5][..], &mut buffer).unwrap());
        assert!(!read_full(&mut &[][..], &mut buffer).unwrap());
        assert!(read_full(&mut &[1u8, 2][..], &mut buffer).is_err());
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("kreokit-clips-frames-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Makes a test video with FFmpeg's lavfi sources (FFmpeg 4.4 compatible).
    fn make_video(path: &Path, filter: &str, seconds: f64) {
        let status = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", filter])
            .args(["-t", &seconds.to_string(), "-pix_fmt", "yuv420p"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The leftmost column with a bright pixel in row `y`.
    fn bright_x(frame: Rgb, y: usize) -> Option<usize> {
        (0..frame.width).find(|&x| frame.data[(y * frame.width + x) * 3] > 200)
    }

    /// A white square moving right at 50 px/s over grey, 25 fps: each sample shows it where
    /// its time says, from a seek too.
    #[test]
    #[ignore]
    fn samples_are_on_the_video_clock() {
        let video = temp("moving-square.mp4");
        make_video(
            &video,
            "color=c=gray:s=320x180:r=25:d=6[bg];color=c=white:s=16x16:r=25:d=6[sq];\
             [bg][sq]overlay=x='t*50':y=82:shortest=1",
            6.0,
        );
        let cancel = AtomicBool::new(false);
        for (from, to) in [(0, 2000), (1000, 3000), (2350, 4000)] {
            let mut seen = Vec::new();
            sample(&video, (320, 180), from, to, &cancel, |t, frame| {
                assert_eq!((frame.width, frame.height), (320, 180));
                seen.push((t, bright_x(frame, 90)));
                Ok(())
            })
            .unwrap();
            let times: Vec<u64> = seen.iter().map(|&(t, _)| t).collect();
            let expected: Vec<u64> = (from..to).step_by(STEP_MS as usize).collect();
            assert_eq!(times, expected, "from {from}");
            // The frame on screen at t: from t − 40 ms (one source frame, 2 px) to t.
            for (t, x) in seen {
                let want = t as f64 * 0.05;
                let got = x.unwrap() as f64;
                assert!(
                    got <= want + 0.5 && got >= want - 2.5,
                    "at {t} ms: x {got}, want {want}"
                );
            }
        }
    }

    /// A picture that starts 0.5 s after the sound: samples stay on the container's clock
    /// (the player's and the transcript's), the first picture held until it starts.
    #[test]
    #[ignore]
    fn a_late_picture_keeps_the_container_clock() {
        let video = temp("late-picture.mp4");
        let status = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=4",
            ])
            .args(["-itsoffset", "0.5", "-f", "lavfi", "-i"])
            .arg(
                "color=c=gray:s=320x180:r=25:d=3[bg];color=c=white:s=16x16:r=25:d=3[sq];\
                 [bg][sq]overlay=x='t*50':y=82:shortest=1",
            )
            .args([
                "-map", "0:a", "-map", "1:v", "-pix_fmt", "yuv420p", "-c:a", "aac",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let info = crate::probe::probe_video(&video).unwrap();
        assert_eq!(info.audio_start_ms, 0, "the sound starts first");
        let mut seen = Vec::new();
        sample(
            &video,
            (320, 180),
            0,
            2000,
            &AtomicBool::new(false),
            |t, frame| {
                seen.push((t, bright_x(frame, 90).unwrap()));
                Ok(())
            },
        )
        .unwrap();
        for (t, x) in seen {
            let want = (t as f64 - 500.0).max(0.0) * 0.05;
            assert!(
                (x as f64) <= want + 0.5 && (x as f64) >= want - 2.5,
                "at {t} ms: x {x}, want {want}"
            );
        }
    }

    /// A hard cut from red to blue at 2 s scores high there and nowhere else.
    #[test]
    #[ignore]
    fn a_camera_cut_scores_high() {
        let video = temp("cut.mp4");
        make_video(
            &video,
            "color=c=red:s=160x90:r=25:d=2[a];color=c=blue:s=160x90:r=25:d=2[b];[a][b]concat=n=2:v=1",
            4.0,
        );
        let mut previous: Option<Histogram> = None;
        let mut scores = Vec::new();
        sample(
            &video,
            (160, 90),
            0,
            4000,
            &AtomicBool::new(false),
            |t, frame| {
                let h = Histogram::of(frame);
                if let Some(p) = &previous {
                    scores.push((t, p.distance(&h)));
                }
                previous = Some(h);
                Ok(())
            },
        )
        .unwrap();
        let cuts: Vec<u64> = scores
            .iter()
            .filter(|(_, d)| *d > 0.3)
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(cuts, [2000], "{scores:?}");
    }

    #[test]
    #[ignore]
    fn cancel_stops_ffmpeg() {
        let video = temp("long-gray.mp4");
        make_video(&video, "color=c=gray:s=160x90:r=25", 20.0);
        let cancel = AtomicBool::new(false);
        let mut count = 0;
        let result = sample(&video, (160, 90), 0, 20_000, &cancel, |_, _| {
            count += 1;
            if count == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        });
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert_eq!(count, 3);
    }
}
