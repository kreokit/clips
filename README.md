<p align="center">
  <img src="assets/brand/kreokit-logo-wide.png" alt="KreoKit" width="420">
</p>

# KreoKit Clips

**Free, offline short clips from your long videos.** Drop in a podcast, interview, talk or livestream, and get vertical clips with captions for TikTok, Reels and Shorts, with no watermark, no account and no upload.

Part of [KreoKit](https://github.com/kreokit), free open-source creator tools.

> 🚧 **Work in progress.** There is no release yet. Follow along or watch the repo to hear about the first alpha.

## Features (planned for v0.1)

- 🎙️ **Transcribe** the whole video locally with Whisper, with word-by-word timings
- ✨ **Find the best moments**: clips that start and end on a full sentence, with a strong hook, scored transparently so you can see *why* each clip was picked
- ✂️ **Review and adjust**: keep or discard candidates, move the in and out points on a timeline, or add your own clips
- 📱 **Reframe to 9:16** that follows the speaker's face, with center crop, fit-with-blurred-background and manual options
- 💬 **Captions** in the styles from [KreoKit Captions](https://github.com/kreokit/captions), word-by-word highlight included
- 📤 **Export all** clips as 1080×1920 MP4 with burned-in captions (plus SRT if you want), never overwriting files without asking
- 🔒 **Private**: your videos never leave your computer

## Build from source

Requirements: Rust (stable), Node.js 20+, pnpm.

```bash
git clone https://github.com/kreokit/clips
cd clips
pnpm install
pnpm tauri dev
```

## Roadmap

- [ ] v0.1 (alpha): transcribe, moment finder, clips board, reframe, captions, export all
- [ ] Later: optional local AI ranking of moments with a permissively licensed model
- [ ] Later: GPU acceleration

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option. The AI models (speech recognition and face detection) are downloaded on first use and are permissively licensed too (MIT / Apache 2.0 / BSD). Third-party components will be listed in `THIRD-PARTY-LICENSES.md`.

Implemented from public specifications, open-source libraries and observed behaviour only. Not affiliated with any company whose products are mentioned as alternatives.
