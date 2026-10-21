/**
 * What the caption overlay shows at a moment. Mirrors the event rules of the ASS export
 * (src-tauri/src/ass.rs) so the preview matches the burned-in video: same layout pages,
 * same text transforms, same timing per mode. Change both together.
 */
import { layout } from "./layout";
import type { Segment, Style, Word } from "./types";

/** Style sizes are for a frame this tall (ass.rs REFERENCE_HEIGHT). */
export const REFERENCE_HEIGHT = 1080;
/** Distance of top/bottom captions from the frame edge (ass.rs MARGIN_V). */
export const MARGIN_V = 86;
/** Pop animation (ass.rs POP_SCALE / POP_MS). */
export const POP_SCALE = 0.8;
export const POP_MS = 120;

/** Index of the segment showing at `ms`, or -1 in a gap. Segments are sorted by start. */
export function findSegmentAt(segments: Segment[], ms: number): number {
  let lo = 0;
  let hi = segments.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (segments[mid].start_ms > ms) hi = mid - 1;
    else if (segments[mid].end_ms <= ms) lo = mid + 1;
    else return mid;
  }
  return -1;
}

export interface OverlayWord {
  text: string;
  /** Shown in the highlight colour. */
  active: boolean;
}

export interface OverlayFrame {
  /** Changes whenever a new caption appears (restarts the pop animation). */
  key: string;
  lines: OverlayWord[][];
}

/** Characters ASS would treat as commands, swapped like ass.rs `escape`. */
export function displayText(text: string): string {
  return text.replace(/[{}\\]/g, (c) => ({ "{": "｛", "}": "｝", "\\": "＼" })[c] ?? c);
}

/** The segment's words as rendered: uppercase if the style says so, ASS-safe characters. */
export function transformed(segment: Segment, style: Style): Segment {
  const t = (s: string) => displayText(style.uppercase ? s.toUpperCase() : s);
  return {
    ...segment,
    text: t(segment.text),
    words: segment.words.map((w: Word) => ({ ...w, text: t(w.text) })),
  };
}

/**
 * The caption on screen at `ms`, or null.
 * - line: the layout page playing
 * - word_highlight: the page, with the word spoken (start <= ms < end) active
 * - one_word: each word from its start until the next word starts (the last until the
 *   segment ends), active
 */
export function overlayFrame(segments: Segment[], ms: number, style: Style): OverlayFrame | null {
  const index = findSegmentAt(segments, ms);
  if (index < 0) return null;
  const segment = transformed(segments[index], style);
  const { words, pages } = layout(segment, style.max_chars_per_line, style.max_lines);

  if (style.mode === "one_word") {
    for (let i = words.length - 1; i >= 0; i--) {
      const end = i + 1 < words.length ? words[i + 1].start_ms : segment.end_ms;
      if (words[i].start_ms <= ms && ms < end) {
        return { key: `${index}:w${i}`, lines: [[{ text: words[i].text, active: true }]] };
      }
    }
    return null;
  }

  const p = pages.findIndex((page) => page.start_ms <= ms && ms < page.end_ms);
  if (p < 0) return null;
  const highlight = style.mode === "word_highlight";
  return {
    key: `${index}:p${p}`,
    lines: pages[p].lines.map((line) =>
      line.map((i) => ({
        text: words[i].text,
        active: highlight && words[i].start_ms <= ms && ms < words[i].end_ms,
      })),
    ),
  };
}

/**
 * An outline as layered text-shadows: copies of the text offset around circles of the
 * outline radius (and half of it, to fill thin strokes), ~1 px apart. Shadows are always
 * drawn behind the text, so the outline grows outward with round corners like libass's,
 * never into the letters. (-webkit-text-stroke draws over the fill, and WebKit on
 * macOS 12 ignores paint-order for HTML text.)
 */
export function outlineShadow(radius: number, color: string): string | undefined {
  if (radius <= 0) return undefined;
  const rings = radius > 2 ? [radius, radius / 2] : [radius];
  const shadows: string[] = [];
  for (const r of rings) {
    const steps = Math.min(48, Math.max(8, Math.ceil(2 * Math.PI * r)));
    for (let i = 0; i < steps; i++) {
      const angle = (2 * Math.PI * i) / steps;
      const x = (r * Math.cos(angle)).toFixed(2);
      const y = (r * Math.sin(angle)).toFixed(2);
      shadows.push(`${x}px ${y}px 0 ${color}`);
    }
  }
  return shadows.join(", ");
}

/**
 * The caption nearest in time to `ms` (its first moment), for showing a dimmed "ghost"
 * that can still be dragged when no caption is on screen. Null without captions.
 */
export function ghostFrame(segments: Segment[], ms: number, style: Style): OverlayFrame | null {
  let best: Segment | null = null;
  let bestDistance = Infinity;
  for (const s of segments) {
    const d = ms < s.start_ms ? s.start_ms - ms : ms >= s.end_ms ? ms - s.end_ms : 0;
    if (d < bestDistance) {
      best = s;
      bestDistance = d;
    }
  }
  if (!best) return null;
  const frame = overlayFrame(segments, best.start_ms, style);
  return frame && { ...frame, key: `ghost:${frame.key}` };
}
