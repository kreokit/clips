//! The face analysis job and its cache: sampled frames → faces + camera cuts, stored per
//! 10-second block of the video in `<project>/faces/`, so clips that overlap, or a clip
//! trimmed in the editor, reuse what's done.
//!
//! - `meta.json` records what the blocks depend on (cache version, face model, sample size
//!   and rate, the video's size and modification time); any difference discards them all.
//! - `block-NNNNN.json` holds the samples of one block ([`BLOCK_MS`]), written atomically
//!   as soon as it is complete, so Cancel (or quitting) loses at most the current block.
//! - A run decodes consecutive missing blocks with one FFmpeg process, starting one sample
//!   early (except at 0:00) so the first sample's cut flag compares with the frame before.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::faces::Detector;
use crate::frames::{self, Look, STEP_MS};
use crate::models::CANCELLED;
use crate::tracks::Sample;

/// Video per cache block.
pub const BLOCK_MS: u64 = 10_000;
const VERSION: u32 = 1;
const META: &str = "meta.json";

/// What the cache depends on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Meta {
    version: u32,
    model_sha256: String,
    sample_size: (usize, usize),
    step_ms: u64,
    video_size: u64,
    video_mtime_ms: u64,
}

/// The facts about the video the job needs.
#[derive(Debug, Clone)]
pub struct Video {
    pub path: PathBuf,
    /// Display size (rotation applied).
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub size: u64,
    pub mtime_ms: u64,
}

impl Video {
    fn meta(&self) -> Meta {
        Meta {
            version: VERSION,
            model_sha256: crate::models::FACE_MODEL.sha256.to_string(),
            sample_size: frames::sample_size(self.width, self.height),
            step_ms: STEP_MS,
            video_size: self.size,
            video_mtime_ms: self.mtime_ms,
        }
    }

    fn blocks(&self) -> u64 {
        self.duration_ms.div_ceil(BLOCK_MS).max(1)
    }
}

fn block_path(dir: &Path, n: u64) -> PathBuf {
    dir.join(format!("block-{n:05}.json"))
}

/// Discards the folder's blocks if they were made for something else.
fn check_meta(dir: &Path, video: &Video) -> Result<(), String> {
    let wanted = video.meta();
    let current: Option<Meta> = fs::read(dir.join(META))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if current.as_ref() == Some(&wanted) {
        return Ok(());
    }
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    write_atomic(&dir.join(META), &wanted)
}

fn write_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    fs::write(&tmp, json)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("cannot save {}: {e}", path.display()))
}

fn read_block(dir: &Path, n: u64) -> Option<Vec<Sample>> {
    serde_json::from_slice(&fs::read(block_path(dir, n)).ok()?).ok()
}

/// The blocks a time range touches.
fn blocks_of(video: &Video, (from, to): (u64, u64)) -> std::ops::Range<u64> {
    let to = to.min(video.duration_ms);
    if to <= from {
        return 0..0;
    }
    (from / BLOCK_MS)..to.div_ceil(BLOCK_MS).min(video.blocks())
}

/// The blocks of `ranges` not analysed yet, in order (checks the cache first).
pub fn missing(dir: &Path, video: &Video, ranges: &[(u64, u64)]) -> Result<Vec<u64>, String> {
    check_meta(dir, video)?;
    let mut blocks: Vec<u64> = ranges
        .iter()
        .flat_map(|&r| blocks_of(video, r))
        .filter(|&n| !block_path(dir, n).is_file())
        .collect();
    blocks.sort_unstable();
    blocks.dedup();
    Ok(blocks)
}

/// The samples from `from_ms` to `to_ms`, if every block they need is analysed.
pub fn samples(dir: &Path, video: &Video, from_ms: u64, to_ms: u64) -> Option<Vec<Sample>> {
    let meta: Meta = serde_json::from_slice(&fs::read(dir.join(META)).ok()?).ok()?;
    if meta != video.meta() {
        return None;
    }
    let mut out = Vec::new();
    for n in blocks_of(video, (from_ms, to_ms)) {
        out.extend(
            read_block(dir, n)?
                .into_iter()
                .filter(|s| (from_ms..to_ms).contains(&s.t_ms)),
        );
    }
    Some(out)
}

/// Progress of [`run`]: blocks done of the blocks this run needs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// Analyses the missing blocks of `ranges`. `load` gives the detector (called only if
/// there is work). Returns [`CANCELLED`] when `cancel` is set; finished blocks are kept.
pub fn run(
    dir: &Path,
    video: &Video,
    ranges: &[(u64, u64)],
    load: impl FnOnce() -> Result<Detector, String>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    let todo = missing(dir, video, ranges)?;
    let total = todo.len();
    on_progress(Progress { done: 0, total });
    if todo.is_empty() {
        return Ok(());
    }
    let mut detector = load()?;
    let mut done = 0;
    // Runs of consecutive blocks: one FFmpeg process each.
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for n in todo {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == n => *last = n,
            _ => runs.push((n, n)),
        }
    }
    for (first, last) in runs {
        let start = first * BLOCK_MS;
        let end = ((last + 1) * BLOCK_MS).min(video.duration_ms);
        // One sample before the block, for the first cut flag.
        let primed_from = start.saturating_sub(STEP_MS);
        let mut previous: Option<Look> = None;
        let mut block = first;
        let mut current: Vec<Sample> = Vec::new();
        frames::sample(
            &video.path,
            (video.width, video.height),
            primed_from,
            end,
            cancel,
            |t_ms, frame| {
                let look = Look::of(frame);
                let cut = previous.as_ref().is_some_and(|p| look.is_cut_from(p));
                previous = Some(look);
                if t_ms < start {
                    return Ok(());
                }
                while t_ms >= (block + 1) * BLOCK_MS {
                    write_atomic(&block_path(dir, block), &current)?;
                    current.clear();
                    block += 1;
                    done += 1;
                    on_progress(Progress { done, total });
                }
                current.push(Sample {
                    t_ms,
                    faces: detector.detect(frame)?,
                    cut,
                });
                Ok(())
            },
        )?;
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        // The run's last block (possibly the video's short last one).
        write_atomic(&block_path(dir, block), &current)?;
        done += 1;
        on_progress(Progress { done, total });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::tests::{face_model, reframe_fixture};

    fn video(name: &str) -> Video {
        let path = reframe_fixture(name);
        let info = crate::probe::probe_video(&path).unwrap();
        let meta = fs::metadata(&path).unwrap();
        Video {
            width: info.width,
            height: info.height,
            duration_ms: info.duration_ms,
            size: meta.len(),
            mtime_ms: 1,
            path,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-clips-analysis-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn fake(duration_ms: u64) -> Video {
        Video {
            path: PathBuf::from("/nowhere.mp4"),
            width: 1920,
            height: 1080,
            duration_ms,
            size: 1,
            mtime_ms: 1,
        }
    }

    #[test]
    fn ranges_map_to_blocks() {
        let v = fake(35_000);
        assert_eq!(blocks_of(&v, (0, 10_000)), 0..1);
        assert_eq!(blocks_of(&v, (9_999, 10_001)), 0..2);
        assert_eq!(blocks_of(&v, (31_000, 99_000)), 3..4);
        assert_eq!(blocks_of(&v, (5_000, 5_000)), 0..0);
    }

    #[test]
    fn a_changed_video_or_model_discards_the_cache() {
        let dir = temp_dir("meta");
        let v = fake(25_000);
        assert_eq!(missing(&dir, &v, &[(0, 25_000)]).unwrap(), [0, 1, 2]);
        write_atomic(&block_path(&dir, 1), &Vec::<Sample>::new()).unwrap();
        assert_eq!(missing(&dir, &v, &[(0, 25_000)]).unwrap(), [0, 2]);
        assert_eq!(samples(&dir, &v, 10_000, 20_000), Some(vec![]));
        let mut changed = v.clone();
        changed.mtime_ms = 2;
        assert_eq!(missing(&dir, &changed, &[(0, 25_000)]).unwrap(), [0, 1, 2]);
        assert!(!block_path(&dir, 1).exists());
        assert_eq!(samples(&dir, &v, 10_000, 20_000), None);
    }

    /// The job's cached samples equal a straight analysis of the whole video, cut flags at
    /// block edges included; a second run does nothing.
    #[test]
    #[ignore]
    fn cached_blocks_match_a_straight_analysis() {
        let v = video("moving-face.mp4");
        let dir = temp_dir("match");
        let cancel = AtomicBool::new(false);
        let mut reports = Vec::new();
        // Analyse the second block first, then everything: two runs, a primed edge.
        run(
            &dir,
            &v,
            &[(10_000, 12_000)],
            || Detector::load(&face_model()),
            &cancel,
            |p| reports.push(p),
        )
        .unwrap();
        run(
            &dir,
            &v,
            &[(0, 12_000)],
            || Detector::load(&face_model()),
            &cancel,
            |p| reports.push(p),
        )
        .unwrap();
        assert_eq!(
            reports,
            [
                Progress { done: 0, total: 1 },
                Progress { done: 1, total: 1 },
                Progress { done: 0, total: 1 },
                Progress { done: 1, total: 1 },
            ]
        );
        let cached = samples(&dir, &v, 0, v.duration_ms).unwrap();
        let straight = crate::tracks::tests::analyse_fixture("moving-face.mp4");
        assert_eq!(cached.len(), straight.len());
        for (a, b) in cached.iter().zip(&straight) {
            assert_eq!(
                (a.t_ms, a.cut, a.faces.len()),
                (b.t_ms, b.cut, b.faces.len())
            );
        }
        // Nothing left to do, and the detector isn't even loaded.
        run(
            &dir,
            &v,
            &[(0, 12_000)],
            || panic!("loaded"),
            &cancel,
            |_| {},
        )
        .unwrap();
    }

    #[test]
    #[ignore]
    fn cancel_keeps_finished_blocks() {
        let v = video("podcast-wide.mp4"); // 20 s: two blocks
        let dir = temp_dir("cancel");
        let cancel = AtomicBool::new(false);
        let result = run(
            &dir,
            &v,
            &[(0, 20_000)],
            || Detector::load(&face_model()),
            &cancel,
            |p| {
                if p.done == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert_eq!(missing(&dir, &v, &[(0, 20_000)]).unwrap(), [1]);
        cancel.store(false, Ordering::Relaxed);
        run(
            &dir,
            &v,
            &[(0, 20_000)],
            || Detector::load(&face_model()),
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(samples(&dir, &v, 0, 20_000).unwrap().len(), 80);
    }
}
