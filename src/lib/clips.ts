import { invoke } from "@tauri-apps/api/core";
import type { TranscriptionSettings } from "./projects";

/** src-tauri/src/moments.rs `FinderSettings`: what the moment finder looks for. */
export interface FinderSettings {
  min_ms: number;
  max_ms: number;
  target_ms: number;
  /** How many clips to propose; null: about one per 2 minutes of video (5–30). */
  count: number | null;
}

export const DEFAULT_FINDER: FinderSettings = { min_ms: 20_000, max_ms: 60_000, target_ms: 35_000, count: null };

/** src-tauri/src/moments.rs `FeatureKind`. */
export type FeatureKind = "hook" | "start" | "ending" | "energy" | "length";

export const FEATURE_NAMES: Record<FeatureKind, string> = {
  hook: "Hook",
  start: "Start",
  ending: "Ending",
  energy: "Energy",
  length: "Length",
};

export interface Reason {
  text: string;
  /** Raises the score (else lowers it, or just informs). */
  good: boolean;
}

export interface Feature {
  kind: FeatureKind;
  /** Out of 100 over all features; 0 when not judged. */
  weight: number;
  /** 0–1. */
  value: number;
  reasons: Reason[];
}

/** src-tauri/src/moments.rs `Moment`: a scored stretch of the video. */
export interface Moment {
  in_ms: number;
  out_ms: number;
  /** 0–100. */
  score: number;
  features: Feature[];
  text: string;
}

export type ClipStatus = "proposed" | "kept" | "discarded";
export type ClipSource = "found" | "manual";

/** src-tauri/src/projects.rs `Clip`: a clip on the board. */
export interface Clip {
  id: number;
  in_ms: number;
  out_ms: number;
  status: ClipStatus;
  source: ClipSource;
  /** In or out point moved by the user; "Find again" leaves it alone. */
  edited: boolean;
  /** Score, reasons and words for the current in/out; null without words inside. */
  moment: Moment | null;
}

/** src-tauri/src/sentences.rs `Boundary`. */
export type Boundary = "edge" | "punctuation" | "pause" | "weak_pause";

/** src-tauri/src/sentences.rs `Text`: the transcript as spoken words and sentences. */
export interface SpokenText {
  words: { start_ms: number; end_ms: number; text: string }[];
  sentences: {
    first: number;
    last: number;
    start_ms: number;
    end_ms: number;
    starts: Boundary;
    ends: Boundary;
    question: boolean;
  }[];
  /** Sound tags ("[Laughter]"). */
  sounds: { start_ms: number; end_ms: number; text: string }[];
}

type Range = [number, number];

export const transcriptText = (id: string, settings: TranscriptionSettings) =>
  invoke<SpokenText>("transcript_text", { id, settings });

export const findMoments = (id: string, settings: TranscriptionSettings, finder: FinderSettings, avoid: Range[], have: number) =>
  invoke<Moment[]>("find_moments", { id, settings, finder, avoid, have });

export const scoreClips = (id: string, settings: TranscriptionSettings, finder: FinderSettings, ranges: Range[]) =>
  invoke<(Moment | null)[]>("score_clips", { id, settings, finder, ranges });

/** A proposal the user hasn't touched: "Find again" replaces it. */
export const untouched = (c: Clip) => c.source === "found" && c.status === "proposed" && !c.edited;

/**
 * What "Find again" sends: the clips that stay (all but untouched proposals) are avoided,
 * and those not discarded count towards the number of clips wanted.
 */
export function findRequest(clips: Clip[]): { avoid: Range[]; have: number } {
  const staying = clips.filter((c) => !untouched(c));
  return {
    avoid: staying.map((c) => [c.in_ms, c.out_ms]),
    have: staying.filter((c) => c.status !== "discarded").length,
  };
}

export const nextId = (clips: Clip[]) => clips.reduce((n, c) => Math.max(n, c.id), 0) + 1;

/** The board after "Find again": untouched proposals replaced by the new ones. */
export function mergeFound(clips: Clip[], found: Moment[]): Clip[] {
  const staying = clips.filter((c) => !untouched(c));
  let id = nextId(clips);
  return [
    ...staying,
    ...found.map(
      (moment): Clip => ({
        id: id++,
        in_ms: moment.in_ms,
        out_ms: moment.out_ms,
        status: "proposed",
        source: "found",
        edited: false,
        moment,
      }),
    ),
  ];
}

/** A manual clip, kept (the user chose it). */
export function manualClip(clips: Clip[], in_ms: number, out_ms: number, moment: Moment | null): Clip {
  return { id: nextId(clips), in_ms, out_ms, status: "kept", source: "manual", edited: false, moment };
}

export type ClipFilter = "all" | ClipStatus;
export type ClipSort = "score" | "time" | "length";

export const clipLength = (c: Clip) => c.out_ms - c.in_ms;
export const clipScore = (c: Clip) => c.moment?.score ?? 0;

/** The board's view: filtered, then sorted (best, earliest or longest first; ties by time). */
export function boardView(clips: Clip[], filter: ClipFilter, sort: ClipSort): Clip[] {
  const shown = filter === "all" ? clips : clips.filter((c) => c.status === filter);
  const key: Record<ClipSort, (c: Clip) => number> = {
    score: (c) => -clipScore(c),
    time: () => 0,
    length: (c) => -clipLength(c),
  };
  return [...shown].sort((a, b) => key[sort](a) - key[sort](b) || a.in_ms - b.in_ms || a.id - b.id);
}

export function countByStatus(clips: Clip[]): Record<ClipFilter, number> {
  const n = { all: clips.length, proposed: 0, kept: 0, discarded: 0 };
  for (const c of clips) n[c.status]++;
  return n;
}

/** The clip's first line for a card: its words up to ~90 characters, at a word end. */
export function firstLine(text: string, max = 90): string {
  if (text.length <= max) return text;
  const cut = text.lastIndexOf(" ", max);
  return `${text.slice(0, cut > 0 ? cut : max)}…`;
}

/** The two or three reasons a card shows: good ones first, in feature order. */
export function topReasons(moment: Moment | null, max = 2): Reason[] {
  if (!moment) return [];
  const all = moment.features.flatMap((f) => (f.weight > 0 ? f.reasons : []));
  const good = all.filter((r) => r.good && !r.text.startsWith("Starts at the beginning") && !r.text.startsWith("Ends cleanly"));
  const bad = all.filter((r) => !r.good && !r.text.startsWith("Hook not judged"));
  return [...good, ...bad].slice(0, max);
}

/** Path of a clip's thumbnail (the frame `ms` into the video), made on first request. */
export const clipThumbnail = (id: string, ms: number) => invoke<string>("clip_thumbnail", { id, ms: Math.round(ms) });

/** Clips with fresh scores; `moments` matches `clips` in order. */
export const withMoments = (clips: Clip[], moments: (Moment | null)[]): Clip[] =>
  clips.map((c, i) => ({ ...c, moment: moments[i] ?? null }));

/** "12:04–12:39". */
export function clipRange(c: Clip, format: (ms: number) => string): string {
  return `${format(c.in_ms)}–${format(c.out_ms)}`;
}

/**
 * `latest` with the scores worked out for `scored` (same order as `moments`), for each
 * clip whose in/out haven't changed since; the rest keep what they had.
 */
export function applyScores(latest: Clip[], scored: Clip[], moments: (Moment | null)[]): Clip[] {
  return latest.map((c) => {
    const i = scored.findIndex((s) => s.id === c.id && s.in_ms === c.in_ms && s.out_ms === c.out_ms);
    return i < 0 ? c : { ...c, moment: moments[i] ?? null };
  });
}

/** Keeps shortest ≤ target ≤ longest, the longest at least 5 s over the shortest. */
export function fixRange(f: FinderSettings): FinderSettings {
  const min_ms = Math.min(f.min_ms, 175_000);
  const max_ms = Math.max(f.max_ms, min_ms + 5_000);
  const target_ms = Math.min(Math.max(f.target_ms, min_ms), max_ms);
  return { ...f, min_ms, max_ms, target_ms };
}
