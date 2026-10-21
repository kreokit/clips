/**
 * Export all (milestone 5): which clips, their numbers and file names, and what each
 * clip sends to the backend (src-tauri/src/batch.rs).
 */
import { invoke } from "@tauri-apps/api/core";
import { OUTPUT } from "./captions/position";
import { fitStyle } from "./captions/textMetrics";
import type { Style } from "./captions/types";
import { Clip, Keyframe, clipCaptions, clipLength, tiktokWarning } from "./clips";
import { faceQueue, reframePath } from "./faces";
import type { Project } from "./projects";
import { Axis, CropPath, axisOf, framingKeyframes } from "./reframe";
import { normalized } from "./transcription";

/** One clip in the export list. */
export interface ExportRow {
  clip: Clip;
  /** Its number in the file name: by time among the kept clips. */
  number: number;
  lengthMs: number;
  /** Under TikTok Rewards, why it won't earn (shown, and unticked at first). */
  note: string | null;
}

/**
 * The clips Export all offers: the kept ones (plus `extra`, e.g. the editor's clip when
 * it isn't kept), in time order and numbered so. Under TikTok Rewards, clips under 61 s
 * carry the note (decided 2026-10-21: listed, unticked by default).
 */
export function exportRows(project: Project, extra: number[] = []): ExportRow[] {
  return project.clips
    .filter((c) => c.status === "kept" || extra.includes(c.id))
    .sort((a, b) => a.in_ms - b.in_ms || a.id - b.id)
    .map((clip, i) => ({
      clip,
      number: i + 1,
      lengthMs: clipLength(clip),
      note: tiktokWarning(clipLength(clip), project.finder),
    }));
}

/** Ticked at first: everything without a note. */
export const initiallyTicked = (rows: ExportRow[]) => new Set(rows.filter((r) => r.note === null).map((r) => r.clip.id));

/** Bytes a clip takes, roughly (batch.rs `estimate_bytes`): 10 Mbit/s video and 192 kbit/s
 * sound, plus 1% for the container. */
export function estimateBytes(lengthMs: number): number {
  return Math.floor((Math.floor(Math.floor(((10_000_000 + 192_000) * lengthMs) / 1000) / 8) * 101) / 100);
}

/** "740 MB", "1.3 GB". */
export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1e6))} MB`;
}

/** src-tauri batch.rs `Plan`. */
export interface ExportPlan {
  files: string[];
  conflicts: string[];
  bytes: number;
  free: number | null;
  enough_space: boolean;
}

/** src-tauri batch.rs `PictureSpec`. */
export type PictureSpec = { mode: "crop"; axis: Axis; keyframes: Keyframe[] } | { mode: "fit" };

/** src-tauri batch.rs `Item`. */
export interface ExportItem {
  clip_id: number;
  number: number;
  in_ms: number;
  out_ms: number;
  picture: PictureSpec;
  style: Style;
}

export type ItemResult =
  | { status: "saved"; clip_id: number; path: string; srt: string | null }
  | { status: "failed"; clip_id: number; error: string }
  | { status: "skipped"; clip_id: number };

export interface ExportProgress {
  index: number;
  total: number;
  percent: number;
}

export const exportPlan = (id: string, folder: string, items: [number, number][], srt: boolean) =>
  invoke<ExportPlan>("export_plan", { id, folder, items, srt });
export const cancelExport = () => invoke<void>("cancel_export");
export const revealExport = (path: string) => invoke<void>("reveal_export", { path });

/** The clip's picture as the export draws it: the same keyframes as the preview. */
export function pictureFor(clip: Clip, auto: CropPath | null, source: { width: number; height: number }): PictureSpec {
  const keyframes = framingKeyframes(clip.framing, auto, clip.in_ms);
  if (!keyframes) return { mode: "fit" };
  return { mode: "crop", axis: auto?.axis ?? axisOf(source.width, source.height), keyframes };
}

/**
 * What the backend needs for one clip: its picture (Follow face clips get their faces
 * analysed first if they aren't yet) and the project's style as this clip shows it,
 * measured with the caption font loaded (like the editor's preview).
 */
export async function prepareItem(project: Project, row: ExportRow): Promise<ExportItem> {
  const { clip } = row;
  let auto: CropPath | null = null;
  if (clip.framing.mode === "track") {
    auto = await reframePath(project.id, clip.in_ms, clip.out_ms);
    if (!auto) {
      await faceQueue.request(project.id, [[clip.in_ms, clip.out_ms]], "now");
      auto = await reframePath(project.id, clip.in_ms, clip.out_ms);
    }
    if (!auto) throw new Error("its faces couldn't be read");
  }
  const style = project.style;
  await document.fonts.load(`${style.bold ? 700 : 400} 100px "${style.font_family}"`);
  const segments = await clipCaptions(project.id, normalized(project.transcription), project.word_fixes, clip.in_ms, clip.out_ms);
  return {
    clip_id: clip.id,
    number: row.number,
    in_ms: clip.in_ms,
    out_ms: clip.out_ms,
    picture: pictureFor(clip, auto, project.video),
    style: fitStyle(segments, style, OUTPUT),
  };
}

/** "3 exported, 1 failed". */
export function exportSummary(results: ItemResult[]): string {
  const count = (status: ItemResult["status"]) => results.filter((r) => r.status === status).length;
  const parts = [`${count("saved")} exported`];
  if (count("failed")) parts.push(`${count("failed")} failed`);
  if (count("skipped")) parts.push(`${count("skipped")} not exported (stopped)`);
  return parts.join(", ");
}

const FOLDER_KEY = "kreokit.exportFolder";

/** The folder chosen last, remembered on this computer. */
export function lastFolder(): string | null {
  try {
    return localStorage.getItem(FOLDER_KEY);
  } catch {
    return null;
  }
}

export function rememberFolder(folder: string): void {
  try {
    localStorage.setItem(FOLDER_KEY, folder);
  } catch {
    // Storage unavailable: the video's folder is offered next time.
  }
}
