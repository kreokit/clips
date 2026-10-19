//! A simple energy-based speech detector, used to refine word timings at pauses.
//!
//! Audio is split into 10 ms frames; a frame counts as voiced when it is louder than the
//! recording's noise floor by a margin. Short dips inside words and short blips in silence
//! are smoothed out, so only real pauses produce silence↔speech transitions.

use crate::audio::SAMPLE_RATE;

pub const FRAME_MS: u64 = 10;
const FRAME_SAMPLES: usize = (SAMPLE_RATE as u64 * FRAME_MS / 1000) as usize;

/// Voiced frames must be this much louder than the noise floor…
const MARGIN_DB: f32 = 12.0;
/// …and at least this loud, so digital silence or hiss never counts.
const MIN_VOICED_DB: f32 = -50.0;
/// Gaps shorter than this are dips inside speech (plosives, short stops), not pauses.
const MIN_PAUSE_MS: u64 = 100;
/// Sounds shorter than this are clicks, not speech.
const MIN_SOUND_MS: u64 = 30;

/// One entry per 10 ms frame: is someone (or something) audible?
pub fn voiced_frames(samples: &[f32]) -> Vec<bool> {
    let levels: Vec<f32> = samples.chunks(FRAME_SAMPLES).map(rms_db).collect();
    if levels.is_empty() {
        return Vec::new();
    }

    // Noise floor: the 10th percentile frame level.
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let threshold = (floor + MARGIN_DB).max(MIN_VOICED_DB);

    let mut voiced: Vec<bool> = levels.iter().map(|&db| db > threshold).collect();
    fill_runs(&mut voiced, false, (MIN_PAUSE_MS / FRAME_MS) as usize);
    fill_runs(&mut voiced, true, (MIN_SOUND_MS / FRAME_MS) as usize);
    voiced
}

/// Times (ms) where speech starts after a pause.
pub fn onsets(voiced: &[bool]) -> Vec<u64> {
    transitions(voiced, false)
}

/// Times (ms) where speech stops before a pause.
pub fn offsets(voiced: &[bool]) -> Vec<u64> {
    transitions(voiced, true)
}

fn transitions(voiced: &[bool], from: bool) -> Vec<u64> {
    voiced
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == from && w[1] != from)
        .map(|(i, _)| (i as u64 + 1) * FRAME_MS)
        .collect()
}

fn rms_db(frame: &[f32]) -> f32 {
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    10.0 * mean_square.max(1e-12).log10()
}

/// Flips interior runs of `value` shorter than `min_len` frames to `!value`.
/// Runs touching either end of the audio are left alone.
fn fill_runs(frames: &mut [bool], value: bool, min_len: usize) {
    let mut i = 0;
    while i < frames.len() {
        if frames[i] != value {
            i += 1;
            continue;
        }
        let start = i;
        while i < frames.len() && frames[i] == value {
            i += 1;
        }
        if start > 0 && i < frames.len() && i - start < min_len {
            frames[start..i].fill(!value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parts`: (milliseconds, amplitude) pieces of a 440 Hz tone.
    fn tone(parts: &[(u64, f32)]) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(ms, amp)| {
                let n = (SAMPLE_RATE as u64 * ms / 1000) as usize;
                (0..n).map(move |i| {
                    amp * (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin()
                })
            })
            .collect()
    }

    #[test]
    fn finds_speech_between_silences() {
        let voiced = voiced_frames(&tone(&[(500, 0.0), (800, 0.3), (400, 0.0), (300, 0.3)]));
        assert_eq!(onsets(&voiced), [500, 1700]);
        assert_eq!(offsets(&voiced), [1300]);
    }

    #[test]
    fn ignores_short_dips_and_clicks() {
        // A 50 ms dip inside speech and a 20 ms click in silence.
        let voiced = voiced_frames(&tone(&[
            (300, 0.0),
            (400, 0.3),
            (50, 0.0),
            (400, 0.3),
            (300, 0.0),
            (20, 0.3),
            (300, 0.0),
        ]));
        assert_eq!(onsets(&voiced), [300]);
        assert_eq!(offsets(&voiced), [1150]);
    }

    #[test]
    fn hiss_below_the_floor_margin_is_silence() {
        let voiced = voiced_frames(&tone(&[(500, 0.002), (500, 0.3), (500, 0.002)]));
        assert_eq!(onsets(&voiced), [500]);
        assert_eq!(offsets(&voiced), [1000]);
    }

    #[test]
    fn continuous_sound_has_no_transitions() {
        let voiced = voiced_frames(&tone(&[(2000, 0.3)]));
        assert!(onsets(&voiced).is_empty() && offsets(&voiced).is_empty());
    }
}
