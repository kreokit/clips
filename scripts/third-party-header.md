# Third-party licences

KreoKit Clips is free software, dual-licensed under MIT or Apache-2.0. It includes or
uses the third-party components below, each under its own licence.

## AI models (downloaded on first use, not bundled)

Only models whose **weights** are MIT, Apache-2.0 or BSD licensed are used. The app
downloads them only from this project's own GitHub releases (`models-vN`), where each file
sits next to its licence, a source note and checksums, and checks every file against a
pinned SHA-256.

| Model | Used for | Licence | Mirror | Original source |
|---|---|---|---|---|
| Whisper Base, Small, Large-v3-turbo (ggml) | speech recognition | **MIT** (weights © 2022 OpenAI; ggml conversion © the ggml authors) | `models-v1` | <https://github.com/openai/whisper>, converted by <https://github.com/ggml-org/whisper.cpp>, files from <https://huggingface.co/ggerganov/whisper.cpp> |
| BlazeFace full-range (ONNX) | face detection for reframing to 9:16 | **Apache-2.0** (© Google LLC, MediaPipe; converted from TFLite to ONNX, format only) | `models-v2` | <https://github.com/google-ai-edge/mediapipe> (`face_detection_full_range.tflite`, model card: BlazeFace full range, Google 2021) |

A matching Whisper model that KreoKit Captions has already downloaded is used from
Captions' folder instead (read-only, after the same checksum check).

## whisper.cpp and ggml (compiled into the app)

Speech recognition uses whisper.cpp and ggml (via the whisper-rs crates), © the
ggml / whisper.cpp authors, **MIT** licence: <https://github.com/ggml-org/whisper.cpp>.

## FFmpeg (separate program)

Development builds use the FFmpeg installed on the computer. Release builds (milestone 6)
will bundle an **LGPL** FFmpeg build (no GPL or non-free parts, no libx264/libx265) as a
separate program, as KreoKit Captions does; its exact source and licence will be listed
here.

## Fonts (bundled)

Inter (interface font), © The Inter Project Authors, **SIL Open Font License 1.1**; the
licence ships with the app as `licenses/Inter-OFL.txt`.

## Icons

Lucide icons, © Lucide contributors, **ISC** licence: <https://lucide.dev>.
