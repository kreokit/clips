#!/usr/bin/env python3
"""Makes the reframe test videos in fixtures/reframe/ (Python stdlib + FFmpeg).

    python3 scripts/make_reframe_fixtures.py [--force]

Existing files are kept (re-encoding changes the bytes); --force rebuilds them.

Synthetic, with the true face boxes (fixtures/faces/ photos pasted along scripted paths):
  moving-face.mp4 / .json   one face: holds, slides across, sways, a camera cut to another
                            face and background, a half-second gap with no face
  two-faces.mp4 / .json     two faces: A bigger, B bigger for one second only (hysteresis
                            must hold A), then a cut after which B is bigger

The .json files list, for every 250 ms sample time, each visible face's box in video
pixels, taken at the frame on screen then (the frame at floor(t × 25) / 25).

Real (Pexels, licences in fixtures/LICENSES.md), trimmed and scaled to 640×360:
  podcast-wide.mp4          two people at microphones, wide shot, no sound in the original
  talking-head.mp4          one person talking at a microphone

Every file gets a silent mono AAC track, so the app can import it (it refuses videos
without sound).
"""

import json
import math
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FACES = ROOT / "fixtures" / "faces"
OUT = ROOT / "fixtures" / "reframe"
W, H, FPS = 640, 360, 25
STEP_MS = 250

PEXELS = {
    # name: (Pexels video id, download URL of the 4K/HD file, start s, duration s)
    "podcast-wide": (
        37874130,
        "https://videos.pexels.com/video-files/37874130/16069127_3840_2160_25fps.mp4",
        10.0,
        20.0,
    ),
    "talking-head": (
        38630316,
        "https://videos.pexels.com/video-files/38630316/16406725_3840_2160_25fps.mp4",
        0.0,
        12.0,
    ),
}


def run(args: list) -> None:
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", *map(str, args)], check=True)


def reference_faces() -> dict:
    """{(photo, index): ([x0, y0, x1, y1], [width, height])} from fixtures/faces/reference.json."""
    data = json.loads((FACES / "reference.json").read_text())
    return {
        (p["photo"], i): (f["box"], p["size"]) for p in data["photos"] for i, f in enumerate(p["faces"])
    }


class Sprite:
    """A head-and-shoulders crop of one photo's face, shown in [start, end) at a fixed
    height (face box height as a share of the video height), its face centre moving through
    keyframes (time s, centre x as a share of the width), at a fixed centre height."""

    def __init__(self, photo, face, start, end, face_height, keys, center_y, sway=None, hidden=None):
        box, (pw, ph) = reference_faces()[(photo, face)]
        self.photo, self.start, self.end = photo, start, end
        self.keys, self.center_y, self.sway, self.hidden = keys, center_y, sway, hidden
        fw, fh = box[2] - box[0], box[3] - box[1]
        # Crop around the face: one face width either side, one height above, 1.8 below.
        self.crop = [
            max(0, round(box[0] - fw)),
            max(0, round(box[1] - fh)),
            min(pw, round(box[2] + fw)),
            min(ph, round(box[3] + 1.8 * fh)),
        ]
        scale = face_height * H / fh
        cw, ch = self.crop[2] - self.crop[0], self.crop[3] - self.crop[1]
        self.size = (2 * round(cw * scale / 2), 2 * round(ch * scale / 2))
        self.scale_x, self.scale_y = self.size[0] / cw, self.size[1] / ch
        # The face box inside the scaled crop.
        self.face = [
            (box[0] - self.crop[0]) * self.scale_x,
            (box[1] - self.crop[1]) * self.scale_y,
            (box[2] - self.crop[0]) * self.scale_x,
            (box[3] - self.crop[1]) * self.scale_y,
        ]

    def visible(self, t: float) -> bool:
        if not (self.start <= t < self.end):
            return False
        return not (self.hidden and self.hidden[0] <= t < self.hidden[1])

    def center_x(self, t: float) -> float:
        """Face centre x in pixels (keyframes, linear between them, plus the sway)."""
        keys = self.keys
        if t <= keys[0][0]:
            share = keys[0][1]
        elif t >= keys[-1][0]:
            share = keys[-1][1]
        else:
            for (t0, x0), (t1, x1) in zip(keys, keys[1:]):
                if t0 <= t <= t1:
                    share = x0 + (x1 - x0) * (t - t0) / (t1 - t0)
                    break
        x = share * W
        if self.sway and self.sway[0] <= t < self.sway[1]:
            x += self.sway[2] * math.sin(2 * math.pi * self.sway[3] * (t - self.sway[0]))
        return x

    def position(self, t: float) -> tuple[int, int]:
        """Top-left of the sprite (overlay truncates to whole pixels)."""
        cx = self.center_x(t)
        fx = (self.face[0] + self.face[2]) / 2
        fy = (self.face[1] + self.face[3]) / 2
        return math.floor(cx - fx), math.floor(self.center_y * H - fy)

    def box(self, t: float) -> list[float]:
        x, y = self.position(t)
        return [round(v, 1) for v in (x + self.face[0], y + self.face[1], x + self.face[2], y + self.face[3])]

    def x_expr(self) -> str:
        """`center_x(t) - face centre`, as an FFmpeg expression (same maths)."""
        keys = self.keys
        expr = f"{keys[-1][1]}"
        for (t0, x0), (t1, x1) in reversed(list(zip(keys, keys[1:]))):
            expr = f"if(lt(t,{t1}),{x0}+({x1}-{x0})*(t-{t0})/({t1}-{t0}),{expr})"
        expr = f"if(lt(t,{keys[0][0]}),{keys[0][1]},{expr})*{W}"
        if self.sway:
            a, b, amp, freq = self.sway
            expr += f"+gte(t,{a})*lt(t,{b})*{amp}*sin(2*PI*{freq}*(t-{a}))"
        fx = (self.face[0] + self.face[2]) / 2
        return f"{expr}-{fx}"

    def y_value(self) -> float:
        fy = (self.face[1] + self.face[3]) / 2
        return self.center_y * H - fy

    def enable(self) -> str:
        expr = f"gte(t,{self.start})*lt(t,{self.end})"
        if self.hidden:
            expr += f"*not(gte(t,{self.hidden[0]})*lt(t,{self.hidden[1]}))"
        return expr


def backgrounds() -> list[tuple[str, list[int]]]:
    """Two face-free backgrounds: (photo, crop x, y, w, h)."""
    return [
        # Faces there (reference.json): x 538–606 and 227–293, y 167–235.
        ("podcast-two-wide.jpg", [650, 0, 310, 174]),  # brick wall and plant, top right
        # Faces at y ≈ 296–341.
        ("studio-wide-small-faces.jpg", [0, 0, 462, 260]),  # dark studio wall and lights
    ]


def render(name: str, duration: float, cut: float | None, sprites: list[Sprite], force: bool) -> None:
    video, truth = OUT / f"{name}.mp4", OUT / f"{name}.json"
    if video.exists() and truth.exists() and not force:
        print(f"kept fixtures/reframe/{name}.mp4")
        return
    inputs, graph = [], []
    for i, (photo, (x, y, w, h)) in enumerate(backgrounds()):
        inputs += ["-loop", 1, "-framerate", FPS, "-t", duration, "-i", FACES / photo]
        graph.append(f"[{i}:v]crop={w}:{h}:{x}:{y},scale={W}:{H},setsar=1,boxblur=4[bg{i}]")
    current = "bg0"
    if cut is not None:
        graph.append(f"[bg0][bg1]overlay=enable='gte(t,{cut})'[bgcut]")
        current = "bgcut"
    first = 2
    for k, sprite in enumerate(sprites):
        inputs += ["-loop", 1, "-framerate", FPS, "-t", duration, "-i", FACES / sprite.photo]
        x0, y0, x1, y1 = sprite.crop
        w, h = sprite.size
        graph.append(f"[{first + k}:v]crop={x1 - x0}:{y1 - y0}:{x0}:{y0},scale={w}:{h},setsar=1[s{k}]")
        graph.append(
            f"[{current}][s{k}]overlay=x='{sprite.x_expr()}':y={sprite.y_value()}"
            f":enable='{sprite.enable()}':eval=frame[v{k}]"
        )
        current = f"v{k}"
    audio = first + len(sprites)  # the silent track: the input after the sprites
    run(
        [
            *inputs,
            "-f", "lavfi", "-t", duration, "-i", "anullsrc=channel_layout=mono:sample_rate=44100",
            "-filter_complex", ";".join(graph),
            "-map", f"[{current}]", "-map", f"{audio}:a",
            "-r", FPS, "-t", duration,
            "-c:v", "libx264", "-crf", 23, "-preset", "medium", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "32k", "-movflags", "+faststart",
            video,
        ]
    )
    samples = []
    for k in range(int(duration * 1000) // STEP_MS):
        t_ms = k * STEP_MS
        frame_t = math.floor(t_ms * FPS / 1000) / FPS
        faces = [
            {"id": s_id, "box": s.box(frame_t)}
            for s_id, s in ((getattr(s, "id", str(i)), s) for i, s in enumerate(sprites))
            if s.visible(frame_t)
        ]
        samples.append({"t_ms": t_ms, "faces": faces})
    truth.write_text(
        json.dumps(
            {
                "video": video.name,
                "size": [W, H],
                "fps": FPS,
                "cut_ms": None if cut is None else round(cut * 1000),
                "samples": samples,
            },
            indent=1,
        )
        + "\n"
    )
    print(f"wrote fixtures/reframe/{name}.mp4 and .json")


def pexels(name: str, force: bool) -> None:
    video = OUT / f"{name}.mp4"
    if video.exists() and not force:
        print(f"kept fixtures/reframe/{name}.mp4")
        return
    _, url, start, duration = PEXELS[name]
    with tempfile.TemporaryDirectory() as tmp:
        original = Path(tmp) / "original.mp4"
        request = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(request) as response, original.open("wb") as f:
            shutil.copyfileobj(response, f)
        run(
            [
                "-ss", start, "-t", duration, "-i", original,
                "-f", "lavfi", "-t", duration, "-i", "anullsrc=channel_layout=mono:sample_rate=44100",
                "-map", "0:v:0", "-map", "1:a",
                "-vf", f"scale={W}:{H}:flags=area,fps={FPS}",
                "-c:v", "libx264", "-crf", 26, "-preset", "medium", "-pix_fmt", "yuv420p",
                "-c:a", "aac", "-b:a", "32k", "-movflags", "+faststart", "-shortest",
                video,
            ]
        )
    print(f"wrote fixtures/reframe/{name}.mp4")


def main() -> None:
    force = "--force" in sys.argv[1:]
    OUT.mkdir(parents=True, exist_ok=True)

    one = Sprite("portrait-frontal.jpg", 0, 0, 8, 0.25, [(0, 0.35), (3, 0.35), (4, 0.65), (8, 0.65)], 0.38,
                 sway=(4, 6, 6, 1.0))
    one.id = "A"
    two = Sprite("portrait-glasses.jpg", 0, 8, 12, 0.32, [(8, 0.25), (12, 0.30)], 0.40, hidden=(10.0, 10.5))
    two.id = "B"
    render("moving-face", 12.0, 8.0, [one, two], force)

    # podcast-two-close.jpg: face 0 is the woman on the right, face 1 the one on the left.
    a = Sprite("podcast-two-close.jpg", 1, 0, 5, 0.30, [(0, 0.27)], 0.40)
    b = Sprite("podcast-two-close.jpg", 0, 0, 2, 0.22, [(0, 0.73)], 0.40)
    b_big = Sprite("podcast-two-close.jpg", 0, 2, 3, 0.33, [(0, 0.73)], 0.40)
    b_again = Sprite("podcast-two-close.jpg", 0, 3, 5, 0.22, [(0, 0.73)], 0.40)
    a_after = Sprite("podcast-two-close.jpg", 1, 5, 10, 0.20, [(0, 0.20)], 0.40)
    b_after = Sprite("podcast-two-close.jpg", 0, 5, 10, 0.32, [(0, 0.62)], 0.40)
    for sprite, face_id in [(a, "A"), (b, "B"), (b_big, "B"), (b_again, "B"), (a_after, "A"), (b_after, "B")]:
        sprite.id = face_id
    render("two-faces", 10.0, 5.0, [a, b, b_big, b_again, a_after, b_after], force)

    for name in PEXELS:
        pexels(name, force)


if __name__ == "__main__":
    main()
