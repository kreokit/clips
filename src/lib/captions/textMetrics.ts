/**
 * Font metrics in the browser, matched to how libass sizes text, and the fitted caption
 * style that preview and exports share.
 */
import { useEffect, useMemo, useState } from "react";
import { Extents, captionExtents, fittedStyle, playResX } from "./position";
import type { Segment, Style } from "./types";

/**
 * libass sizes text so that the font's ascender + descender equals the style's font
 * size (measured on a burned frame: Arial Bold at 200 gives a 128 px "H"), while CSS
 * sizes the em. The em per font-size unit, measured once per font once it has loaded.
 */
const sizeRatios = new Map<string, number>();
export function emPerFontSize(family: string, bold: boolean): number {
  const font = `${bold ? 700 : 400} 100px "${family}"`;
  const key = `${family}|${bold}`;
  let ratio = sizeRatios.get(key);
  if (ratio === undefined) {
    // Bundled fonts load on first use; measuring before that would measure (and cache)
    // the fallback font. Until then use a typical ratio; useFontsLoaded re-renders.
    if (!document.fonts.check(font)) {
      void document.fonts.load(font);
      return 1 / 1.15;
    }
    const m = context(font).measureText("H");
    const height = m.fontBoundingBoxAscent + m.fontBoundingBoxDescent;
    ratio = height > 0 ? 100 / height : 1 / 1.15;
    sizeRatios.set(key, ratio);
  }
  return ratio;
}

let canvas: CanvasRenderingContext2D | null = null;
function context(font: string): CanvasRenderingContext2D {
  canvas ??= document.createElement("canvas").getContext("2d")!;
  canvas.font = font;
  return canvas;
}

/** Width of a line of caption text in script pixels (1080-high frame) for `style`. */
export function lineMeasurer(style: Style): (text: string) => number {
  const em = style.font_size * emPerFontSize(style.font_family, style.bold);
  const ctx = context(`${style.bold ? 700 : 400} 100px "${style.font_family}"`);
  // Inter's UI features must not apply to caption text (see App.css .overlay).
  ctx.fontKerning = "normal";
  return (text) => (ctx.measureText(text).width * em) / 100;
}

/** Re-renders when web fonts finish loading (so caption metrics get re-measured). */
export function useFontsLoaded(): number {
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    const bump = () => setGeneration((g) => g + 1);
    document.fonts.addEventListener("loadingdone", bump);
    return () => document.fonts.removeEventListener("loadingdone", bump);
  }, []);
  return generation;
}

/** The largest caption box of the project for `style`, measured with its real font. */
export function measureExtents(segments: Segment[], style: Style): Extents {
  return captionExtents(segments, style, lineMeasurer(style));
}

/**
 * The style with its position fitted inside a `video` frame, as preview and exports use
 * it. Without a known video size the style is returned as is.
 */
export function fitStyle(
  segments: Segment[],
  style: Style,
  video: { width: number; height: number } | null,
): Style {
  if (!video) return style;
  return fittedStyle(style, measureExtents(segments, style), playResX(video.width, video.height));
}

/** fitStyle for React, re-measured when the captions, style, video or fonts change. */
export function useFittedStyle(
  segments: Segment[],
  style: Style | undefined,
  video: { width: number; height: number } | null,
): Style | undefined {
  const fonts = useFontsLoaded();
  return useMemo(
    () => (style ? fitStyle(segments, style, video) : undefined),
    // fonts: re-measure once caption fonts have loaded.
    [segments, style, video, fonts],
  );
}
