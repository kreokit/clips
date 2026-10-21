//! Auto-saved projects in the app data folder (no Save dialog, CLAUDE.md "Projects").
//!
//! Each imported video gets `<app data>/projects/<id>/project.json`, next to its caches
//! (thumbnail now; transcript, candidates and face tracks in later milestones). The id is
//! a hash of the video path, size and modification time, so importing the same video again
//! opens its project. Home's recent list is this folder, newest first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ffmpeg;
use crate::moments::{FinderSettings, Moment};
use crate::probe::VideoInfo;
use crate::transcription::Settings;

/// Current `project.json` format version. Bump it when the format changes incompatibly,
/// and teach [`load`] to read the older versions. New optional fields don't need a bump
/// (`#[serde(default)]`).
pub const FORMAT_VERSION: u32 = 1;

const PROJECT_FILE: &str = "project.json";
const THUMBNAIL_FILE: &str = "thumbnail.jpg";
/// The finder settings chosen last (in any project), for new projects; in the projects
/// folder, next to the project folders.
const LAST_FINDER_FILE: &str = "last-finder.json";
/// A project's transcript cache, inside its folder (see `transcription.rs`).
pub const TRANSCRIPT_DIR: &str = "transcript";
/// The face analysis cache (`analysis.rs`).
pub const FACES_DIR: &str = "faces";
/// Transcripts of deleted projects, by the id a new project of the same video would get,
/// so opening that video again doesn't transcribe it again. In the projects folder (not
/// a project: its name isn't an id).
const KEPT_DIR: &str = "kept-transcripts";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub id: String,
    /// Shown on Home and used for export names: the video's file name without extension.
    pub name: String,
    pub video_path: String,
    /// Size and modification time of the video when it was imported or located.
    pub video_size: u64,
    pub video_mtime_ms: u64,
    pub video: VideoInfo,
    /// The Transcribe screen's choices (missing in projects made before milestone 2).
    #[serde(default)]
    pub transcription: Settings,
    /// What the moment finder looks for (milestone 3).
    #[serde(default)]
    pub finder: FinderSettings,
    /// The clips board: proposed, kept and discarded clips, found or made by hand.
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Unix milliseconds.
    pub created_at: u64,
    pub opened_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipStatus {
    Proposed,
    Kept,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipSource {
    /// Proposed by the moment finder.
    Found,
    /// Made by hand.
    Manual,
}

/// A clip on the board. Its in/out points are on the video's clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Unique within the project.
    pub id: u32,
    pub in_ms: u64,
    pub out_ms: u64,
    pub status: ClipStatus,
    pub source: ClipSource,
    /// The user moved its in or out point; "Find again" leaves it alone.
    #[serde(default)]
    pub edited: bool,
    /// Its score, reasons and words for the current in/out (`None`: no words inside).
    #[serde(default)]
    pub moment: Option<Moment>,
}

/// A project plus what the Home screen needs to show it.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectEntry {
    #[serde(flatten)]
    pub project: Project,
    /// The video is still at `video_path`; otherwise Home offers "Locate video…".
    pub video_exists: bool,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Size and modification time (Unix ms) of the file at `path`.
fn fingerprint(path: &Path) -> Result<(u64, u64), String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    Ok((meta.len(), mtime))
}

/// The project id of a video: 16 hex digits of SHA-256 over path, size and mtime (stable
/// across Rust versions, unlike `DefaultHasher`).
pub fn project_id(video_path: &str, size: u64, mtime_ms: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(video_path.as_bytes());
    hasher.update([0]);
    hasher.update(size.to_le_bytes());
    hasher.update(mtime_ms.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Ids come from the webview, so only accept the exact format we make: never a path.
pub fn project_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(root.join(id))
    } else {
        Err(format!("invalid project id {id:?}"))
    }
}

/// The video path as stored: absolute and without `..` or symlinks where possible, so the
/// same file always gets the same id.
fn normalize(video: &Path) -> String {
    fs::canonicalize(video)
        .unwrap_or_else(|_| video.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn display_name(video: &Path) -> String {
    video
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into())
}

/// Writes `bytes` to `path` through a temporary file and a rename, so a crash never leaves
/// a half-written file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)
        .and_then(|_| fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("cannot save {}: {e}", path.display())
        })
}

pub fn load(root: &Path, id: &str) -> Result<Project, String> {
    let file = project_dir(root, id)?.join(PROJECT_FILE);
    let json = fs::read_to_string(&file).map_err(|e| format!("cannot open project: {e}"))?;
    let project: Project =
        serde_json::from_str(&json).map_err(|e| format!("damaged project file: {e}"))?;
    if project.version > FORMAT_VERSION {
        return Err("this project was saved by a newer version of KreoKit Clips".into());
    }
    if project.id != id {
        return Err("damaged project file: wrong id".into());
    }
    Ok(Project {
        finder: project.finder.clone().upgraded(),
        ..project
    })
}

/// The finder settings for a new project: the last ones chosen, else the default
/// (TikTok Rewards).
pub fn last_finder(root: &Path) -> FinderSettings {
    fs::read(root.join(LAST_FINDER_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<FinderSettings>(&b).ok())
        .map(FinderSettings::upgraded)
        .unwrap_or_default()
}

/// Remembers `finder` for new projects.
pub fn remember_finder(root: &Path, finder: &FinderSettings) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let json = serde_json::to_vec_pretty(finder).map_err(|e| e.to_string())?;
    write_atomic(&root.join(LAST_FINDER_FILE), &json)
}

/// Saves `project` (the autosave). Returns it with `updated_at` set.
pub fn save(root: &Path, project: &Project, now: u64) -> Result<Project, String> {
    let dir = project_dir(root, &project.id)?;
    if !dir.join(PROJECT_FILE).is_file() {
        return Err("this project was removed".into());
    }
    let project = Project {
        version: FORMAT_VERSION,
        updated_at: now,
        ..project.clone()
    };
    write_project(&dir, &project)?;
    Ok(project)
}

fn write_project(dir: &Path, project: &Project) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create project folder: {e}"))?;
    let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(PROJECT_FILE), (json + "\n").as_bytes())
}

/// Moves the project's clips `ms` later (with its transcript, see
/// `transcription::upgrade`), within the video, and saves it.
pub fn shift_clips(root: &Path, project: Project, ms: u64) -> Result<Project, String> {
    let end = project.video.duration_ms;
    let later = |t: u64| (t + ms).min(end);
    let clips = project
        .clips
        .into_iter()
        .map(|c| Clip {
            in_ms: later(c.in_ms),
            out_ms: later(c.out_ms),
            moment: c.moment.map(|mut m| {
                (m.in_ms, m.out_ms) = (later(m.in_ms), later(m.out_ms));
                m
            }),
            ..c
        })
        .collect();
    let project = Project { clips, ..project };
    write_project(&project_dir(root, &project.id)?, &project)?;
    Ok(project)
}

/// Removes other projects of the same video that hold nothing (no clips, no transcript),
/// left by earlier versions that made a new project whenever the file's time changed.
fn remove_empty_duplicates(root: &Path, project: &Project, others: &[Project]) {
    for p in others {
        let Ok(dir) = project_dir(root, &p.id) else {
            continue;
        };
        if p.id != project.id
            && p.video_path == project.video_path
            && p.clips.is_empty()
            && !dir.join(TRANSCRIPT_DIR).exists()
        {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// Every readable project, most recently opened first. Damaged folders are skipped.
fn all(root: &Path) -> Vec<Project> {
    let mut projects: Vec<Project> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| load(root, &entry.file_name().to_string_lossy()).ok())
        .collect();
    projects.sort_by(|a, b| b.opened_at.cmp(&a.opened_at).then(a.id.cmp(&b.id)));
    projects
}

pub fn list(root: &Path) -> Vec<ProjectEntry> {
    all(root)
        .into_iter()
        .map(|project| ProjectEntry {
            video_exists: Path::new(&project.video_path).is_file(),
            project,
        })
        .collect()
}

/// Opens the project of `video`, creating it (with `probe`) on first import. A video
/// that was located somewhere else keeps its project, found by path and fingerprint.
pub fn open_video(
    root: &Path,
    video: &Path,
    probe: impl FnOnce(&Path) -> Result<VideoInfo, String>,
    now: u64,
) -> Result<Project, String> {
    let video_path = normalize(video);
    let (size, mtime) = fingerprint(Path::new(&video_path))?;
    let id = project_id(&video_path, size, mtime);

    // Its project; else the project of the same file that has changed since (rewritten,
    // re-exported, touched), which is refreshed rather than duplicated. Its transcript
    // is checked against the new size and time when it is used.
    let same_file = |p: &Project| p.video_size == size && p.video_mtime_ms == mtime;
    let others = all(root);
    let existing = load(root, &id)
        .ok()
        .or_else(|| {
            others
                .iter()
                .find(|p| p.video_path == video_path && same_file(p))
                .cloned()
        })
        .or_else(|| others.iter().find(|p| p.video_path == video_path).cloned());
    if let Some(project) = existing {
        let project = if same_file(&project) {
            project
        } else {
            let info = probe(Path::new(&video_path))?;
            if !info.has_audio {
                return Err("This video has no sound, so there is nothing to transcribe.".into());
            }
            Project {
                video_size: size,
                video_mtime_ms: mtime,
                video: info,
                ..project
            }
        };
        let project = Project {
            opened_at: now,
            ..project
        };
        write_project(&project_dir(root, &project.id)?, &project)?;
        remove_empty_duplicates(root, &project, &others);
        return Ok(project);
    }

    let info = probe(Path::new(&video_path))?;
    if !info.has_audio {
        return Err("This video has no sound, so there is nothing to transcribe.".into());
    }
    // The transcript of a deleted project of this video comes back, with its settings.
    let dir = project_dir(root, &id)?;
    let kept = root.join(KEPT_DIR).join(&id);
    let mut transcription = Settings::default();
    if kept.is_dir() {
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create project folder: {e}"))?;
        let transcript = dir.join(TRANSCRIPT_DIR);
        if fs::rename(&kept, &transcript).is_ok() {
            transcription = crate::transcription::cached_settings(&transcript).unwrap_or_default();
        }
    }
    let project = Project {
        version: FORMAT_VERSION,
        id: id.clone(),
        name: display_name(Path::new(&video_path)),
        video_path,
        video_size: size,
        video_mtime_ms: mtime,
        video: info,
        transcription,
        finder: last_finder(root),
        clips: Vec::new(),
        created_at: now,
        opened_at: now,
        updated_at: now,
    };
    write_project(&dir, &project)?;
    Ok(project)
}

/// Opens a project from Home's list: marks it as just opened.
pub fn open(root: &Path, id: &str, now: u64) -> Result<Project, String> {
    let project = Project {
        opened_at: now,
        ..load(root, id)?
    };
    write_project(&project_dir(root, id)?, &project)?;
    Ok(project)
}

/// Points project `id` at its video's new location. Refuses a video that is clearly a
/// different one (other size on screen, or more than a second longer or shorter).
pub fn locate_video(
    root: &Path,
    id: &str,
    video: &Path,
    probe: impl FnOnce(&Path) -> Result<VideoInfo, String>,
    now: u64,
) -> Result<Project, String> {
    let project = load(root, id)?;
    let video_path = normalize(video);
    let (size, mtime) = fingerprint(Path::new(&video_path))?;
    let info = probe(Path::new(&video_path))?;
    let old = &project.video;
    if (info.width, info.height) != (old.width, old.height)
        || info.duration_ms.abs_diff(old.duration_ms) > 1_000
    {
        return Err("This looks like a different video (its length or size doesn't match).".into());
    }
    let project = Project {
        video_path,
        video_size: size,
        video_mtime_ms: mtime,
        video: info,
        opened_at: now,
        updated_at: now,
        ..project
    };
    write_project(&project_dir(root, id)?, &project)?;
    Ok(project)
}

/// Deletes the project folder (never the video).
/// Deletes the project's folder (never its video). Its transcript is kept for the next
/// project of the same video (see [`KEPT_DIR`]).
pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    let dir = project_dir(root, id)?;
    let transcript = dir.join(TRANSCRIPT_DIR);
    if let (Ok(project), true) = (load(root, id), transcript.is_dir()) {
        let key = project_id(
            &project.video_path,
            project.video_size,
            project.video_mtime_ms,
        );
        let kept = root.join(KEPT_DIR);
        let _ = fs::remove_dir_all(kept.join(&key));
        let _ = fs::create_dir_all(&kept);
        let _ = fs::rename(&transcript, kept.join(key));
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove project: {e}")),
    }
}

/// The project's cached thumbnail, making it with FFmpeg if needed (a frame about one
/// second in, or the first frame for very short videos), 480 px wide.
/// Deletes the project's transcript, for "Transcribe again" (clips stay).
pub fn discard_transcript(root: &Path, id: &str) -> Result<(), String> {
    match fs::remove_dir_all(project_dir(root, id)?.join(TRANSCRIPT_DIR)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove the transcript: {e}")),
    }
}

pub fn thumbnail(root: &Path, id: &str) -> Result<PathBuf, String> {
    let project = load(root, id)?;
    let out = project_dir(root, id)?.join(THUMBNAIL_FILE);
    if out.is_file() {
        return Ok(out);
    }
    grab_frame(Path::new(&project.video_path), &[1_000, 0], &out)
}

/// Folder of the clip thumbnails, `<project>/thumbs/<ms>.jpg`.
const CLIP_THUMBS_DIR: &str = "thumbs";

/// A clip's thumbnail: the frame `ms` into the video (rounded to 0.1 s, so nearby in
/// points share one), made on first request.
pub fn clip_thumbnail(root: &Path, id: &str, ms: u64) -> Result<PathBuf, String> {
    let project = load(root, id)?;
    let ms = ms / 100 * 100;
    let dir = project_dir(root, id)?.join(CLIP_THUMBS_DIR);
    let out = dir.join(format!("{ms}.jpg"));
    if out.is_file() {
        return Ok(out);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    grab_frame(
        Path::new(&project.video_path),
        &[ms, ms.saturating_sub(1_000)],
        &out,
    )
}

/// Writes one frame of `video`, 480 px wide, to `out`: the first of the `seeks` (ms) that
/// gives a frame (the last frames of a video can fail to decode).
fn grab_frame(video: &Path, seeks: &[u64], out: &Path) -> Result<PathBuf, String> {
    if !video.is_file() {
        return Err("video not found".into());
    }
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    // Requests can overlap (Home and the project screen, React's StrictMode), so each one
    // writes its own temporary file; whichever finishes first puts it in place.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = out.with_file_name(format!(
        "frame-{}-{}.tmp.jpg",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for &seek in seeks {
        let ok = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-y", "-ss"])
            .arg(format!("{}.{:03}", seek / 1000, seek % 1000))
            .arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "4"])
            .arg(&tmp)
            .status()
            .is_ok_and(|s| s.success());
        if ok && tmp.is_file() {
            let renamed = fs::rename(&tmp, out);
            let _ = fs::remove_file(&tmp);
            return match renamed {
                Ok(()) => Ok(out.to_path_buf()),
                Err(_) if out.is_file() => Ok(out.to_path_buf()),
                Err(e) => Err(e.to_string()),
            };
        }
    }
    let _ = fs::remove_file(&tmp);
    Err("could not make a thumbnail".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WhisperModel;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-clips-project-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn info(duration_ms: u64, has_audio: bool) -> VideoInfo {
        VideoInfo {
            duration_ms,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: 0,
            has_audio,
            audio_start_ms: 0,
        }
    }

    /// A temp root with a projects folder and a fake video file.
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(name);
        let video = dir.join("My Talk.mp4");
        fs::write(&video, b"not really a video").unwrap();
        let root = dir.join("projects");
        (root, video)
    }

    fn probe_ok(_: &Path) -> Result<VideoInfo, String> {
        Ok(info(600_000, true))
    }

    fn probe_never(_: &Path) -> Result<VideoInfo, String> {
        panic!("probed again")
    }

    #[test]
    fn creates_a_project_on_first_import() {
        let (root, video) = setup("create");
        let p = open_video(&root, &video, probe_ok, 5).unwrap();
        assert_eq!(p.name, "My Talk");
        assert_eq!(p.version, FORMAT_VERSION);
        assert_eq!((p.created_at, p.opened_at, p.updated_at), (5, 5, 5));
        assert_eq!(p.video, info(600_000, true));
        assert!(root.join(&p.id).join(PROJECT_FILE).is_file());
        assert_eq!(load(&root, &p.id).unwrap(), p);
    }

    fn touch(path: &Path, secs: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn a_changed_file_reopens_its_project_instead_of_a_second_one() {
        let (root, video) = setup("changed");
        touch(&video, 1_000_000);
        let first = open_video(&root, &video, probe_ok, 1).unwrap();
        let first = Project {
            clips: vec![Clip {
                id: 1,
                in_ms: 0,
                out_ms: 30_000,
                status: ClipStatus::Kept,
                source: ClipSource::Manual,
                edited: false,
                moment: None,
            }],
            ..first
        };
        save(&root, &first, 2).unwrap();
        // Rewritten (e.g. exported again): new time and size, same path.
        fs::write(&video, b"a longer, re-exported video").unwrap();
        touch(&video, 2_000_000);
        let longer = |_: &Path| Ok(info(700_000, true));
        let again = open_video(&root, &video, longer, 3).unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.clips, first.clips);
        assert_eq!(again.video.duration_ms, 700_000);
        assert_eq!(again.video_mtime_ms, 2_000_000_000);
        assert_eq!(list(&root).len(), 1);
        // Once refreshed, it opens without probing.
        assert_eq!(
            open_video(&root, &video, probe_never, 4).unwrap().id,
            first.id
        );
    }

    #[test]
    fn empty_duplicates_from_older_versions_are_cleaned_up() {
        let (root, video) = setup("duplicates");
        touch(&video, 1_000_000);
        let old = open_video(&root, &video, probe_ok, 1).unwrap();
        // What earlier versions did when the file's time changed: a second project.
        touch(&video, 2_000_000);
        let (size, mtime) = fingerprint(&video).unwrap();
        let path = normalize(&video);
        let newer = Project {
            id: project_id(&path, size, mtime),
            video_mtime_ms: mtime,
            opened_at: 2,
            ..old.clone()
        };
        write_project(&root.join(&newer.id), &newer).unwrap();
        // A third, older one with clips stays (its clips are the user's work).
        let kept = Project {
            id: "00000000000000aa".into(),
            opened_at: 0,
            clips: vec![Clip {
                id: 1,
                in_ms: 0,
                out_ms: 30_000,
                status: ClipStatus::Kept,
                source: ClipSource::Manual,
                edited: false,
                moment: None,
            }],
            ..old.clone()
        };
        write_project(&root.join(&kept.id), &kept).unwrap();
        assert_eq!(list(&root).len(), 3);

        let opened = open_video(&root, &video, probe_never, 3).unwrap();
        assert_eq!(opened.id, newer.id);
        let mut ids: Vec<_> = list(&root).into_iter().map(|e| e.project.id).collect();
        ids.sort();
        assert_eq!(ids, [kept.id.clone(), newer.id.clone()]);
    }

    /// A fake finished transcript cache made with `model`.
    fn fake_transcript(root: &Path, id: &str, model: &str) {
        let dir = root.join(id).join(TRANSCRIPT_DIR);
        fs::create_dir_all(&dir).unwrap();
        let settings = serde_json::json!({
            "model": model, "language": "en", "custom_words": "", "hide_non_speech": true
        });
        fs::write(
            dir.join("meta.json"),
            serde_json::json!({ "version": 2, "settings": settings, "video_size": 1,
                                "video_mtime_ms": 1, "limits_ms": [1, 2, 3] })
            .to_string(),
        )
        .unwrap();
        fs::write(dir.join("transcript.json"), b"{}").unwrap();
    }

    #[test]
    fn deleting_keeps_the_video_and_the_transcript_for_a_fresh_project() {
        let (root, video) = setup("delete");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let p = Project {
            name: "Renamed".into(),
            clips: vec![Clip {
                id: 1,
                in_ms: 0,
                out_ms: 30_000,
                status: ClipStatus::Kept,
                source: ClipSource::Manual,
                edited: false,
                moment: None,
            }],
            ..p
        };
        save(&root, &p, 2).unwrap();
        fake_transcript(&root, &p.id, "large-v3-turbo");
        remove(&root, &p.id).unwrap();
        assert!(video.is_file(), "the video is never touched");
        assert!(!root.join(&p.id).exists());
        assert!(list(&root).is_empty(), "kept transcripts aren't projects");

        // Opening the video again: a fresh project with the transcript and its settings.
        let fresh = open_video(&root, &video, probe_ok, 5).unwrap();
        assert_eq!(fresh.id, p.id);
        assert_eq!((fresh.created_at, fresh.name.as_str()), (5, "My Talk"));
        assert!(fresh.clips.is_empty());
        assert_eq!(fresh.transcription.model, WhisperModel::LargeV3Turbo);
        assert_eq!(fresh.transcription.language.as_deref(), Some("en"));
        assert!(
            root.join(&p.id)
                .join(TRANSCRIPT_DIR)
                .join("transcript.json")
                .is_file()
        );
        assert!(!root.join(KEPT_DIR).join(&p.id).exists());

        // Without a transcript, nothing is kept.
        remove(&root, &p.id).unwrap();
        let again = open_video(&root, &video, probe_ok, 6).unwrap();
        discard_transcript(&root, &again.id).unwrap();
        remove(&root, &again.id).unwrap();
        assert_eq!(
            open_video(&root, &video, probe_ok, 7)
                .unwrap()
                .transcription,
            Settings::default()
        );
    }

    #[test]
    fn transcribe_again_discards_only_the_transcript() {
        let (root, video) = setup("discard");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        fake_transcript(&root, &p.id, "small");
        discard_transcript(&root, &p.id).unwrap();
        assert!(!root.join(&p.id).join(TRANSCRIPT_DIR).exists());
        assert_eq!(load(&root, &p.id).unwrap().id, p.id);
        discard_transcript(&root, &p.id).unwrap(); // nothing left: fine
    }

    #[test]
    fn importing_again_opens_the_same_project_without_probing() {
        let (root, video) = setup("reopen");
        let first = open_video(&root, &video, probe_ok, 1).unwrap();
        let again = open_video(&root, &video, |_| panic!("probed again"), 9).unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!((again.created_at, again.opened_at), (1, 9));
        assert_eq!(list(&root).len(), 1);
    }

    #[test]
    fn opening_from_the_list_moves_it_to_the_top() {
        let (root, video) = setup("open");
        let other = video.with_file_name("other.mp4");
        fs::write(&other, b"x").unwrap();
        let a = open_video(&root, &video, probe_ok, 1).unwrap();
        open_video(&root, &other, probe_ok, 2).unwrap();
        let opened = open(&root, &a.id, 3).unwrap();
        assert_eq!((opened.opened_at, opened.updated_at), (3, 1));
        assert_eq!(list(&root)[0].project.id, a.id);
        assert!(open(&root, "0123456789abcdef", 4).is_err());
    }

    #[test]
    fn ids_follow_path_size_and_mtime() {
        let a = project_id("/v/a.mp4", 10, 20);
        assert_eq!(a.len(), 16);
        assert_eq!(a, project_id("/v/a.mp4", 10, 20));
        assert_ne!(a, project_id("/v/b.mp4", 10, 20));
        assert_ne!(a, project_id("/v/a.mp4", 11, 20));
        assert_ne!(a, project_id("/v/a.mp4", 10, 21));
    }

    #[test]
    fn rejects_a_video_without_sound() {
        let (root, video) = setup("silent");
        let err = open_video(&root, &video, |_| Ok(info(1_000, false)), 1).unwrap_err();
        assert!(err.contains("no sound"), "{err}");
        assert!(list(&root).is_empty());
    }

    #[test]
    fn probe_errors_create_nothing() {
        let (root, video) = setup("probe-error");
        let err = open_video(&root, &video, |_| Err("not a video".into()), 1).unwrap_err();
        assert_eq!(err, "not a video");
        assert!(list(&root).is_empty());
    }

    #[test]
    fn lists_newest_first_and_flags_missing_videos() {
        let (root, video) = setup("list");
        let other = video.with_file_name("other.mp4");
        fs::write(&other, b"x").unwrap();
        let a = open_video(&root, &video, probe_ok, 1).unwrap();
        let b = open_video(&root, &other, probe_ok, 2).unwrap();
        fs::remove_file(&other).unwrap();
        fs::create_dir_all(root.join("not-a-project")).unwrap();
        fs::create_dir_all(root.join("0123456789abcdef")).unwrap();
        fs::write(root.join("0123456789abcdef").join(PROJECT_FILE), "{").unwrap();

        let entries = list(&root);
        let ids: Vec<&str> = entries.iter().map(|e| e.project.id.as_str()).collect();
        assert_eq!(ids, [b.id.as_str(), a.id.as_str()]);
        assert!(!entries[0].video_exists);
        assert!(entries[1].video_exists);
    }

    #[test]
    fn clips_are_saved_and_older_projects_have_none() {
        let (root, video) = setup("clips");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        assert!(p.clips.is_empty());
        assert_eq!(p.finder, FinderSettings::default());
        let clip = Clip {
            id: 3,
            in_ms: 1_000,
            out_ms: 31_000,
            status: ClipStatus::Kept,
            source: ClipSource::Manual,
            edited: true,
            moment: None,
        };
        let with_clips = Project {
            clips: vec![clip.clone()],
            ..p.clone()
        };
        save(&root, &with_clips, 2).unwrap();
        assert_eq!(load(&root, &p.id).unwrap().clips, [clip]);

        // A project.json from before milestone 3: no finder settings, no clips.
        let path = root.join(&p.id).join(PROJECT_FILE);
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("clips");
        json.as_object_mut().unwrap().remove("finder");
        fs::write(&path, json.to_string()).unwrap();
        let old = load(&root, &p.id).unwrap();
        assert!(old.clips.is_empty());
        assert_eq!(old.finder, FinderSettings::default());
    }

    #[test]
    fn clips_move_with_an_upgraded_transcript() {
        let (root, video) = setup("shift");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let end = p.video.duration_ms;
        let clip = |id, in_ms, out_ms| Clip {
            id,
            in_ms,
            out_ms,
            status: ClipStatus::Proposed,
            source: ClipSource::Found,
            edited: false,
            moment: None,
        };
        let p = Project {
            clips: vec![clip(1, 1_000, 31_000), clip(2, end - 20_000, end - 300)],
            ..p
        };
        let shifted = shift_clips(&root, p, 677).unwrap();
        let ranges: Vec<_> = shifted.clips.iter().map(|c| (c.in_ms, c.out_ms)).collect();
        assert_eq!(ranges, [(1_677, 31_677), (end - 19_323, end)]);
        assert_eq!(load(&root, &shifted.id).unwrap().clips, shifted.clips);
    }

    #[test]
    fn new_projects_take_the_last_finder_settings() {
        use crate::moments::Platform;
        let (root, video) = setup("last-finder");
        let other = video.with_file_name("other.mp4");
        fs::write(&other, b"y").unwrap();
        // Nothing chosen yet: TikTok Rewards.
        let a = open_video(&root, &video, probe_ok, 1).unwrap();
        assert_eq!(a.finder.platform, Some(Platform::TiktokRewards));
        let shorts = FinderSettings::for_platform(Platform::Shorts);
        remember_finder(&root, &shorts).unwrap();
        let b = open_video(&root, &other, probe_ok, 2).unwrap();
        assert_eq!(b.finder, shorts);
        // Existing projects keep theirs; the file isn't taken for a project.
        assert_eq!(
            open_video(&root, &video, probe_ok, 3).unwrap().finder,
            a.finder
        );
        assert_eq!(list(&root).len(), 2);
    }

    #[test]
    fn older_projects_get_their_preset_from_their_lengths() {
        use crate::moments::Platform;
        let (root, video) = setup("upgrade-finder");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let path = root.join(&p.id).join(PROJECT_FILE);
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        json["finder"] = serde_json::json!({"min_ms": 20000, "max_ms": 60000, "target_ms": 35000, "count": null});
        fs::write(&path, json.to_string()).unwrap();
        assert_eq!(
            load(&root, &p.id).unwrap().finder.platform,
            Some(Platform::Shorts)
        );
    }

    #[test]
    fn saves_atomically_and_stamps_the_time() {
        let (root, video) = setup("save");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let renamed = Project {
            name: "Episode 12".into(),
            ..p
        };
        let saved = save(&root, &renamed, 7).unwrap();
        assert_eq!(saved.updated_at, 7);
        assert_eq!(load(&root, &saved.id).unwrap().name, "Episode 12");
        let leftovers: Vec<_> = fs::read_dir(root.join(&saved.id))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn saving_a_removed_project_fails() {
        let (root, video) = setup("save-removed");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        remove(&root, &p.id).unwrap();
        assert!(save(&root, &p, 2).is_err());
        assert!(!root.join(&p.id).exists());
    }

    #[test]
    fn remove_deletes_the_project_but_not_the_video() {
        let (root, video) = setup("remove");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        remove(&root, &p.id).unwrap();
        assert!(list(&root).is_empty());
        assert!(video.is_file());
        remove(&root, &p.id).unwrap(); // already gone: fine
    }

    #[test]
    fn rejects_ids_that_are_paths() {
        let root = temp_dir("ids");
        for id in ["..", "../x", "/etc", "0123456789abcdeg", "abc", ""] {
            assert!(load(&root, id).is_err(), "{id}");
            assert!(remove(&root, id).is_err(), "{id}");
        }
    }

    #[test]
    fn locating_a_moved_video_keeps_the_project() {
        let (root, video) = setup("locate");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let moved = video.with_file_name("moved.mp4");
        fs::rename(&video, &moved).unwrap();
        assert!(!list(&root)[0].video_exists);

        let located = locate_video(&root, &p.id, &moved, probe_ok, 4).unwrap();
        assert_eq!(located.id, p.id);
        assert_eq!(located.video_path, normalize(&moved));
        assert!(list(&root)[0].video_exists);
        // Importing the moved file opens the same project.
        let again = open_video(&root, &moved, |_| panic!("probed again"), 5).unwrap();
        assert_eq!(again.id, p.id);
        assert_eq!(list(&root).len(), 1);
    }

    #[test]
    fn locating_a_different_video_is_refused() {
        let (root, video) = setup("locate-other");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let err = locate_video(&root, &p.id, &video, |_| Ok(info(30_000, true)), 2).unwrap_err();
        assert!(err.contains("different video"), "{err}");
        assert_eq!(load(&root, &p.id).unwrap(), p);
    }

    #[test]
    fn newer_format_is_refused() {
        let (root, video) = setup("newer");
        let p = open_video(&root, &video, probe_ok, 1).unwrap();
        let newer = Project {
            version: FORMAT_VERSION + 1,
            ..p.clone()
        };
        write_project(&root.join(&p.id), &newer).unwrap();
        assert!(load(&root, &p.id).unwrap_err().contains("newer version"));
    }

    /// Needs ffmpeg and ffprobe: `cargo test imports_the_fixture -- --ignored`.
    #[test]
    #[ignore]
    fn imports_the_fixture_and_makes_a_thumbnail() {
        let root = temp_dir("thumbs");
        let video = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/en/hello.mp4");
        let p = open_video(&root, &video, crate::probe::probe_video, 1).unwrap();
        assert_eq!(p.name, "hello");
        let thumb = thumbnail(&root, &p.id).unwrap();
        assert!(fs::metadata(&thumb).unwrap().len() > 0);
        assert_eq!(thumbnail(&root, &p.id).unwrap(), thumb); // cached

        // Overlapping first requests all succeed.
        fs::remove_file(&thumb).unwrap();
        let (root, id) = (std::sync::Arc::new(root), p.id);
        let requests: Vec<_> = (0..4)
            .map(|_| {
                let (root, id) = (root.clone(), id.clone());
                std::thread::spawn(move || thumbnail(&root, &id))
            })
            .collect();
        for request in requests {
            assert_eq!(request.join().unwrap().unwrap(), thumb);
        }
        let leftovers = fs::read_dir(root.join(&id))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);

        // Clip thumbnails, rounded to 0.1 s.
        let clip = clip_thumbnail(&root, &id, 1_234).unwrap();
        assert!(clip.ends_with("thumbs/1200.jpg"), "{}", clip.display());
        assert!(fs::metadata(&clip).unwrap().len() > 0);
        assert_eq!(clip_thumbnail(&root, &id, 1_299).unwrap(), clip);
        assert!(
            clip_thumbnail(&root, &id, 3_600_000).is_err(),
            "past the end"
        );
    }
}
