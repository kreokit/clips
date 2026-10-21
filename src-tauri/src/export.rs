//! Export: each clip as a 1080×1920 MP4 with its captions burned in.
//!
//! The picture (crop along the clip's path, or Fit + blur) goes through one FFmpeg filter
//! graph ([`video_graph`]) that ends in constant frame rate, yuv420p and BT.709 tags, with
//! the captions drawn last by libass (`ass` filter, as in KreoKit Captions).

use crate::reframe::{self, Axis, Keyframe};

/// The exported frame.
pub const OUT: (u32, u32) = (1080, 1920);

/// How the clip's picture fills the 9:16 frame.
#[derive(Debug, Clone, Copy)]
pub enum Picture<'a> {
    /// The 9:16 crop along `keyframes` (Follow face, Center, Manual).
    Crop { keyframes: &'a [Keyframe], axis: Axis },
    /// The whole frame, over a blurred, darkened copy of itself (the editor's preview:
    /// a 54×96 copy, blurred and at 70% brightness).
    FitBlur,
}

/// Frame rates the export snaps to (a source's average rate is rarely exact).
const RATES: &[(u32, u32)] = &[
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// The export's constant frame rate for a source averaging `fps`: the standard rate
/// within 0.5%, else the nearest whole rate, between 24 and 60 (phones record variable
/// rates; high-speed video plays at 60).
pub fn output_rate(fps: f64) -> (u32, u32) {
    if !fps.is_finite() || fps <= 0.0 {
        return (30, 1);
    }
    RATES
        .iter()
        .copied()
        .find(|&(n, d)| (fps / (n as f64 / d as f64) - 1.0).abs() < 0.005)
        .unwrap_or(((fps.round() as u32).clamp(24, 60), 1))
}

/// The `-filter_complex` graph from input `[0:v]` to `[v]`: the picture at 1080×1920
/// (the clip read from `start_ms`, so the crop path's times shift by it), constant
/// `rate`, yuv420p tagged BT.709 limited range, then `captions` (an `ass=…` filter) if any.
/// The tags come before the captions so libass's colours are converted for BT.709.
pub fn video_graph(
    picture: Picture,
    source: (u32, u32),
    start_ms: u64,
    (rate_n, rate_d): (u32, u32),
    captions: Option<&str>,
) -> String {
    let (w, h) = OUT;
    let picture = match picture {
        Picture::Crop { keyframes, axis } => {
            format!("[0:v]{}", reframe::crop_filter(keyframes, axis, source, start_ms, OUT))
        }
        Picture::FitBlur => format!(
            "[0:v]split=2[bg][fg];\
             [bg]scale=54:96:force_original_aspect_ratio=increase:flags=area,crop=54:96,\
             colorchannelmixer=rr=0.7:gg=0.7:bb=0.7,scale=270:480:flags=bicubic,boxblur=8:2,\
             scale={w}:{h}:flags=bicubic[back];\
             [fg]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos[front];\
             [back][front]overlay=x=(W-w)/2:y=(H-h)/2,setsar=1"
        ),
    };
    let captions = captions.map(|c| format!(",{c}")).unwrap_or_default();
    format!(
        "{picture},fps=fps={rate_n}/{rate_d},format=yuv420p,\
         setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709{captions}[v]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn frame_rates_snap_to_standard_ones() {
        assert_eq!(output_rate(29.97), (30000, 1001));
        assert_eq!(output_rate(29.970_03), (30000, 1001));
        assert_eq!(output_rate(25.0), (25, 1));
        assert_eq!(output_rate(23.976), (24000, 1001));
        assert_eq!(output_rate(59.94), (60000, 1001));
        // A phone's variable rate, slow and fast sources, nonsense.
        assert_eq!(output_rate(29.6), (30, 1));
        assert_eq!(output_rate(15.0), (24, 1));
        assert_eq!(output_rate(120.0), (60, 1));
        assert_eq!(output_rate(0.0), (30, 1));
        assert_eq!(output_rate(f64::NAN), (30, 1));
    }

    #[test]
    fn the_graph_ends_tagged_with_the_captions_last() {
        let keyframes = [Keyframe { t_ms: 0, pos: 0.5, jump: false, ease: false }];
        let crop = Picture::Crop { keyframes: &keyframes, axis: Axis::X };
        let g = video_graph(crop, (1920, 1080), 0, (30, 1), Some("ass=filename=captions.ass"));
        assert!(g.starts_with("[0:v]crop=w=608:h=1080:"), "{g}");
        assert!(g.ends_with(
            ",fps=fps=30/1,format=yuv420p,setparams=range=tv:color_primaries=bt709:\
             color_trc=bt709:colorspace=bt709,ass=filename=captions.ass[v]"
        ));
        let fit = video_graph(Picture::FitBlur, (1920, 1080), 0, (25, 1), None);
        assert!(fit.contains("overlay=x=(W-w)/2:y=(H-h)/2"));
        assert!(fit.ends_with("colorspace=bt709[v]"));
    }

    /// Mean brightness (0–255) of each output row of a gray 1080×1920 frame.
    fn row_means(source: &str, graph: &str) -> Vec<f64> {
        let output = Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-f", "lavfi", "-i", source, "-filter_complex", graph])
            .args(["-map", "[v]", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let (w, h) = (OUT.0 as usize, OUT.1 as usize);
        assert_eq!(output.stdout.len(), w * h);
        output
            .stdout
            .chunks(w)
            .map(|row| row.iter().map(|&v| v as f64).sum::<f64>() / w as f64)
            .collect()
    }

    #[test]
    #[ignore = "needs FFmpeg"]
    fn fit_blur_puts_the_whole_frame_over_a_darker_copy() {
        let graph = video_graph(Picture::FitBlur, (1920, 1080), 0, (30, 1), None);
        let rows = row_means("color=white:size=1920x1080:duration=1", &graph);
        // A 16:9 source fits as 1080×608 in the middle (rows 656–1263).
        let middle: Vec<f64> = rows[700..1220].to_vec();
        let above: Vec<f64> = rows[0..600].to_vec();
        let below: Vec<f64> = rows[1320..].to_vec();
        assert!(middle.iter().all(|&v| v > 250.0), "front {:?}", &middle[..3]);
        // The copy behind: 70% brightness, the same above and below.
        for v in above.iter().chain(&below) {
            assert!((v - 0.7 * 255.0).abs() < 8.0, "back {v}");
        }

        // A 9:16 source fills the frame.
        let graph = video_graph(Picture::FitBlur, (1080, 1920), 0, (30, 1), None);
        let rows = row_means("color=white:size=1080x1920:duration=1", &graph);
        assert!(rows.iter().all(|&v| v > 250.0));
    }
}
