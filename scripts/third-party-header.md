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
Captions' folder instead (read-only, after the same checksum check). The BlazeFace licence
text also ships with the app as `licenses/MODEL-BLAZEFACE-LICENSE.txt`.

## whisper.cpp and ggml (compiled into the app)

Speech recognition uses whisper.cpp and ggml (via the whisper-rs crates), © the
ggml / whisper.cpp authors, **MIT** licence: <https://github.com/ggml-org/whisper.cpp>.

## ONNX Runtime (compiled into the app)

Face detection runs with **ONNX Runtime 1.22.0**, © Microsoft Corporation, **MIT** licence
(<https://github.com/microsoft/onnxruntime>), statically linked through the `ort` crate
(2.0.0-rc.10, MIT OR Apache-2.0) from pyke's prebuilt libraries. ONNX Runtime contains
third-party components (for example protobuf, Abseil, Eigen, XNNPACK), each under its own
licence; their notices ship with the app as `licenses/ONNXRUNTIME-ThirdPartyNotices.txt`,
next to `licenses/ONNXRUNTIME-LICENSE.txt`.

On Windows, ONNX Runtime is linked against `DirectML.dll`. KreoKit Clips does **not** ship
that file: the copy that comes with Windows 10 (version 1903 or later) and Windows 11 is
used, and face detection runs on the CPU only.

## FFmpeg (bundled program)

KreoKit Clips bundles `ffmpeg` and `ffprobe` from **FFmpeg 9.0.2** as separate programs
(sidecars); the app runs them, it does not link to them.

- Licence: **GNU Lesser General Public License** (LGPL-2.1-or-later for the macOS builds,
  LGPL-3.0-or-later for the Windows and Linux builds). No GPL or non-free components are
  enabled. Full text: `licenses/FFMPEG-LICENSE.txt` in the app, and
  <https://www.gnu.org/licenses/lgpl-3.0.html>.
- Source code: FFmpeg <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>. The exact
  builds, their sources and SHA-256 checksums are listed in
  [src-tauri/binaries/README.md](src-tauri/binaries/README.md):
  - Windows / Linux: BtbN FFmpeg-Builds, LGPL variant, release `autobuild-2026-09-30-13-08`
    (revision `n9.0.2-17-g2a571b6068`), <https://github.com/BtbN/FFmpeg-Builds>
  - macOS: built from source by `scripts/build-ffmpeg-macos.sh` in this repository
- You may replace the bundled FFmpeg with your own build: put it next to the app's
  executable under the same name, or point `KREOKIT_FFMPEG` / `KREOKIT_FFPROBE` at it.

Libraries built into the bundled FFmpeg include:

| Library | Licence | Source |
|---|---|---|
| libass | ISC | <https://github.com/libass/libass> |
| FreeType | FreeType Licence (FTL) | <https://freetype.org> |
| FriBidi | LGPL-2.1-or-later | <https://github.com/fribidi/fribidi> |
| HarfBuzz | MIT ("Old MIT") | <https://github.com/harfbuzz/harfbuzz> |
| Fontconfig (Windows / Linux builds) | MIT-style | <https://www.freedesktop.org/wiki/Software/fontconfig/> |
| OpenH264 (Windows / Linux builds) | BSD-2-Clause | <https://github.com/cisco/openh264> |

The Windows and Linux builds contain further LGPL-compatible libraries enabled by BtbN's
LGPL variant; the complete list is in `ffmpeg -buildconf` and in BtbN's build scripts.

Portions of this software are copyright © The FreeType Project (https://freetype.org).
All rights reserved.

OpenH264 in the Windows and Linux FFmpeg builds is compiled from source. Its BSD licence
covers the code; Cisco's H.264 patent licence applies only to binaries distributed by
Cisco, so it does not extend to this build. KreoKit Clips uses OpenH264 on Linux, and on Windows only when Media Foundation's H.264
encoder is not available.

## Fonts (bundled)

All under the **SIL Open Font License 1.1**; licence files ship next to the fonts (the
`fonts` folder), the same files as in KreoKit Captions (`src-tauri/resources/fonts/README.md`).

| Font | Use | Copyright | Licence file |
|---|---|---|---|
| Inter | interface and captions | © The Inter Project Authors | `fonts/OFL-Inter.txt` |
| Montserrat | captions | © The Montserrat Project Authors | `fonts/OFL-Montserrat.txt` |
| Poppins | captions | © The Poppins Project Authors | `fonts/OFL-Poppins.txt` |
| Bebas Neue | captions | © Dharma Type | `fonts/OFL-BebasNeue.txt` |

## Icons

Lucide icons, © Lucide contributors, **ISC** licence: <https://lucide.dev>.
