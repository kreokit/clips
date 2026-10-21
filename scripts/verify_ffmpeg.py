#!/usr/bin/env python3
"""Checks the bundled ffmpeg/ffprobe sidecars for one target before they ship.

    python3 scripts/verify_ffmpeg.py <rust-target-triple>

Adapted from KreoKit Captions. Fails unless:
- the build is LGPL: no --enable-gpl / --enable-nonfree / libx264 / libx265 in its
  configuration, and `ffmpeg -L` prints the GNU Lesser General Public License
- every filter KreoKit Clips uses exists (FILTERS; e.g. boxblur is GPL-only and missing)
- at least one encoder Clips allows in release builds exists
- (macOS) the executables run on macOS 12
- a 9:16 clip like the app's export (crop along a moving path, and Fit + blur, each with a
  caption in the bundled Montserrat) encodes and probes as 1080x1920 H.264 yuv420p with
  an accepted profile, trying the allowed encoders in the app's order (src-tauri/src/export.rs)

The release binary's --self-test then exports through the app's own code as well.
"""

import json
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARIES = ROOT / "src-tauri" / "binaries"
FONTS = ROOT / "src-tauri" / "resources" / "fonts"

# Mirrors allowed_encoders(false) in src-tauri/src/export.rs.
ALLOWED = {
    "darwin": ["h264_videotoolbox", "libopenh264"],
    "windows": ["h264_mf", "libopenh264"],
    "linux": ["libopenh264"],
}
# Mirrors accepted_profiles() in src-tauri/src/export.rs.
PROFILES = {
    "h264_mf": ["High", "Main"],
    "libopenh264": ["High", "Main", "Constrained Baseline"],
}
FORBIDDEN_CONFIG = ["--enable-gpl", "--enable-nonfree", "--enable-libx264", "--enable-libx265"]
# Every filter the app runs: export graph and audio (export.rs), frame sampling
# (frames.rs), thumbnails (projects.rs), audio extraction (audio.rs).
FILTERS = [
    "ass", "crop", "scale", "setsar", "split", "colorchannelmixer", "avgblur", "overlay",
    "fps", "tpad", "format", "setparams", "aresample", "apad",
]

ASS = """[Script Info]
ScriptType: v4.00+
PlayResX: 608
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Montserrat,56,&H0035F1C8,&H0035F1C8,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,3,0,2,20,20,280,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,KreoKit release check
"""

TAIL = (
    "fps=fps=30/1,tpad=stop_mode=clone:stop_duration=1,format=yuv420p,"
    "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,"
    "ass=filename=captions.ass:fontsdir=fonts[v]"
)
# The same shapes as export::video_graph: a crop moving with t, and Fit + blur.
GRAPHS = {
    "crop": "[0:v]crop=w=608:h=1080:x='min(max(0,t*400),1312)':y=0,"
    "scale=1080:1920:flags=lanczos,setsar=1," + TAIL,
    "fit + blur": "[0:v]split=2[bg][fg];"
    "[bg]scale=54:96:force_original_aspect_ratio=increase:flags=area,crop=54:96,"
    "colorchannelmixer=rr=0.7:gg=0.7:bb=0.7,scale=270:480:flags=bicubic,"
    "avgblur=sizeX=8,avgblur=sizeX=8,scale=1080:1920:flags=bicubic[back];"
    "[fg]scale=1080:1920:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos[front];"
    "[back][front]overlay=x=(W-w)/2:y=(H-h)/2,setsar=1," + TAIL,
}


def run(*args: str, cwd: Path | None = None) -> str:
    result = subprocess.run(args, capture_output=True, text=True, cwd=cwd)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:3])}… failed:\n{result.stderr[-2000:]}")
    return result.stdout + result.stderr


def fail(message: str) -> None:
    sys.exit(f"FAIL: {message}")


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    triple = sys.argv[1]
    exe = ".exe" if "windows" in triple else ""
    os_name = "darwin" if "apple" in triple else "windows" if "windows" in triple else "linux"
    ffmpeg = BINARIES / f"ffmpeg-{triple}{exe}"
    ffprobe = BINARIES / f"ffprobe-{triple}{exe}"
    for tool in (ffmpeg, ffprobe):
        if not tool.is_file():
            fail(f"missing {tool.relative_to(ROOT)}; run scripts/fetch_ffmpeg.py {triple}")

    version = run(str(ffmpeg), "-hide_banner", "-version").splitlines()[0]
    config = run(str(ffmpeg), "-hide_banner", "-buildconf")
    bad = [flag for flag in FORBIDDEN_CONFIG if flag in config]
    if bad:
        fail(f"not an LGPL build: {', '.join(bad)}")
    # The licence text is wrapped differently per version, so compare words, not lines.
    licence = " ".join(run(str(ffmpeg), "-hide_banner", "-L").split())
    if "GNU Lesser General Public License" not in licence:
        fail("ffmpeg -L does not show the LGPL")
    run(str(ffprobe), "-hide_banner", "-version")

    filters_out = run(str(ffmpeg), "-hide_banner", "-filters")
    missing = [f for f in FILTERS if not re.search(rf"^\s*\S+\s+{f}\s", filters_out, re.M)]
    if missing:
        fail(f"filters missing: {', '.join(missing)}")
    encoders_out = run(str(ffmpeg), "-hide_banner", "-encoders")
    available = [e for e in ALLOWED[os_name] if re.search(rf"^\s*V\S*\s+{e}\s", encoders_out, re.M)]
    if not available:
        fail(f"none of the allowed encoders {ALLOWED[os_name]} is available")

    if os_name == "darwin" and platform.system() == "Darwin":
        for tool in (ffmpeg, ffprobe):
            minos = re.search(r"minos (\S+)", run("otool", "-l", str(tool)))
            if not minos or tuple(map(int, minos.group(1).split("."))) > (12, 0):
                fail(f"{tool.name} needs macOS {minos.group(1) if minos else '?'}, not 12.0")

    results = [export_test(ffmpeg, ffprobe, available, name, graph) for name, graph in GRAPHS.items()]
    print(f"OK {triple}: {version}")
    print(f"   LGPL, {len(FILTERS)} filters, encoders {available}")
    for line in results:
        print(f"   {line}")


def export_test(ffmpeg: Path, ffprobe: Path, encoders: list[str], name: str, graph: str) -> str:
    ffmpeg, ffprobe = ffmpeg.resolve(), ffprobe.resolve()  # FFmpeg runs in the temp dir
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        (work / "captions.ass").write_text(ASS)
        shutil.copytree(FONTS, work / "fonts", ignore=shutil.ignore_patterns("*.txt", "*.md"))
        errors = []
        for encoder in encoders:
            extra = ["-allow_sw", "1"] if encoder == "h264_videotoolbox" else []
            try:
                run(
                    str(ffmpeg), "-nostdin", "-y", "-v", "error",
                    "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=1",
                    "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=1",
                    "-filter_complex", graph, "-map", "[v]", "-map", "1:a",
                    "-c:v", encoder, "-profile:v", "high", *extra, "-b:v", "10000000",
                    "-af", "aresample=async=1:first_pts=0,apad",
                    "-c:a", "aac", "-ar", "48000", "-ac", "2", "-t", "1",
                    "-movflags", "+faststart", "-f", "mp4", "out.mp4",
                    cwd=work,
                )
            except RuntimeError as e:
                errors.append(f"{encoder}: {e}")
                continue
            info = json.loads(run(
                str(ffprobe), "-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=width,height,codec_name,profile,pix_fmt:format=duration",
                "-of", "json", str(work / "out.mp4"),
            ))
            stream, duration = info["streams"][0], float(info["format"]["duration"])
            got = (stream["width"], stream["height"], stream["codec_name"], stream["pix_fmt"])
            if got != (1080, 1920, "h264", "yuv420p"):
                fail(f"{name}: unexpected output stream {stream}")
            if stream.get("profile") not in PROFILES.get(encoder, ["High"]):
                fail(f"{name}: H.264 profile {stream.get('profile')} from {encoder}")
            if abs(duration - 1.0) > 0.15:
                fail(f"{name}: unexpected output duration {duration}")
            return f"{name}: 1080x1920 H.264 {stream['profile']} yuv420p with {encoder}"
        fail(f"{name}: no allowed encoder could export the test clip:\n" + "\n".join(errors))
        return ""


if __name__ == "__main__":
    main()
