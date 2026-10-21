//! Export all: file names, the plan shown before exporting (names, files already there,
//! size estimate, free space), and one clip from captions to a checked file in place.
//!
//! Names (decided 2026-10-21): `<video>-clip-01.mp4`, the video's name made safe for
//! Windows and macOS and shortened to [`MAX_STEM_CHARS`]; clips are numbered by time among
//! the project's kept clips, so a clip keeps its number whichever are exported.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::ass;
use crate::captions::{self, WordFix};
use crate::export::{self, ClipJob, Picture};
use crate::probe::VideoInfo;
use crate::reframe::{Axis, Keyframe};
use crate::sentences::Text;
use crate::style::Style;
use crate::subtitles;

/// The longest video name kept in file names, in characters.
pub const MAX_STEM_CHARS: usize = 60;
/// Free space to keep after an export (decided 2026-10-21).
pub const DISK_MARGIN: u64 = 2 * 1024 * 1024 * 1024;

/// Characters that break file names on Windows or macOS.
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Names Windows reserves for devices (with any extension).
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `name` as the start of a file name that works on Windows and macOS: forbidden and
/// control characters become spaces, runs of spaces collapse, no leading dot (hidden on
/// macOS) or trailing dot or space (dropped by Windows), at most [`MAX_STEM_CHARS`]
/// characters (cut at a space when one is near the end), "video" if nothing is left.
pub fn safe_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if FORBIDDEN.contains(&c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut stem = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if stem.chars().count() > MAX_STEM_CHARS {
        let cut: String = stem.chars().take(MAX_STEM_CHARS).collect();
        stem = match cut.rfind(' ') {
            Some(space) if cut[..space].chars().count() >= MAX_STEM_CHARS * 2 / 3 => {
                cut[..space].to_string()
            }
            _ => cut,
        };
    }
    let stem = stem
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    if stem.is_empty() {
        return "video".into();
    }
    if RESERVED.contains(&stem.split('.').next().unwrap_or("").to_lowercase().as_str()) {
        return format!("{stem}_");
    }
    stem
}

/// `<stem>-clip-07`: numbers with at least two digits, more when the count needs them.
pub fn clip_stem(stem: &str, number: u32, digits: usize) -> String {
    format!("{stem}-clip-{number:0digits$}")
}

/// Digits for numbering up to `largest`.
pub fn digits_for(largest: u32) -> usize {
    largest.to_string().len().max(2)
}

/// Bytes a clip of `length_ms` takes, roughly: the target bitrates plus 1% for the
/// container.
pub fn estimate_bytes(length_ms: u64) -> u64 {
    let bits = (export::VIDEO_BITRATE + export::AUDIO_BITRATE) * length_ms / 1000;
    bits / 8 * 101 / 100
}

/// What Export all will do, shown before it starts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    /// The video file names, in the order of the items.
    pub files: Vec<String>,
    /// Names (videos and SRTs) already in the folder: Replace or Keep both.
    pub conflicts: Vec<String>,
    /// Estimated size of the export.
    pub bytes: u64,
    /// Free space on the folder's drive, if known.
    pub free: Option<u64>,
    /// Free space minus the export leaves at least [`DISK_MARGIN`] (true if unknown).
    pub enough_space: bool,
}

/// The plan for exporting clips (`number`, length) of the video `video_name` to `folder`.
pub fn plan(folder: &Path, video_name: &str, items: &[(u32, u64)], srt: bool) -> Plan {
    let stem = safe_stem(video_name);
    let digits = digits_for(items.iter().map(|i| i.0).max().unwrap_or(1));
    let stems: Vec<String> = items
        .iter()
        .map(|&(n, _)| clip_stem(&stem, n, digits))
        .collect();
    let existing = export::names_in(folder);
    let mut conflicts = Vec::new();
    for s in &stems {
        for ext in if srt {
            &["mp4", "srt"][..]
        } else {
            &["mp4"][..]
        } {
            let name = format!("{s}.{ext}");
            if existing.contains(&name.to_lowercase()) {
                conflicts.push(name);
            }
        }
    }
    let bytes = items.iter().map(|&(_, ms)| estimate_bytes(ms)).sum();
    let free = free_bytes(folder);
    Plan {
        files: stems.iter().map(|s| format!("{s}.mp4")).collect(),
        conflicts,
        bytes,
        free,
        enough_space: free.is_none_or(|f| f >= bytes.saturating_add(DISK_MARGIN)),
    }
}

/// Space available to this user on the drive holding `path`.
#[cfg(unix)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `stats` a valid out pointer.
    let result = unsafe { libc::statvfs(c.as_ptr(), &mut stats) };
    (result == 0).then(|| stats.f_bavail as u64 * stats.f_frsize as u64)
}

#[cfg(windows)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated; null pointers are allowed for the totals.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

/// How a clip fills the frame, as the editor sends it (the frontend resolves Follow face,
/// Center and Manual into keyframes, as for the preview).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PictureSpec {
    Crop {
        axis: Axis,
        keyframes: Vec<Keyframe>,
    },
    Fit,
}

/// One clip to export.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Item {
    pub clip_id: u32,
    /// Its number in the file name.
    pub number: u32,
    pub in_ms: u64,
    pub out_ms: u64,
    pub picture: PictureSpec,
    /// The project's style as this clip shows it (fitted in the editor's measurements).
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ItemResult {
    Saved {
        clip_id: u32,
        path: String,
        srt: Option<String>,
    },
    Failed {
        clip_id: u32,
        error: String,
    },
    Skipped {
        clip_id: u32,
    },
}

/// What every clip of one Export all shares.
pub struct Batch<'a> {
    pub video: &'a Path,
    pub info: &'a VideoInfo,
    pub video_name: &'a str,
    pub text: &'a Text,
    pub fixes: &'a [WordFix],
    pub folder: &'a Path,
    /// Numbers up to this get the same number of digits.
    pub largest_number: u32,
    pub srt: bool,
    /// Replace files already there (else Keep both: -2, -3…).
    pub replace: bool,
    /// TikTok Rewards: clips of 61 s or more must come out over 60.0 s in every stream.
    pub tiktok: bool,
    pub fonts: Option<&'a Path>,
}

/// TikTok Rewards clips meant to be eligible (61 s or more; shorter ones are exported
/// only when the user ticks them, knowing they aren't).
pub const TIKTOK_ELIGIBLE_MS: u64 = 61_000;

/// Exports one clip: its captions, the video under a temporary name, checked with
/// ffprobe, then put in place (and its SRT next to it). Returns the paths.
pub fn export_one(
    batch: &Batch,
    item: &Item,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u32),
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let out_ms = item.out_ms.min(batch.info.duration_ms);
    if out_ms <= item.in_ms {
        return Err("the clip is empty".into());
    }
    let length_ms = out_ms - item.in_ms;
    let segments = captions::clip_captions(batch.text, batch.fixes, item.in_ms, out_ms);
    let script = (!segments.is_empty())
        .then(|| ass::to_ass(&segments, &item.style, export::OUT.0, export::OUT.1));
    let picture = match &item.picture {
        PictureSpec::Crop { axis, keyframes } => Picture::Crop {
            keyframes,
            axis: *axis,
        },
        PictureSpec::Fit => Picture::FitBlur,
    };
    let job = ClipJob {
        video: batch.video,
        source: (batch.info.width, batch.info.height),
        fps: batch.info.fps,
        in_ms: item.in_ms,
        out_ms,
        picture,
        ass: script.as_deref(),
        fonts: batch.fonts,
    };
    let stem = clip_stem(
        &safe_stem(batch.video_name),
        item.number,
        digits_for(batch.largest_number),
    );
    let dest = batch.folder.join(format!("{stem}.mp4"));
    let tmp = export::temp_path(&dest);
    let encoder = export::encode(&job, &tmp, cancel, on_progress)?;
    if cfg!(debug_assertions) {
        // Shows in the `pnpm tauri dev` terminal (e.g. with KREOKIT_DEV_ENCODER).
        eprintln!("export: {} encoded with {encoder}", dest.display());
    }
    let over_a_minute = batch.tiktok && length_ms >= TIKTOK_ELIGIBLE_MS;
    let checked = export::probe_output(&tmp).and_then(|facts| {
        export::check(
            &facts,
            encoder,
            length_ms,
            export::output_rate(batch.info.fps),
            over_a_minute,
        )
    });
    if let Err(e) = checked {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let placed = export::place(&tmp, &dest, batch.replace)?;
    let srt = if batch.srt && !segments.is_empty() {
        let text = subtitles::to_srt(&subtitles::cues(&segments, &item.style));
        let srt_dest = placed.with_extension("srt");
        let srt_tmp = export::temp_path(&srt_dest);
        fs::write(&srt_tmp, text).map_err(|e| format!("cannot save the subtitles: {e}"))?;
        Some(export::place(&srt_tmp, &srt_dest, batch.replace)?)
    } else {
        None
    };
    Ok((placed, srt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_on_windows_and_macos() {
        assert_eq!(
            safe_stem("Talk: what/why? <part 1>"),
            "Talk what why part 1"
        );
        assert_eq!(safe_stem("  ..hidden.  "), "hidden");
        assert_eq!(safe_stem("tab\there\nnew"), "tab here new");
        assert_eq!(safe_stem("con"), "con_");
        assert_eq!(safe_stem("NUL.backup"), "NUL.backup_");
        assert_eq!(safe_stem("???"), "video");
        assert_eq!(
            safe_stem("Émission spéciale – épisode 3"),
            "Émission spéciale – épisode 3"
        );
    }

    #[test]
    fn long_titles_are_shortened_at_a_word() {
        let title = "Do THIS Habit 10 Minutes Before Sleeping _ Dr. Joe Dispenza Explains _ Raj Shamani Clips";
        let stem = safe_stem(title);
        assert!(stem.chars().count() <= MAX_STEM_CHARS, "{stem}");
        assert_eq!(
            stem,
            "Do THIS Habit 10 Minutes Before Sleeping _ Dr. Joe Dispenza"
        );
        // One long word: cut at the limit.
        let word = "x".repeat(80);
        assert_eq!(safe_stem(&word).len(), MAX_STEM_CHARS);
        // Never in the middle of a character.
        assert_eq!(safe_stem(&"é".repeat(70)).chars().count(), MAX_STEM_CHARS);
    }

    #[test]
    fn clips_are_numbered_with_enough_digits() {
        assert_eq!(clip_stem("talk", 1, digits_for(9)), "talk-clip-01");
        assert_eq!(clip_stem("talk", 12, digits_for(12)), "talk-clip-12");
        assert_eq!(clip_stem("talk", 7, digits_for(120)), "talk-clip-007");
    }

    #[test]
    fn the_plan_lists_conflicts_size_and_space() {
        let dir = std::env::temp_dir().join("kreokit-clips-batch-plan");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Talk-clip-02.MP4"), b"old").unwrap();
        fs::write(dir.join("talk-clip-03.srt"), b"old").unwrap();
        let p = plan(
            &dir,
            "talk",
            &[(1, 61_000), (2, 70_000), (3, 30_000)],
            false,
        );
        assert_eq!(
            p.files,
            ["talk-clip-01.mp4", "talk-clip-02.mp4", "talk-clip-03.mp4"]
        );
        assert_eq!(p.conflicts, ["talk-clip-02.mp4"]);
        assert_eq!(
            plan(&dir, "talk", &[(3, 30_000)], true).conflicts,
            ["talk-clip-03.srt"]
        );
        // ~10.2 Mbit/s: 161 s ≈ 207 MB.
        assert_eq!(p.bytes, estimate_bytes(161_000));
        assert!((200_000_000..215_000_000).contains(&p.bytes), "{}", p.bytes);
        assert!(p.free.is_some_and(|f| f > 0));
        assert_eq!(p.enough_space, p.free.unwrap() >= p.bytes + DISK_MARGIN);
        // A huge export never fits.
        assert!(!plan(&dir, "talk", &[(1, u64::MAX / 20_000_000)], false).enough_space);
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn exports_a_clip_with_captions_and_srt_without_replacing() {
        use crate::sentences::{Boundary, Sentence, Token};
        let dir = std::env::temp_dir().join("kreokit-clips-batch-one");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let video = crate::export::tests::make_source(&dir, "25", 8.0, 8.0, 0.0);
        let info = crate::probe::probe_video(&video).unwrap();
        let words = ["Hello", "there", "friend."];
        let text = Text {
            words: words
                .iter()
                .enumerate()
                .map(|(i, w)| Token {
                    start_ms: 1_000 + i as u64 * 500,
                    end_ms: 1_400 + i as u64 * 500,
                    text: w.to_string(),
                    norm: w.to_lowercase(),
                    segment: 0,
                })
                .collect(),
            sentences: vec![Sentence {
                first: 0,
                last: 2,
                start_ms: 1_000,
                end_ms: 2_400,
                starts: Boundary::Edge,
                ends: Boundary::Edge,
                question: false,
            }],
            sounds: Vec::new(),
        };
        let fixes = [WordFix {
            start_ms: 1_500,
            was: "there".into(),
            text: "THERE".into(),
        }];
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/fonts");
        let batch = Batch {
            video: &video,
            info: &info,
            video_name: "My talk: part 1?",
            text: &text,
            fixes: &fixes,
            folder: &out,
            largest_number: 3,
            srt: true,
            replace: false,
            tiktok: true,
            fonts: Some(&fonts),
        };
        let item = Item {
            clip_id: 9,
            number: 3,
            in_ms: 500,
            out_ms: 6_500,
            picture: PictureSpec::Fit,
            style: Style::default(),
        };
        let cancel = AtomicBool::new(false);
        let (mp4, srt) = export_one(&batch, &item, &cancel, |_| {}).unwrap();
        assert_eq!(mp4, out.join("My talk part 1-clip-03.mp4"));
        let srt = srt.unwrap();
        let subtitles = fs::read_to_string(&srt).unwrap();
        assert!(subtitles.contains("00:00:00,500 --> "), "{subtitles}");
        assert!(subtitles.contains("Hello THERE\nfriend."), "{subtitles}");
        // 6 s under TikTok Rewards is fine: it isn't meant to be eligible.
        // Again: Keep both gives -2 names; nothing is replaced, no temporary file is left.
        let (again, again_srt) = export_one(&batch, &item, &cancel, |_| {}).unwrap();
        assert_eq!(again, out.join("My talk part 1-clip-03-2.mp4"));
        assert_eq!(again_srt.unwrap(), out.join("My talk part 1-clip-03-2.srt"));
        let mut names: Vec<String> = export::names_in(&out).into_iter().collect();
        names.sort();
        assert_eq!(
            names,
            [
                "my talk part 1-clip-03-2.mp4",
                "my talk part 1-clip-03-2.srt",
                "my talk part 1-clip-03.mp4",
                "my talk part 1-clip-03.srt"
            ]
        );
    }
}
