import { invoke } from "@tauri-apps/api/core";
import type { Style } from "./captions/types";
import type { Clip, FinderSettings } from "./clips";

/** src-tauri/src/probe.rs `VideoInfo`. */
export interface VideoInfo {
  duration_ms: number;
  /** Display size (rotation applied). */
  width: number;
  height: number;
  fps: number;
  rotation: number;
  has_audio: boolean;
}

/** src-tauri/src/models.rs `WhisperModel`. */
export type WhisperModel = "base" | "small" | "large-v3-turbo";

/** src-tauri/src/transcription.rs `Settings`: the Transcribe screen's choices. */
export interface TranscriptionSettings {
  model: WhisperModel;
  /** A Whisper language code, or null to detect it. */
  language: string | null;
  custom_words: string;
  hide_non_speech: boolean;
}

/** src-tauri/src/projects.rs `Project`: one auto-saved project per imported video. */
export interface Project {
  version: number;
  id: string;
  name: string;
  video_path: string;
  video_size: number;
  video_mtime_ms: number;
  video: VideoInfo;
  transcription: TranscriptionSettings;
  finder: FinderSettings;
  clips: Clip[];
  /** The caption look of every clip in the project. */
  style: Style;
  created_at: number;
  opened_at: number;
  updated_at: number;
}

export interface ProjectEntry extends Project {
  /** The video is still where it was; otherwise Home offers "Locate video…". */
  video_exists: boolean;
}

export const VIDEO_EXTENSIONS = ["mp4", "mov", "m4v", "mkv", "webm", "avi"];

export const listProjects = () => invoke<ProjectEntry[]>("list_projects");
export const openVideo = (path: string) => invoke<Project>("open_video", { path });
export const openProject = (id: string) => invoke<Project>("open_project", { id });
export const saveProject = (project: Project) => invoke<Project>("save_project", { project });
export const locateVideo = (id: string, path: string) => invoke<Project>("locate_video", { id, path });
/** Deletes the project (never its video); its transcript is kept for the same video. */
export const removeProject = (id: string) => invoke<void>("remove_project", { id });
/** Deletes the project's transcript ("Transcribe again"); its clips stay. */
export const discardTranscript = (id: string) => invoke<void>("discard_transcript", { id });
export const projectThumbnail = (id: string) => invoke<string>("project_thumbnail", { id });

export function isVideoPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return path.includes(".") && VIDEO_EXTENSIONS.includes(ext);
}

/** "0:09", "12:05", "1:02:03". */
export function formatDuration(ms: number): string {
  const total = Math.round(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** "29.97 fps", "25 fps". */
export function formatFps(fps: number): string {
  const rounded = Math.round(fps * 100) / 100;
  return `${rounded} fps`;
}

/** "Landscape 16:9", "Vertical 9:16", "Square", or the plain ratio for others. */
export function describeShape(width: number, height: number): string {
  const ratio = width / height;
  const near = (r: number) => Math.abs(ratio - r) < 0.02;
  if (near(1)) return "Square";
  if (near(16 / 9)) return "Landscape 16:9";
  if (near(9 / 16)) return "Vertical 9:16";
  if (near(4 / 3)) return "Landscape 4:3";
  if (near(3 / 4)) return "Vertical 3:4";
  return ratio > 1 ? `Landscape ${ratio.toFixed(2)}:1` : `Vertical 1:${(1 / ratio).toFixed(2)}`;
}

/** "84 MB", "1.2 GB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1_000_000) return `${Math.max(1, Math.round(bytes / 1000))} KB`;
  if (bytes < 1_000_000_000) return `${Math.round(bytes / 1_000_000)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "just now", "5 min ago", "3 h ago", "yesterday", "Monday", "12 Oct", "12 Oct 2025". */
export function relativeTime(ms: number, now: number): string {
  const diff = now - ms;
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)} min ago`;
  const then = new Date(ms);
  const today = new Date(now);
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  if (ms >= startOfToday) return `${Math.floor(diff / HOUR)} h ago`;
  if (ms >= startOfToday - DAY) return "yesterday";
  if (ms >= startOfToday - 6 * DAY) return then.toLocaleDateString("en-GB", { weekday: "long" });
  const sameYear = then.getFullYear() === today.getFullYear();
  return then.toLocaleDateString("en-GB", { day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) });
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
