//! `probe_video`: reads duration, display size, frame rate and the audio stream of a video
//! with ffprobe.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ffmpeg;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub duration_ms: u64,
    /// Size as displayed: rotated phone videos (90° / 270°) have width and height swapped
    /// relative to the encoded frames.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Clockwise rotation applied on display (0, 90, 180 or 270).
    pub rotation: u32,
    /// Whether there is an audio stream. Without one there is nothing to transcribe.
    pub has_audio: bool,
}

pub fn probe_video(path: &Path) -> Result<VideoInfo, String> {
    if !path.is_file() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let output = Command::new(&ffprobe)
        .args(["-v", "error"])
        .args([
            "-show_entries",
            "format=duration:stream=codec_type,width,height,avg_frame_rate,r_frame_rate,duration,disposition:stream_side_data=rotation",
        ])
        .args(["-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffprobe.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "this file can't be read as a video: {}",
            stderr.trim()
        ));
    }
    parse_ffprobe_json(&String::from_utf8_lossy(&output.stdout))
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

/// Parses the JSON printed by the ffprobe invocation in [`probe_video`].
pub fn parse_ffprobe_json(json: &str) -> Result<VideoInfo, String> {
    let parsed: FfprobeOutput =
        serde_json::from_str(json).map_err(|e| format!("unexpected ffprobe output: {e}"))?;

    // The first real video stream; cover art (MP3/M4A artwork) counts as a video stream
    // in ffprobe but is a still image.
    let stream = parsed
        .streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video")
                && s.disposition.as_ref().is_none_or(|d| d.attached_pic == 0)
        })
        .ok_or("the file has no video stream")?;
    let has_audio = parsed
        .streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));

    let (coded_width, coded_height) = stream
        .width
        .zip(stream.height)
        .ok_or("the video stream has no resolution")?;

    // avg_frame_rate is "0/0" for some containers; fall back to r_frame_rate.
    let fps = [&stream.avg_frame_rate, &stream.r_frame_rate]
        .into_iter()
        .flatten()
        .find_map(|rate| parse_rate(rate))
        .ok_or("the video stream has no frame rate")?;

    // The container duration covers all streams; fall back to the video stream's own.
    let seconds = parsed
        .format
        .and_then(|f| f.duration)
        .or_else(|| stream.duration.clone())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or("the video has no duration")?;

    let rotation = stream
        .side_data_list
        .iter()
        .find_map(|d| d.rotation)
        .map(normalize_rotation)
        .unwrap_or(0);
    let (width, height) = if rotation % 180 == 90 {
        (coded_height, coded_width)
    } else {
        (coded_width, coded_height)
    };

    Ok(VideoInfo {
        duration_ms: (seconds * 1000.0).round() as u64,
        width,
        height,
        fps,
        rotation,
        has_audio,
    })
}

/// ffprobe reports the display matrix rotation counter-clockwise (-90 for a phone held
/// upright). Returns the clockwise rotation snapped to 0, 90, 180 or 270.
fn normalize_rotation(degrees: f64) -> u32 {
    let quarter = (-degrees / 90.0).round() as i64;
    (quarter.rem_euclid(4) * 90) as u32
}

/// Parses an ffprobe rational like "30000/1001". Returns `None` for "0/0" and other
/// unusable values.
fn parse_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/')?;
    let (num, den) = (
        num.trim().parse::<f64>().ok()?,
        den.trim().parse::<f64>().ok()?,
    );
    let fps = num / den;
    (fps.is_finite() && fps > 0.0).then_some(fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffprobe `-of json` output for a 1080p 29.97 fps clip with audio.
    const SAMPLE: &str = r#"{
        "programs": [],
        "stream_groups": [],
        "streams": [
            {
                "codec_type": "video",
                "width": 1920,
                "height": 1080,
                "r_frame_rate": "30000/1001",
                "avg_frame_rate": "30000/1001",
                "duration": "12.312300",
                "disposition": { "default": 1, "attached_pic": 0 }
            },
            { "codec_type": "audio", "r_frame_rate": "0/0", "avg_frame_rate": "0/0" }
        ],
        "format": {
            "duration": "12.345000"
        }
    }"#;

    #[test]
    fn parses_typical_output() {
        let info = parse_ffprobe_json(SAMPLE).unwrap();
        assert_eq!(info.duration_ms, 12_345);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert!((info.fps - 29.97).abs() < 0.001, "fps was {}", info.fps);
        assert_eq!(info.rotation, 0);
        assert!(info.has_audio);
    }

    #[test]
    fn falls_back_to_r_frame_rate_and_stream_duration() {
        let json = r#"{
            "streams": [{ "codec_type": "video", "width": 720, "height": 1280,
                          "avg_frame_rate": "0/0", "r_frame_rate": "25/1", "duration": "3.0004" }],
            "format": {}
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert_eq!(
            info,
            VideoInfo {
                duration_ms: 3_000,
                width: 720,
                height: 1280,
                fps: 25.0,
                rotation: 0,
                has_audio: false,
            }
        );
    }

    #[test]
    fn swaps_size_of_rotated_phone_video() {
        let json = r#"{
            "streams": [{ "codec_type": "video", "width": 1920, "height": 1080,
                          "avg_frame_rate": "30/1",
                          "side_data_list": [{ "rotation": -90 }] },
                        { "codec_type": "audio" }],
            "format": { "duration": "5.0" }
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert_eq!((info.width, info.height, info.rotation), (1080, 1920, 90));
    }

    #[test]
    fn normalizes_rotations() {
        assert_eq!(normalize_rotation(0.0), 0);
        assert_eq!(normalize_rotation(-90.0), 90);
        assert_eq!(normalize_rotation(90.0), 270);
        assert_eq!(normalize_rotation(180.0), 180);
        assert_eq!(normalize_rotation(-180.0), 180);
        assert_eq!(normalize_rotation(270.0), 90);
    }

    #[test]
    fn skips_cover_art() {
        let json = r#"{
            "streams": [{ "codec_type": "audio" },
                        { "codec_type": "video", "width": 600, "height": 600,
                          "avg_frame_rate": "0/0", "r_frame_rate": "90000/1",
                          "disposition": { "attached_pic": 1 } }],
            "format": { "duration": "1800.0" }
        }"#;
        assert_eq!(
            parse_ffprobe_json(json).unwrap_err(),
            "the file has no video stream"
        );
    }

    #[test]
    fn rejects_file_without_video_stream() {
        let json =
            r#"{ "streams": [{ "codec_type": "audio" }], "format": { "duration": "60.0" } }"#;
        assert_eq!(
            parse_ffprobe_json(json).unwrap_err(),
            "the file has no video stream"
        );
    }

    #[test]
    fn rejects_missing_duration() {
        let json = r#"{ "streams": [{ "codec_type": "video", "width": 640, "height": 480, "avg_frame_rate": "30/1" }] }"#;
        assert_eq!(
            parse_ffprobe_json(json).unwrap_err(),
            "the video has no duration"
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_ffprobe_json("not json").is_err());
    }

    #[test]
    fn rejects_missing_file() {
        let err = probe_video(Path::new("/no/such/video.mp4")).unwrap_err();
        assert!(err.contains("doesn't exist"), "{err}");
    }

    #[test]
    fn parses_rates() {
        assert_eq!(parse_rate("60/1"), Some(60.0));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("30/0"), None);
        assert_eq!(parse_rate("abc"), None);
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(name)
    }

    /// Needs ffprobe installed: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn probes_the_fixture() {
        let info = probe_video(&fixture("en/hello.mp4")).unwrap();
        assert_eq!((info.width, info.height), (640, 360));
        assert_eq!(info.fps, 25.0);
        assert!(info.has_audio);
        assert!((9_000..10_000).contains(&info.duration_ms), "{info:?}");
    }

    /// Needs ffmpeg and ffprobe installed: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn probes_generated_video_without_audio() {
        let dir = std::env::temp_dir().join("kreokit-clips-probe-test");
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("silent.mp4");
        let status = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-y", "-v", "error", "-f", "lavfi"])
            .args(["-i", "testsrc=duration=2:size=640x360:rate=24"])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success());

        let info = probe_video(&clip).unwrap();
        assert_eq!(
            info,
            VideoInfo {
                duration_ms: 2_000,
                width: 640,
                height: 360,
                fps: 24.0,
                rotation: 0,
                has_audio: false,
            }
        );
    }

    /// Needs ffprobe installed: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn rejects_a_file_that_is_not_a_video() {
        let err = probe_video(&fixture("en/hello.txt")).unwrap_err();
        assert!(err.contains("can't be read as a video"), "{err}");
    }
}
