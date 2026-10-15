# KreoKit Clips: build spec for Claude Code

Read this before every task. It is the source of truth for scope and architecture.
App #3 of the KreoKit suite. App #1 is KreoKit Captions (`kreokit/captions`), app #2 is KreoKit Cutout (`kreokit/cutout`). Reuse both wherever possible.

## What we are building

A cross-platform desktop app that turns a long video (podcast, interview, talk, livestream) into **vertical short clips with captions**, fully offline:
import long video → transcribe → find the best moments → review and adjust clips → reframe to 9:16 (follow the speaker) → styled captions → export all clips.

Target users: creators worldwide who repurpose long content for TikTok, Reels and Shorts. English-first.

## Hard rules (same as the other apps)

- **No proprietary code or assets.** Build only from public docs, open-source libraries and observed behaviour. Never copy another product's name, logo, icons or exact UI.
- **Offline first.** No servers, no accounts, no telemetry, no cloud AI APIs. The only network calls are one-time model downloads from our own mirror releases.
- **Licences must stay MIT/Apache-compatible, including model weights.** Only MIT / Apache-2.0 / BSD weights. Verify every model's license from its official source before adding it; never use non-commercial or research-only weights. Record everything in `THIRD-PARTY-LICENSES.md`. Mirror model files in a `models-v1` release on `kreokit/clips` with license + SOURCE.md + SHA256SUMS.
- Every commit builds and passes `cargo test` and `pnpm test` on its own. Small topic-based commits.
- Only screenshot the KreoKit window, never the full screen.
- Fixtures: only our own or free-licensed videos (Pexels/Pixabay, license noted per file), or synthetic ones (macOS `say` + ffmpeg test patterns).

## Lessons from Captions and Cutout (must follow)

- **Release builds use a newer macOS SDK than dev builds.** Weak imports can crash on older macOS. Run `check_macos_binary.py` (copy from Cutout, with its two documented allow-listed symbols) on every macOS release build.
- **whisper.cpp on macOS:** build with `-UACCELERATE_NEW_LAPACK -UACCELERATE_LAPACK_ILP64` (classic Accelerate `cblas_sgemm`), exactly as Captions does. Never the macOS 13.3+ variant.
- **ONNX Runtime:** `ort` 2.0.0-rc.10 + ONNX Runtime 1.22 static, pinned for Intel Mac / macOS 12 support, exactly as Cutout does.
- **FFmpeg:** LGPL-only sidecars (no libx264/libx265, no GPL), built and checked exactly as Captions does.
- Release workflow: build → all checks (bundle, licenses, macOS binary check, `--self-test`) → upload only if everything passed → draft pre-release.
- Dev port: use **1440** (Captions 1420, Cutout 1430).

## Reuse

Copy (don't reinvent), as listed in the other apps' "Reusable" sections:
- From **Captions**: Whisper transcription pipeline (word timings, edge refinement, custom words, sound-tag filter), caption styles/presets, shared line-layout rules (Rust + TS with shared test cases), ASS generator, burn-in export, free caption positioning, bundled caption fonts, FFmpeg sidecar scripts, waveform, timeline component.
- From **Cutout**: ONNX Runtime setup and model download/mirror/checksum code, memory check (`memory.rs`), batch grid pattern, safe no-overwrite writing, "Export all" flow.
- From both: design tokens, UI kit, app shell, release workflow, license checks, issue templates, "Report a problem", brand assets.

## Stack

- Tauri 2 (Rust) + React + TypeScript + Vite, pnpm
- whisper-rs (Whisper), `ort` (ONNX Runtime) for face detection, FFmpeg sidecar for decode/encode
- No LLM in v0.1 (see "Moment finder")

## Core pipeline

1. **Probe + audio extract** (ffmpeg).
2. **Transcribe** the whole video with word timings (chunked for long videos, progress + cancel, resumable cache per video).
3. **Moment finder** proposes candidate clips (default 20–60 s).
4. **User review:** keep/discard candidates, adjust in/out points on the timeline, add manual clips.
5. **Reframe** each clip to 9:16: detect faces on sampled frames, build a smoothed crop path that follows the active speaker; fallback center crop; manual override.
6. **Captions** per clip using Captions' styles (positions relative to the 9:16 frame).
7. **Export** all clips: 1080×1920 MP4 with burned-in captions, plus optional SRT per clip.

## Moment finder (v0.1: transparent heuristics, no LLM)

Score candidate windows built from sentence/pause boundaries (never cut mid-sentence):
- Self-contained: starts at a sentence start, ends at a sentence end, minimal dangling references at the start ("and", "so", "but", "that").
- Hook strength in the first 3 s: questions, numbers, strong statements, "the secret/mistake/truth is…", direct address ("you").
- Energy: speech rate and audio loudness relative to the video's average; laughter/applause tags.
- Length fit to the target duration.
Show the score breakdown in the UI ("why this clip"), so users trust it and can sort/filter. Keep the scoring in one pure, unit-tested Rust module. (Backlog: optional local LLM ranking with a permissively licensed model.)

## Reframe

- Face detection: a small permissively licensed ONNX model (candidates: YuNet from OpenCV Zoo, BlazeFace from MediaPipe; **verify weights licence first**).
- Sample ~2–5 fps, track faces, pick the active speaker (largest/most central face, or the face whose mouth region moves while that segment is spoken when feasible).
- Smooth the crop path (no jitter; limit speed; hold position during short gaps), keep the face in the upper-middle third.
- Fallbacks: center crop, or "fit with blurred background" (letterbox inside 9:16).
- Manual override: drag the crop window on a keyframe.

## Screens (v0.1)

1. **Home:** drop zone, recent projects.
2. **Processing:** transcribe + find moments, with progress and cancel (long videos take time on CPU).
3. **Clips board:** grid/list of candidate clips with score, length, first line of text; keep/discard; sort.
4. **Clip editor:** 9:16 preview with crop + captions, timeline with in/out handles, crop mode (track / center / fit+blur / manual), caption style panel (from Captions).
5. **Export all:** folder, naming `<video>-clip-01.mp4`, no silent overwrites, progress, stop, summary.

## Milestones

1. **Scaffold:** repo from Captions/Cutout foundations, dev port 1440, Home, probe a video.
2. **Transcribe long videos:** port Captions' pipeline incl. macOS build flags; chunking, progress, cancel, cache; tested on a 10+ minute synthetic video.
3. **Moment finder + clips board:** scoring module with unit tests, candidate list UI, in/out editing on a timeline, manual clips.
4. **Reframe:** face model (license verified, mirrored), detection + tracking + smoothing, crop modes, preview.
5. **Captions + export all:** reuse styles/layout/ASS/burn-in on the 9:16 output, batch export with safe writing.
6. **Release:** reuse the release workflow, `--self-test` (transcribe a short fixture, find ≥1 moment, reframe, export one clip and verify size/duration), draft pre-release v0.1.0-alpha.

## Tests (our oracle)

- Transcription: reuse Captions' WER fixtures.
- Moment finder: unit tests on synthetic transcripts (never cuts mid-sentence, respects min/max length, hook/energy features change the ranking as expected).
- Reframe: synthetic video with a moving face-like target or a free-licensed talking-head clip; the crop keeps the target inside the frame; the crop path is smooth (bounded speed/jerk).
- Export: every clip is exactly 1080×1920, duration matches in/out, captions burned at the expected position, N clips → N files, no overwrites.

## Dev environment

- macOS 12 Monterey, Intel Core i5 (CPU only), ~13 GB free disk. Use Base for dev transcription; long videos are slow on this CPU, so always show progress and allow cancel.
