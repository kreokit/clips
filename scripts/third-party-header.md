# Third-party licences

KreoKit Clips is free software, dual-licensed under MIT or Apache-2.0. It includes or
uses the third-party components below, each under its own licence.

## AI models (downloaded on first use, not bundled)

Only models whose **weights** are MIT, Apache-2.0 or BSD licensed are used. The app will
download them from this project's own GitHub release `models-v1`, where each file sits
next to its licence, a source note and checksums. None is used yet: Whisper (speech
recognition) arrives with milestone 2, BlazeFace full-range (face detection, Apache-2.0)
with milestone 4. Each one is listed here, with its original source, when it is added.

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
