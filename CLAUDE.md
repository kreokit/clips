# KreoKit Clips: build spec for Claude Code

Read this before every task. It is the source of truth for scope and architecture.
**Status (2026-10-20):** Milestone 1 (Scaffold) is done and tested. Milestone 2 (transcribe long videos) is built and waiting for the user's test.
App #3 of the KreoKit suite. App #1 is KreoKit Captions (`kreokit/captions`), app #2 is KreoKit Cutout (`kreokit/cutout`). Reuse both wherever possible.

## What we are building

A cross-platform desktop app that turns a long video (podcast, interview, talk, livestream) into **vertical short clips with captions**, fully offline:
import long video → transcribe → find the best moments → review and adjust clips → reframe to 9:16 (follow the speaker) → styled captions → export all clips.

Target users: creators worldwide who repurpose long content for TikTok, Reels and Shorts. English-first.

## Hard rules (same as the other apps)

- **No proprietary code or assets.** Build only from public docs, open-source libraries and observed behaviour. Never copy another product's name, logo, icons or exact UI.
- **Offline first.** No servers, no accounts, no telemetry, no cloud AI APIs. The only network calls are one-time model downloads from our own mirror releases.
- **Licences must stay MIT/Apache-compatible, including model weights.** Only MIT / Apache-2.0 / BSD weights. Verify every model's license from its official source before adding it; never use non-commercial or research-only weights. Record everything in `THIRD-PARTY-LICENSES.md`. Mirror model files in `models-vN` releases on `kreokit/clips` (never change a published one) with license + SOURCE.md + SHA256SUMS.
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

- **Face model (decided 2026-10-19): MediaPipe BlazeFace full-range** (Apache-2.0, clean training data). It ships as TFLite; in M4 convert it to ONNX in a GitHub Actions workflow (reproducible, pinned tool versions, checked against the TFLite output, like Cutout's "Export BiRefNet"), then mirror it in `models-v2` (`models-v1` holds the Whisper models and is never changed) with licence, SOURCE.md and SHA256SUMS. Still verify the licence from the official source and record it in `THIRD-PARTY-LICENSES.md` when adding it.
- **Excluded face models:** YuNet, SCRFD and YOLO-face (training-data / licence risk). Don't revisit without asking.
- Sample ~2–5 fps, track faces, pick the active speaker (largest/most central face, or the face whose mouth region moves while that segment is spoken when feasible).
- Smooth the crop path (no jitter; limit speed; hold position during short gaps), keep the face in the upper-middle third.
- Fallbacks: center crop, or "fit with blurred background" (letterbox inside 9:16).
- Manual override: drag the crop window on a keyframe.

## Projects (decided 2026-10-19)

- **Auto-saved, no Save dialog.** Each imported video gets a project in the app data folder (`<app data>/projects/<id>/project.json`, plus its caches: transcript, candidates, face tracks). Every change is saved automatically (debounced, atomic temp file + rename, as Captions' `project.rs`); there is no Save / Save As.
- The project id is a hash of the video path + size + mtime, so importing the same video again opens its project. Recent projects on Home come from this folder; a missing video shows "Locate video…".
- Projects can be removed from Home (deletes the project folder, never the video).

## Scaffold (milestone 1)

- **Copied from Cutout:** tokens, base styles, UI kit (`src/ui/`, with `logic.test.ts`), top bar, macOS overlay title bar, Home layout, issue templates, `deny.toml`, `third_party.py`, `check_macos_binary.py`. **From Captions:** `ffmpeg.rs` (dev uses the system FFmpeg; `KREOKIT_FFMPEG` / `KREOKIT_FFPROBE` override), `probe.rs`, `fixtures/en/hello.mp4`, `scripts/make-fixtures.sh`
- **CPU check** (`cpu.rs`): the union of both engines' baselines: whisper.cpp release builds (AVX2, FMA, F16C) and ONNX Runtime 1.22 (SSSE3, SSE4.1, SSE4.2, POPCNT). The dialog offers Quit or Continue anyway
- **Probe** (`probe.rs`): duration, **display** size (90°/270° rotation swaps width and height; `rotation` is clockwise), fps, `has_audio`; cover art is not a video stream. Importing a video without sound is refused (nothing to transcribe)
- **Projects** (`projects.rs`, `src/store/project.ts`): `open_video` (create or reopen by id; a located video is found by path + fingerprint), `open_project`, `save_project` (autosave), `locate_video` (refuses a video whose size differs or length differs by more than 1 s), `remove_project`, `project_thumbnail` (`<project>/thumbnail.jpg`, served through the asset protocol, scope `$APPDATA/projects/*/thumbnail.jpg`). Ids from the webview are checked to be 16 hex digits. The frontend autosave (`src/lib/autosave.ts`) waits 600 ms, never overlaps saves, and is flushed before switching projects and when the window closes
- **Project screen** (`ProjectView.tsx`): a placeholder with the probed facts and the editable project name, until milestones 2–3 replace it
- **Dev aid:** `KREOKIT_DEV_OPEN=/path/video.mp4 pnpm tauri dev` opens that video at startup (debug builds only)
- **CI** (`ci.yml`): Linux runs everything incl. the FFmpeg tests (`--include-ignored`), cargo-deny and the THIRD-PARTY check; macOS and Windows run the frontend build and tests and the Rust tests

## Transcription (milestone 2)

- **Models** (`models.rs`): Base / Small / Large-v3-turbo from our `models-v1` mirror only (made by the manual "Mirror Whisper models" workflow: HF revision, sizes and SHA-256 pinned), stored in `<app data>/models`. A same-sized file in Captions' sibling folder (`…/com.kreokit.captions/models`) is used in place, read-only, after a SHA-256 check that is remembered in `shared-checked.json` while its size and mtime stay the same. `prepare_model` checks or downloads (`model-progress` events); `cancel` stops it
- **Engine** (`transcribe.rs`): Captions' pipeline unchanged (DTW word timings, edge refinement, sound-tag filter, custom-words prompt), split into `Engine::load` (once per run) and `Engine::transcribe` (per part). `wer.rs` from Captions (tests only until the M6 self-test)
- **Audio** (`audio.rs`): streamed from ffmpeg as 16 kHz f32 through a pipe, never fully in memory, no WAV on disk; a loudness level per second (dBFS) for M3
- **Parts** (`chunks.rs`): cut in the middle of the longest pause between 4:30 and 5:30 (nearest 5:00 on a tie), else at 5:00, judged only from audio since the part's start; audio is always decoded from the start, so cuts are the same on every run. Silent parts skip Whisper (it invents "Thank you." for silence)
- **Job** (`transcription.rs`): `<project>/transcript/` holds `meta.json` (settings, video size + mtime, part limits; any change discards the folder), `part-NNN.json` written as each part finishes (resume = skip them), and at the end `transcript.json` + `loudness.json` (part files removed). The model loads only if a part needs it. Language detected on the first part with sound, then fixed; each prompt is the custom words + the previous part's last 30 words. Times are clamped into their part and never go backwards. Progress: position, part of ~N, ETA from the measured speed (shown after 5 s of audio)
- **Commands**: `transcript_status(id, settings)` (finished transcript, or finished parts for Resume), `transcribe(id, settings)` (one at a time, `transcription-progress` events). Settings live in `project.transcription` (`#[serde(default)]`; Base in dev, Small in releases); the frontend trims custom words before sending (`normalized`), so stray spaces don't discard a cache
- **Keep awake** (`awake.rs`): while `transcribe` runs, an IOKit `PreventUserIdleSystemSleep` assertion on macOS ("KreoKit Clips: transcribing" in `pmset -g assertions`), `SetThreadExecutionState` on Windows; direct system calls, no new dependency. Nothing on Linux yet
- **UI** (`TranscribePanel.tsx`, logic in `src/lib/transcription.ts`): settings → progress with Cancel ("Resume (n of N parts done)" afterwards) → read-only transcript with "Change settings". Going Home while it runs asks first; leaving the project cancels
- **Fixture** `fixtures/en/long.mp4` (10:32, two voices, from `long-script.txt`): the ignored test `transcribes_the_long_fixture_in_parts` cancels in part 2, resumes, and checks the text equals an uninterrupted run; WER 0.031 with Base, ~85 s on the dev Mac. CI (Linux) runs it with the base model fetched from our mirror
- **CI builds whisper.cpp with a fixed CPU baseline** (`GGML_NATIVE=OFF`, AVX2/FMA/F16C on x86-64), like the releases: with the default `-march=native`, a build cached by rust-cache on a newer runner CPU crashed the tests with SIGILL on an older one. The macOS job uses the classic Accelerate flags. Change the rust-cache key whenever `GGML_*`/`CXXFLAGS` change
- **Git push of large fixtures:** `git -c http.postBuffer=157286400 push` (the default buffer fails with HTTP 400)

## Screens (v0.1)

1. **Home:** drop zone, recent projects.
2. **Processing:** transcribe + find moments, with progress and cancel (long videos take time on CPU).
3. **Clips board:** grid/list of candidate clips with score, length, first line of text; keep/discard; sort.
4. **Clip editor:** 9:16 preview with crop + captions, timeline with in/out handles, crop mode (track / center / fit+blur / manual), caption style panel (from Captions).
5. **Export all:** folder, naming `<video>-clip-01.mp4`, no silent overwrites, progress, stop, summary.

## Milestones

1. ✅ **Scaffold:** repo from Captions/Cutout foundations, dev port 1440, Home, probe a video.
2. **Transcribe long videos:** port Captions' pipeline incl. macOS build flags; chunking, progress, cancel, cache; tested on a 10+ minute synthetic video.
   - Decided 2026-10-20: the three ggml Whisper models (Base, Small, Large-v3-turbo) are mirrored unchanged in `models-v1` on `kreokit/clips` (licence, SOURCE.md, SHA256SUMS); the app downloads only from there. A matching model already in Captions' models folder is used read-only after a SHA-256 check instead of downloading it again. The computer is kept awake only while a job runs (macOS, and Windows via `SetThreadExecutionState`); check the new dependency's licence.
   - `models-v1` is published (2026-10-20). GitHub shows it as "Latest" while it's the only release; mark the app release as latest when it's published (as in Cutout, never link `/releases/latest` for the app before that).
3. **Moment finder + clips board:** scoring module with unit tests, candidate list UI, in/out editing on a timeline, manual clips.
4. **Reframe:** BlazeFace full-range converted to ONNX in Actions (license verified, mirrored), detection + tracking + smoothing, crop modes, preview.
5. **Captions + export all:** reuse styles/layout/ASS/burn-in on the 9:16 output, batch export with safe writing.
6. **Release:** reuse the release workflow, `--self-test` (transcribe a short fixture, find ≥1 moment, reframe, export one clip and verify size/duration), draft pre-release v0.1.0-alpha.

## Tests (our oracle)

- Transcription: reuse Captions' WER fixtures.
- Moment finder: unit tests on synthetic transcripts (never cuts mid-sentence, respects min/max length, hook/energy features change the ranking as expected).
- Reframe: synthetic video with a moving face-like target or a free-licensed talking-head clip; the crop keeps the target inside the frame; the crop path is smooth (bounded speed/jerk).
- Export: every clip is exactly 1080×1920, duration matches in/out, captions burned at the expected position, N clips → N files, no overwrites.

## Backlog

- **One shared KreoKit models folder** for all apps (for the future Hub), instead of each app's own folder plus Clips' read-only reuse of Captions' models.

## Dev environment

- macOS 12 Monterey, Intel Core i5 (CPU only), ~24 GB free disk (2026-10-20, after `cargo clean` in Captions and Cutout; a Tauri build folder takes several GB). Use Base for dev transcription; long videos are slow on this CPU, so always show progress and allow cancel.
