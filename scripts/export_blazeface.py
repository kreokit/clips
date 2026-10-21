#!/usr/bin/env python3
"""Converts MediaPipe BlazeFace full-range (dense) to ONNX for KreoKit Clips' reframe, reproducibly.

Runs in .github/workflows/export-blazeface.yml (pinned versions in that file):

    python scripts/export_blazeface.py --out dist --photos fixtures/faces

Model: `face_detection_full_range.tflite` (Apache-2.0, Google MediaPipe), downloaded from the
URL and SHA-256 that MediaPipe's own build pins (third_party/external_files.bzl at COMMIT).
The dense file, not the sparse one MediaPipe's graph uses: same F-score, slightly better
recall, and only ordinary convolutions to convert.

1. Converts it with tf2onnx (input stays NHWC, as the app feeds interleaved RGB), twice in
   separate processes, and requires both files to be byte-identical.
2. Runs TFLite and ONNX Runtime on the same inputs (the fixture photos fitted into the input
   with padding, as MediaPipe's ImageToTensor does, plus noise and a blank frame) and
   requires the raw outputs to match and the decoded detections (MediaPipe's anchors,
   sigmoid scores, threshold, weighted NMS) to be the same faces.
3. Writes the file, blazeface.LICENSE.txt, blazeface.SOURCE.md, SHA256SUMS.txt (the models-v2
   attachments), report.json, report.md, reference.json (the detections, for
   the Rust detector's tests) and previews/*.jpg (boxes and keypoints drawn on the photos).
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import tensorflow as tf
import tf2onnx
from PIL import Image, ImageDraw

COMMIT = "81623bc7364926f35f8d2cf75d52314208a5327a"
SOURCE_URL = (
    "https://storage.googleapis.com/mediapipe-assets/modules/face_detection/"
    "face_detection_full_range.tflite?generation=1782183493595806"
)
SOURCE_SHA256 = "99bf9494d84f50acc6617d89873f71bf6635a841ea699c17cb3377f9507cfec3"
OUT_NAME = "blazeface_full_range.onnx"
OPSET = 17

# mediapipe/modules/face_detection/face_detection_full_range.pbtxt + face_detection.pbtxt.
SIZE = 192
STRIDE = 4
ANCHOR_OFFSET = 0.5
NUM_BOXES = (SIZE // STRIDE) ** 2  # 2304: one fixed-size anchor per cell
SCALE = 192.0
SCORE_CLIP = 100.0
MIN_SCORE = 0.6
NMS_IOU = 0.3
KEYPOINTS = ["right_eye", "left_eye", "nose_tip", "mouth", "right_ear_tragion", "left_ear_tragion"]

# How closely ONNX Runtime must match TFLite for the file to pass.
MAX_RAW_DIFF = 1e-3  # box/keypoint regressors, in input pixels (of 192)
MAX_LOGIT_DIFF = 1e-3
MIN_IOU = 0.99
MAX_KEYPOINT_DIFF = 0.5 / SIZE  # half an input pixel, normalized


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(dest: Path) -> None:
    with urllib.request.urlopen(SOURCE_URL) as response:
        dest.write_bytes(response.read())
    got = sha256(dest)
    if got != SOURCE_SHA256:
        sys.exit(f"{dest.name}: SHA-256 {got}, expected {SOURCE_SHA256}")


def letterbox(image: Image.Image) -> tuple[np.ndarray, dict]:
    """The whole image fitted into SIZE×SIZE, centred, padded with black; RGB in [-1, 1]."""
    image = image.convert("RGB")
    scale = SIZE / max(image.width, image.height)
    w, h = round(image.width * scale), round(image.height * scale)
    x0, y0 = (SIZE - w) // 2, (SIZE - h) // 2
    canvas = Image.new("RGB", (SIZE, SIZE))
    canvas.paste(image.resize((w, h), Image.BILINEAR), (x0, y0))
    tensor = np.asarray(canvas, dtype=np.float32) / 127.5 - 1.0
    return tensor[None], {"x0": x0, "y0": y0, "w": w, "h": h}


def anchors() -> np.ndarray:
    cells = SIZE // STRIDE
    ys, xs = np.mgrid[0:cells, 0:cells]
    centers = np.stack([(xs + ANCHOR_OFFSET) / cells, (ys + ANCHOR_OFFSET) / cells], axis=-1)
    return centers.reshape(-1, 2).astype(np.float32)


def decode(regressors: np.ndarray, logits: np.ndarray) -> list[dict]:
    """MediaPipe's TensorsToDetections (reverse_output_order, fixed anchor size 1) + weighted NMS.
    Coordinates are normalized to the SIZE×SIZE input."""
    raw = regressors.reshape(NUM_BOXES, 16)
    scores = 1.0 / (1.0 + np.exp(-np.clip(logits.reshape(NUM_BOXES), -SCORE_CLIP, SCORE_CLIP)))
    centers = anchors()
    detections = []
    for i in np.flatnonzero(scores >= MIN_SCORE):
        cx, cy = raw[i, 0] / SCALE + centers[i, 0], raw[i, 1] / SCALE + centers[i, 1]
        w, h = raw[i, 2] / SCALE, raw[i, 3] / SCALE
        points = raw[i, 4:].reshape(6, 2) / SCALE + centers[i]
        detections.append(
            {
                "score": float(scores[i]),
                "box": [float(cx - w / 2), float(cy - h / 2), float(cx + w / 2), float(cy + h / 2)],
                "keypoints": points.tolist(),
            }
        )
    return weighted_nms(detections)


def iou(a: list[float], b: list[float]) -> float:
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    inter = ix * iy
    union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / union if union > 0 else 0.0


def weighted_nms(detections: list[dict]) -> list[dict]:
    """MediaPipe's WEIGHTED algorithm: each cluster (IoU > NMS_IOU with its best) is replaced by
    the score-weighted average of its boxes and keypoints, keeping the best score."""
    remaining = sorted(detections, key=lambda d: -d["score"])
    kept = []
    while remaining:
        best = remaining[0]
        cluster = [d for d in remaining if iou(best["box"], d["box"]) > NMS_IOU]
        remaining = [d for d in remaining if iou(best["box"], d["box"]) <= NMS_IOU]
        weights = np.array([d["score"] for d in cluster])
        weights /= weights.sum()
        kept.append(
            {
                "score": best["score"],
                "box": np.average([d["box"] for d in cluster], axis=0, weights=weights).tolist(),
                "keypoints": np.average(
                    [d["keypoints"] for d in cluster], axis=0, weights=weights
                ).tolist(),
            }
        )
    return kept


class Tflite:
    def __init__(self, path: Path):
        self.interpreter = tf.lite.Interpreter(model_path=str(path))
        self.interpreter.allocate_tensors()
        self.input = self.interpreter.get_input_details()[0]
        self.outputs = self.interpreter.get_output_details()

    def run(self, x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        self.interpreter.set_tensor(self.input["index"], x)
        self.interpreter.invoke()
        tensors = [self.interpreter.get_tensor(o["index"]) for o in self.outputs]
        return split_outputs(tensors)


class Onnx:
    def __init__(self, path: Path):
        options = ort.SessionOptions()
        options.intra_op_num_threads = 1  # the app runs it on one thread beside FFmpeg
        self.session = ort.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])
        self.input = self.session.get_inputs()[0].name

    def run(self, x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        return split_outputs(self.session.run(None, {self.input: x}))


def split_outputs(tensors: list[np.ndarray]) -> tuple[np.ndarray, np.ndarray]:
    """(regressors [1, 2304, 16], logits [1, 2304, 1]), told apart by their last dimension."""
    by_width = {t.shape[-1]: t for t in tensors}
    return by_width[16], by_width[1]


def compare(name: str, x: np.ndarray, lite: Tflite, onnx_model: Onnx) -> dict:
    lite_reg, lite_logits = lite.run(x)
    onnx_reg, onnx_logits = onnx_model.run(x)
    lite_dets, onnx_dets = decode(lite_reg, lite_logits), decode(onnx_reg, onnx_logits)
    result = {
        "input": name,
        "max_regressor_diff": float(np.abs(lite_reg - onnx_reg).max()),
        "max_logit_diff": float(np.abs(lite_logits - onnx_logits).max()),
        "tflite_faces": len(lite_dets),
        "onnx_faces": len(onnx_dets),
        "min_iou": 1.0,
        "max_keypoint_diff": 0.0,
        "max_score_diff": 0.0,
    }
    for a, b in zip(lite_dets, onnx_dets):
        result["min_iou"] = min(result["min_iou"], iou(a["box"], b["box"]))
        result["max_keypoint_diff"] = max(
            result["max_keypoint_diff"],
            float(np.abs(np.array(a["keypoints"]) - np.array(b["keypoints"])).max()),
        )
        result["max_score_diff"] = max(result["max_score_diff"], abs(a["score"] - b["score"]))
    result["passed"] = (
        result["max_regressor_diff"] <= MAX_RAW_DIFF
        and result["max_logit_diff"] <= MAX_LOGIT_DIFF
        and result["tflite_faces"] == result["onnx_faces"]
        and result["min_iou"] >= MIN_IOU
        and result["max_keypoint_diff"] <= MAX_KEYPOINT_DIFF
    )
    result["detections"] = onnx_dets
    return result


def to_image(detection: dict, fit: dict, image: Image.Image) -> dict:
    """A detection from input coordinates to the photo's pixels."""
    scale = image.width / fit["w"]

    def point(x: float, y: float) -> list[float]:
        return [(x * SIZE - fit["x0"]) * scale, (y * SIZE - fit["y0"]) * scale]

    box = detection["box"]
    return {
        "score": detection["score"],
        "box": point(box[0], box[1]) + point(box[2], box[3]),
        "keypoints": {k: point(*p) for k, p in zip(KEYPOINTS, detection["keypoints"])},
    }


def preview(image: Image.Image, faces: list[dict], dest: Path) -> None:
    image = image.convert("RGB")
    draw = ImageDraw.Draw(image)
    width = max(2, image.width // 300)
    for face in faces:
        draw.rectangle(face["box"], outline=(200, 255, 0), width=width)
        draw.text((face["box"][0] + 4, face["box"][1] + 4), f"{face['score']:.2f}", fill=(200, 255, 0))
        for x, y in face["keypoints"].values():
            r = width * 1.5
            draw.ellipse([x - r, y - r, x + r, y + r], fill=(255, 64, 64))
    image.save(dest, quality=85)


def model_facts(path: Path) -> dict:
    model = onnx.load(str(path))
    onnx.checker.check_model(model, full_check=True)

    def shape(value) -> list:
        return [d.dim_value or d.dim_param for d in value.type.tensor_type.shape.dim]

    return {
        "opset": max(o.version for o in model.opset_import if o.domain in ("", "ai.onnx")),
        "inputs": [{"name": v.name, "shape": shape(v)} for v in model.graph.input],
        "outputs": [{"name": v.name, "shape": shape(v)} for v in model.graph.output],
        "ops": sorted({n.op_type for n in model.graph.node}),
        "nonstandard_domains": sorted({n.domain for n in model.graph.node if n.domain not in ("", "ai.onnx")}),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--photos", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "previews").mkdir(exist_ok=True)

    source = args.out / "face_detection_full_range.tflite"
    download(source)
    lite = Tflite(source)
    tflite_input = {
        "shape": lite.input["shape"].tolist(),
        "dtype": str(np.dtype(lite.input["dtype"])),
    }
    if tflite_input["shape"] != [1, SIZE, SIZE, 3]:
        sys.exit(f"Unexpected TFLite input {tflite_input}; the settings above assume 1×192×192×3")

    target = args.out / OUT_NAME
    reproducible = convert(source, target) == convert(source, args.out / "second-run.onnx")
    (args.out / "second-run.onnx").unlink()
    facts = model_facts(target)
    onnx_model = Onnx(target)

    results, reference = [], []
    for photo in sorted(args.photos.glob("*.jpg")):
        image = Image.open(photo)
        x, fit = letterbox(image)
        result = compare(photo.name, x, lite, onnx_model)
        faces = [to_image(d, fit, image) for d in result.pop("detections")]
        preview(image, faces, args.out / "previews" / photo.name)
        reference.append({"photo": photo.name, "size": [image.width, image.height], "faces": faces})
        results.append(result)

    rng = np.random.default_rng(7)
    for name, x in [
        ("noise", rng.uniform(-1, 1, (1, SIZE, SIZE, 3)).astype(np.float32)),
        ("black", np.full((1, SIZE, SIZE, 3), -1.0, dtype=np.float32)),
    ]:
        result = compare(name, x, lite, onnx_model)
        result.pop("detections")
        results.append(result)

    x, _ = letterbox(Image.open(sorted(args.photos.glob("*.jpg"))[0]))
    onnx_model.run(x)
    start = time.perf_counter()
    for _ in range(100):
        onnx_model.run(x)
    ms_per_frame = (time.perf_counter() - start) * 10

    passed = all(r["passed"] for r in results) and not facts["nonstandard_domains"] and reproducible
    release = release_files(args.out, target, facts)
    sums = {p.name: sha256(p) for p in [target, *release]}
    (args.out / "SHA256SUMS.txt").write_text("".join(f"{h}  {n}\n" for n, h in sums.items()))
    report = {
        "passed": passed,
        "reproducible": reproducible,
        "source": {"url": SOURCE_URL, "sha256": SOURCE_SHA256, "mediapipe_commit": COMMIT},
        "tflite_input": tflite_input,
        "onnx": facts | {"file": OUT_NAME, "bytes": target.stat().st_size, "sha256": sums[OUT_NAME]},
        "versions": {
            "tensorflow": tf.__version__,
            "tf2onnx": tf2onnx.__version__,
            "onnx": onnx.__version__,
            "onnxruntime": ort.__version__,
        },
        "limits": {
            "max_regressor_diff": MAX_RAW_DIFF,
            "max_logit_diff": MAX_LOGIT_DIFF,
            "min_iou": MIN_IOU,
            "max_keypoint_diff": MAX_KEYPOINT_DIFF,
        },
        "onnxruntime_ms_per_frame_1_thread": round(ms_per_frame, 2),
        "results": results,
    }
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    (args.out / "reference.json").write_text(
        json.dumps(
            {
                "model_sha256": sums[OUT_NAME],
                "input": "photo fitted into 192×192 (centred, black padding, bilinear), RGB/127.5 − 1",
                "min_score": MIN_SCORE,
                "nms_iou": NMS_IOU,
                "photos": reference,
            },
            indent=2,
        )
        + "\n"
    )
    (args.out / "report.md").write_text(markdown(report, reference))
    print((args.out / "report.md").read_text())
    if not passed:
        sys.exit("The ONNX file failed a check (see report.md)")


LICENSE_URL = f"https://raw.githubusercontent.com/google-ai-edge/mediapipe/{COMMIT}/LICENSE"
MODEL_CARD_URL = "https://drive.google.com/file/d/1jpQt8TB1nMFQ49VSSBKdNEdQOygNRvCP/preview"


def convert(source: Path, target: Path) -> str:
    """tf2onnx in its own process with a fixed hash seed: its folded-constant names follow
    Python's set order, which otherwise changes every run. Returns the file's SHA-256."""
    env = os.environ | {"PYTHONHASHSEED": "0"}
    subprocess.run(
        [sys.executable, "-m", "tf2onnx.convert", "--tflite", str(source), "--opset", str(OPSET),
         "--output", str(target)],
        env=env, check=True, capture_output=True,
    )
    return sha256(target)


def release_files(out: Path, target: Path, facts: dict) -> list[Path]:
    """The models-v2 attachments besides the model: licence text and SOURCE.md (named as in
    models-v1)."""
    license_file = out / "blazeface.LICENSE.txt"
    with urllib.request.urlopen(LICENSE_URL) as response:
        license_file.write_bytes(response.read())
    if b"Apache License" not in license_file.read_bytes()[:200]:
        sys.exit(f"{LICENSE_URL} is not the Apache License")
    source = out / "blazeface.SOURCE.md"
    source.write_text(
        f"""# BlazeFace full-range (face detector), converted to ONNX

`{target.name}` is Google MediaPipe's **BlazeFace full-range (dense)** face detector,
converted from TFLite to ONNX for KreoKit Clips (reframing clips to 9:16). The weights are
unchanged; only the file format differs.

- **Original:** `face_detection_full_range.tflite`, {SOURCE_URL}
  (SHA-256 `{SOURCE_SHA256}`, as pinned by MediaPipe's own build in
  `third_party/external_files.bzl` at commit
  [`{COMMIT}`](https://github.com/google-ai-edge/mediapipe/tree/{COMMIT}))
- **Licence:** Apache License 2.0 (`blazeface.LICENSE.txt`, from the MediaPipe repository at
  that commit). The model card also states Apache License 2.0 and that the model was trained
  on consented images: "MediaPipe BlazeFace Model Card (Full Range)", Valentin Bazarevsky and
  Yury Kartynnik, Google, 2021-01-19, {MODEL_CARD_URL}
- **Paper:** V. Bazarevsky et al., "BlazeFace: Sub-millisecond Neural Face Detection on Mobile
  GPUs", CVPR Workshop on Computer Vision for AR/VR, 2019. https://arxiv.org/abs/1907.05047
- **Conversion:** tf2onnx {tf2onnx.__version__} (tensorflow {tf.__version__}, onnx
  {onnx.__version__}), opset {facts['opset']}, run with PYTHONHASHSEED=0 (byte-reproducible), by `scripts/export_blazeface.py` in the
  "Export BlazeFace" workflow of https://github.com/kreokit/clips, and checked against the
  TFLite model with onnxruntime {ort.__version__} (raw outputs and decoded faces).
- **Input:** `input`, float32 [1, 192, 192, 3] (NHWC, RGB in [-1, 1]). **Outputs:** box and
  keypoint regressors [1, 2304, 16] and score logits [1, 2304, 1], decoded with MediaPipe's
  SSD anchors (one per cell, stride 4) as in `mediapipe/modules/face_detection`.

Changes: format conversion only (TFLite → ONNX); see the Apache License, section 4(b).
"""
    )
    return [license_file, source]


def markdown(report: dict, reference: list[dict]) -> str:
    onnx_facts = report["onnx"]
    lines = [
        f"# BlazeFace full-range → ONNX: {'PASSED' if report['passed'] else 'FAILED'}",
        "",
        f"- Source: `face_detection_full_range.tflite`, SHA-256 `{report['source']['sha256']}` "
        f"(pinned by MediaPipe `{COMMIT[:12]}` `third_party/external_files.bzl`)",
        f"- TFLite input: {report['tflite_input']['shape']} {report['tflite_input']['dtype']}",
        f"- ONNX: `{onnx_facts['file']}`, {onnx_facts['bytes']:,} bytes, opset {onnx_facts['opset']}, "
        f"SHA-256 `{onnx_facts['sha256']}`",
        f"- Reproducible (two conversions, byte-identical): {'yes' if report['reproducible'] else '**no**'}",
        f"- ONNX inputs: {onnx_facts['inputs']}",
        f"- ONNX outputs: {onnx_facts['outputs']}",
        f"- Ops: {', '.join(onnx_facts['ops'])}"
        + (f" (non-standard domains: {onnx_facts['nonstandard_domains']})" if onnx_facts["nonstandard_domains"] else ""),
        f"- Tools: " + ", ".join(f"{k} {v}" for k, v in report["versions"].items()),
        f"- ONNX Runtime, 1 thread: {report['onnxruntime_ms_per_frame_1_thread']} ms per frame",
        "",
        "| Input | Faces (TFLite / ONNX) | Max regressor diff | Max logit diff | Min IoU | Max keypoint diff | Passed |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in report["results"]:
        lines.append(
            f"| {r['input']} | {r['tflite_faces']} / {r['onnx_faces']} | {r['max_regressor_diff']:.2e} | "
            f"{r['max_logit_diff']:.2e} | {r['min_iou']:.4f} | {r['max_keypoint_diff']:.2e} | "
            f"{'yes' if r['passed'] else '**no**'} |"
        )
    lines += ["", "Limits: " + ", ".join(f"{k} {v:g}" for k, v in report["limits"].items()), ""]
    lines += ["## Faces found (ONNX, photo pixels)", ""]
    for photo in reference:
        scores = ", ".join(f"{f['score']:.2f}" for f in photo["faces"]) or "none"
        lines.append(f"- `{photo['photo']}` ({photo['size'][0]}×{photo['size'][1]}): {len(photo['faces'])} face(s), scores {scores}")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    main()
