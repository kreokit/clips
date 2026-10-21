//! Face detection for the reframe: MediaPipe BlazeFace full-range, run with ONNX Runtime.
//!
//! The model ([`models::FACE_MODEL`], from our `models-v2` mirror) takes one 192×192 RGB
//! frame (NHWC, values in [-1, 1]) and returns box + keypoint regressors [1, 2304, 16] and
//! score logits [1, 2304, 1], one per SSD anchor. Its tensor names and shapes are checked
//! when it loads, so a wrong file fails there rather than producing nonsense.
//!
//! Pre- and post-processing follow MediaPipe's `face_detection_full_range` graph, as
//! `scripts/export_blazeface.py` does (and `fixtures/faces/reference.json` holds its
//! results): the frame is fitted into the input (centred, padded with black, triangle
//! filter like PIL's bilinear), one fixed-size anchor per 4×4 cell, sigmoid scores
//! (logits clipped to ±100) ≥ 0.6, weighted non-maximum suppression at IoU 0.3.
//!
//! **Small faces:** a 16:9 frame fitted into 192 px leaves a face under ~8% of the frame
//! height only a few pixels. When the whole frame finds nothing, [`Detector::detect`]
//! looks again at two overlapping squares along the long side (1.78× the resolution for
//! 16:9) and merges what they find. Only then, so frames with a face cost one model run.
//!
//! [`models::FACE_MODEL`]: crate::models::FACE_MODEL

// Used by the face analysis job from M4 step 8 on.
#![allow(dead_code)]

use std::path::Path;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{TensorRef, ValueType};
use serde::{Deserialize, Serialize};

/// Model input side in pixels.
pub const INPUT_SIZE: usize = 192;
const CELLS: usize = INPUT_SIZE / 4;
/// SSD anchors: one per 4×4 cell of the input.
pub const NUM_ANCHORS: usize = CELLS * CELLS;
/// Box (4) + six keypoints (12) per anchor.
pub const NUM_COORDS: usize = 16;

const INPUT: &str = "input";
const REGRESSORS: &str = "reshaped_regressor_face_4";
const SCORES: &str = "reshaped_classifier_face_4";

const MIN_SCORE: f32 = 0.6;
const SCORE_CLIP: f32 = 100.0;
const NMS_IOU: f32 = 0.3;
/// Frames at least this much wider than tall (or taller than wide) get the two-tile pass:
/// 3:2 and 16:9, not 4:3 (its tiles would add only a third more detail).
const TILE_MIN_ASPECT: f32 = 1.5;

/// Keypoint indices, from the person's point of view (MediaPipe's order).
pub const RIGHT_EYE: usize = 0;
pub const LEFT_EYE: usize = 1;
pub const NOSE: usize = 2;
pub const MOUTH: usize = 3;
pub const RIGHT_EAR: usize = 4;
pub const LEFT_EAR: usize = 5;

/// A face in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub score: f32,
    /// Box corners: left, top, right, bottom.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// (x, y) of the six keypoints ([`RIGHT_EYE`] … [`LEFT_EAR`]).
    pub keypoints: [[f32; 2]; 6],
}

impl Face {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn center(&self) -> [f32; 2] {
        [(self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0]
    }

    fn map(&self, f: impl Fn(f32, f32) -> [f32; 2]) -> Face {
        let [x0, y0] = f(self.x0, self.y0);
        let [x1, y1] = f(self.x1, self.y1);
        Face {
            score: self.score,
            x0,
            y0,
            x1,
            y1,
            keypoints: self.keypoints.map(|[x, y]| f(x, y)),
        }
    }
}

pub fn iou(a: &Face, b: &Face) -> f32 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let union = a.width() * a.height() + b.width() * b.height() - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// A frame: tightly packed RGB, 3 bytes per pixel.
#[derive(Clone, Copy)]
pub struct Rgb<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
}

/// A part of a frame, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Region {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

/// The loaded face model, with its input buffer.
pub struct Detector {
    session: Session,
    input: Vec<f32>,
}

impl Detector {
    /// Loads the model at `path` for one thread (FFmpeg decodes beside it, and one 192×192
    /// frame takes a few milliseconds).
    pub fn load(path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(1))
            .and_then(|b| b.commit_from_file(path))
            .map_err(|e| format!("cannot load the face model: {e}"))?;
        check_signature(&session)?;
        Ok(Self {
            session,
            input: vec![0.0; INPUT_SIZE * INPUT_SIZE * 3],
        })
    }

    /// The faces in `frame`, best first: the whole frame, and if that finds nothing, two
    /// overlapping tiles (wide or tall frames only).
    pub fn detect(&mut self, frame: Rgb) -> Result<Vec<Face>, String> {
        let faces = self.detect_whole(frame)?;
        if faces.is_empty() {
            self.detect_tiled(frame)
        } else {
            Ok(faces)
        }
    }

    /// The whole frame fitted into the input.
    pub fn detect_whole(&mut self, frame: Rgb) -> Result<Vec<Face>, String> {
        let all = Region {
            x: 0,
            y: 0,
            width: frame.width,
            height: frame.height,
        };
        self.detect_region(frame, all)
    }

    /// Two overlapping squares along the long side, merged; nothing for frames closer to
    /// square than [`TILE_MIN_ASPECT`].
    pub fn detect_tiled(&mut self, frame: Rgb) -> Result<Vec<Face>, String> {
        let mut faces = Vec::new();
        for region in tiles(frame.width, frame.height) {
            faces.extend(self.detect_region(frame, region)?);
        }
        Ok(weighted_nms(faces))
    }

    fn detect_region(&mut self, frame: Rgb, region: Region) -> Result<Vec<Face>, String> {
        let fit = fit_into_input(frame, region, &mut self.input);
        let input = TensorRef::from_array_view(([1, INPUT_SIZE, INPUT_SIZE, 3], &self.input[..]))
            .map_err(|e| format!("face model input: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| format!("the face model failed: {e}"))?;
        let (_, regressors) = outputs[REGRESSORS]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("unexpected face model output: {e}"))?;
        let (_, logits) = outputs[SCORES]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("unexpected face model output: {e}"))?;
        let faces = weighted_nms(decode(regressors, logits));
        // Input pixels → region pixels → frame pixels.
        Ok(faces
            .iter()
            .map(|face| {
                face.map(|x, y| {
                    [
                        region.x as f32 + (x * INPUT_SIZE as f32 - fit.x0) / fit.scale,
                        region.y as f32 + (y * INPUT_SIZE as f32 - fit.y0) / fit.scale,
                    ]
                })
            })
            .collect())
    }
}

fn shape_of(value_type: &ValueType) -> Vec<i64> {
    value_type
        .tensor_shape()
        .map(|shape| shape.to_vec())
        .unwrap_or_default()
}

fn check_signature(session: &Session) -> Result<(), String> {
    let side = INPUT_SIZE as i64;
    let anchors = NUM_ANCHORS as i64;
    let inputs: Vec<_> = session
        .inputs
        .iter()
        .map(|i| (i.name.as_str(), shape_of(&i.input_type)))
        .collect();
    let mut outputs: Vec<_> = session
        .outputs
        .iter()
        .map(|o| (o.name.as_str(), shape_of(&o.output_type)))
        .collect();
    outputs.sort();
    let expected_inputs = vec![(INPUT, vec![1, side, side, 3])];
    let mut expected_outputs = vec![
        (REGRESSORS, vec![1, anchors, NUM_COORDS as i64]),
        (SCORES, vec![1, anchors, 1]),
    ];
    expected_outputs.sort();
    if inputs != expected_inputs || outputs != expected_outputs {
        return Err(format!(
            "unexpected face model: inputs {inputs:?}, outputs {outputs:?}"
        ));
    }
    Ok(())
}

/// Two squares (side = the short side) at both ends of the long side. For 16:9 they
/// overlap by 1/8 of the width, so a face up to that wide at the seam is whole in one.
fn tiles(width: usize, height: usize) -> Vec<Region> {
    let (long, short) = (width.max(height), width.min(height));
    if short == 0 || (long as f32) < short as f32 * TILE_MIN_ASPECT {
        return Vec::new();
    }
    let square = |offset: usize| {
        if width >= height {
            Region {
                x: offset,
                y: 0,
                width: short,
                height: short,
            }
        } else {
            Region {
                x: 0,
                y: offset,
                width: short,
                height: short,
            }
        }
    };
    vec![square(0), square(long - short)]
}

/// Where the region landed in the input: offset of its top-left corner and input pixels
/// per region pixel.
#[derive(Debug, Clone, Copy)]
struct Fit {
    x0: f32,
    y0: f32,
    scale: f32,
}

/// Scales `region` of `frame` to fit the input (keeping its aspect), centres it on black,
/// and writes RGB / 127.5 − 1 into `out` (NHWC).
fn fit_into_input(frame: Rgb, region: Region, out: &mut [f32]) -> Fit {
    let scale = INPUT_SIZE as f32 / region.width.max(region.height) as f32;
    let w = ((region.width as f32 * scale).round() as usize).clamp(1, INPUT_SIZE);
    let h = ((region.height as f32 * scale).round() as usize).clamp(1, INPUT_SIZE);
    let (x0, y0) = ((INPUT_SIZE - w) / 2, (INPUT_SIZE - h) / 2);
    out.fill(-1.0);

    // Separable triangle filter (PIL's "bilinear", which widens with the scale factor):
    // rows first into `tmp`, then columns.
    let cols = filter_taps(region.x, region.width, frame.width, w);
    let rows = filter_taps(region.y, region.height, frame.height, h);
    let mut tmp = vec![0.0f32; region.height * w * 3];
    for (ry, tmp_row) in tmp.chunks_exact_mut(w * 3).enumerate() {
        let src = &frame.data[(region.y + ry) * frame.width * 3..][..frame.width * 3];
        for (ox, (start, weights)) in cols.iter().enumerate() {
            let mut acc = [0.0f32; 3];
            for (k, wt) in weights.iter().enumerate() {
                let px = &src[(start + k) * 3..][..3];
                for c in 0..3 {
                    acc[c] += px[c] as f32 * wt;
                }
            }
            tmp_row[ox * 3..ox * 3 + 3].copy_from_slice(&acc);
        }
    }
    for (oy, (start, weights)) in rows.iter().enumerate() {
        let dst = &mut out[((y0 + oy) * INPUT_SIZE + x0) * 3..][..w * 3];
        for (i, value) in dst.iter_mut().enumerate() {
            let acc: f32 = weights
                .iter()
                .enumerate()
                .map(|(k, wt)| tmp[(start - region.y + k) * w * 3 + i] * wt)
                .sum();
            *value = acc.clamp(0.0, 255.0) / 127.5 - 1.0;
        }
    }
    Fit {
        x0: x0 as f32,
        y0: y0 as f32,
        scale: w as f32 / region.width as f32,
    }
}

/// For each of `out_len` output pixels covering source pixels `start..start + len` (of a
/// line `full_len` long): the first source pixel and the normalized weights from there.
fn filter_taps(
    start: usize,
    len: usize,
    full_len: usize,
    out_len: usize,
) -> Vec<(usize, Vec<f32>)> {
    let ratio = len as f32 / out_len as f32;
    let support = ratio.max(1.0);
    let (lo_limit, hi_limit) = (start, (start + len).min(full_len));
    (0..out_len)
        .map(|i| {
            let center = start as f32 + (i as f32 + 0.5) * ratio;
            let lo = ((center - support).floor().max(lo_limit as f32)) as usize;
            let hi = ((center + support).ceil().min(hi_limit as f32)) as usize;
            let mut weights: Vec<f32> = (lo..hi.max(lo + 1))
                .map(|j| (1.0 - ((j as f32 + 0.5 - center) / support).abs()).max(0.0))
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum > 0.0 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else {
                weights = vec![1.0];
            }
            (lo.min(hi_limit - 1), weights)
        })
        .collect()
}

/// Faces from the raw outputs, in input coordinates normalized to 0–1 (MediaPipe's
/// TensorsToDetections with reverse_output_order and fixed-size anchors).
fn decode(regressors: &[f32], logits: &[f32]) -> Vec<Face> {
    let scale = INPUT_SIZE as f32;
    let mut faces = Vec::new();
    for (i, &logit) in logits.iter().enumerate().take(NUM_ANCHORS) {
        let score = 1.0 / (1.0 + (-logit.clamp(-SCORE_CLIP, SCORE_CLIP)).exp());
        if score < MIN_SCORE {
            continue;
        }
        let ax = ((i % CELLS) as f32 + 0.5) / CELLS as f32;
        let ay = ((i / CELLS) as f32 + 0.5) / CELLS as f32;
        let raw = &regressors[i * NUM_COORDS..][..NUM_COORDS];
        let (cx, cy) = (raw[0] / scale + ax, raw[1] / scale + ay);
        let (w, h) = (raw[2] / scale, raw[3] / scale);
        let mut keypoints = [[0.0; 2]; 6];
        for (k, point) in keypoints.iter_mut().enumerate() {
            *point = [raw[4 + 2 * k] / scale + ax, raw[5 + 2 * k] / scale + ay];
        }
        faces.push(Face {
            score,
            x0: cx - w / 2.0,
            y0: cy - h / 2.0,
            x1: cx + w / 2.0,
            y1: cy + h / 2.0,
            keypoints,
        });
    }
    faces
}

/// MediaPipe's WEIGHTED suppression: best first, each cluster (IoU > [`NMS_IOU`] with its
/// best) becomes the score-weighted average of its boxes and keypoints, with the best score.
fn weighted_nms(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept = Vec::new();
    while let Some(&best) = faces.first() {
        let (cluster, rest): (Vec<Face>, Vec<Face>) =
            faces.into_iter().partition(|f| iou(&best, f) > NMS_IOU);
        faces = rest;
        let total: f32 = cluster.iter().map(|f| f.score).sum();
        let mut merged = Face {
            score: best.score,
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: 0.0,
            keypoints: [[0.0; 2]; 6],
        };
        for f in &cluster {
            let w = f.score / total;
            merged.x0 += f.x0 * w;
            merged.y0 += f.y0 * w;
            merged.x1 += f.x1 * w;
            merged.y1 += f.y1 * w;
            for (m, p) in merged.keypoints.iter_mut().zip(f.keypoints) {
                m[0] += p[0] * w;
                m[1] += p[1] * w;
            }
        }
        kept.push(merged);
    }
    kept
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::models::{self, FACE_MODEL};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::AtomicBool;

    /// The face model in the app's models folder (`models::tests::dev_dirs`), downloaded
    /// there if missing (2 MB). Elsewhere than macOS, a temporary folder.
    /// Tests run in parallel; they share one download.
    pub(crate) fn face_model() -> PathBuf {
        static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            let dir = if cfg!(target_os = "macos") || std::env::var_os("KREOKIT_APP_DATA").is_some()
            {
                models::tests::dev_dirs().ours
            } else {
                std::env::temp_dir().join("kreokit-clips-face-model")
            };
            models::download(FACE_MODEL, &dir, &AtomicBool::new(false), |_| {}).unwrap()
        })
        .clone()
    }

    fn faces_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/faces")
    }

    /// An image decoded to RGB by FFmpeg.
    pub(crate) fn decode_image(path: &Path, width: usize, height: usize) -> Vec<u8> {
        let output = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout.len(),
            width * height * 3,
            "{}",
            path.display()
        );
        output.stdout
    }

    fn face(score: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> Face {
        Face {
            score,
            x0,
            y0,
            x1,
            y1,
            keypoints: [[(x0 + x1) / 2.0, (y0 + y1) / 2.0]; 6],
        }
    }

    #[test]
    fn anchors_are_row_major_cell_centres() {
        // A zero regressor puts the box centre on its anchor; anchor 50 is cell (2, 1).
        let regressors = vec![0.0; NUM_ANCHORS * NUM_COORDS];
        let mut logits = vec![-10.0; NUM_ANCHORS];
        logits[50] = 10.0;
        let faces = decode(&regressors, &logits);
        assert_eq!(faces.len(), 1);
        assert!((faces[0].center()[0] - 2.5 / 48.0).abs() < 1e-6);
        assert!((faces[0].center()[1] - 1.5 / 48.0).abs() < 1e-6);
    }

    #[test]
    fn decodes_boxes_and_keypoints_in_input_pixels() {
        let mut regressors = vec![0.0; NUM_ANCHORS * NUM_COORDS];
        let mut logits = vec![-10.0; NUM_ANCHORS];
        let i = 24 * CELLS + 24; // anchor centre (0.5104, 0.5104)
        logits[i] = 0.0; // score 0.5: below the threshold
        regressors[i * NUM_COORDS..][..6].copy_from_slice(&[9.6, -19.2, 38.4, 19.2, 1.92, 0.0]);
        assert!(decode(&regressors, &logits).is_empty());
        logits[i] = 1.0;
        let f = decode(&regressors, &logits)[0];
        let a = 24.5 / 48.0;
        assert!((f.score - 0.7311).abs() < 1e-3);
        assert!((f.center()[0] - (a + 0.05)).abs() < 1e-6);
        assert!((f.center()[1] - (a - 0.1)).abs() < 1e-6);
        assert!((f.width() - 0.2).abs() < 1e-6 && (f.height() - 0.1).abs() < 1e-6);
        assert!((f.keypoints[RIGHT_EYE][0] - (a + 0.01)).abs() < 1e-6);
        assert!((f.keypoints[RIGHT_EYE][1] - a).abs() < 1e-6);
    }

    #[test]
    fn weighted_nms_averages_each_cluster() {
        let faces = weighted_nms(vec![
            face(0.6, 2.0, 0.0, 12.0, 10.0),
            face(0.9, 0.0, 0.0, 10.0, 10.0),
            face(0.7, 50.0, 50.0, 60.0, 60.0),
        ]);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].score, 0.9);
        assert!((faces[0].x0 - 0.8).abs() < 1e-5, "{}", faces[0].x0); // 0.6·2 / 1.5
        assert!((faces[0].x1 - 10.8).abs() < 1e-5);
        assert_eq!(faces[1].x0, 50.0);
    }

    #[test]
    fn tiles_cover_wide_and_tall_frames() {
        assert_eq!(
            tiles(1920, 1080),
            [
                Region {
                    x: 0,
                    y: 0,
                    width: 1080,
                    height: 1080
                },
                Region {
                    x: 840,
                    y: 0,
                    width: 1080,
                    height: 1080
                },
            ]
        );
        assert_eq!(
            tiles(1080, 1920)[1],
            Region {
                x: 0,
                y: 840,
                width: 1080,
                height: 1080
            }
        );
        assert!(
            tiles(1440, 1080).is_empty(),
            "4:3 is close enough to square"
        );
        assert_eq!(tiles(1500, 1000).len(), 2);
        assert!(tiles(0, 0).is_empty());
    }

    #[test]
    fn fitting_centres_and_pads_with_black() {
        // A white 40×20 frame: 192×96 in the middle, black (−1) above and below.
        let data = vec![255u8; 40 * 20 * 3];
        let frame = Rgb {
            data: &data,
            width: 40,
            height: 20,
        };
        let mut out = vec![0.0; INPUT_SIZE * INPUT_SIZE * 3];
        let all = Region {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };
        let fit = fit_into_input(frame, all, &mut out);
        assert_eq!((fit.x0, fit.y0, fit.scale), (0.0, 48.0, 4.8));
        let at = |x: usize, y: usize| out[(y * INPUT_SIZE + x) * 3];
        assert_eq!(at(0, 47), -1.0);
        assert!((at(0, 48) - 1.0).abs() < 1e-5);
        assert!((at(191, 143) - 1.0).abs() < 1e-5);
        assert_eq!(at(100, 144), -1.0);
    }

    #[test]
    fn downscaling_averages_neighbours() {
        // Vertical stripes 1 px wide average to grey when shrunk 4×.
        let (w, h) = (768, 768);
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| if i % 2 == 0 { [255u8; 3] } else { [0u8; 3] })
            .collect();
        let frame = Rgb {
            data: &data,
            width: w,
            height: h,
        };
        let mut out = vec![0.0; INPUT_SIZE * INPUT_SIZE * 3];
        fit_into_input(
            frame,
            Region {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
            &mut out,
        );
        let mid = out[(96 * INPUT_SIZE + 96) * 3];
        assert!(mid.abs() < 0.02, "{mid}");
    }

    #[test]
    fn a_wrong_file_is_refused() {
        let path = std::env::temp_dir().join("kreokit-clips-not-a-model.onnx");
        std::fs::write(&path, b"not a model").unwrap();
        let error = Detector::load(&path).err().unwrap();
        assert!(error.starts_with("cannot load the face model"), "{error}");
    }

    /// `cargo test face_model_loads -- --ignored` (downloads the model from models-v2).
    #[test]
    #[ignore]
    fn face_model_loads() {
        Detector::load(&face_model()).unwrap();
    }

    #[derive(Deserialize)]
    struct Reference {
        photos: Vec<ReferencePhoto>,
    }

    #[derive(Deserialize)]
    struct ReferencePhoto {
        photo: String,
        size: [usize; 2],
        faces: Vec<ReferenceFace>,
    }

    #[derive(Deserialize)]
    struct ReferenceFace {
        score: f32,
        #[serde(rename = "box")]
        bbox: [f32; 4],
        keypoints: std::collections::BTreeMap<String, [f32; 2]>,
    }

    /// `region` of `frame` scaled to `out_w`×`out_h` with the input's filter.
    fn resize(frame: Rgb, region: Region, out_w: usize, out_h: usize) -> Vec<u8> {
        let cols = filter_taps(region.x, region.width, frame.width, out_w);
        let rows = filter_taps(region.y, region.height, frame.height, out_h);
        let mut out = vec![0u8; out_w * out_h * 3];
        for (oy, (ry, wy)) in rows.iter().enumerate() {
            for (ox, (rx, wx)) in cols.iter().enumerate() {
                for c in 0..3 {
                    let mut acc = 0.0;
                    for (j, a) in wy.iter().enumerate() {
                        for (i, b) in wx.iter().enumerate() {
                            acc += frame.data[((ry + j) * frame.width + rx + i) * 3 + c] as f32
                                * a
                                * b;
                        }
                    }
                    out[(oy * out_w + ox) * 3 + c] = acc.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    /// How small a face the detector finds in a wide frame, with and without the tiles:
    /// real faces from the fixture photos (head and shoulders, 3× the face box) pasted into
    /// a 640×360 frame (the size the frame sampler decodes) at known heights, left, centre
    /// and right. `cargo test --release small_faces_on_wide_frames -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn small_faces_on_wide_frames() {
        let reference: Reference =
            serde_json::from_slice(&std::fs::read(faces_dir().join("reference.json")).unwrap())
                .unwrap();
        let mut detector = Detector::load(&face_model()).unwrap();
        let (fw, fh) = (640usize, 360usize);
        // Background: a soft gradient with texture, no faces.
        let background: Vec<u8> = (0..fw * fh)
            .flat_map(|i| {
                let (x, y) = (i % fw, i / fw);
                let t = ((x * 7 + y * 13) % 23) as u8;
                [
                    60 + (x * 80 / fw) as u8 + t,
                    70 + (y * 60 / fh) as u8 + t,
                    90 + t,
                ]
            })
            .collect();
        // Every face of the three close-up photos.
        let mut sources = Vec::new();
        for photo in reference
            .photos
            .iter()
            .filter(|p| !p.photo.starts_with("studio"))
        {
            let [w, h] = photo.size;
            let data = decode_image(&faces_dir().join(&photo.photo), w, h);
            for f in &photo.faces {
                let [x0, y0, x1, y1] = f.bbox;
                let (cx, cy, side) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (y1 - y0).max(x1 - x0));
                let crop_x0 = (cx - 1.5 * side).max(0.0) as usize;
                let crop_y0 = (cy - 1.5 * side).max(0.0) as usize;
                let crop_x1 = ((cx + 1.5 * side) as usize).min(w);
                let crop_y1 = ((cy + 1.5 * side) as usize).min(h);
                let region = Region {
                    x: crop_x0,
                    y: crop_y0,
                    width: crop_x1 - crop_x0,
                    height: crop_y1 - crop_y0,
                };
                let face_in_crop = [
                    x0 - crop_x0 as f32,
                    y0 - crop_y0 as f32,
                    x1 - crop_x0 as f32,
                    y1 - crop_y0 as f32,
                ];
                sources.push((
                    photo.photo.clone(),
                    data.clone(),
                    w,
                    h,
                    region,
                    face_in_crop,
                ));
            }
        }
        println!("{} faces from {} photos; frame {fw}×{fh}", sources.len(), 3);
        println!("face height | whole frame | + tiles when empty | ms whole / tiles");
        let mut results = Vec::new();
        for percent in [3.0f32, 4.0, 5.0, 6.0, 7.0, 8.0, 10.0, 12.0, 15.0, 20.0] {
            let (mut whole_hits, mut tiled_hits, mut total) = (0, 0, 0);
            let (mut whole_ms, mut tile_ms, mut tile_runs) = (0.0, 0.0, 0);
            for (_, data, w, h, region, face) in &sources {
                let source = Rgb {
                    data,
                    width: *w,
                    height: *h,
                };
                let scale = percent / 100.0 * fh as f32 / (face[3] - face[1]);
                let (cw, ch) = (
                    ((region.width as f32 * scale).round() as usize).clamp(1, fw),
                    ((region.height as f32 * scale).round() as usize).clamp(1, fh),
                );
                let patch = resize(source, *region, cw, ch);
                for at in [0.2f32, 0.5, 0.8] {
                    let px = ((at * fw as f32) as usize)
                        .saturating_sub(cw / 2)
                        .min(fw - cw);
                    let py = ((0.4 * fh as f32) as usize)
                        .saturating_sub(ch / 2)
                        .min(fh - ch);
                    let mut frame = background.clone();
                    for row in 0..ch {
                        frame[((py + row) * fw + px) * 3..][..cw * 3]
                            .copy_from_slice(&patch[row * cw * 3..][..cw * 3]);
                    }
                    let truth = Face {
                        score: 1.0,
                        x0: px as f32 + face[0] * cw as f32 / region.width as f32,
                        y0: py as f32 + face[1] * ch as f32 / region.height as f32,
                        x1: px as f32 + face[2] * cw as f32 / region.width as f32,
                        y1: py as f32 + face[3] * ch as f32 / region.height as f32,
                        keypoints: [[0.0; 2]; 6],
                    };
                    let rgb = Rgb {
                        data: &frame,
                        width: fw,
                        height: fh,
                    };
                    let found = |faces: &[Face]| faces.iter().any(|f| iou(f, &truth) > 0.3);
                    let start = std::time::Instant::now();
                    let whole = detector.detect_whole(rgb).unwrap();
                    whole_ms += start.elapsed().as_secs_f64() * 1000.0;
                    total += 1;
                    if found(&whole) {
                        whole_hits += 1;
                        tiled_hits += 1;
                    } else if whole.is_empty() {
                        let start = std::time::Instant::now();
                        let tiled = detector.detect_tiled(rgb).unwrap();
                        tile_ms += start.elapsed().as_secs_f64() * 1000.0;
                        tile_runs += 1;
                        if found(&tiled) {
                            tiled_hits += 1;
                        }
                    }
                }
            }
            println!(
                "{percent:>10}% | {whole_hits:>2}/{total} | {tiled_hits:>2}/{total} | {:.1} / {:.1}",
                whole_ms / total as f64,
                if tile_runs > 0 {
                    tile_ms / tile_runs as f64
                } else {
                    0.0
                }
            );
            results.push((percent, whole_hits, tiled_hits, total));
        }
        // Measured 2026-10-21: the whole frame finds every face from 15% of the frame
        // height, the tiles every face from 8% (17/18 at 7%, 13/18 at 6%, none under 5%).
        for &(percent, whole, tiled, total) in &results {
            if percent >= 15.0 {
                assert_eq!(whole, total, "whole frame, {percent}%");
            }
            if percent >= 8.0 {
                assert_eq!(tiled, total, "with tiles, {percent}%");
            }
        }
    }

    #[derive(Deserialize)]
    pub(crate) struct Truth {
        pub samples: Vec<TruthSample>,
    }

    #[derive(Deserialize)]
    pub(crate) struct TruthSample {
        pub t_ms: u64,
        pub faces: Vec<TruthFace>,
    }

    #[derive(Deserialize)]
    pub(crate) struct TruthFace {
        pub id: String,
        #[serde(rename = "box")]
        pub bbox: [f32; 4],
    }

    pub(crate) fn reframe_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/reframe")
            .join(name)
    }

    /// Every sample of a fixture video with the faces found in it.
    pub(crate) fn detect_video(name: &str) -> Vec<(u64, Vec<Face>)> {
        let video = reframe_fixture(name);
        let info = crate::probe::probe_video(&video).unwrap();
        let mut detector = Detector::load(&face_model()).unwrap();
        let mut found = Vec::new();
        crate::frames::sample(
            &video,
            (info.width, info.height),
            0,
            info.duration_ms,
            &AtomicBool::new(false),
            |t, frame| {
                found.push((t, detector.detect(frame)?));
                Ok(())
            },
        )
        .unwrap();
        found
    }

    /// The synthetic fixtures' faces are found where the truth says, every sample:
    /// `cargo test --release finds_the_synthetic_faces -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn finds_the_synthetic_faces() {
        for name in ["moving-face", "two-faces"] {
            let truth: Truth = serde_json::from_slice(
                &std::fs::read(reframe_fixture(&format!("{name}.json"))).unwrap(),
            )
            .unwrap();
            let found = detect_video(&format!("{name}.mp4"));
            assert_eq!(found.len(), truth.samples.len(), "{name}");
            let (mut hits, mut total, mut extra, mut worst) = (0, 0, 0, 1.0f32);
            for ((t, faces), sample) in found.iter().zip(&truth.samples) {
                assert_eq!(*t, sample.t_ms);
                for want in &sample.faces {
                    let [x0, y0, x1, y1] = want.bbox;
                    let want = Face {
                        score: 1.0,
                        x0,
                        y0,
                        x1,
                        y1,
                        keypoints: [[0.0; 2]; 6],
                    };
                    total += 1;
                    let best = faces.iter().map(|f| iou(f, &want)).fold(0.0, f32::max);
                    if best > 0.5 {
                        hits += 1;
                        worst = worst.min(best);
                    } else {
                        println!("{name} {t} ms: missed a face (best IoU {best:.2})");
                    }
                }
                extra += faces.len().saturating_sub(sample.faces.len());
            }
            println!("{name}: {hits}/{total} faces found, worst IoU {worst:.2}, {extra} extra");
            assert_eq!(hits, total, "{name}");
            assert_eq!(extra, 0, "{name}");
        }
    }

    /// What the detector sees in the real Pexels clips (no truth; prints a summary). Every
    /// sample has a face. Measured 2026-10-21: podcast-wide (faces ~10% of the height,
    /// turned sideways) finds both people in 45 of 80 samples and one in the rest (the
    /// tiles run only when nothing is found); the tracker bridges the gaps.
    /// `cargo test --release real_clip_faces -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_clip_faces() {
        for name in ["podcast-wide.mp4", "talking-head.mp4"] {
            let found = detect_video(name);
            let mut counts = [0usize; 4];
            for (_, faces) in &found {
                counts[faces.len().min(3)] += 1;
            }
            let heights: Vec<f32> = found
                .iter()
                .flat_map(|(_, faces)| faces.iter().map(|f| f.height() / 360.0 * 100.0))
                .collect();
            let mean = heights.iter().sum::<f32>() / heights.len().max(1) as f32;
            println!(
                "{name}: {} samples; 0/1/2/3+ faces: {counts:?}; mean face height {mean:.1}% of the frame",
                found.len()
            );
            assert_eq!(counts[0], 0, "{name}: samples without a face");
        }
    }

    /// The Rust pipeline finds the faces the export script found (Python, PIL resize):
    /// `cargo test matches_the_export_reference -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn matches_the_export_reference() {
        let reference: Reference =
            serde_json::from_slice(&std::fs::read(faces_dir().join("reference.json")).unwrap())
                .unwrap();
        let mut detector = Detector::load(&face_model()).unwrap();
        for photo in &reference.photos {
            let [w, h] = photo.size;
            let data = decode_image(&faces_dir().join(&photo.photo), w, h);
            let faces = detector
                .detect_whole(Rgb {
                    data: &data,
                    width: w,
                    height: h,
                })
                .unwrap();
            assert_eq!(faces.len(), photo.faces.len(), "{}", photo.photo);
            for (got, want) in faces.iter().zip(&photo.faces) {
                let [x0, y0, x1, y1] = want.bbox;
                let want_face = Face {
                    score: want.score,
                    x0,
                    y0,
                    x1,
                    y1,
                    keypoints: [[0.0; 2]; 6],
                };
                let overlap = iou(got, &want_face);
                let size = want_face.height();
                let mouth = want.keypoints["mouth"];
                let mouth_error =
                    (got.keypoints[MOUTH][0] - mouth[0]).hypot(got.keypoints[MOUTH][1] - mouth[1]);
                println!(
                    "{}: score {:.3} (want {:.3}), IoU {overlap:.3}, mouth off by {:.1}% of the face",
                    photo.photo,
                    got.score,
                    want.score,
                    100.0 * mouth_error / size
                );
                assert!(overlap > 0.9, "{}: IoU {overlap}", photo.photo);
                assert!((got.score - want.score).abs() < 0.05, "{}", photo.photo);
                assert!(mouth_error < 0.05 * size, "{}", photo.photo);
            }
        }
    }
}
