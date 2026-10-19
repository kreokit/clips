#!/usr/bin/env bash
# Generates the synthetic English test clips in fixtures/en/ (macOS only: uses `say`).
# Existing clips are kept (re-encoding changes the bytes); pass --force to rebuild them.
#
#   <name>.txt  reference transcript (the exact text spoken)
#   <name>.mp4  640x360 dark background + speech, starting after 1 s of silence
#
#   hello   plain English, for probe, accuracy and timing tests (same clip as KreoKit Captions)
#   brands  made-up names, for the custom-words test (same clip as KreoKit Captions)
#   long    a 10.5-minute interview, two voices, from long-script.txt (transcription in parts)
set -euo pipefail

cd "$(dirname "$0")/../fixtures/en"

VOICE="Samantha"
FORCE="${1:-}"

# Prefer the hardware encoder; fall back to libx264 (fine for a test file, never bundled).
encoder="h264_videotoolbox"
ffmpeg -hide_banner -encoders 2>/dev/null | grep -q " $encoder " || encoder="libx264"

make_clip() {
  local name="$1" text="$2"
  if [[ -f "$name.mp4" && "$FORCE" != "--force" ]]; then
    echo "kept fixtures/en/$name.mp4"
    return
  fi
  echo "$text" > "$name.txt"

  local tmp
  tmp="$(mktemp -d)"
  say -v "$VOICE" -o "$tmp/speech.aiff" "$text"
  ffmpeg -nostdin -y -v error \
    -f lavfi -i "color=c=0x1e1e28:s=640x360:r=25" \
    -i "$tmp/speech.aiff" \
    -filter_complex "[1:a]adelay=1000:all=1,apad=pad_dur=1[a]" \
    -map 0:v -map "[a]" -shortest \
    -c:v "$encoder" -b:v 200k -pix_fmt yuv420p \
    -c:a aac -b:a 64k -ac 1 -ar 44100 \
    -movflags +faststart \
    "$name.mp4"
  rm -rf "$tmp"
  echo "wrote fixtures/en/$name.mp4 ($(du -h "$name.mp4" | cut -f1)) with $encoder"
}

make_clip hello "Welcome to the caption test. The quick brown fox jumps over the lazy dog. \
Every video deserves clear and accurate captions."

make_clip brands "Welcome back to KreoKit. Today Zylvana and Morvelle show how Quillhaven \
makes captions."

# The long interview: each line of long-script.txt spoken by its voice (H: Samantha,
# G: Daniel; 170 words a minute), a short gap after every turn (0.4–1.0 s, varying), and "PAUSE n" silences.
# 1 fps picture and 32 kbit/s audio keep the file small enough for git.
make_long() {
  local name="long"
  if [[ -f "$name.mp4" && "$FORCE" != "--force" ]]; then
    echo "kept fixtures/en/$name.mp4"
    return
  fi
  local tmp i=0 line voice text gap
  tmp="$(mktemp -d)"
  : > "$tmp/list.txt"
  : > "$name.txt"
  silence() { ffmpeg -nostdin -y -v error -f lavfi -i anullsrc=r=22050:cl=mono -t "$1" "$2"; }
  while IFS= read -r line; do
    case "$line" in
      "" | "#"*) continue ;;
      "PAUSE "*) silence "${line#PAUSE }" "$tmp/$i.wav" ;;
      "H: "* | "G: "*)
        [[ "$line" == H:* ]] && voice="$VOICE" || voice="Daniel"
        text="${line:3}"
        echo "$text" >> "$name.txt"
        say -v "$voice" -r 170 -o "$tmp/$i.aiff" "$text"
        ffmpeg -nostdin -y -v error -i "$tmp/$i.aiff" -ar 22050 -ac 1 "$tmp/$i-speech.wav"
        gap="0.$((4 + (i % 4) * 2))"
        silence "$gap" "$tmp/$i-gap.wav"
        printf "file '%s'\n" "$i-speech.wav" "$i-gap.wav" >> "$tmp/list.txt"
        i=$((i + 1))
        continue
        ;;
      *) echo "unknown line in long-script.txt: $line" >&2; exit 1 ;;
    esac
    echo "file '$i.wav'" >> "$tmp/list.txt"
    i=$((i + 1))
  done < long-script.txt

  ffmpeg -nostdin -y -v error -f concat -safe 0 -i "$tmp/list.txt" -c copy "$tmp/speech.wav"
  # -shortest overshoots by seconds at 1 fps, so cut both streams at the audio's length
  # (1 s lead-in + speech + 1 s tail).
  local seconds
  seconds=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$tmp/speech.wav")
  seconds=$(echo "$seconds + 2" | bc)
  ffmpeg -nostdin -y -v error \
    -f lavfi -i "color=c=0x1e1e28:s=640x360:r=1" \
    -i "$tmp/speech.wav" \
    -filter_complex "[1:a]adelay=1000:all=1,apad=pad_dur=1[a]" \
    -map 0:v -map "[a]" -t "$seconds" \
    -c:v "$encoder" -b:v 20k -pix_fmt yuv420p \
    -c:a aac -b:a 32k -ac 1 -ar 22050 \
    -movflags +faststart \
    "$name.mp4"
  rm -rf "$tmp"
  echo "wrote fixtures/en/$name.mp4 ($(du -h "$name.mp4" | cut -f1)) with $encoder"
}

make_long
