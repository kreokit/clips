//! Faces over time, and who the crop follows (pure; no video or model here).
//!
//! 1. **Shots:** samples are split at camera cuts (`frames::Look::is_cut_from`); nothing
//!    is linked across a cut.
//! 2. **Tracks:** within a shot, each sample's faces are linked to the tracks seen in the
//!    last [`MAX_GAP_MS`] (overlap or nearby centre, best pairs first); the rest start new
//!    tracks. A track bridges gaps up to [`MAX_GAP_MS`] by interpolation (the detector
//!    misses faint or turned faces now and then).
//! 3. **Subject** (decided 2026-10-21): the biggest face, a little more if central, among
//!    tracks seen at least [`MIN_SAMPLES`] times. It only changes when another face has
//!    been [`SWITCH_MARGIN`]× better for [`SWITCH_HOLD_MS`]; the analysis is offline, so
//!    the switch is moved back to when that face started being better. At a cut the
//!    choice starts over. Speaker changes are cuts, not pans (`reframe.rs`).

// Used by the face analysis job and the crop path from M4 step 7 on.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::faces::{Face, iou};

/// Gaps a track bridges (and the longest a face may be unseen and still be linked).
pub const MAX_GAP_MS: u64 = 1000;
/// A track must be seen this often to become the subject (no one-off false detections).
pub const MIN_SAMPLES: usize = 3;
/// How much better (score ratio) another face must be to take over…
pub const SWITCH_MARGIN: f32 = 1.15;
/// …and for how long.
pub const SWITCH_HOLD_MS: u64 = 2000;
/// Scores are averaged over this window either side, so size noise doesn't flip them.
/// (±1 s shortened a real 3 s lead to under the hold time.)
const SMOOTH_MS: u64 = 500;

/// One sampled frame's faces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub t_ms: u64,
    pub faces: Vec<Face>,
    /// A camera cut between the previous sample and this one.
    pub cut: bool,
}

/// One face followed through a shot: the samples (indices into the input) where it was
/// seen, and its face there.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: usize,
    pub seen: Vec<(usize, Face)>,
}

impl Track {
    fn last(&self) -> &(usize, Face) {
        self.seen.last().unwrap()
    }

    /// The face at sample `i`: seen there, or interpolated between the samples either side
    /// if they are at most [`MAX_GAP_MS`] apart. `None` outside the track or in a long gap.
    pub fn face_at(&self, samples: &[Sample], i: usize) -> Option<(Face, bool)> {
        let pos = self.seen.partition_point(|&(j, _)| j < i);
        if let Some(&(j, face)) = self.seen.get(pos)
            && j == i
        {
            return Some((face, true));
        }
        let (&(a, fa), &(b, fb)) = (self.seen.get(pos.checked_sub(1)?)?, self.seen.get(pos)?);
        let (ta, tb, t) = (samples[a].t_ms, samples[b].t_ms, samples[i].t_ms);
        if tb - ta > MAX_GAP_MS {
            return None;
        }
        let k = (t - ta) as f32 / (tb - ta) as f32;
        let mix = |x: f32, y: f32| x + (y - x) * k;
        let mut face = fa;
        face.score = mix(fa.score, fb.score);
        face.x0 = mix(fa.x0, fb.x0);
        face.y0 = mix(fa.y0, fb.y0);
        face.x1 = mix(fa.x1, fb.x1);
        face.y1 = mix(fa.y1, fb.y1);
        for (p, q) in face.keypoints.iter_mut().zip(fb.keypoints) {
            p[0] = mix(p[0], q[0]);
            p[1] = mix(p[1], q[1]);
        }
        Some((face, false))
    }
}

/// How alike a face last seen `gap_ms` ago and a new detection are (0: not the same
/// person). A face may move about its width per second unseen.
fn similarity(a: &Face, b: &Face, gap_ms: u64) -> f32 {
    let overlap = iou(a, b);
    let [ax, ay] = a.center();
    let [bx, by] = b.center();
    let reach = a.width().max(b.width()).max(1.0) * (1.0 + gap_ms as f32 / 1000.0);
    let near = 1.0 - ((ax - bx).hypot(ay - by) / reach);
    overlap.max(near * 0.5).max(0.0)
}

/// The start of each shot (sample indices); the first is 0.
pub fn shots(samples: &[Sample]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend((1..samples.len()).filter(|&i| samples[i].cut));
    starts
}

/// Links the faces into tracks, shot by shot. Track ids count up from 0 in order of
/// appearance.
pub fn track(samples: &[Sample]) -> Vec<Track> {
    let mut done: Vec<Track> = Vec::new();
    let mut open: Vec<Track> = Vec::new();
    let mut next_id = 0;
    for (i, sample) in samples.iter().enumerate() {
        if sample.cut {
            done.append(&mut open);
        }
        // Tracks unseen for longer than the gap are finished.
        let (stale, fresh): (Vec<Track>, Vec<Track>) = open
            .into_iter()
            .partition(|t| sample.t_ms - samples[t.last().0].t_ms > MAX_GAP_MS);
        done.extend(stale);
        open = fresh;

        // Best pairs first.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (ti, t) in open.iter().enumerate() {
            for (fi, f) in sample.faces.iter().enumerate() {
                let gap = sample.t_ms - samples[t.last().0].t_ms;
                let s = similarity(&t.last().1, f, gap);
                if s > 0.1 {
                    pairs.push((s, ti, fi));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut track_used = vec![false; open.len()];
        let mut face_used = vec![false; sample.faces.len()];
        for (_, ti, fi) in pairs {
            if !track_used[ti] && !face_used[fi] {
                track_used[ti] = true;
                face_used[fi] = true;
                open[ti].seen.push((i, sample.faces[fi]));
            }
        }
        for (fi, f) in sample.faces.iter().enumerate() {
            if !face_used[fi] {
                open.push(Track {
                    id: next_id,
                    seen: vec![(i, *f)],
                });
                next_id += 1;
            }
        }
    }
    done.append(&mut open);
    done.sort_by_key(|t| t.id);
    done
}

/// The face the crop follows at one sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub track: usize,
    pub face: Face,
    /// Detected in this sample (else bridged between two detections).
    pub seen: bool,
}

/// How much a face is worth following: its height as a share of the frame's, up to 25%
/// more when centred.
fn worth(face: &Face, frame_width: f32, frame_height: f32) -> f32 {
    let off_center = ((face.center()[0] / frame_width) - 0.5).abs() * 2.0; // 0 centre, 1 edge
    face.height() / frame_height * (1.0 + 0.25 * (1.0 - off_center.min(1.0)))
}

/// Who the crop follows at each sample (`None`: no face to follow there).
pub fn subjects(samples: &[Sample], tracks: &[Track], frame: (f32, f32)) -> Vec<Option<Subject>> {
    let mut out = vec![None; samples.len()];
    let starts = shots(samples);
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(samples.len());
        choose_in_shot(samples, tracks, frame, start..end, &mut out);
    }
    out
}

fn choose_in_shot(
    samples: &[Sample],
    tracks: &[Track],
    (fw, fh): (f32, f32),
    range: std::ops::Range<usize>,
    out: &mut [Option<Subject>],
) {
    let eligible: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.seen.len() >= MIN_SAMPLES && range.contains(&t.seen[0].0))
        .collect();
    if eligible.is_empty() {
        return;
    }
    // faces[c][i - start]: candidate c's face at sample i, if present.
    let faces: Vec<Vec<Option<(Face, bool)>>> = eligible
        .iter()
        .map(|t| range.clone().map(|i| t.face_at(samples, i)).collect())
        .collect();
    // Smoothed worth (0 where absent).
    let raw: Vec<Vec<f32>> = faces
        .iter()
        .map(|fs| {
            fs.iter()
                .map(|f| f.map_or(0.0, |(f, _)| worth(&f, fw, fh)))
                .collect()
        })
        .collect();
    let len = range.len();
    let score: Vec<Vec<f32>> = raw
        .iter()
        .zip(&faces)
        .map(|(r, fs)| {
            (0..len)
                .map(|i| {
                    if fs[i].is_none() {
                        return 0.0;
                    }
                    let t = samples[range.start + i].t_ms;
                    let window: Vec<f32> = (0..len)
                        .filter(|&j| {
                            fs[j].is_some()
                                && samples[range.start + j].t_ms.abs_diff(t) <= SMOOTH_MS
                        })
                        .map(|j| r[j])
                        .collect();
                    window.iter().sum::<f32>() / window.len() as f32
                })
                .collect()
        })
        .collect();
    let best_at = |i: usize, except: Option<usize>| -> Option<usize> {
        (0..eligible.len())
            .filter(|&c| Some(c) != except && score[c][i] > 0.0)
            .max_by(|&a, &b| score[a][i].total_cmp(&score[b][i]))
    };

    let mut choice: Vec<Option<usize>> = vec![None; len];
    let mut current = best_at(0, None);
    // A challenger's run: (candidate, first sample where it was better).
    let mut run: Option<(usize, usize)> = None;
    for i in 0..len {
        match current {
            Some(c) if score[c][i] > 0.0 => {
                let challenger =
                    best_at(i, Some(c)).filter(|&o| score[o][i] > score[c][i] * SWITCH_MARGIN);
                run = match (challenger, run) {
                    (Some(o), Some((r, from))) if o == r => Some((r, from)),
                    (Some(o), _) => Some((o, i)),
                    (None, _) => None,
                };
                if let Some((o, from)) = run {
                    let held = samples[range.start + i].t_ms - samples[range.start + from].t_ms;
                    if held >= SWITCH_HOLD_MS {
                        choice[from..i].iter_mut().for_each(|x| *x = Some(o));
                        current = Some(o);
                        run = None;
                    }
                }
            }
            // The subject is gone: the best face present, if any.
            _ => {
                current = best_at(i, None);
                run = None;
            }
        }
        choice[i] = current.filter(|&c| score[c][i] > 0.0);
    }
    for (i, c) in choice.into_iter().enumerate() {
        out[range.start + i] = c.and_then(|c| {
            faces[c][i].map(|(face, seen)| Subject {
                track: eligible[c].id,
                face,
                seen,
            })
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const FRAME: (f32, f32) = (640.0, 360.0);

    /// A face with its centre at (x, y) and the given height (square box).
    fn face(x: f32, y: f32, h: f32) -> Face {
        Face {
            score: 0.9,
            x0: x - h / 2.0,
            y0: y - h / 2.0,
            x1: x + h / 2.0,
            y1: y + h / 2.0,
            keypoints: [[x, y]; 6],
        }
    }

    /// Samples every 250 ms from `faces_at(t_ms)`.
    fn samples(seconds: f32, faces_at: impl Fn(u64) -> Vec<Face>) -> Vec<Sample> {
        (0..(seconds * 4.0) as u64)
            .map(|k| Sample {
                t_ms: k * 250,
                faces: faces_at(k * 250),
                cut: false,
            })
            .collect()
    }

    fn analyse(s: &[Sample]) -> (Vec<Track>, Vec<Option<Subject>>) {
        let tracks = track(s);
        let subjects = subjects(s, &tracks, FRAME);
        (tracks, subjects)
    }

    fn who(subjects: &[Option<Subject>]) -> Vec<Option<usize>> {
        subjects.iter().map(|s| s.map(|s| s.track)).collect()
    }

    #[test]
    fn a_moving_face_is_one_track() {
        let s = samples(5.0, |t| vec![face(200.0 + t as f32 * 0.04, 150.0, 80.0)]);
        let (tracks, subjects) = analyse(&s);
        assert_eq!(tracks.len(), 1);
        assert!(
            subjects
                .iter()
                .all(|s| s.is_some_and(|s| s.track == 0 && s.seen))
        );
    }

    #[test]
    fn short_misses_are_bridged() {
        // Missing at 1000 and 1250 ms: bridged halfway between 750 and 1500.
        let s = samples(3.0, |t| {
            if (1000..=1250).contains(&t) {
                vec![]
            } else {
                vec![face(100.0 + t as f32 * 0.1, 150.0, 80.0)]
            }
        });
        let (tracks, subjects) = analyse(&s);
        assert_eq!(tracks.len(), 1);
        let at_1000 = subjects[4].unwrap();
        assert!(!at_1000.seen);
        assert!((at_1000.face.center()[0] - 200.0).abs() < 1e-3);
    }

    #[test]
    fn a_long_gap_starts_a_new_track_and_leaves_no_subject() {
        let s = samples(5.0, |t| {
            if (1000..2500).contains(&t) {
                vec![]
            } else {
                vec![face(300.0, 150.0, 80.0)]
            }
        });
        let (tracks, subjects) = analyse(&s);
        assert_eq!(tracks.len(), 2);
        assert!(
            subjects[4..10].iter().all(Option::is_none),
            "{:?}",
            who(&subjects)
        );
        assert_eq!(subjects[10].unwrap().track, 1);
    }

    #[test]
    fn one_off_detections_are_never_followed() {
        // A big false face for one sample next to the real one.
        let s = samples(3.0, |t| {
            let mut f = vec![face(200.0, 150.0, 60.0)];
            if t == 1000 {
                f.push(face(500.0, 150.0, 150.0));
            }
            f
        });
        let (tracks, subjects) = analyse(&s);
        assert_eq!(tracks.len(), 2);
        assert!(subjects.iter().all(|s| s.unwrap().track == 0));
    }

    #[test]
    fn a_short_lead_does_not_switch_but_a_long_one_does_from_its_start() {
        // A (left, 100 px) and B (right, 80 px); B grows to 130 px for 1 s, later for 3 s.
        let b_size = |t: u64| {
            if (2000..3000).contains(&t) || (5000..8000).contains(&t) {
                130.0
            } else {
                80.0
            }
        };
        let s = samples(12.0, |t| {
            vec![face(160.0, 150.0, 100.0), face(480.0, 150.0, b_size(t))]
        });
        let (_, subjects) = analyse(&s);
        let who = who(&subjects);
        // Scores are smoothed over ±0.5 s, so B's lead is a little shorter than its growth:
        // under 2 s around 2–3 s, about 2.5 s from just after 5 s.
        assert!(who[..20].iter().all(|&w| w == Some(0)), "{who:?}");
        let switch = who.iter().position(|&w| w == Some(1)).unwrap();
        assert!(
            (20..=22).contains(&switch),
            "switched at sample {switch}: {who:?}"
        );
        assert!(who[22..32].iter().all(|&w| w == Some(1)), "{who:?}");
        // Back to A when A has led for 2 s again (100 vs 80 px after 8 s: 1.25×), moved
        // back to when its lead began.
        let back = who.iter().rposition(|&w| w == Some(1)).unwrap() + 1;
        assert!(
            (31..=34).contains(&back),
            "back to A at sample {back}: {who:?}"
        );
        assert!(who[back..].iter().all(|&w| w == Some(0)), "{who:?}");
    }

    #[test]
    fn a_cut_starts_over() {
        // Before the cut A is bigger; after it, B (no 2 s wait), and no track crosses it.
        let mut s = samples(6.0, |t| {
            if t < 3000 {
                vec![face(160.0, 150.0, 120.0), face(480.0, 150.0, 80.0)]
            } else {
                vec![face(160.0, 150.0, 70.0), face(480.0, 150.0, 120.0)]
            }
        });
        s[12].cut = true;
        let (tracks, subjects) = analyse(&s);
        assert_eq!(tracks.len(), 4);
        assert!(
            tracks
                .iter()
                .all(|t| t.seen.iter().all(|&(i, _)| (i < 12) == (t.seen[0].0 < 12)))
        );
        let who = who(&subjects);
        assert!(who[..12].iter().all(|&w| w == Some(0)), "{who:?}");
        assert!(who[12..].iter().all(|&w| w == Some(3)), "{who:?}");
    }

    #[test]
    fn a_lost_subject_hands_over_at_once() {
        // A leaves at 2 s for good; B (smaller) takes over without waiting.
        let s = samples(5.0, |t| {
            let mut f = vec![face(480.0, 150.0, 70.0)];
            if t < 2000 {
                f.push(face(160.0, 150.0, 120.0));
            }
            f
        });
        let (_, subjects) = analyse(&s);
        let who = who(&subjects);
        assert_eq!(who[7], Some(1));
        assert!(who[8..].iter().all(|&w| w == Some(0)), "{who:?}");
    }

    #[test]
    fn centred_faces_are_worth_a_little_more() {
        let centre = worth(&face(320.0, 180.0, 80.0), 640.0, 360.0);
        let edge = worth(&face(40.0, 180.0, 80.0), 640.0, 360.0);
        assert!(centre > edge && centre < edge * 1.3);
    }

    /// Detection + cut flags for a reframe fixture, as the analysis job will make them.
    pub(crate) fn analyse_fixture(name: &str) -> Vec<Sample> {
        use crate::faces::Detector;
        use crate::faces::tests::{face_model, reframe_fixture};
        use crate::frames::{self, Look};
        let video = reframe_fixture(name);
        let info = crate::probe::probe_video(&video).unwrap();
        let mut detector = Detector::load(&face_model()).unwrap();
        let mut previous: Option<Look> = None;
        let mut out = Vec::new();
        frames::sample(
            &video,
            (info.width, info.height),
            0,
            info.duration_ms,
            &std::sync::atomic::AtomicBool::new(false),
            |t_ms, frame| {
                let look = Look::of(frame);
                let cut = previous.as_ref().is_some_and(|p| look.is_cut_from(p));
                previous = Some(look);
                out.push(Sample {
                    t_ms,
                    faces: detector.detect(frame)?,
                    cut,
                });
                Ok(())
            },
        )
        .unwrap();
        out
    }

    fn truth(name: &str) -> crate::faces::tests::Truth {
        let path = crate::faces::tests::reframe_fixture(name);
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// The subject on the synthetic fixtures is the face the truth says, on every sample
    /// with one: `cargo test --release follows_the_synthetic_subjects -- --ignored`.
    #[test]
    #[ignore]
    fn follows_the_synthetic_subjects() {
        // moving-face: the only face; none while it's hidden. two-faces: A, then B after
        // the cut at 5 s (B's one-second growth is under the margin anyway).
        for (name, expected) in [
            (
                "moving-face",
                Box::new(|_: u64| None) as Box<dyn Fn(u64) -> Option<&'static str>>,
            ),
            (
                "two-faces",
                Box::new(|t: u64| Some(if t < 5000 { "A" } else { "B" })),
            ),
        ] {
            let samples = analyse_fixture(&format!("{name}.mp4"));
            let tracks = track(&samples);
            let subjects = subjects(&samples, &tracks, FRAME);
            let truth = truth(&format!("{name}.json"));
            let mut wrong = Vec::new();
            for ((sample, subject), want) in samples.iter().zip(&subjects).zip(&truth.samples) {
                let want_face = match expected(sample.t_ms) {
                    Some(id) => want.faces.iter().find(|f| f.id == id),
                    None => want.faces.first(),
                };
                let ok = match (subject, want_face) {
                    (None, None) => true,
                    (Some(s), Some(w)) => {
                        let [x0, y0, x1, y1] = w.bbox;
                        iou(
                            &s.face,
                            &Face {
                                score: 1.0,
                                x0,
                                y0,
                                x1,
                                y1,
                                keypoints: [[0.0; 2]; 6],
                            },
                        ) > 0.5
                    }
                    _ => false,
                };
                if !ok {
                    wrong.push(sample.t_ms);
                }
            }
            assert!(wrong.is_empty(), "{name}: wrong subject at {wrong:?} ms");
        }
    }

    /// The real clips: a subject on every sample, and it doesn't flap between the two
    /// people: `cargo test --release steady_on_the_real_clips -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn steady_on_the_real_clips() {
        for name in ["podcast-wide.mp4", "talking-head.mp4"] {
            let samples = analyse_fixture(name);
            let tracks = track(&samples);
            let subjects = subjects(&samples, &tracks, FRAME);
            let who: Vec<Option<usize>> = subjects.iter().map(|s| s.map(|s| s.track)).collect();
            let switches = who.windows(2).filter(|w| w[0] != w[1]).count();
            println!(
                "{name}: {} tracks, {} cuts, {switches} subject changes, subjects {:?}",
                tracks.len(),
                samples.iter().filter(|s| s.cut).count(),
                who
            );
            assert!(who.iter().all(Option::is_some), "{name}");
            assert!(switches <= 1, "{name}: {switches} changes");
        }
    }
}
