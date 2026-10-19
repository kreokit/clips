//! Where a long video's audio is cut into pieces for Whisper.
//!
//! Pieces are about 5 minutes long and end in the middle of the longest pause between 4:30
//! and 5:30, so no word is cut in two; without any pause they end at exactly 5:00. The cut
//! depends only on the audio from the piece's start, and the audio is always decoded from
//! the beginning, so the same video is cut at the same places on every run. That is what
//! lets a cancelled transcription resume from its finished pieces.

use crate::audio::{SAMPLE_RATE, ms_to_samples};
use crate::vad;

/// Piece lengths in milliseconds: cut in a pause between `min` and `max`, else at `target`.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub min_ms: u64,
    pub target_ms: u64,
    pub max_ms: u64,
}

pub const LIMITS: Limits = Limits {
    min_ms: 270_000,
    target_ms: 300_000,
    max_ms: 330_000,
};

impl Limits {
    /// Samples to buffer before a cut can be decided.
    pub fn max_samples(&self) -> usize {
        ms_to_samples(self.max_ms)
    }
}

/// Where the piece starting at `buffer[0]` ends, as a sample count, or `None` while more
/// audio is needed. `at_end`: the audio has no more samples after `buffer`.
pub fn next_cut(buffer: &[f32], at_end: bool, limits: &Limits) -> Option<usize> {
    let max = limits.max_samples();
    if buffer.len() <= max {
        return at_end.then_some(buffer.len());
    }
    let frame = vad::FRAME_MS as usize * SAMPLE_RATE as usize / 1000;
    let window = &buffer[..max];
    let voiced = vad::voiced_frames(window);
    let min_frame = (limits.min_ms / vad::FRAME_MS) as usize;
    let target_frame = (limits.target_ms / vad::FRAME_MS) as usize;

    // Pauses (runs of unvoiced frames) whose middle lies in [min, max]: the longest wins,
    // then the one nearest the target.
    let mut best: Option<(usize, usize)> = None; // (length, middle frame)
    let mut i = 0;
    while i < voiced.len() {
        if voiced[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < voiced.len() && !voiced[i] {
            i += 1;
        }
        let (len, middle) = (i - start, (start + i) / 2);
        if middle < min_frame {
            continue;
        }
        let better = best.is_none_or(|(best_len, best_mid)| {
            len > best_len
                || (len == best_len && middle.abs_diff(target_frame) < best_mid.abs_diff(target_frame))
        });
        if better {
            best = Some((len, middle));
        }
    }
    let cut_frame = best.map_or(target_frame, |(_, middle)| middle);
    Some((cut_frame * frame).min(max))
}

/// Whether `samples` contain anything audible. Silent pieces are not sent to Whisper,
/// which tends to invent text ("Thank you.") for silence.
pub fn has_sound(samples: &[f32]) -> bool {
    vad::voiced_frames(samples).contains(&true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::samples_to_ms;

    /// Small limits so the test signals stay short: cut between 4.5 and 5.5 s, else at 5 s.
    const SMALL: Limits = Limits {
        min_ms: 4_500,
        target_ms: 5_000,
        max_ms: 5_500,
    };

    /// `parts`: (milliseconds, amplitude) pieces of a 440 Hz tone; amplitude 0 is silence.
    fn tone(parts: &[(u64, f32)]) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(ms, amp)| {
                (0..ms_to_samples(ms)).map(move |i| {
                    amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32)
                        .sin()
                })
            })
            .collect()
    }

    /// Like recorded speech: starts with 0.7 s of silence, so the speech detector's noise
    /// floor (its quietest 10% of frames) is silence. That pause is too early to cut in.
    fn speech(parts: &[(u64, f32)]) -> Vec<f32> {
        let mut all = vec![(700, 0.0)];
        all.extend_from_slice(parts);
        tone(&all)
    }

    fn cut_ms(buffer: &[f32]) -> u64 {
        samples_to_ms(next_cut(buffer, false, &SMALL).expect("a cut"))
    }

    #[test]
    fn waits_for_enough_audio() {
        let audio = speech(&[(4_800, 0.5)]);
        assert_eq!(next_cut(&audio, false, &SMALL), None);
        // At the end, whatever is left is the last piece.
        assert_eq!(next_cut(&audio, true, &SMALL), Some(audio.len()));
        assert_eq!(next_cut(&[], true, &SMALL), Some(0));
    }

    #[test]
    fn cuts_in_the_middle_of_the_longest_pause() {
        // A longer pause at 4.6–5.0 s and a shorter one at 5.2–5.35 s: the longer one wins.
        let audio = speech(&[
            (3_900, 0.5),
            (400, 0.0), // 4.6–5.0 s
            (200, 0.5),
            (150, 0.0), // 5.2–5.35 s
            (2_000, 0.5),
        ]);
        assert_eq!(cut_ms(&audio), 4_800);
    }

    #[test]
    fn equal_pauses_prefer_the_target() {
        let audio = speech(&[
            (3_850, 0.5),
            (200, 0.0), // middle 4.65 s
            (300, 0.5),
            (200, 0.0), // middle 5.15 s, nearer 5 s
            (2_000, 0.5),
        ]);
        assert_eq!(cut_ms(&audio), 5_150);
    }

    #[test]
    fn ignores_pauses_outside_the_window() {
        let audio = speech(&[(2_300, 0.5), (1_000, 0.0), (1_700, 0.5), (800, 0.0), (500, 0.5)]);
        // 3–4 s is too early; 5.7–6.5 s starts after the window: cut at the target.
        assert_eq!(cut_ms(&audio), 5_000);
    }

    #[test]
    fn without_pauses_cuts_at_the_target() {
        assert_eq!(cut_ms(&speech(&[(5_300, 0.5)])), 5_000);
    }

    #[test]
    fn a_pause_running_past_the_window_still_counts() {
        // Silence from 5.2 s to the end of the 5.5 s window (and beyond).
        let audio = speech(&[(4_500, 0.5), (2_000, 0.0), (500, 0.5)]);
        assert_eq!(cut_ms(&audio), 5_350);
    }

    #[test]
    fn the_same_audio_is_cut_the_same_way() {
        let audio = speech(&[(4_000, 0.4), (300, 0.0), (3_000, 0.6)]);
        let first = next_cut(&audio, false, &SMALL);
        // More audio after the window doesn't change the decision.
        let mut longer = audio.clone();
        longer.extend(tone(&[(5_000, 0.9)]));
        assert_eq!(next_cut(&longer, false, &SMALL), first);
        assert_eq!(next_cut(&audio, true, &SMALL), first);
    }

    #[test]
    fn whole_video_pieces_stay_within_the_limits() {
        // 31 s of "speech" with a pause every 1.3 s, cut piece by piece like the transcriber.
        let mut parts = Vec::new();
        for _ in 0..24 {
            parts.push((1_100, 0.5));
            parts.push((200, 0.0));
        }
        let audio = tone(&parts);
        let mut start = 0;
        let mut lengths = Vec::new();
        while start < audio.len() {
            let rest = &audio[start..];
            let at_end = rest.len() <= SMALL.max_samples();
            let n = next_cut(rest, at_end, &SMALL).unwrap();
            assert!(n > 0);
            lengths.push(samples_to_ms(n));
            start += n;
        }
        assert_eq!(start, audio.len());
        let (last, full) = lengths.split_last().unwrap();
        for &ms in full {
            assert!((SMALL.min_ms..=SMALL.max_ms).contains(&ms), "{lengths:?}");
        }
        assert!(*last <= SMALL.max_ms);
    }

    #[test]
    fn real_limits_are_about_five_minutes() {
        assert_eq!(LIMITS.max_samples(), 330 * SAMPLE_RATE as usize);
        assert!(LIMITS.min_ms < LIMITS.target_ms && LIMITS.target_ms < LIMITS.max_ms);
    }

    #[test]
    fn silence_has_no_sound() {
        assert!(!has_sound(&tone(&[(2_000, 0.0)])));
        assert!(!has_sound(&[]));
        assert!(has_sound(&tone(&[(1_000, 0.0), (500, 0.5), (1_000, 0.0)])));
    }
}
