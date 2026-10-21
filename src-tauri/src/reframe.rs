//! The 9:16 crop and its path over a clip (pure).
//!
//! **Crop** (decided 2026-10-21, no zoom in v0.1): the largest 9:16 window, i.e. the full
//! height of a landscape source, moving only sideways. A source taller than 9:16 gets the
//! full width, moving only up and down, with the face at [`FACE_HEIGHT_IN_CROP`] from the
//! top (the upper-middle third).
//!
//! **Path** from the subject at each sample (`tracks::subjects`), in *segments*: a camera
//! cut or a change of subject starts a new one, and the crop **jumps** there (cut, never a
//! pan). Within a segment:
//! - a 5-sample median removes one-off jumps of the detection;
//! - the crop **holds still** while the face stays within [`DEAD_ZONE`] of its centre
//!   (people sway and lean; a camera that follows every lean looks nervous);
//! - when the face has been outside it for [`LEAVE_MS`], the crop **moves** to where the face
//!   settles, starting when it began to move (the analysis is offline), eased in and out
//!   (smootherstep: speed and acceleration both start and end at zero, so no jolts), no
//!   faster than [`MAX_SPEED`];
//! - samples without a subject hold the last position (the first one, at the start).
//!
//! A segment with no subject is centred, unless it is shorter than [`NO_FACE_HOLD_MS`]
//! (the crop stays where it was); a clip with no face at all is centred and
//! marked [`Path::no_face`], so the editor can say "No face found, using center crop".
//!
//! Positions are the crop centre, as a share of the source width (landscape) or height
//! (tall), 0–1. Keyframes are joined linearly ([`position_at`]), or along the smootherstep
//! curve when marked `ease` (a move); a keyframe marked `jump` starts fresh instead of
//! joining the one before (a cut). The editor's preview (TS) and the export (FFmpeg
//! expression) draw the same curve.

// Used by the face analysis job and the editor from M4 step 8 on.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::tracks::{Sample, Subject};

/// The output aspect, width / height.
pub const ASPECT: f32 = 9.0 / 16.0;
/// Where the face centre sits in a tall source's crop, from the top (share of its height).
pub const FACE_HEIGHT_IN_CROP: f32 = 0.38;
/// The face may wander this far from the crop centre (share of the crop width) before the
/// crop follows.
pub const DEAD_ZONE: f32 = 0.15;
/// …for this long.
pub const LEAVE_MS: u64 = 500;
/// The fastest the crop moves within a shot, in source widths (or heights) per second.
/// (0.3 let a face crossing 30% of the width in a second slip half out of the crop.)
pub const MAX_SPEED: f32 = 0.4;
/// The shortest move: quick enough to keep up, slow enough to look like a camera.
pub const MIN_MOVE_MS: u64 = 600;
/// A segment without a face shorter than this keeps the crop where it was (else centred).
pub const NO_FACE_HOLD_MS: u64 = 2000;
/// Smootherstep's top speed is this many times its average.
const EASE_PEAK: f32 = 1.875;

/// The crop window's size as shares of the source: (width, height).
pub fn crop_size(width: u32, height: u32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    if w / h >= ASPECT {
        (ASPECT * h / w, 1.0)
    } else {
        (1.0, w / h / ASPECT)
    }
}

/// Which way the crop moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// Landscape (and square) sources: the crop slides sideways.
    X,
    /// Sources taller than 9:16: the crop slides up and down.
    Y,
}

pub fn axis(width: u32, height: u32) -> Axis {
    if crop_size(width, height).1 >= 1.0 {
        Axis::X
    } else {
        Axis::Y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub t_ms: u64,
    /// The crop centre on [`Path::axis`], 0–1.
    pub pos: f32,
    /// A cut: don't join this keyframe to the one before.
    #[serde(default)]
    pub jump: bool,
    /// Reached from the keyframe before along the smootherstep curve (a camera move),
    /// rather than in a straight line.
    #[serde(default)]
    pub ease: bool,
}

/// Smootherstep: 0 → 1 with zero speed and acceleration at both ends.
pub fn ease(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    k * k * k * (k * (6.0 * k - 15.0) + 10.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub axis: Axis,
    /// The crop window's size as shares of the source (width, height).
    pub crop: (f32, f32),
    pub keyframes: Vec<Keyframe>,
    /// No face anywhere in the clip: the path is the centre crop.
    pub no_face: bool,
}

/// The crop centre at `t_ms`: between keyframes linear (eased into an `ease` keyframe), a
/// jump at a `jump` keyframe, constant before the first and after the last. 0.5 without
/// keyframes.
pub fn position_at(keyframes: &[Keyframe], t_ms: u64) -> f32 {
    let after = keyframes.partition_point(|k| k.t_ms <= t_ms);
    match (
        after.checked_sub(1).map(|i| &keyframes[i]),
        keyframes.get(after),
    ) {
        (None, None) => 0.5,
        (None, Some(next)) => next.pos,
        (Some(prev), None) => prev.pos,
        (Some(prev), Some(next)) => {
            if next.jump || next.t_ms == prev.t_ms {
                prev.pos
            } else {
                let k = (t_ms - prev.t_ms) as f32 / (next.t_ms - prev.t_ms) as f32;
                let k = if next.ease { ease(k) } else { k };
                prev.pos + (next.pos - prev.pos) * k
            }
        }
    }
}

/// The centre crop.
pub fn centered(width: u32, height: u32, from_ms: u64) -> Path {
    Path {
        axis: axis(width, height),
        crop: crop_size(width, height),
        keyframes: vec![Keyframe {
            t_ms: from_ms,
            pos: 0.5,
            jump: false,
            ease: false,
        }],
        no_face: false,
    }
}

/// The automatic path over `samples` (one clip's, in order) for a source of
/// `width`×`height` whose samples were taken at `frame` size (the faces' pixels).
pub fn auto_path(
    samples: &[Sample],
    subjects: &[Option<Subject>],
    (width, height): (u32, u32),
    frame: (f32, f32),
) -> Path {
    let start_ms = samples.first().map_or(0, |s| s.t_ms);
    let mut path = centered(width, height, start_ms);
    if subjects.iter().all(Option::is_none) {
        path.no_face = !samples.is_empty();
        return path;
    }
    let (cw, ch) = path.crop;
    let half = if path.axis == Axis::X {
        cw / 2.0
    } else {
        ch / 2.0
    };
    let span = if path.axis == Axis::X { cw } else { ch };
    // Where the crop centre must be to frame each sample's subject.
    let wanted: Vec<Option<f32>> = subjects
        .iter()
        .map(|s| {
            s.map(|s| {
                let [x, y] = s.face.center();
                let pos = match path.axis {
                    Axis::X => x / frame.0,
                    Axis::Y => y / frame.1 + (0.5 - FACE_HEIGHT_IN_CROP) * ch,
                };
                pos.clamp(half, 1.0 - half)
            })
        })
        .collect();

    let mut keyframes = Vec::new();
    for (start, end) in segments(samples, subjects) {
        let times: Vec<u64> = samples[start..end].iter().map(|s| s.t_ms).collect();
        let targets = fill_gaps(&median5(&wanted[start..end]));
        let jump = start > 0;
        let lasts = samples
            .get(end)
            .map_or(samples[end - 1].t_ms + 250, |s| s.t_ms)
            - times[0];
        match targets {
            // A short stretch without a face (a face popping out, a quick cutaway) keeps
            // the crop where it was, rather than flashing to the centre and back.
            None if lasts < NO_FACE_HOLD_MS && !keyframes.is_empty() => {}
            None => keyframes.push(Keyframe {
                t_ms: times[0],
                pos: 0.5,
                jump,
                ease: false,
            }),
            Some(targets) => {
                let current = keyframes.last().map(|k: &Keyframe| k.pos);
                let mut seg = follow(&times, &targets, span * DEAD_ZONE, current);
                // A new shot framed within the dead zone of the old one needs no jump.
                seg[0].jump = jump && current != Some(seg[0].pos);
                keyframes.extend(seg);
            }
        }
    }
    path.keyframes = keyframes;
    path
}

/// Sample ranges between cuts and subject changes.
fn segments(samples: &[Sample], subjects: &[Option<Subject>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut last_track = None;
    for i in 0..samples.len() {
        let track = subjects[i].map(|s| s.track);
        let changed = track.is_some() && last_track.is_some() && track != last_track;
        if i > start && (samples[i].cut || changed) {
            out.push((start, i));
            start = i;
        }
        if samples[i].cut {
            last_track = None;
        }
        if track.is_some() {
            last_track = track;
        }
    }
    if start < samples.len() {
        out.push((start, samples.len()));
    }
    out
}

/// Median of each sample and up to two either side (present ones only).
fn median5(values: &[Option<f32>]) -> Vec<Option<f32>> {
    (0..values.len())
        .map(|i| {
            values[i]?;
            let mut window: Vec<f32> = values[i.saturating_sub(2)..(i + 3).min(values.len())]
                .iter()
                .flatten()
                .copied()
                .collect();
            window.sort_by(f32::total_cmp);
            Some(window[window.len() / 2])
        })
        .collect()
}

/// Gaps hold the last value (the first one at the start); `None` if there is none.
fn fill_gaps(values: &[Option<f32>]) -> Option<Vec<f32>> {
    let first = values.iter().flatten().next().copied()?;
    let mut last = first;
    Some(
        values
            .iter()
            .map(|v| {
                if let Some(v) = v {
                    last = *v;
                }
                last
            })
            .collect(),
    )
}

/// Holds and eased moves through one segment's targets, starting where the crop already
/// is (`current`) if the face settles within the dead zone of it.
fn follow(times: &[u64], targets: &[f32], dead_zone: f32, current: Option<f32>) -> Vec<Keyframe> {
    let first = settle(times, targets, 0);
    let first = current
        .filter(|c| (c - first).abs() <= dead_zone)
        .unwrap_or(first);
    let mut keyframes = vec![Keyframe {
        t_ms: times[0],
        pos: first,
        jump: false,
        ease: false,
    }];
    let mut pos = keyframes[0].pos;
    let mut busy_until = times[0];
    let mut outside_since: Option<usize> = None;
    for i in 0..times.len() {
        if times[i] < busy_until {
            continue;
        }
        if (targets[i] - pos).abs() <= dead_zone {
            outside_since = None;
            continue;
        }
        let since = *outside_since.get_or_insert(i);
        if times[i] - times[since] < LEAVE_MS && i + 1 < times.len() {
            continue;
        }
        // Move from when the face began to move (the last sample it was still about where
        // the crop is) to where it settles.
        let to = settle(times, targets, i);
        let began = (0..since)
            .rev()
            .find(|&j| (targets[j] - pos).abs() <= dead_zone / 3.0)
            .map_or(since, |j| j.min(since));
        let start = times[began].max(busy_until);
        let distance = (to - pos).abs();
        // Long enough that the curve's top speed stays under the limit.
        let duration = ((distance * EASE_PEAK / MAX_SPEED * 1000.0) as u64).max(MIN_MOVE_MS);
        if keyframes.last().is_some_and(|k| k.t_ms < start) {
            keyframes.push(Keyframe {
                t_ms: start,
                pos,
                jump: false,
                ease: false,
            });
        }
        keyframes.push(Keyframe {
            t_ms: start + duration,
            pos: to,
            jump: false,
            ease: true,
        });
        pos = to;
        busy_until = start + duration;
        outside_since = None;
    }
    keyframes
}

/// Where the face settles after sample `i`: the median target over the next second.
fn settle(times: &[u64], targets: &[f32], i: usize) -> f32 {
    let mut window: Vec<f32> = (i..times.len())
        .take_while(|&j| times[j] - times[i] <= 1000)
        .map(|j| targets[j])
        .collect();
    window.sort_by(f32::total_cmp);
    window[window.len() / 2]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::faces::Face;

    const FRAME: (f32, f32) = (640.0, 360.0);
    const SOURCE: (u32, u32) = (1920, 1080);

    fn face_at(x: f32) -> Face {
        Face {
            score: 0.9,
            x0: x - 30.0,
            y0: 100.0,
            x1: x + 30.0,
            y1: 160.0,
            keypoints: [[x, 130.0]; 6],
        }
    }

    /// Samples every 250 ms with the subject's face centre at `x(t)` (frame pixels).
    fn clip(seconds: f32, x: impl Fn(u64) -> Option<f32>) -> (Vec<Sample>, Vec<Option<Subject>>) {
        (0..(seconds * 4.0) as u64)
            .map(|k| {
                let t = k * 250;
                let face = x(t).map(face_at);
                (
                    Sample {
                        t_ms: t,
                        faces: face.into_iter().collect(),
                        cut: false,
                    },
                    face.map(|face| Subject {
                        track: 0,
                        face,
                        seen: true,
                    }),
                )
            })
            .unzip()
    }

    fn path(samples: &[Sample], subjects: &[Option<Subject>]) -> Path {
        auto_path(samples, subjects, SOURCE, FRAME)
    }

    /// Fastest speed and largest acceleration change along the path, sampled every 40 ms
    /// (whole milliseconds, so the steps are even; jumps excluded): (source widths / s,
    /// widths / s³).
    pub(crate) fn speed_and_jerk(p: &Path, from: u64, to: u64) -> (f32, f32) {
        let dt = 40u64;
        let jumps: Vec<u64> = p
            .keyframes
            .iter()
            .filter(|k| k.jump)
            .map(|k| k.t_ms)
            .collect();
        let pos: Vec<(u64, f32)> = (from..=to)
            .step_by(dt as usize)
            .map(|t| (t, position_at(&p.keyframes, t)))
            .collect();
        let near_jump = |t: u64| jumps.iter().any(|&j| t.abs_diff(j) < 4 * dt);
        let mut max_speed = 0.0f32;
        let mut max_jerk = 0.0f32;
        let s = dt as f32 / 1000.0;
        for w in pos.windows(4) {
            if w.iter().any(|&(t, _)| near_jump(t)) {
                continue;
            }
            let v: Vec<f32> = w.windows(2).map(|p| (p[1].1 - p[0].1) / s).collect();
            let a: Vec<f32> = v.windows(2).map(|q| (q[1] - q[0]) / s).collect();
            max_speed = max_speed.max(v[0].abs());
            max_jerk = max_jerk.max(((a[1] - a[0]) / s).abs());
        }
        (max_speed, max_jerk)
    }

    #[test]
    fn crop_sizes() {
        let (w, h) = crop_size(1920, 1080);
        assert!((w - 0.31640625).abs() < 1e-6 && h == 1.0);
        assert_eq!(axis(1920, 1080), Axis::X);
        assert_eq!(crop_size(1080, 1920), (1.0, 1.0));
        assert_eq!(axis(1080, 1920), Axis::X);
        let (w, h) = crop_size(1000, 2400); // taller than 9:16
        assert!(w == 1.0 && (h - 0.7407407).abs() < 1e-6);
        assert_eq!(axis(1000, 2400), Axis::Y);
    }

    #[test]
    fn positions_join_linearly_and_jump_at_cuts() {
        let k = |t_ms, pos, jump, ease| Keyframe {
            t_ms,
            pos,
            jump,
            ease,
        };
        let keys = [
            k(1000, 0.2, false, false),
            k(2000, 0.4, false, false),
            k(3000, 0.8, true, false),
            k(4000, 0.4, false, true),
        ];
        assert_eq!(position_at(&keys, 0), 0.2);
        assert!((position_at(&keys, 1500) - 0.3).abs() < 1e-6);
        assert_eq!(position_at(&keys, 2999), 0.4);
        assert_eq!(position_at(&keys, 3000), 0.8);
        // Eased: halfway in time is halfway in position, a quarter in is well under.
        assert!((position_at(&keys, 3500) - 0.6).abs() < 1e-6);
        assert!(position_at(&keys, 3250) > 0.75);
        assert_eq!(position_at(&keys, 9000), 0.4);
        assert_eq!(position_at(&[], 5), 0.5);
    }

    #[test]
    fn a_swaying_face_holds_the_crop_still() {
        // ±12 px of 640 (about 6% of the crop width) at 1 Hz, plus a one-off detection jump.
        let (s, subj) = clip(8.0, |t| {
            let sway = 12.0 * (t as f32 / 1000.0 * std::f32::consts::TAU).sin();
            Some(if t == 3000 { 500.0 } else { 300.0 + sway })
        });
        let p = path(&s, &subj);
        assert_eq!(p.keyframes.len(), 1, "{:?}", p.keyframes);
        assert!((p.keyframes[0].pos - 300.0 / 640.0).abs() < 0.02);
    }

    #[test]
    fn a_slide_is_followed_smoothly_and_keeps_the_face_in_the_crop() {
        // 0.35 → 0.65 of the width over 1 s (from 3 s), then still.
        let x = |t: u64| {
            let k = ((t as f32 - 3000.0) / 1000.0).clamp(0.0, 1.0);
            0.35 + 0.3 * k
        };
        let (s, subj) = clip(10.0, |t| Some(x(t) * 640.0));
        let p = path(&s, &subj);
        let (cw, _) = p.crop;
        for t in (0..10_000).step_by(50) {
            let centre = position_at(&p.keyframes, t);
            assert!(
                (x(t) - centre).abs() < cw / 2.0 - 0.02,
                "face out of the crop at {t} ms"
            );
        }
        assert!(
            (position_at(&p.keyframes, 9000) - 0.65).abs() < 0.02,
            "settles on the face"
        );
        let (speed, jerk) = speed_and_jerk(&p, 0, 10_000);
        assert!(speed <= MAX_SPEED * 1.05, "speed {speed}");
        // Smootherstep's jerk is 60·distance/duration³ at most: 0.3 over 1.4 s ≈ 6.5.
        assert!(jerk < 10.0, "jerk {jerk}");
    }

    #[test]
    fn a_cut_or_a_new_subject_jumps() {
        let (mut s, mut subj) = clip(8.0, |t| Some(if t < 4000 { 160.0 } else { 480.0 }));
        s[16].cut = true;
        let p = path(&s, &subj);
        let jump = p.keyframes.iter().find(|k| k.jump).expect("a jump");
        assert_eq!(jump.t_ms, 4000);
        assert!((position_at(&p.keyframes, 3999) - 0.25).abs() < 0.01);
        assert!((position_at(&p.keyframes, 4000) - 0.75).abs() < 0.01);

        // The same without a cut but with another subject from 4 s.
        s[16].cut = false;
        for x in subj.iter_mut().skip(16) {
            x.as_mut().unwrap().track = 1;
        }
        let p = path(&s, &subj);
        assert_eq!(
            p.keyframes
                .iter()
                .filter(|k| k.jump)
                .map(|k| k.t_ms)
                .collect::<Vec<_>>(),
            [4000]
        );
    }

    #[test]
    fn gaps_hold_and_no_face_centres() {
        // No subject for the first second and from 4 to 5 s.
        let (s, subj) = clip(8.0, |t| {
            (t >= 1000 && !(4000..5000).contains(&t)).then_some(160.0)
        });
        let p = path(&s, &subj);
        assert!(!p.no_face);
        for t in (0..8000).step_by(250) {
            assert!((position_at(&p.keyframes, t) - 0.25).abs() < 0.01, "{t}");
        }
        let (s, subj) = clip(4.0, |_| None);
        let p = path(&s, &subj);
        assert!(p.no_face);
        assert_eq!(
            p.keyframes,
            [Keyframe {
                t_ms: 0,
                pos: 0.5,
                jump: false,
                ease: false
            }]
        );
    }

    #[test]
    fn a_short_faceless_shot_keeps_the_crop_but_a_long_one_centres() {
        // Cut to 1 s without a face, then back: no jump to the centre.
        let (mut s, subj) = clip(8.0, |t| (!(3000..4000).contains(&t)).then_some(160.0));
        s[12].cut = true;
        s[16].cut = true;
        let p = path(&s, &subj);
        assert!(
            p.keyframes.iter().all(|k| (k.pos - 0.25).abs() < 0.01),
            "{:?}",
            p.keyframes
        );
        // 3 s without a face: centred.
        let (mut s, subj) = clip(8.0, |t| (!(3000..6000).contains(&t)).then_some(160.0));
        s[12].cut = true;
        s[24].cut = true;
        let p = path(&s, &subj);
        assert!(
            (position_at(&p.keyframes, 4000) - 0.5).abs() < 1e-6,
            "{:?}",
            p.keyframes
        );
    }

    #[test]
    fn the_crop_stays_inside_the_frame() {
        // A face at the very left edge: the crop stops at the edge.
        let (s, subj) = clip(3.0, |_| Some(5.0));
        let p = path(&s, &subj);
        assert!((p.keyframes[0].pos - p.crop.0 / 2.0).abs() < 1e-6);
    }

    #[test]
    fn tall_sources_move_up_and_down_with_the_face_in_the_upper_middle() {
        // 1000×2400 source sampled at 266×640; the face centre at 30% of the height.
        let (s, subj): (Vec<Sample>, Vec<Option<Subject>>) = (0..8)
            .map(|k| {
                let mut face = face_at(133.0);
                face.y0 = 0.3 * 640.0 - 20.0;
                face.y1 = 0.3 * 640.0 + 20.0;
                (
                    Sample {
                        t_ms: k * 250,
                        faces: vec![face],
                        cut: false,
                    },
                    Some(Subject {
                        track: 0,
                        face,
                        seen: true,
                    }),
                )
            })
            .unzip();
        let p = auto_path(&s, &subj, (1000, 2400), (266.0, 640.0));
        assert_eq!(p.axis, Axis::Y);
        let ch = p.crop.1;
        let top = p.keyframes[0].pos - ch / 2.0;
        // The face sits 38% down the crop.
        assert!(
            ((0.3 - top) / ch - FACE_HEIGHT_IN_CROP).abs() < 0.01,
            "{p:?}"
        );
    }

    fn fixture_path(name: &str) -> (Path, Vec<Sample>) {
        let samples = crate::tracks::tests::analyse_fixture(name);
        let tracks = crate::tracks::track(&samples);
        let subjects = crate::tracks::subjects(&samples, &tracks, FRAME);
        (auto_path(&samples, &subjects, (640, 360), FRAME), samples)
    }

    /// The synthetic fixtures end to end (detector, tracks, path): the subject's whole face
    /// is inside the crop on every sample that has one, and the path is smooth:
    /// `cargo test --release crops_the_synthetic_fixtures -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn crops_the_synthetic_fixtures() {
        for (name, subject) in [("moving-face", None), ("two-faces", Some(("A", 5000)))] {
            let (p, _) = fixture_path(&format!("{name}.mp4"));
            let truth: crate::faces::tests::Truth = serde_json::from_slice(
                &std::fs::read(crate::faces::tests::reframe_fixture(&format!(
                    "{name}.json"
                )))
                .unwrap(),
            )
            .unwrap();
            println!("{name}: {:?}", p.keyframes);
            let half = p.crop.0 / 2.0;
            for sample in &truth.samples {
                let want = match subject {
                    Some((first, switch)) => {
                        let id = if sample.t_ms < switch { first } else { "B" };
                        sample.faces.iter().find(|f| f.id == id)
                    }
                    None => sample.faces.first(),
                };
                let Some(want) = want else { continue };
                let centre = position_at(&p.keyframes, sample.t_ms);
                let (left, right) = (want.bbox[0] / 640.0, want.bbox[2] / 640.0);
                assert!(
                    left >= centre - half - 0.005 && right <= centre + half + 0.005,
                    "{name} at {} ms: face {left:.3}–{right:.3}, crop {:.3}–{:.3}",
                    sample.t_ms,
                    centre - half,
                    centre + half
                );
            }
            let (speed, jerk) = speed_and_jerk(&p, 0, 10_000);
            println!("{name}: top speed {speed:.2} widths/s, jerk {jerk:.1}");
            assert!(speed <= MAX_SPEED * 1.05 && jerk < 15.0, "{name}");
        }
    }

    /// The real clips: few moves, smooth: `cargo test --release crops_the_real_clips --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn crops_the_real_clips() {
        for name in ["podcast-wide.mp4", "talking-head.mp4"] {
            let (p, samples) = fixture_path(name);
            let end = samples.last().unwrap().t_ms;
            let moves = p.keyframes.iter().filter(|k| k.ease).count();
            let jumps = p.keyframes.iter().filter(|k| k.jump).count();
            let (speed, jerk) = speed_and_jerk(&p, 0, end);
            println!(
                "{name}: {moves} moves, {jumps} jumps, top speed {speed:.2}, jerk {jerk:.1}: {:?}",
                p.keyframes
            );
            assert!(!p.no_face);
            assert!(speed <= MAX_SPEED * 1.05 && jerk < 15.0, "{name}");
            assert!(
                moves + jumps <= end as usize / 4000 + 1,
                "{name}: too restless"
            );
        }
    }
}
