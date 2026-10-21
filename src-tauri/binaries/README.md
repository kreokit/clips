# FFmpeg sidecar binaries

Release builds bundle `ffmpeg` and `ffprobe` as Tauri sidecars, one pair per target, named
`ffmpeg-<target-triple>[.exe]` / `ffprobe-<target-triple>[.exe]`. The sidecar list lives in
`../tauri.release.conf.json` (Tauri refuses to start, even in `tauri dev`, when a listed
sidecar is missing). Development uses the FFmpeg on your system instead (`PATH`, or
`KREOKIT_FFMPEG` / `KREOKIT_FFPROBE`; see `src/ffmpeg.rs`).

The binaries are not committed. CI puts them here with

```bash
python3 scripts/fetch_ffmpeg.py <target-triple>   # download or build, checksum-verified
python3 scripts/verify_ffmpeg.py <target-triple>  # licence + filters + a real 9:16 export
```

## Licence

Copied from KreoKit Captions (the same builds). All bundled FFmpeg builds are **LGPL**: built without `--enable-gpl` and
`--enable-nonfree`, so without libx264, libx265 or any other GPL-only library.
`scripts/verify_ffmpeg.py` fails the release if that ever changes: it checks the build
configuration, that `ffmpeg -L` shows the GNU Lesser General Public License, that every
filter Clips uses is there (GPL-only filters such as `boxblur` are not), and that a 9:16
clip with a caption can be exported with an encoder KreoKit Clips allows in release builds
(`h264_videotoolbox` on macOS, `h264_mf` or `libopenh264` on Windows, `libopenh264` on
Linux; never libx264).

FFmpeg runs as a separate program; KreoKit doesn't link to it. Its licence text and this
source information ship with the app (THIRD-PARTY-LICENSES.md).

## Exact sources

FFmpeg **9.0.2** on every platform.

### Windows x64 and Linux x64: BtbN FFmpeg-Builds, LGPL static variant

- Release: <https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-30-13-08>
  (a month-end build; BtbN keeps these for two years)
- FFmpeg revision: `n9.0.2-17-g2a571b6068` (the 9.0.2 release plus 17 fixes from the
  `release/9.0` branch)
- Licence: **LGPL-3.0-or-later** (configured with `--enable-version3`); includes libass,
  freetype, fribidi, harfbuzz, fontconfig and libopenh264; `--disable-libx264
  --disable-libx265`
- Build scripts and full dependency list: <https://github.com/BtbN/FFmpeg-Builds>

| Target | File | SHA-256 |
|---|---|---|
| `x86_64-pc-windows-msvc` | `ffmpeg-n9.0.2-17-g2a571b6068-win64-lgpl-9.0.zip` | `6b264b9e6019103f601d98c292bd332fd87acf1c5e941ddff4fb71760fe63432` |
| `x86_64-unknown-linux-gnu` | `ffmpeg-n9.0.2-17-g2a571b6068-linux64-lgpl-9.0.tar.xz` | `2d41cbea0ca1a15029b638330740f78d6f5aeb062355bf425433fc938705350a` |

### macOS (Apple Silicon and Intel): built from source in CI

No trustworthy LGPL macOS builds exist, so `scripts/build-ffmpeg-macos.sh` builds one
from these official tarballs (static libraries, macOS 12.0 minimum, `--disable-autodetect`,
VideoToolbox / AudioToolbox, libass with CoreText; no GPL or nonfree parts). Licence:
**LGPL-2.1-or-later**.

| Component | Version | Source | SHA-256 |
|---|---|---|---|
| FFmpeg | 9.0.2 | <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz> | `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e` |
| FreeType | 2.14.3 | <https://download.savannah.gnu.org/releases/freetype/freetype-2.14.3.tar.xz> | `36bc4f1cc413335368ee656c42afca65c5a3987e8768cc28cf11ba775e785a5f` |
| FriBidi | 1.0.17 | <https://github.com/fribidi/fribidi/releases/download/v1.0.17/fribidi-1.0.17.tar.xz> | `6949dcde27d41cebad1fd741fcafc36d55a1020d2d872d4a6eb3914caabbada2` |
| HarfBuzz | 14.6.0 | <https://github.com/harfbuzz/harfbuzz/releases/download/14.6.0/harfbuzz-14.6.0.tar.xz> | `d07a007327277708a2a73ae437887cdbaf282937f6d03ca5467723e9099af586` |
| libass | 0.17.5 | <https://github.com/libass/libass/releases/download/0.17.5/libass-0.17.5.tar.xz> | `2dca25c0e0c837ddf00b52011b3f82cac1e4ddd3ad018227806b0c2288864acc` |

## Updating

Change the version, URL and SHA-256 in `scripts/fetch_ffmpeg.py` (BtbN) or
`scripts/build-ffmpeg-macos.sh` (macOS), update this file and THIRD-PARTY-LICENSES.md, and
let CI run `verify_ffmpeg.py`.

## TODO

- [ ] The BtbN executables are large (~130 MB each, statically linked with every
      dependency). A minimal custom build with only what KreoKit uses would make the
      Windows and Linux installers much smaller.
