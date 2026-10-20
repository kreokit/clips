/**
 * The clip editor's maths: in/out points with the finder's padding, snapping to sentence
 * and word edges, dragging the clip's edges, and what the editor shows around a clip.
 * Pure functions (tested in clipEdit.test.ts).
 */
import type { Boundary, FinderSettings, SpokenText } from "./clips";
import type { View } from "./timeline";

type Word = SpokenText["words"][number];

/** Lead-in before the first word and tail after the last (as src-tauri/src/moments.rs). */
export const LEAD_MS = 150;
export const TAIL_MS = 300;
/** A clip is never shorter than this. */
export const MIN_CLIP_MS = 1_000;

/** In point before word `i`: a short lead-in, at most half the gap to the word before. */
export function inPointBefore(words: Word[], i: number): number {
  const start = words[i].start_ms;
  const limit = i === 0 ? 0 : start - Math.floor(Math.max(0, start - words[i - 1].end_ms) / 2);
  return Math.max(start - LEAD_MS, limit, 0);
}

/** Out point after word `i`: a short tail, at most half the gap to the next word. */
export function outPointAfter(words: Word[], i: number, durationMs: number): number {
  const end = words[i].end_ms;
  const next = words[i + 1];
  const limit = next ? end + Math.floor(Math.max(0, next.start_ms - end) / 2) : Math.max(durationMs, end);
  return Math.min(end + TAIL_MS, limit);
}

/** A time an edge can snap to; sentence edges pull from further away than word edges. */
export interface Target {
  ms: number;
  sentence: boolean;
}

/** Where in points (`ins`) and out points (`outs`) snap: before each word / after each. */
export function snapTargets(text: SpokenText, durationMs: number): { ins: Target[]; outs: Target[] } {
  const starts = new Set(text.sentences.map((s) => s.first));
  const ends = new Set(text.sentences.map((s) => s.last));
  const ins = text.words.map((_, i) => ({ ms: inPointBefore(text.words, i), sentence: starts.has(i) }));
  const outs = text.words.map((_, i) => ({ ms: outPointAfter(text.words, i, durationMs), sentence: ends.has(i) }));
  return {
    ins: [{ ms: 0, sentence: true }, ...ins],
    outs: [...outs, { ms: durationMs, sentence: true }],
  };
}

/** `ms` moved onto the nearest sentence edge within 2 × `snapMs`, else the nearest word edge
 * within `snapMs`, else unchanged. */
export function snapEdge(ms: number, targets: Target[], snapMs: number): number {
  if (snapMs <= 0) return ms;
  const nearest = (limit: number, sentence: boolean) => {
    let best: number | null = null;
    for (const t of targets) {
      if (sentence && !t.sentence) continue;
      const d = Math.abs(t.ms - ms);
      if (d <= limit && (best === null || d < Math.abs(best - ms))) best = t.ms;
    }
    return best;
  };
  return nearest(2 * snapMs, true) ?? nearest(snapMs, false) ?? ms;
}

export type EdgeKind = "in" | "out" | "move";

export interface Range {
  in_ms: number;
  out_ms: number;
}

/**
 * The clip after dragging its in point, out point or whole body by `deltaMs` from
 * `original`: edges snap (see [`snapEdge`]), stay inside the video and at least
 * MIN_CLIP_MS apart; a moved clip keeps its length and snaps by whichever edge is closer.
 */
export function dragClip(
  original: Range,
  kind: EdgeKind,
  deltaMs: number,
  { durationMs, ins, outs, snapMs }: { durationMs: number; ins: Target[]; outs: Target[]; snapMs: number },
): Range {
  const clamp = (v: number, lo: number, hi: number) => Math.round(Math.min(Math.max(v, lo), hi));
  if (kind === "in") {
    const ms = snapEdge(original.in_ms + deltaMs, ins, snapMs);
    return { in_ms: clamp(ms, 0, original.out_ms - MIN_CLIP_MS), out_ms: original.out_ms };
  }
  if (kind === "out") {
    const ms = snapEdge(original.out_ms + deltaMs, outs, snapMs);
    return { in_ms: original.in_ms, out_ms: clamp(ms, original.in_ms + MIN_CLIP_MS, durationMs) };
  }
  const length = original.out_ms - original.in_ms;
  let start = original.in_ms + deltaMs;
  const byIn = snapEdge(start, ins, snapMs) - start;
  const byOut = snapEdge(start + length, outs, snapMs) - (start + length);
  start += byIn !== 0 && (byOut === 0 || Math.abs(byIn) <= Math.abs(byOut)) ? byIn : byOut;
  const in_ms = clamp(start, 0, Math.max(0, durationMs - length));
  return { in_ms, out_ms: in_ms + length };
}

/** Which part of the clip block a pointer at `x` grabs: an edge (8 px) or the body. */
export function hitEdge(x: number, left: number, width: number, edgePx = 8): EdgeKind {
  const edge = Math.min(edgePx, width / 3);
  if (x - left <= edge) return "in";
  if (left + width - x <= edge) return "out";
  return "move";
}

/** The word being spoken at `ms` (the last one started by then), or -1. */
export function wordIndexAt(words: Word[], ms: number): number {
  let lo = 0;
  let hi = words.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (words[mid].start_ms <= ms) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/** Words whose middle falls inside the clip: [first, last], or null without words. */
export function clipWords(words: Word[], clip: Range): [number, number] | null {
  const inside = (w: Word) => {
    const mid = (w.start_ms + w.end_ms) / 2;
    return mid >= clip.in_ms && mid < clip.out_ms;
  };
  const first = words.findIndex(inside);
  if (first < 0) return null;
  let last = first;
  while (last + 1 < words.length && inside(words[last + 1])) last++;
  return [first, last];
}

/** The words the editor's transcript shows: the clip's sentences plus one before and after. */
export function contextWords(text: SpokenText, clip: Range): [number, number] | null {
  const inside = clipWords(text.words, clip);
  if (!inside) {
    const i = Math.max(0, wordIndexAt(text.words, clip.in_ms));
    return text.words.length ? [i, i] : null;
  }
  const sentenceOf = (w: number) => text.sentences.findIndex((s) => s.first <= w && w <= s.last);
  const a = sentenceOf(inside[0]);
  const b = sentenceOf(inside[1]);
  const before = text.sentences[Math.max(0, a - 1)];
  const after = text.sentences[Math.min(text.sentences.length - 1, b + 1)];
  return [before ? before.first : inside[0], after ? after.last : inside[1]];
}

/** A warning when the clip is outside the finder's range, else null. */
export function lengthNote(lengthMs: number, finder: FinderSettings): string | null {
  const s = (ms: number) => `${Math.round(ms / 1000)} s`;
  if (lengthMs < finder.min_ms) return `Shorter than ${s(finder.min_ms)}`;
  if (lengthMs > finder.max_ms) return `Longer than ${s(finder.max_ms)}`;
  return null;
}

/** The timeline's first view of a clip: the clip with room on both sides (≥ 10 s). */
export function clipView(clip: Range, width: number, durationMs: number): View {
  const length = clip.out_ms - clip.in_ms;
  const pad = Math.max(10_000, length * 0.3);
  const from = Math.max(0, clip.in_ms - pad);
  const to = Math.min(Math.max(durationMs, clip.out_ms), clip.out_ms + pad);
  return { startMs: from, msPerPx: Math.max(2, (to - from) / Math.max(width, 1)) };
}

/** Marks between sentences for the timeline: where, and how sure (punctuation or a pause). */
export function sentenceMarks(text: SpokenText): { ms: number; kind: Boundary }[] {
  return text.sentences.slice(0, -1).map((s) => {
    const next = text.words[s.last + 1];
    const end = text.words[s.last].end_ms;
    return { ms: next ? (end + next.start_ms) / 2 : end, kind: s.ends };
  });
}
