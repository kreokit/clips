<p align="center">
  <img src="assets/brand/kreokit-logo-wide.png" alt="KreoKit" width="420">
</p>

# KreoKit Clips

**Free, offline short clips from your long videos.** Drop in a podcast, interview, talk or livestream, and get vertical clips with captions for TikTok, Reels and Shorts, with no watermark, no account and no upload.

Part of [KreoKit](https://github.com/kreokit), free open-source creator tools.

🌐 **Website: [kreokit.vercel.app/clips](https://kreokit.vercel.app/clips)**: download for your computer and see how to install it.

## Download

**[All versions on the Releases page →](https://github.com/kreokit/clips/releases)** (currently [v0.1.0-alpha](https://github.com/kreokit/clips/releases/tag/v0.1.0-alpha), an early test version)

| Your computer | Download |
|---|---|
| Mac with Apple Silicon (M1, M2, M3, M4…) | [KreoKit.Clips_0.1.0_aarch64.dmg](https://github.com/kreokit/clips/releases/download/v0.1.0-alpha/KreoKit.Clips_0.1.0_aarch64.dmg) |
| Mac with Intel processor | [KreoKit.Clips_0.1.0_x64.dmg](https://github.com/kreokit/clips/releases/download/v0.1.0-alpha/KreoKit.Clips_0.1.0_x64.dmg) |
| Windows 10 / 11 | [KreoKit.Clips_0.1.0_x64-setup.exe](https://github.com/kreokit/clips/releases/download/v0.1.0-alpha/KreoKit.Clips_0.1.0_x64-setup.exe) |
| Linux (any distribution) | [KreoKit.Clips_0.1.0_amd64.AppImage](https://github.com/kreokit/clips/releases/download/v0.1.0-alpha/KreoKit.Clips_0.1.0_amd64.AppImage) |
| Linux (Debian / Ubuntu) | [KreoKit.Clips_0.1.0_amd64.deb](https://github.com/kreokit/clips/releases/download/v0.1.0-alpha/KreoKit.Clips_0.1.0_amd64.deb) |

Not sure which Mac you have? Apple menu → **About This Mac**: "Chip: Apple M…" means Apple Silicon, "Processor: Intel…" means Intel.

### Installing (the alpha isn't signed yet)

**macOS:** open the `.dmg` and drag **KreoKit Clips** to Applications, then open it. If macOS says it can't verify the app, go to **System Settings → Privacy & Security** (macOS 12: **System Preferences → Security & Privacy → General**) and click **Open Anyway**. If it says the app "is damaged", run this once in Terminal and open it again:

```bash
xattr -dr com.apple.quarantine "/Applications/KreoKit Clips.app"
```

**Windows:** run the installer. If SmartScreen says "Windows protected your PC", click **More info → Run anyway**.

**Linux:** make the AppImage executable and run it, or install the `.deb`:

```bash
chmod +x KreoKit.Clips_*.AppImage
./KreoKit.Clips_*.AppImage
# or: sudo apt install ./KreoKit.Clips_*_amd64.deb
```

Found a problem? [Open an issue](https://github.com/kreokit/clips/issues/new/choose) or use **⋯ → Report a problem** in the app.

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
