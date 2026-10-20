//! Audio for Whisper: 16 kHz mono 32-bit float samples, streamed from ffmpeg.
//!
//! A long video is never decoded into memory as a whole (an hour is ~230 MB of samples)
//! and no WAV file is written: ffmpeg writes raw `f32le` samples to a pipe, and the
//! transcriber reads them chunk by chunk. While reading, [`Loudness`] keeps one level per
//! second for the moment finder's "energy" score.

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::JoinHandle;

use crate::ffmpeg;

/// Whisper expects 16 kHz mono audio.
pub const SAMPLE_RATE: u32 = 16_000;

/// Milliseconds → sample index (and back) at [`SAMPLE_RATE`].
pub fn ms_to_samples(ms: u64) -> usize {
    (ms * SAMPLE_RATE as u64 / 1000) as usize
}

pub fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / SAMPLE_RATE as u64
}

/// A running ffmpeg that decodes a video's audio. Dropping it stops ffmpeg.
pub struct AudioStream {
    child: Child,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<String>>,
    /// Bytes of a sample split across two reads.
    pending: Vec<u8>,
    finished: bool,
}

impl AudioStream {
    #[cfg(test)]
    pub fn open(video: &Path) -> Result<Self, String> {
        Self::open_at(video, 0)
    }

    /// Starts `start_ms` into the audio (ffmpeg seeks, then trims to the exact time).
    pub fn open_at(video: &Path, start_ms: u64) -> Result<Self, String> {
        let ffmpeg = ffmpeg::locate("ffmpeg")?;
        let mut command = Command::new(&ffmpeg);
        command.args(["-nostdin", "-v", "error"]);
        if start_ms > 0 {
            command.args([
                "-ss",
                &format!("{}.{:03}", start_ms / 1000, start_ms % 1000),
            ]);
        }
        let mut child = command
            .arg("-i")
            .arg(video)
            .args(["-vn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string()])
            .args(["-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
        let stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        // Read stderr on the side so a chatty ffmpeg can never block on a full pipe.
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Self {
            child,
            stdout,
            stderr: Some(stderr),
            pending: Vec::new(),
            finished: false,
        })
    }

    /// Appends up to `max` samples to `out`; returns how many. 0 means the audio ended
    /// (and ffmpeg succeeded; a decoding error is returned instead).
    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, String> {
        if self.finished || max == 0 {
            return Ok(0);
        }
        let mut bytes = std::mem::take(&mut self.pending);
        let want = max * 4;
        let start = bytes.len();
        bytes.resize(want, 0);
        let mut filled = start;
        while filled < want {
            match self.stdout.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("cannot read the audio: {e}")),
            }
        }
        bytes.truncate(filled);
        let whole = filled / 4 * 4;
        self.pending = bytes.split_off(whole);
        let before = out.len();
        out.extend(samples_from_f32le(&bytes));
        let n = out.len() - before;
        if n == 0 {
            self.finished = true;
            self.finish()?;
        }
        Ok(n)
    }

    /// Waits for ffmpeg and turns a failure into a readable error.
    fn finish(&mut self) -> Result<(), String> {
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        if status.success() {
            return Ok(());
        }
        if stderr.contains("does not contain any stream") {
            return Err("this video has no audio track".into());
        }
        Err(format!(
            "ffmpeg could not read the audio: {}",
            stderr.trim()
        ))
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Converts little-endian `f32` bytes to samples. A trailing partial sample is dropped.
fn samples_from_f32le(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks.iter().map(|&b| f32::from_le_bytes(b)).collect()
}

/// Loudness per second of audio (RMS in dBFS), fed sample by sample as the audio streams.
#[derive(Debug, Default)]
pub struct Loudness {
    levels: Vec<f32>,
    sum_squares: f64,
    count: usize,
}

/// The level of digital silence (RMS is clamped here).
pub const SILENCE_DB: f32 = -100.0;

impl Loudness {
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.sum_squares += (s as f64) * (s as f64);
            self.count += 1;
            if self.count == SAMPLE_RATE as usize {
                self.close_second();
            }
        }
    }

    fn close_second(&mut self) {
        let mean = self.sum_squares / self.count as f64;
        let db = (10.0 * mean.max(1e-10).log10()) as f32;
        // One decimal is plenty and keeps loudness.json small.
        self.levels.push((db.max(SILENCE_DB) * 10.0).round() / 10.0);
        self.sum_squares = 0.0;
        self.count = 0;
    }

    /// One level per second; a last partial second counts if it is at least 0.1 s long.
    pub fn finish(mut self) -> Vec<f32> {
        if self.count >= SAMPLE_RATE as usize / 10 {
            self.close_second();
        }
        self.levels
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// All of a video's audio at once (tests only; the app streams it).
    pub(crate) fn read_all(video: &Path) -> Vec<f32> {
        let mut stream = AudioStream::open(video).unwrap();
        let mut samples = Vec::new();
        while stream.read(&mut samples, 1 << 20).unwrap() > 0 {}
        samples
    }

    #[test]
    fn decodes_f32le_samples() {
        let bytes: Vec<u8> = [0.5f32, -1.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .chain([0x00, 0x01]) // partial trailing sample
            .collect();
        assert_eq!(samples_from_f32le(&bytes), vec![0.5, -1.0]);
    }

    #[test]
    fn converts_between_ms_and_samples() {
        assert_eq!(ms_to_samples(1500), 24_000);
        assert_eq!(samples_to_ms(24_000), 1500);
        assert_eq!(samples_to_ms(15), 0);
    }

    #[test]
    fn loudness_is_one_level_per_second() {
        let sr = SAMPLE_RATE as usize;
        let mut loudness = Loudness::default();
        // Fed in uneven pieces: a full-scale square wave (0 dB), then 1.5 s at 0.1 (-20 dB).
        let loud = vec![1.0f32; sr];
        loudness.push(&loud[..1234]);
        loudness.push(&loud[1234..]);
        loudness.push(&vec![0.1f32; sr * 3 / 2]);
        assert_eq!(loudness.finish(), vec![0.0, -20.0, -20.0]);

        let mut quiet = Loudness::default();
        quiet.push(&vec![0.0f32; sr]);
        quiet.push(&vec![0.5f32; sr / 20]); // under 0.1 s: dropped
        assert_eq!(quiet.finish(), vec![SILENCE_DB]);
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/en")
            .join(name)
    }

    /// Needs FFmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn streams_the_whole_audio_in_pieces() {
        let mut stream = AudioStream::open(&fixture("hello.mp4")).unwrap();
        let mut samples = Vec::new();
        // An odd piece size, so pieces never line up with ffmpeg's writes.
        while stream.read(&mut samples, 12_345).unwrap() > 0 {}
        assert_eq!(stream.read(&mut samples, 10).unwrap(), 0);
        let seconds = samples.len() as f64 / SAMPLE_RATE as f64;
        let probed = crate::probe::probe_video(&fixture("hello.mp4"))
            .unwrap()
            .duration_ms as f64
            / 1000.0;
        assert!((seconds - probed).abs() < 0.1, "{seconds} s vs {probed} s");
        assert!(
            samples.iter().any(|s| s.abs() > 0.05),
            "audio is not silent"
        );
    }

    /// Needs FFmpeg.
    #[test]
    #[ignore]
    fn starts_at_a_time() {
        let all = read_all(&fixture("hello.mp4"));
        let mut stream = AudioStream::open_at(&fixture("hello.mp4"), 2_500).unwrap();
        let mut late = Vec::new();
        while stream.read(&mut late, 1 << 20).unwrap() > 0 {}
        let skipped = all.len() - late.len();
        assert!(
            skipped.abs_diff(ms_to_samples(2_500)) <= ms_to_samples(5),
            "{skipped}"
        );
        // The same samples, give or take a few at the start.
        let diff: f32 = late[1000..2000]
            .iter()
            .zip(&all[skipped + 1000..skipped + 2000])
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff < 1.0, "{diff}");
    }

    #[test]
    #[ignore]
    fn stopping_early_ends_ffmpeg() {
        let mut stream = AudioStream::open(&fixture("hello.mp4")).unwrap();
        let mut samples = Vec::new();
        assert_eq!(stream.read(&mut samples, 1000).unwrap(), 1000);
        drop(stream); // must not hang
    }

    #[test]
    #[ignore]
    fn reports_a_file_that_is_not_a_video() {
        let mut stream = AudioStream::open(&fixture("hello.txt")).unwrap();
        let err = stream.read(&mut Vec::new(), 1000).unwrap_err();
        assert!(err.starts_with("ffmpeg could not read the audio"), "{err}");
    }
}
