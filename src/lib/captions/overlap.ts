/**
 * Captions over a face (decided 2026-10-21): the editor warns when the caption box covers
 * a face in the 9:16 frame, e.g. after a manual crop or a caption dragged up. Everything
 * here is in shares (0–1) of the exported 9:16 frame unless it says otherwise.
 */
import type { Axis } from "../reframe";
import { REFERENCE_HEIGHT, anchorPoint, playResX } from "./position";
import type { Style } from "./types";

export interface Rect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** The ASS script of the 1080 × 1920 export is 608 × 1080. */
const SCRIPT_W = playResX(1080, 1920);

/**
 * The box of a caption whose lines are `lineWidths` wide (script pixels) in the fitted
 * `style`, outline included, placed like the export (position.ts `placement`).
 */
export function captionRect(lineWidths: number[], style: Style): Rect | null {
  if (lineWidths.length === 0) return null;
  const outline = style.outline_width;
  const w = Math.max(...lineWidths) + 2 * outline;
  const h = lineWidths.length * style.font_size + 2 * outline;
  const { x, y } = anchorPoint(style);
  const cx = x * SCRIPT_W;
  const ay = y * REFERENCE_HEIGHT;
  const top = style.position === "top" ? ay : style.position === "middle" ? ay - h / 2 : ay - h;
  return { x0: (cx - w / 2) / SCRIPT_W, x1: (cx + w / 2) / SCRIPT_W, y0: top / REFERENCE_HEIGHT, y1: (top + h) / REFERENCE_HEIGHT };
}

/** How the source fills the 9:16 frame at a moment. */
export type FrameView =
  | { kind: "crop"; axis: Axis; span: number; pos: number }
  | { kind: "fit"; source: { width: number; height: number } };

/** A face box in shares of the source frame, mapped into the 9:16 frame (the crop is
 * kept inside the source, as the export's crop filter does). */
export function faceInFrame([x0, y0, x1, y1]: [number, number, number, number], view: FrameView): Rect {
  if (view.kind === "crop") {
    const start = Math.min(Math.max(view.pos - view.span / 2, 0), 1 - view.span);
    const f = (v: number) => (v - start) / view.span;
    return view.axis === "x" ? { x0: f(x0), x1: f(x1), y0, y1 } : { x0, x1, y0: f(y0), y1: f(y1) };
  }
  const { width, height } = view.source;
  const scale = Math.min(1080 / width, 1920 / height);
  const [w, h] = [(width * scale) / 1080, (height * scale) / 1920];
  const [left, top] = [(1 - w) / 2, (1 - h) / 2];
  return { x0: left + x0 * w, x1: left + x1 * w, y0: top + y0 * h, y1: top + y1 * h };
}

const area = (r: Rect) => Math.max(0, r.x1 - r.x0) * Math.max(0, r.y1 - r.y0);
const intersect = (a: Rect, b: Rect): Rect => ({
  x0: Math.max(a.x0, b.x0),
  y0: Math.max(a.y0, b.y0),
  x1: Math.min(a.x1, b.x1),
  y1: Math.min(a.y1, b.y1),
});
const FRAME: Rect = { x0: 0, y0: 0, x1: 1, y1: 1 };

/** A face counts as covered when the caption hides this much of its visible part. */
export const COVERED_SHARE = 0.2;
/** Faces mostly outside the crop don't count. */
const VISIBLE_SHARE = 0.5;

/** Whether `caption` covers one of `faces` (in the 9:16 frame). */
export function coversAFace(caption: Rect, faces: Rect[]): boolean {
  return faces.some((face) => {
    const visible = intersect(face, FRAME);
    if (area(visible) < VISIBLE_SHARE * area(face) || area(visible) === 0) return false;
    return area(intersect(caption, visible)) >= COVERED_SHARE * area(visible);
  });
}

/** Face-analysis samples: time (video clock) and face boxes in shares of the source. */
export type FaceSamples = [number, [number, number, number, number][]][];

/**
 * The sample times (video clock) where the caption on screen covers a face. `captionAt`
 * gives the caption box at a time (null: no caption then), `viewAt` the framing.
 */
export function coveredTimes(
  samples: FaceSamples,
  captionAt: (ms: number) => Rect | null,
  viewAt: (ms: number) => FrameView,
): number[] {
  const out: number[] = [];
  for (const [ms, boxes] of samples) {
    if (boxes.length === 0) continue;
    const caption = captionAt(ms);
    if (!caption) continue;
    const view = viewAt(ms);
    if (coversAFace(caption, boxes.map((b) => faceInFrame(b, view)))) out.push(ms);
  }
  return out;
}

/** Warn once captions cover a face for at least this long (samples are 250 ms apart). */
export const WARN_MS = 500;

/** The time to show for the warning (the first covered sample), or null: no warning. */
export function overlapWarning(times: number[], stepMs = 250): number | null {
  return times.length * stepMs >= WARN_MS ? times[0] : null;
}
