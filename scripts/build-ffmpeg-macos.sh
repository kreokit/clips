#!/usr/bin/env bash
# Builds an LGPL FFmpeg (ffmpeg + ffprobe) for macOS from pinned source tarballs.
#
#   scripts/build-ffmpeg-macos.sh <x86_64|arm64> <output-dir>
#
# Static freetype, fribidi, harfbuzz and libass are built first (meson), then FFmpeg with
# --disable-autodetect and only what KreoKit Clips needs: VideoToolbox / AudioToolbox
# encoders, the ass filter with its font stack, and the decoders/demuxers FFmpeg enables
# by default. No --enable-gpl, no --enable-nonfree, no libx264. Minimum macOS 12.0.
#
# Needs: Xcode command line tools, meson, ninja, pkgconf, nasm (x86_64 only).
# Versions and checksums are listed in src-tauri/binaries/README.md; keep them in sync.
set -euo pipefail

ARCH="${1:?arch: x86_64 or arm64}"
OUT="$(mkdir -p "${2:?output dir}" && cd "$2" && pwd)"
MIN_MACOS="12.0"

# name version url sha256
SOURCES=(
  "ffmpeg 9.0.2 https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz 8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
  "freetype 2.14.3 https://download.savannah.gnu.org/releases/freetype/freetype-2.14.3.tar.xz 36bc4f1cc413335368ee656c42afca65c5a3987e8768cc28cf11ba775e785a5f"
  "fribidi 1.0.17 https://github.com/fribidi/fribidi/releases/download/v1.0.17/fribidi-1.0.17.tar.xz 6949dcde27d41cebad1fd741fcafc36d55a1020d2d872d4a6eb3914caabbada2"
  "harfbuzz 14.6.0 https://github.com/harfbuzz/harfbuzz/releases/download/14.6.0/harfbuzz-14.6.0.tar.xz d07a007327277708a2a73ae437887cdbaf282937f6d03ca5467723e9099af586"
  "libass 0.17.5 https://github.com/libass/libass/releases/download/0.17.5/libass-0.17.5.tar.xz 2dca25c0e0c837ddf00b52011b3f82cac1e4ddd3ad018227806b0c2288864acc"
)

WORK="$OUT/work"
PREFIX="$OUT/deps"
rm -rf "$WORK" "$PREFIX"
mkdir -p "$WORK" "$PREFIX"

export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"
export CFLAGS="-arch $ARCH -mmacosx-version-min=$MIN_MACOS -O2"
export CXXFLAGS="$CFLAGS"
export LDFLAGS="-arch $ARCH -mmacosx-version-min=$MIN_MACOS"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"

fetch() {
  local name="$1" version="$2" url="$3" sha="$4"
  local file="$WORK/$(basename "$url")"
  echo "== $name $version"
  curl -fsSL -o "$file" "$url"
  echo "$sha  $file" | shasum -a 256 -c -
  tar -xf "$file" -C "$WORK"
}

meson_build() {
  local dir="$1"; shift
  (cd "$WORK/$dir" &&
    meson setup build --prefix="$PREFIX" --libdir=lib --buildtype=release \
      --default-library=static -Db_staticpic=true "$@" &&
    meson compile -C build &&
    meson install -C build)
}

for source in "${SOURCES[@]}"; do
  # shellcheck disable=SC2086
  fetch $source
done

meson_build freetype-2.14.3 \
  -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled -Dpng=disabled -Dzlib=disabled
meson_build fribidi-1.0.17 -Ddocs=false -Dbin=false -Dtests=false
meson_build harfbuzz-14.6.0 \
  -Dfreetype=enabled -Dglib=disabled -Dgobject=disabled -Dcairo=disabled -Dicu=disabled \
  -Dtests=disabled -Ddocs=disabled -Dutilities=disabled -Dintrospection=disabled \
  -Dchafa=disabled -Dpng=disabled -Dzlib=disabled -Dgraphite2=disabled
meson_build libass-0.17.5 \
  -Dfontconfig=disabled -Dcoretext=enabled -Drequire-system-font-provider=true -Dtest=disabled \
  -Dlibunibreak=disabled

ASM=()
[[ "$ARCH" == "arm64" ]] || ASM=(--x86asmexe=nasm)

(cd "$WORK/ffmpeg-9.0.2" &&
  ./configure --prefix="$OUT" --arch="$ARCH" --target-os=darwin \
    --cc="clang -arch $ARCH" --cxx="clang++ -arch $ARCH" \
    --extra-cflags="-mmacosx-version-min=$MIN_MACOS -I$PREFIX/include" \
    --extra-ldflags="-mmacosx-version-min=$MIN_MACOS -L$PREFIX/lib" \
    --pkg-config-flags=--static ${ASM[@]+"${ASM[@]}"} \
    --disable-debug --disable-doc --disable-ffplay --disable-network \
    --disable-autodetect \
    --enable-videotoolbox --enable-audiotoolbox \
    --enable-libass --enable-libfreetype --enable-libfribidi --enable-libharfbuzz \
    --enable-zlib --enable-iconv &&
  make -j"$(sysctl -n hw.ncpu)" &&
  make install)

# Licence summary plus the full LGPL 2.1 text, shipped with the app.
cat "$WORK/ffmpeg-9.0.2/LICENSE.md" "$WORK/ffmpeg-9.0.2/COPYING.LGPLv2.1" > "$OUT/LICENSE.md"

# The executables must only depend on system libraries and frameworks.
for tool in ffmpeg ffprobe; do
  if otool -L "$OUT/bin/$tool" | tail -n +2 | grep -vE "^\s+(/usr/lib/|/System/Library/)"; then
    echo "error: $tool links to non-system libraries" >&2
    exit 1
  fi
done
echo "built $OUT/bin/ffmpeg and ffprobe for $ARCH (macOS $MIN_MACOS+)"
