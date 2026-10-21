/**
 * Caption placement: where a caption's anchor sits in the frame. Mirrors `placement` in
 * src-tauri/src/ass.rs, and both are tested against fixtures/position-cases.json, so the
 * preview and the exports put captions in exactly the same place.
 */
import { layout } from "./layout";
import { transformed } from "./overlay";
import type { Position, Segment, Style } from "./types";

/** Style sizes and ASS scripts use a frame this tall (ass.rs REFERENCE_HEIGHT). */
export const REFERENCE_HEIGHT = 1080;
/**
 * The Top / Bottom preset spots, just clear of the parts TikTok, Reels and Shorts cover
 * with their own interface (safeArea.ts: top 10%, bottom 22%). Mirrors style.rs.
 */
export const PRESET_Y: Record<Position, number> = {
  top: 0.12,
  middle: 0.5,
  bottom: 0.74,
};

/** ASS alignment of each anchor: horizontally centred, top / middle / bottom edge. */
export const ANCHOR_CODE: Record<Position, 8 | 5 | 2> = { top: 8, middle: 5, bottom: 2 };

const clamp01 = (v: number) => Math.min(Math.max(v, 0), 1);

/** The anchor as fractions of the frame (0–1); presets when no free position is set. */
export function anchorPoint(style: Style): { x: number; y: number } {
  return {
    x: clamp01(style.pos_x ?? 0.5),
    y: clamp01(style.pos_y ?? PRESET_Y[style.position]),
  };
}

/** The ASS script width for a video: 1080 high, as wide as the video's shape. */
export function playResX(width: number, height: number): number {
  return Math.max(1, Math.round((REFERENCE_HEIGHT * width) / Math.max(height, 1)));
}

/** The ASS alignment and the anchor in script coordinates, rounded like ass.rs. */
export function placement(style: Style, playRes: number): { an: 8 | 5 | 2; x: number; y: number } {
  const { x, y } = anchorPoint(style);
  return {
    an: ANCHOR_CODE[style.position],
    x: Math.round(x * playRes),
    y: Math.round(y * REFERENCE_HEIGHT),
  };
}

/** Which edge anchors a caption placed at height `y`: thirds of the frame. */
export function anchorForY(y: number): Position {
  if (y < 1 / 3) return "top";
  if (y > 2 / 3) return "bottom";
  return "middle";
}

/** The largest caption box the project will show, in script pixels (1080-high frame),
 * outline included. */
export interface Extents {
  width: number;
  height: number;
}

/**
 * Measures every caption page the exports will produce (same text transforms and line
 * layout; each word in one_word mode). `measure` gives a line's width in script pixels
 * for the style's font at its size.
 */
export function captionExtents(
  segments: Segment[],
  style: Style,
  measure: (text: string) => number,
): Extents {
  let width = 0;
  let lines = 0;
  for (const segment of segments) {
    const seg = transformed(segment, style);
    const { words, pages } = layout(seg, style.max_chars_per_line, style.max_lines);
    if (style.mode === "one_word") {
      for (const w of words) width = Math.max(width, measure(w.text));
      if (words.length) lines = Math.max(lines, 1);
      continue;
    }
    for (const page of pages) {
      lines = Math.max(lines, page.lines.length);
      for (const line of page.lines) {
        width = Math.max(width, measure(line.map((i) => words[i].text).join(" ")));
      }
    }
  }
  const outline = 2 * style.outline_width;
  return lines === 0 ? { width: 0, height: 0 } : { width: width + outline, height: lines * style.font_size + outline };
}

/**
 * The anchor moved just enough that a caption box of `extents` stays inside the frame
 * (horizontally centred on the anchor; vertically below, around or above it for top /
 * middle / bottom). A box larger than the frame is centred. Returns fractions (0–1).
 */
export function fitAnchor(style: Style, extents: Extents, playRes: number): { x: number; y: number } {
  const { x, y } = anchorPoint(style);
  const halfW = Math.min(extents.width, playRes) / 2;
  const h = Math.min(extents.height, REFERENCE_HEIGHT);
  const [minY, maxY] = {
    top: [0, REFERENCE_HEIGHT - h],
    middle: [h / 2, REFERENCE_HEIGHT - h / 2],
    bottom: [h, REFERENCE_HEIGHT],
  }[style.position];
  const px = Math.min(Math.max(x * playRes, halfW), playRes - halfW);
  const py = Math.min(Math.max(y * REFERENCE_HEIGHT, minY), maxY);
  return { x: px / playRes, y: py / REFERENCE_HEIGHT };
}

/** Text may use this much of the frame's width / height before it is shrunk to fit. */
export const FIT_LIMIT = 0.98;

/**
 * The style as preview and exports use it: if the largest caption is wider or taller than
 * the frame, the font size shrinks just enough to fit (same line breaks, outline kept);
 * then the position is moved inside the frame (fitAnchor).
 */
export function fittedStyle(style: Style, extents: Extents, playRes: number): Style {
  let fitted = style;
  let ext = extents;
  const outline = 2 * style.outline_width;
  const textW = extents.width - outline;
  const textH = extents.height - outline;
  if (textW > 0 && textH > 0) {
    const factor = Math.min(
      1,
      (playRes * FIT_LIMIT - outline) / textW,
      (REFERENCE_HEIGHT * FIT_LIMIT - outline) / textH,
    );
    if (factor < 1) {
      const font_size = Math.max(1, Math.floor(style.font_size * factor));
      const scale = font_size / style.font_size;
      fitted = { ...style, font_size };
      ext = { width: textW * scale + outline, height: textH * scale + outline };
    }
  }
  const { x, y } = fitAnchor(fitted, ext, playRes);
  return { ...fitted, pos_x: x, pos_y: y };
}

/** An anchor in fractions of the frame, with the edge it marks. */
export interface Anchor {
  x: number;
  y: number;
  position: Position;
}

/** Guide lines to draw while snapped, as fractions of the frame. */
export interface Guides {
  vertical?: number;
  horizontal?: number;
}

/** Vertical offset from the anchor to the box centre, for a box `h` tall. */
const centreOffset = (position: Position, h: number) =>
  position === "top" ? h / 2 : position === "bottom" ? -h / 2 : 0;

/**
 * Where a caption dragged by (dx, dy) ends up. Everything is in fractions of the frame;
 * `box` is the caption's size. The box centre moves with the pointer, snaps (within
 * `snap.x` / `snap.y`, null to turn snapping off) to the horizontal and vertical centre
 * and its edges to the Top / Bottom preset spots, and stays inside the frame. The anchor
 * edge follows the thirds of the frame (anchorForY).
 */
export function dragAnchor(
  start: Anchor,
  dx: number,
  dy: number,
  box: { w: number; h: number },
  snap: { x: number; y: number } | null,
): Anchor & { guides: Guides } {
  let cx = start.x + dx;
  let cy = start.y + centreOffset(start.position, box.h) + dy;
  const guides: Guides = {};

  if (snap) {
    if (Math.abs(cx - 0.5) <= snap.x) {
      cx = 0.5;
      guides.vertical = 0.5;
    }
    // Candidates: centre on the middle, top edge on the Top spot, bottom edge on Bottom.
    const candidates = [
      { centre: 0.5, line: 0.5 },
      { centre: PRESET_Y.top + box.h / 2, line: PRESET_Y.top },
      { centre: PRESET_Y.bottom - box.h / 2, line: PRESET_Y.bottom },
    ];
    let best: (typeof candidates)[number] | null = null;
    for (const c of candidates) {
      const d = Math.abs(cy - c.centre);
      if (d <= snap.y && (!best || d < Math.abs(cy - best.centre))) best = c;
    }
    if (best) {
      cy = best.centre;
      guides.horizontal = best.line;
    }
  }

  const halfW = Math.min(box.w, 1) / 2;
  const halfH = Math.min(box.h, 1) / 2;
  cx = Math.min(Math.max(cx, halfW), 1 - halfW);
  cy = Math.min(Math.max(cy, halfH), 1 - halfH);
  const position = anchorForY(cy);
  return { x: cx, y: cy - centreOffset(position, box.h), position, guides };
}
