//! Audio peaks for the timeline waveform (from KreoKit Captions), read for just the part
//! of the video the timeline shows, so a long video is never decoded whole again.

use std::path::Path;

use crate::audio::{AudioStream, SAMPLE_RATE, ms_to_samples};

/// Peak levels per second of audio (one per 10 ms).
pub const PEAKS_PER_SECOND: u32 = 100;
/// Levels are drawn on a decibel scale from this floor up to 0 dBFS.
const FLOOR_DB: f32 = -48.0;
/// The longest stretch one request may read.
pub const MAX_RANGE_MS: u64 = 15 * 60_000;

/// One byte per 10 ms: the loudest sample in that slice on a 0–255 dB scale (FLOOR_DB
/// and quieter is 0, full scale is 255). A dB scale keeps quiet speech visible next to
/// loud parts.
pub fn peaks(samples: &[f32]) -> Vec<u8> {
    let bucket = (SAMPLE_RATE / PEAKS_PER_SECOND) as usize;
    samples
        .chunks(bucket)
        .map(|chunk| {
            let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let db = 20.0 * peak.max(1e-6).log10();
            ((db - FLOOR_DB) / -FLOOR_DB)
                .clamp(0.0, 1.0)
                .mul_add(255.0, 0.5) as u8
        })
        .collect()
}

/// Peaks of `video` from `from_ms` to `to_ms` (at most [`MAX_RANGE_MS`]); shorter when the
/// audio ends first.
pub fn range(video: &Path, from_ms: u64, to_ms: u64) -> Result<Vec<u8>, String> {
    let to_ms = to_ms.min(from_ms + MAX_RANGE_MS);
    let wanted = ms_to_samples(to_ms.saturating_sub(from_ms));
    let mut stream = AudioStream::open_at(video, from_ms)?;
    let mut samples = Vec::with_capacity(wanted);
    while samples.len() < wanted {
        let left = wanted - samples.len();
        if stream.read(&mut samples, left.min(1 << 18))? == 0 {
            break;
        }
    }
    samples.truncate(wanted);
    Ok(peaks(&samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_level_per_10_ms_on_a_db_scale() {
        let mut samples = vec![0.0f32; 160]; // silence
        samples.extend(vec![1.0f32; 160]); // full scale
        samples.extend(vec![0.0039f32; 160]); // about -48 dB
        samples.extend(vec![0.0631f32; 160]); // about -24 dB: halfway
        samples.extend(vec![-0.5f32; 80]); // a short last bucket, negative
        let levels = peaks(&samples);
        assert_eq!(levels.len(), 5);
        assert_eq!(levels[0], 0);
        assert_eq!(levels[1], 255);
        assert!(levels[2] <= 1, "{}", levels[2]);
        assert!((126..=130).contains(&levels[3]), "{}", levels[3]);
        assert!((205..=225).contains(&levels[4]), "{}", levels[4]);
    }

    #[test]
    fn empty_audio_has_no_peaks() {
        assert!(peaks(&[]).is_empty());
    }

    /// Needs ffmpeg: `cargo test reads_a_range -- --ignored`.
    #[test]
    #[ignore]
    fn reads_a_range_of_the_fixture() {
        let video = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en/hello.mp4");
        let levels = range(&video, 500, 2_500).unwrap();
        assert_eq!(levels.len(), 200);
        assert!(levels.iter().any(|&l| l > 100), "speech is there");
        // Past the end of the audio: what there is.
        let duration = crate::probe::probe_video(&video).unwrap().duration_ms;
        let tail = range(&video, duration - 300, duration + 5_000).unwrap();
        assert!(tail.len() <= 31, "{}", tail.len());
    }
}
