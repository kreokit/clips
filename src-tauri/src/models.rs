//! Model catalog, where a model is found, and the one-time model downloads.
//!
//! Models are downloaded only from our own mirror releases on kreokit/clips: the ggml
//! Whisper files from `models-v1` (made by `.github/workflows/mirror-whisper-models.yml`)
//! and the BlazeFace face detector from `models-v2` (made by `export-blazeface.yml`);
//! sources, licences and attribution are listed there and in THIRD-PARTY-LICENSES.md.
//! They are stored in `<app data dir>/models/` and checked against a pinned size and SHA-256.
//! These downloads are the app's only network calls.
//!
//! **Models from KreoKit Captions.** Captions downloads the same files into its own app
//! data folder. A model found there is used in place, read-only (Clips never writes into
//! that folder), after its SHA-256 has been checked once; the check is remembered in
//! `<models>/shared-checked.json` for as long as the file's size and modification time
//! stay the same. (Backlog: one shared KreoKit models folder for all apps.)

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CANCELLED: &str = "cancelled";

const RELEASES: &str = "https://github.com/kreokit/clips/releases/download";

/// One file in one of our mirror releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorFile {
    pub release: &'static str,
    pub name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
}

impl MirrorFile {
    fn url(self) -> String {
        format!("{RELEASES}/{}/{}", self.release, self.name)
    }

    pub fn path_in(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.name)
    }

    /// A file counts as downloaded once the complete file is in place. Downloads land in a
    /// `.part` file and are only renamed after the checksum matches.
    pub fn is_downloaded(self, models_dir: &Path) -> bool {
        has_size(&self.path_in(models_dir), self.size_bytes)
    }
}

/// MediaPipe BlazeFace full-range, converted to ONNX (`scripts/export_blazeface.py`,
/// byte-reproducible; workflow run 39524366294).

pub const FACE_MODEL: MirrorFile = MirrorFile {
    release: "models-v2",
    name: "blazeface_full_range.onnx",
    size_bytes: 2_067_035,
    sha256: "5be762f9d218872003bb780b5aec19a366bb883ee3fbd43ee27841d5604478e0",
};

/// Captions' bundle identifier; its app data folder sits next to ours.
const CAPTIONS_APP_ID: &str = "com.kreokit.captions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Base,
    Small,
    LargeV3Turbo,
}

impl WhisperModel {
    pub const ALL: [WhisperModel; 3] = [Self::Base, Self::Small, Self::LargeV3Turbo];

    /// The file in `models-v1` (unchanged from Hugging Face ggerganov/whisper.cpp; the
    /// mirror workflow checks the same sizes and SHA-256).
    pub fn file(self) -> MirrorFile {
        let (name, size_bytes, sha256) = match self {
            Self::Base => (
                "ggml-base.bin",
                147_951_465,
                "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            ),
            Self::Small => (
                "ggml-small.bin",
                487_601_967,
                "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            ),
            Self::LargeV3Turbo => (
                "ggml-large-v3-turbo.bin",
                1_624_555_275,
                "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            ),
        };
        MirrorFile {
            release: "models-v1",
            name,
            size_bytes,
            sha256,
        }
    }
}

fn has_size(path: &Path, size: u64) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size)
}

/// Where the app looks for models: its own folder, and Captions' (read-only).
#[derive(Debug, Clone)]
pub struct ModelDirs {
    pub ours: PathBuf,
    pub captions: Option<PathBuf>,
}

impl ModelDirs {
    /// `app_data_dir` is ours (`…/com.kreokit.clips`); Captions' is its sibling.
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            ours: app_data_dir.join("models"),
            captions: app_data_dir
                .parent()
                .map(|parent| parent.join(CAPTIONS_APP_ID).join("models")),
        }
    }

    fn shared_checks(&self) -> PathBuf {
        self.ours.join("shared-checked.json")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelSource {
    /// Downloaded by Clips.
    Ours,
    /// Found in KreoKit Captions' folder (its checksum is checked before first use).
    Captions,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: WhisperModel,
    pub size_bytes: u64,
    /// Where the model is, or `None` if it must be downloaded first.
    pub source: Option<ModelSource>,
}

/// A quick look (file sizes only, no checksums), for the model picker.
pub fn list(dirs: &ModelDirs) -> Vec<ModelInfo> {
    WhisperModel::ALL
        .iter()
        .map(|&id| ModelInfo {
            id,
            size_bytes: id.file().size_bytes,
            source: find(id, dirs).map(|(_, source)| source),
        })
        .collect()
}

/// Our own complete file first, else a file of the right size in Captions' folder.
fn find(model: WhisperModel, dirs: &ModelDirs) -> Option<(PathBuf, ModelSource)> {
    let file = model.file();
    if file.is_downloaded(&dirs.ours) {
        return Some((file.path_in(&dirs.ours), ModelSource::Ours));
    }
    let shared = file.path_in(dirs.captions.as_deref()?);
    has_size(&shared, file.size_bytes).then_some((shared, ModelSource::Captions))
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// The model file to load, or `None` if it has to be downloaded. A file from Captions is
/// checksummed the first time (reported through `on_check`); a damaged one is skipped.
/// Returns [`CANCELLED`] if `cancel` is set during the check.
pub fn resolve(
    model: WhisperModel,
    dirs: &ModelDirs,
    cancel: &AtomicBool,
    on_check: impl FnMut(Progress),
) -> Result<Option<PathBuf>, String> {
    match find(model, dirs) {
        None => Ok(None),
        Some((path, ModelSource::Ours)) => Ok(Some(path)),
        Some((path, ModelSource::Captions)) => {
            let ok = check_shared(
                &path,
                model.file().size_bytes,
                model.file().sha256,
                &dirs.shared_checks(),
                cancel,
                on_check,
            )?;
            Ok(ok.then_some(path))
        }
    }
}

/// A shared file that passed the checksum, as it was then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SharedCheck {
    path: PathBuf,
    size: u64,
    mtime_ms: u64,
    sha256: String,
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Whether the shared file at `path` has the expected size and SHA-256. Only reads it;
/// a passed check is remembered in `checks_file` (one of our own files).
fn check_shared(
    path: &Path,
    size: u64,
    sha256: &str,
    checks_file: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<bool, String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let current = SharedCheck {
        path: path.to_path_buf(),
        size: meta.len(),
        mtime_ms: mtime_ms(&meta),
        sha256: sha256.to_string(),
    };
    let mut checks: Vec<SharedCheck> = fs::read(checks_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if checks.contains(&current) {
        return Ok(true);
    }
    if current.size != size {
        return Ok(false);
    }

    let file = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match copy_verified(file, std::io::sink(), size, sha256, cancel, on_progress) {
        Ok(()) => {}
        Err(e) if e == CORRUPT => return Ok(false),
        Err(e) => return Err(e),
    }

    // Remembering is only a shortcut: if it can't be written, the next run checks again.
    checks.retain(|c| c.path != current.path);
    checks.push(current);
    if let Some(dir) = checks_file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&checks) {
        let tmp = checks_file.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, checks_file);
        }
    }
    Ok(true)
}

/// Downloads `file` from our mirror into `models_dir`, verifying size and SHA-256.
/// `on_progress` is called roughly every 1 MB. Returns [`CANCELLED`] if `cancel` is set.
pub fn download(
    file: MirrorFile,
    models_dir: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let target = file.path_in(models_dir);
    if file.is_downloaded(models_dir) {
        return Ok(target);
    }
    fs::create_dir_all(models_dir).map_err(|e| format!("cannot create models folder: {e}"))?;
    let part = models_dir.join(format!("{}.part", file.name));

    let result = (|| {
        let response = ureq::get(&file.url())
            .call()
            .map_err(|e| format!("model download failed: {e}"))?;
        let reader = response.into_body().into_reader();
        let out = File::create(&part).map_err(|e| format!("cannot write model file: {e}"))?;
        copy_verified(
            reader,
            BufWriter::new(out),
            file.size_bytes,
            file.sha256,
            cancel,
            on_progress,
        )
        .map_err(|e| match e.as_str() {
            CORRUPT => format!("the downloaded {} is damaged; please try again", file.name),
            _ => e,
        })?;
        match fs::rename(&part, &target) {
            Ok(()) => Ok(target.clone()),
            // Another download of the same file finished first (and took the `.part`).
            Err(_) if file.is_downloaded(models_dir) => Ok(target.clone()),
            Err(e) => Err(format!("cannot save model file: {e}")),
        }
    })();

    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

const CORRUPT: &str = "corrupt";

/// Copies `reader` into `out`, reporting progress, and checks the size and SHA-256 at
/// the end ([`CORRUPT`] if they don't match).
fn copy_verified(
    mut reader: impl Read,
    mut out: impl Write,
    total_bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    let mut hasher = Sha256::new();
    let mut done_bytes = 0u64;
    let mut last_reported = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("model download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| format!("cannot write model file: {e}"))?;
        done_bytes += n as u64;
        if done_bytes - last_reported >= 1 << 20 || done_bytes == total_bytes {
            last_reported = done_bytes;
            on_progress(Progress {
                done_bytes,
                total_bytes,
            });
        }
    }
    out.flush()
        .map_err(|e| format!("cannot write model file: {e}"))?;
    if done_bytes != total_bytes || to_hex(&hasher.finalize()) != sha256 {
        return Err(CORRUPT.into());
    }
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Clips' app data dir on macOS, used by the ignored tests that need a real model.
    /// Override with `KREOKIT_APP_DATA`.
    pub(crate) fn dev_dirs() -> ModelDirs {
        let app_data = std::env::var_os("KREOKIT_APP_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join("Library/Application Support/com.kreokit.clips")
            });
        ModelDirs::new(&app_data)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-clips-models-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `<dir>/com.kreokit.clips` and its Captions sibling, both with an empty models folder.
    fn temp_dirs(name: &str) -> ModelDirs {
        let dirs = ModelDirs::new(&temp_dir(name).join("com.kreokit.clips"));
        fs::create_dir_all(&dirs.ours).unwrap();
        fs::create_dir_all(dirs.captions.as_ref().unwrap()).unwrap();
        dirs
    }

    /// A stand-in file of the right size (`find` checks sizes only).
    fn fake(dir: &Path, model: WhisperModel) {
        File::create(model.file().path_in(dir))
            .unwrap()
            .set_len(model.file().size_bytes)
            .unwrap();
    }

    #[test]
    fn model_ids_match_the_frontend() {
        let ids: Vec<String> = WhisperModel::ALL
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        assert_eq!(ids, [r#""base""#, r#""small""#, r#""large-v3-turbo""#]);
    }

    #[test]
    fn downloads_only_from_our_mirror() {
        assert_eq!(
            WhisperModel::LargeV3Turbo.file().url(),
            "https://github.com/kreokit/clips/releases/download/models-v1/ggml-large-v3-turbo.bin"
        );
        assert_eq!(
            FACE_MODEL.url(),
            "https://github.com/kreokit/clips/releases/download/models-v2/blazeface_full_range.onnx"
        );
    }

    #[test]
    fn captions_folder_is_our_sibling() {
        let dirs = ModelDirs::new(Path::new("/data/com.kreokit.clips"));
        assert_eq!(dirs.ours, Path::new("/data/com.kreokit.clips/models"));
        assert_eq!(
            dirs.captions.as_deref(),
            Some(Path::new("/data/com.kreokit.captions/models"))
        );
    }

    #[test]
    fn partial_file_is_not_downloaded() {
        let dirs = temp_dirs("partial");
        fs::write(WhisperModel::Base.file().path_in(&dirs.ours), b"truncated").unwrap();
        assert!(!WhisperModel::Base.file().is_downloaded(&dirs.ours));
        assert_eq!(list(&dirs)[0].source, None);
    }

    #[test]
    fn finds_our_model_first_then_captions() {
        let dirs = temp_dirs("find");
        let captions = dirs.captions.clone().unwrap();
        let sources = |dirs: &ModelDirs| list(dirs).iter().map(|m| m.source).collect::<Vec<_>>();
        assert_eq!(sources(&dirs), [None, None, None]);

        fake(&captions, WhisperModel::Small);
        fs::write(WhisperModel::Base.file().path_in(&captions), b"truncated").unwrap();
        assert_eq!(sources(&dirs), [None, Some(ModelSource::Captions), None]);

        fake(&dirs.ours, WhisperModel::Small);
        assert_eq!(sources(&dirs), [None, Some(ModelSource::Ours), None]);

        // No Captions folder at all (no parent directory).
        let alone = ModelDirs {
            ours: dirs.ours.clone(),
            captions: None,
        };
        assert_eq!(sources(&alone), [None, Some(ModelSource::Ours), None]);
    }

    #[test]
    fn resolve_never_checks_our_own_files() {
        let dirs = temp_dirs("resolve");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            resolve(WhisperModel::Base, &dirs, &cancel, |_| {}),
            Ok(None)
        );
        fake(&dirs.ours, WhisperModel::Base);
        assert_eq!(
            resolve(WhisperModel::Base, &dirs, &cancel, |_| panic!("checked")),
            Ok(Some(WhisperModel::Base.file().path_in(&dirs.ours)))
        );
        // A same-sized fake in Captions' folder fails the checksum and isn't used.
        fake(dirs.captions.as_ref().unwrap(), WhisperModel::Small);
        assert_eq!(
            resolve(WhisperModel::Small, &dirs, &cancel, |_| {}),
            Ok(None)
        );
    }

    #[test]
    fn checks_a_shared_file_once_and_only_reads_it() {
        let dir = temp_dir("shared");
        let shared = dir.join("captions").join("m.bin");
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(&shared, b"hello model").unwrap();
        let sha = to_hex(&Sha256::digest(b"hello model"));
        let checks = dir.join("ours").join("shared-checked.json");
        let cancel = AtomicBool::new(false);
        let mut hashed = 0;
        let mut check =
            |sha: &str| check_shared(&shared, 11, sha, &checks, &cancel, |_| hashed += 1).unwrap();

        assert!(!check(&"0".repeat(64)));
        assert!(!checks.exists(), "a failed check isn't remembered");
        assert!(check(&sha));
        assert!(check(&sha)); // remembered: not read again
        drop(check);
        assert_eq!(hashed, 2);
        assert_eq!(fs::read(&shared).unwrap(), b"hello model");
        assert_eq!(
            fs::read_dir(shared.parent().unwrap()).unwrap().count(),
            1,
            "nothing is written next to the shared file"
        );

        // A changed file is checked again. (Set the time: on Linux a quick rewrite can keep
        // the same modification time.)
        fs::write(&shared, b"hello mode!").unwrap();
        File::options()
            .write(true)
            .open(&shared)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        assert!(!check_shared(&shared, 11, &sha, &checks, &cancel, |_| {}).unwrap());
        // A wrong size fails without reading.
        assert!(!check_shared(&shared, 12, &sha, &checks, &cancel, |_| panic!()).unwrap());
        // Cancelling stops the check.
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            check_shared(&shared, 11, &sha, &checks, &cancel, |_| {}).unwrap_err(),
            CANCELLED
        );
    }

    #[test]
    fn verifies_size_and_checksum() {
        let dir = temp_dir("verify");
        let data = b"hello model";
        let good = to_hex(&Sha256::digest(data));
        let bad = "0".repeat(64);
        let cancel = AtomicBool::new(false);
        let mut reports = Vec::new();
        let file = || File::create(dir.join("m.part")).unwrap();
        copy_verified(&data[..], file(), 11, &good, &cancel, |p| {
            reports.push(p.done_bytes)
        })
        .unwrap();
        assert_eq!(reports, [11]);
        assert_eq!(fs::read(dir.join("m.part")).unwrap(), data);
        assert_eq!(
            copy_verified(&data[..], file(), 11, &bad, &cancel, |_| {}).unwrap_err(),
            CORRUPT
        );
        assert_eq!(
            copy_verified(&data[..], file(), 12, &good, &cancel, |_| {}).unwrap_err(),
            CORRUPT
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            copy_verified(&data[..], file(), 11, &good, &cancel, |_| {}).unwrap_err(),
            CANCELLED
        );
    }

    #[test]
    fn hex_encodes() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xab]), "000fab");
    }

    /// Downloads the base model (148 MB) from the mirror into a temporary folder:
    /// `cargo test downloads_base_model -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_base_model() {
        let dirs = temp_dirs("download");
        let cancel = AtomicBool::new(false);
        let path = download(WhisperModel::Base.file(), &dirs.ours, &cancel, |_| {}).unwrap();
        assert!(
            WhisperModel::Base
                .file()
                .is_downloaded(path.parent().unwrap())
        );
        let _ = fs::remove_file(path); // 148 MB; disk is tight on the dev Mac
    }

    /// Uses Captions' real base model, if this computer has it:
    /// `cargo test reuses_captions_base_model -- --ignored`.
    #[test]
    #[ignore]
    fn reuses_captions_base_model() {
        let real = dev_dirs();
        let Some(captions) = real.captions else {
            return;
        };
        if !WhisperModel::Base.file().path_in(&captions).exists() {
            eprintln!("no Captions base model here; skipped");
            return;
        }
        let dirs = ModelDirs {
            ours: temp_dir("reuse"),
            captions: Some(captions.clone()),
        };
        let cancel = AtomicBool::new(false);
        let path = resolve(WhisperModel::Base, &dirs, &cancel, |_| {}).unwrap();
        assert_eq!(path, Some(WhisperModel::Base.file().path_in(&captions)));
        assert!(dirs.shared_checks().exists());
    }
}
