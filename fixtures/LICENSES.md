# Fixture licences

Every fixture file, where it comes from and its licence. Only our own or free-licensed media
(Pexels / Pixabay, licence checked on the source page) and synthetic files; no people from
movies, TV or YouTube.

## Synthetic (ours)

| File | Source | Licence |
|---|---|---|
| `en/hello.mp4`, `en/hello.txt` | Synthetic: macOS `say` (voice Samantha) over a flat colour, made by `scripts/make-fixtures.sh` (copied from KreoKit Captions) | Ours, MIT OR Apache-2.0 |
| `en/brands.mp4`, `en/brands.txt` | Synthetic, the same way (made-up names, for the custom-words test; copied from KreoKit Captions) | Ours, MIT OR Apache-2.0 |
| `en/long.mp4`, `en/long.txt`, `en/long-script.txt` | Synthetic: an interview written for KreoKit (`long-script.txt`), spoken by macOS `say` (Samantha and Daniel) with pauses, 10:32, made by `scripts/make-fixtures.sh` | Ours, MIT OR Apache-2.0 |

## Face photos (`faces/`)

For the face detector (BlazeFace export check, `scripts/export_blazeface.py`, and the Rust
detector tests). Downloaded from Pexels on 2026-10-21 under the
[Pexels License](https://www.pexels.com/license/) (free to use and modify, no attribution
required; credited here anyway), scaled to 960 px on the long side and re-saved as JPEG,
otherwise unchanged.

| File | Case | Author | Source |
|---|---|---|---|
| `faces/portrait-frontal.jpg` | one large frontal face (black and white) | centaurus picture | <https://www.pexels.com/photo/portrait-of-a-man-looking-at-the-camera-12275727/> |
| `faces/portrait-glasses.jpg` | one face with glasses, busy background | Uvaish Khan | <https://www.pexels.com/photo/man-looking-at-the-camera-3543425/> |
| `faces/studio-wide-small-faces.jpg` | two small faces in a wide studio shot | cottonbro studio | <https://www.pexels.com/photo/a-man-and-woman-talking-together-6883805/> |
| `faces/podcast-two-close.jpg` | two people at microphones, faces turned | George Milton | <https://www.pexels.com/photo/positive-black-woman-talking-to-radio-host-6954162/> |
| `faces/podcast-two-wide.jpg` | the same set, wider, smaller faces | George Milton | <https://www.pexels.com/photo/multiethnic-women-talking-in-broadcasting-studio-6953672/> |

## Reframe videos (`reframe/`)

Made by `scripts/make_reframe_fixtures.py` (FFmpeg, libx264 for the test files only; they
are never bundled). Every file has a silent mono AAC track so the app can import it.

| File | Source | Licence |
|---|---|---|
| `reframe/moving-face.mp4`, `.json` | Synthetic: `faces/portrait-frontal.jpg` and `faces/portrait-glasses.jpg` moved along scripted paths over blurred, face-free crops of `faces/podcast-two-wide.jpg` and `faces/studio-wide-small-faces.jpg`; the `.json` holds the true face boxes | Pexels License (the photos above); our arrangement MIT OR Apache-2.0 |
| `reframe/two-faces.mp4`, `.json` | Synthetic, the same way, from the two faces of `faces/podcast-two-close.jpg` | Pexels License (the photos above); our arrangement MIT OR Apache-2.0 |
| `reframe/podcast-wide.mp4` | "Professional podcast interview in studio setting" by Mihaela Claudia Puscas, <https://www.pexels.com/video/professional-podcast-interview-in-studio-setting-37874130/>, downloaded 2026-10-21; 0:10–0:30, scaled to 640×360 (the original has no sound) | [Pexels License](https://www.pexels.com/license/) |
| `reframe/talking-head.mp4` | "Professional podcast interview in modern studio" by Mihaela Claudia Puscas, <https://www.pexels.com/video/professional-podcast-interview-in-modern-studio-38630316/>, downloaded 2026-10-21; 0:00–0:12, scaled to 640×360 (the original has no sound) | [Pexels License](https://www.pexels.com/license/) |
