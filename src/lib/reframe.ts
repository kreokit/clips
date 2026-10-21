import type { Clip, Framing, Keyframe } from "./clips";

/** src-tauri/src/reframe.rs `Axis`: which way the crop moves. */
export type Axis = "x" | "y";

/** src-tauri/src/reframe.rs `Path`: the automatic crop path of one clip. */
export interface CropPath {
  axis: Axis;
  /** The crop window's size as shares of the source: [width, height]. */
  crop: [number, number];
  keyframes: Keyframe[];
  /** No face anywhere in the clip: the centre crop. */
  no_face: boolean;
}

/** The output aspect, width / height. */
export const ASPECT = 9 / 16;

/** Smootherstep (reframe.rs `ease`): 0 → 1 with zero speed and acceleration at both ends. */
export function ease(k: number): number {
  const t = Math.min(Math.max(k, 0), 1);
  return t * t * t * (t * (6 * t - 15) + 10);
}

/**
 * The crop centre at `ms` (reframe.rs `position_at`; same cases in
 * fixtures/reframe/path-cases.json): linear between keyframes, eased into an `ease` one, a
 * jump at a `jump` one, constant outside them; 0.5 without keyframes.
 */
export function positionAt(keyframes: Keyframe[], ms: number): number {
  // The first keyframe after `ms` (keyframes at `ms` count as before).
  let after = 0;
  while (after < keyframes.length && keyframes[after].t_ms <= ms) after++;
  const prev = keyframes[after - 1];
  const next = keyframes[after];
  if (!prev && !next) return 0.5;
  if (!prev) return next.pos;
  if (!next) return prev.pos;
  if (next.jump || next.t_ms === prev.t_ms) return prev.pos;
  const k = (ms - prev.t_ms) / (next.t_ms - prev.t_ms);
  return prev.pos + (next.pos - prev.pos) * (next.ease ? ease(k) : k);
}

/** reframe.rs `crop_size`: the largest 9:16 window, as shares of a `width`×`height` source. */
export function cropSize(width: number, height: number): [number, number] {
  const w = Math.max(width, 1);
  const h = Math.max(height, 1);
  return w / h >= ASPECT ? [(ASPECT * h) / w, 1] : [1, w / h / ASPECT];
}

export function axisOf(width: number, height: number): Axis {
  return cropSize(width, height)[1] >= 1 ? "x" : "y";
}

/** A centre-crop path for a source (also the stand-in while faces are being found). */
export function centerPath(width: number, height: number, fromMs: number): CropPath {
  return {
    axis: axisOf(width, height),
    crop: cropSize(width, height),
    keyframes: [{ t_ms: fromMs, pos: 0.5, jump: false, ease: false }],
    no_face: false,
  };
}

/**
 * The keyframes a clip is shown with: the automatic path (Track; the centre until it is
 * known), the centre, or the user's keyframes. Fit + blur has no crop (`null`).
 */
export function framingKeyframes(framing: Framing, auto: CropPath | null, inMs: number): Keyframe[] | null {
  switch (framing.mode) {
    case "track":
      return auto ? auto.keyframes : [{ t_ms: inMs, pos: 0.5, jump: false, ease: false }];
    case "center":
      return [{ t_ms: inMs, pos: 0.5, jump: false, ease: false }];
    case "manual":
      return framing.manual.length ? framing.manual : [{ t_ms: inMs, pos: 0.5, jump: false, ease: false }];
    case "fit_blur":
      return null;
  }
}

/**
 * Manual keyframes to start from, so switching to Manual changes nothing yet: the
 * automatic path's keyframes inside the clip, plus where it is at the in point.
 */
export function seedManual(auto: CropPath | null, clip: Pick<Clip, "in_ms" | "out_ms">): Keyframe[] {
  if (!auto) return [{ t_ms: clip.in_ms, pos: 0.5, jump: false, ease: false }];
  const start: Keyframe = { t_ms: clip.in_ms, pos: positionAt(auto.keyframes, clip.in_ms), jump: false, ease: false };
  return [start, ...auto.keyframes.filter((k) => k.t_ms > clip.in_ms && k.t_ms < clip.out_ms)];
}

/** Keyframes closer than this to the playhead are moved rather than a new one added. */
export const KEYFRAME_REACH_MS = 150;

/**
 * Sets the crop at `ms` to `pos` (clamped so the crop stays inside the frame): moves a
 * keyframe within reach, else adds one, eased in from the one before (a camera move).
 */
export function setKeyframe(keys: Keyframe[], ms: number, pos: number, span: number): Keyframe[] {
  const half = span / 2;
  const p = Math.min(Math.max(pos, half), 1 - half);
  const near = keys.findIndex((k) => Math.abs(k.t_ms - ms) <= KEYFRAME_REACH_MS);
  if (near >= 0) return keys.map((k, i) => (i === near ? { ...k, pos: p } : k));
  const added: Keyframe = { t_ms: Math.round(ms), pos: p, jump: false, ease: true };
  return [...keys, added].sort((a, b) => a.t_ms - b.t_ms);
}

/** Removes the keyframe within reach of `ms`, if any (the last one always stays). */
export function removeKeyframe(keys: Keyframe[], ms: number): Keyframe[] {
  if (keys.length <= 1) return keys;
  const near = keys.findIndex((k) => Math.abs(k.t_ms - ms) <= KEYFRAME_REACH_MS);
  return near < 0 ? keys : keys.filter((_, i) => i !== near);
}

export function keyframeNear(keys: Keyframe[], ms: number): Keyframe | undefined {
  return keys.find((k) => Math.abs(k.t_ms - ms) <= KEYFRAME_REACH_MS);
}

export interface Box {
  width: number;
  height: number;
}

export interface Placement {
  width: number;
  height: number;
  left: number;
  top: number;
}

/**
 * Where to draw a `source`-sized video so that the crop centred at `pos` fills a 9:16
 * `box` exactly (the video is scaled to the box's height, or width for tall sources).
 */
export function cropPlacement(source: Box, box: Box, axis: Axis, pos: number): Placement {
  const aspect = source.width / Math.max(source.height, 1);
  if (axis === "x") {
    const height = box.height;
    const width = height * aspect;
    return { width, height, left: box.width / 2 - pos * width, top: 0 };
  }
  const width = box.width;
  const height = width / aspect;
  return { width, height, left: 0, top: box.height / 2 - pos * height };
}

/** Fit + blur: the whole video inside the box, centred. */
export function fitPlacement(source: Box, box: Box): Placement {
  const scale = Math.min(box.width / source.width, box.height / source.height);
  const width = source.width * scale;
  const height = source.height * scale;
  return { width, height, left: (box.width - width) / 2, top: (box.height - height) / 2 };
}

/** The largest 9:16 box inside `area`. */
export function portraitBox(area: Box): Box {
  const height = Math.min(area.height, area.width / ASPECT);
  return { width: Math.floor(height * ASPECT), height: Math.floor(height) };
}

/** Decided 2026-10-21: sources under 1080p (short side) look soft at 1080×1920. */
export function lowResolution(width: number, height: number): boolean {
  return Math.min(width, height) < 1080;
}

export const CROP_MODES: { value: Framing["mode"]; label: string }[] = [
  { value: "track", label: "Follow face" },
  { value: "center", label: "Center" },
  { value: "fit_blur", label: "Fit + blur" },
  { value: "manual", label: "Manual" },
];
