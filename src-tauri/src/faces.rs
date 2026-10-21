//! Face detection for the reframe: MediaPipe BlazeFace full-range, run with ONNX Runtime.
//!
//! The model ([`models::FACE_MODEL`], from our `models-v2` mirror) takes one 192×192 RGB
//! frame (NHWC, values in [-1, 1]) and returns box + keypoint regressors [1, 2304, 16] and
//! score logits [1, 2304, 1], one per SSD anchor. Its tensor names and shapes are checked
//! when it loads, so a wrong file fails there rather than producing nonsense.
//!
//! [`models::FACE_MODEL`]: crate::models::FACE_MODEL

// Used by the face analysis job from M4 step 8 on.
#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::ValueType;

/// Model input side in pixels.
pub const INPUT_SIZE: usize = 192;
/// SSD anchors: one per 4×4 cell of the input.
pub const NUM_ANCHORS: usize = (INPUT_SIZE / 4) * (INPUT_SIZE / 4);
/// Box (4) + six keypoints (12) per anchor.
pub const NUM_COORDS: usize = 16;

const INPUT: &str = "input";
const REGRESSORS: &str = "reshaped_regressor_face_4";
const SCORES: &str = "reshaped_classifier_face_4";

/// The loaded face model.
pub struct Detector {
    session: Session,
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
        Ok(Self { session })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{self, FACE_MODEL};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    /// The face model in the app's models folder (`models::tests::dev_dirs`), downloaded
    /// there if missing (2 MB). Elsewhere than macOS, a temporary folder.
    pub(crate) fn face_model() -> PathBuf {
        let dir = if cfg!(target_os = "macos") || std::env::var_os("KREOKIT_APP_DATA").is_some() {
            models::tests::dev_dirs().ours
        } else {
            std::env::temp_dir().join("kreokit-clips-face-model")
        };
        models::download(FACE_MODEL, &dir, &AtomicBool::new(false), |_| {}).unwrap()
    }

    /// `cargo test face_model_loads -- --ignored` (downloads the model from models-v2).
    #[test]
    #[ignore]
    fn face_model_loads() {
        Detector::load(&face_model()).unwrap();
    }

    #[test]
    fn a_wrong_file_is_refused() {
        let path = std::env::temp_dir().join("kreokit-clips-not-a-model.onnx");
        std::fs::write(&path, b"not a model").unwrap();
        let error = Detector::load(&path).err().unwrap();
        assert!(error.starts_with("cannot load the face model"), "{error}");
    }
}
