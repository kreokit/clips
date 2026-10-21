<p align="center">
  <img src="assets/brand/kreokit-logo-wide.png" alt="KreoKit" width="420">
</p>

# KreoKit Clips

**Free, offline short clips from your long videos.** Drop in a podcast, interview, talk or livestream, and get vertical clips with captions for TikTok, Reels and Shorts, with no watermark, no account and no upload.

Part of [KreoKit](https://github.com/kreokit), free open-source creator tools.

> 🚧 **Alpha coming soon.** The first test version (v0.1.0-alpha) is being checked before it's published. Watch the repo to hear when it's out.

## Features

- 🎙️ **Transcribe** the whole video locally with Whisper, with word-by-word timings; cancel any time and resume later
- ✨ **Find the best moments**: clips that start and end on a full sentence, with a strong hook, scored transparently so you can see *why* each clip was picked; presets for TikTok Rewards (61–90 s), Shorts / Reels (20–60 s) or your own lengths
- ✂️ **Review and adjust**: keep or discard candidates, move the in and out points on a timeline, or add your own clips
- 📱 **Reframe to 9:16** that follows the speaker's face, with center crop, fit-with-blurred-background and manual options
- 💬 **Captions** in the styles from [KreoKit Captions](https://github.com/kreokit/captions), word-by-word highlight included
- 📤 **Export all** clips as 1080×1920 MP4 with burned-in captions (plus SRT if you want), never overwriting files without asking
- 🔒 **Private**: your videos never leave your computer

### System requirements

- **macOS 12 Monterey** or later, **Windows 10** (version 1903 or later) **/ 11** (64-bit), or **Linux** with glibc 2.35+ (e.g. Ubuntu 22.04+)
- On Intel / AMD computers, a processor with **AVX2, FMA and F16C** (most Intel CPUs since 2013 and AMD CPUs since 2015). The app tells you if yours isn't supported.
- Disk space for the AI models, downloaded once on first use: the Whisper model you pick (Base 148 MB, Small 488 MB, Large 1.6 GB) and the face model (2 MB)

## Build from source

Requirements: Rust (stable), Node.js 20+, pnpm, and FFmpeg on your `PATH` (development builds use the system FFmpeg; releases bundle an LGPL build, see [src-tauri/binaries/README.md](src-tauri/binaries/README.md)).

```bash
git clone https://github.com/kreokit/clips
cd clips
pnpm install
pnpm tauri dev
```

## Roadmap

- [x] v0.1 (alpha): transcribe, moment finder, clips board, reframe, captions, export all
- [ ] Later: optional local AI ranking of moments with a permissively licensed model
- [ ] Later: GPU acceleration

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option. The AI models (speech recognition and face detection) are downloaded on first use and are permissively licensed too (MIT / Apache 2.0 / BSD). Third-party components and their licences are listed in [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).

Implemented from public specifications, open-source libraries and observed behaviour only. Not affiliated with any company whose products are mentioned as alternatives.
