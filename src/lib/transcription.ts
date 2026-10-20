import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TranscriptionSettings, WhisperModel, formatBytes, formatDuration } from "./projects";

/** src-tauri/src/models.rs `ModelSource`: where a ready model is. */
export type ModelSource = "ours" | "captions";

/** src-tauri/src/models.rs `ModelInfo`. */
export interface ModelInfo {
  id: WhisperModel;
  size_bytes: number;
  /** null: not on this computer yet, it downloads first. */
  source: ModelSource | null;
}

/** `model-progress` events from `prepare_model`. */
export interface ModelProgress {
  stage: "checking" | "downloading";
  done_bytes: number;
  total_bytes: number;
}

/** src-tauri/src/transcription.rs `Progress` (`transcription-progress` events). */
export interface Progress {
  stage: "reading-audio" | "loading-model" | "transcribing" | "finishing";
  position_ms: number;
  duration_ms: number;
  /** The part being worked on (1-based) and how many there are (0 while reading). */
  part: number;
  parts: number;
  eta_ms: number | null;
}

/** src-tauri/src/transcribe.rs `Word`, `Segment`, `Transcript`. */
export interface Word {
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface Segment {
  id: number;
  start_ms: number;
  end_ms: number;
  text: string;
  words: Word[];
}

export interface Transcript {
  language: string;
  segments: Segment[];
}

/** src-tauri/src/lib.rs `TranscriptStatus`. */
export interface TranscriptStatus {
  transcript: Transcript | null;
  /** Saved work (finished parts, then the checkpoint inside the next) reaches this far. */
  saved_until_ms: number;
  parts: number;
}

/** Error message the backend uses when a job is cancelled. */
export const CANCELLED = "cancelled";

export const listModels = () => invoke<ModelInfo[]>("list_models");
export const transcriptStatus = (id: string, settings: TranscriptionSettings) =>
  invoke<TranscriptStatus>("transcript_status", { id, settings });
export const cancelJob = () => invoke<void>("cancel");

/** Makes the model ready (checks Captions' copy or downloads it), reporting progress. */
export async function prepareModel(model: WhisperModel, onProgress: (p: ModelProgress) => void): Promise<void> {
  const unlisten = await listen<ModelProgress>("model-progress", (e) => onProgress(e.payload));
  try {
    await invoke("prepare_model", { model });
  } finally {
    unlisten();
  }
}

export async function transcribe(
  id: string,
  settings: TranscriptionSettings,
  onProgress: (p: Progress) => void,
): Promise<Transcript> {
  const unlisten = await listen<Progress>("transcription-progress", (e) => onProgress(e.payload));
  try {
    return await invoke<Transcript>("transcribe", { id, settings });
  } finally {
    unlisten();
  }
}

export const MODEL_NAMES: Record<WhisperModel, { label: string; speed: string }> = {
  base: { label: "Base", speed: "fast" },
  small: { label: "Small", speed: "balanced" },
  "large-v3-turbo": { label: "Large", speed: "most accurate" },
};

/** The hint under a model choice: speed, then whether it's ready. */
export function modelHint(m: ModelInfo): string {
  const where = m.source === "ours" ? "ready" : m.source === "captions" ? "from Captions" : formatBytes(m.size_bytes);
  return `${MODEL_NAMES[m.id].speed} · ${where}`;
}

/** "auto" means detect; the others are Whisper language codes. English first, then A–Z. */
export const LANGUAGES: { code: string; label: string }[] = [
  { code: "auto", label: "Detect automatically" },
  { code: "en", label: "English" },
  { code: "sq", label: "Albanian" },
  { code: "ar", label: "Arabic" },
  { code: "bs", label: "Bosnian" },
  { code: "bg", label: "Bulgarian" },
  { code: "zh", label: "Chinese" },
  { code: "hr", label: "Croatian" },
  { code: "nl", label: "Dutch" },
  { code: "fr", label: "French" },
  { code: "de", label: "German" },
  { code: "el", label: "Greek" },
  { code: "hi", label: "Hindi" },
  { code: "id", label: "Indonesian" },
  { code: "it", label: "Italian" },
  { code: "ja", label: "Japanese" },
  { code: "ko", label: "Korean" },
  { code: "mk", label: "Macedonian" },
  { code: "pl", label: "Polish" },
  { code: "pt", label: "Portuguese" },
  { code: "ro", label: "Romanian" },
  { code: "ru", label: "Russian" },
  { code: "sr", label: "Serbian" },
  { code: "es", label: "Spanish" },
  { code: "tr", label: "Turkish" },
  { code: "uk", label: "Ukrainian" },
  { code: "vi", label: "Vietnamese" },
];

export function languageName(code: string): string {
  return LANGUAGES.find((l) => l.code === code)?.label ?? code;
}

const MINUTE = 60_000;

/** "less than a minute left", "about 8 min left", "about 1 h 5 min left". */
export function formatEta(ms: number): string {
  if (ms < MINUTE) return "less than a minute left";
  const minutes = Math.round(ms / MINUTE);
  if (minutes < 60) return `about ${minutes} min left`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m ? `about ${h} h ${m} min left` : `about ${h} h left`;
}

/** What the progress area shows: a status line, a detail line and the bar's 0–100. */
export interface ProgressView {
  label: string;
  detail: string | null;
  percent: number | null;
}

export function describeProgress(p: Progress): ProgressView {
  const percent = p.duration_ms > 0 ? Math.min(100, Math.floor((p.position_ms / p.duration_ms) * 100)) : null;
  const where = `${formatDuration(p.position_ms)} of ${formatDuration(p.duration_ms)}`;
  switch (p.stage) {
    case "reading-audio":
      return { label: "Reading the audio…", detail: `${where} · once, to plan the parts`, percent };
    case "loading-model":
      return { label: "Loading the model…", detail: null, percent };
    case "transcribing": {
      const part = p.parts > 1 ? ` · part ${p.part} of ${p.parts}` : "";
      const eta = p.eta_ms === null ? "measuring speed…" : formatEta(p.eta_ms);
      return { label: `Transcribing${part}`, detail: `${where} · ${eta}`, percent };
    }
    case "finishing":
      return { label: "Finishing…", detail: null, percent: 100 };
  }
}

export function describeModelProgress(p: ModelProgress, model: WhisperModel): ProgressView {
  const percent = p.total_bytes > 0 ? Math.floor((p.done_bytes / p.total_bytes) * 100) : null;
  const name = MODEL_NAMES[model].label;
  if (p.stage === "checking") {
    return { label: `Checking the ${name} model from KreoKit Captions…`, detail: null, percent };
  }
  return {
    label: `Downloading the ${name} model`,
    detail: `${formatBytes(p.done_bytes)} of ${formatBytes(p.total_bytes)} · once, then it works offline`,
    percent,
  };
}

const hasSaved = (s: TranscriptStatus | null): s is TranscriptStatus => !!s && !s.transcript && s.saved_until_ms > 0;

/** The start button: a fresh start, or a resume from where a cancelled run's saved work ends. */
export function startLabel(status: TranscriptStatus | null): string {
  return hasSaved(status) ? `Resume from ${formatDuration(status.saved_until_ms)}` : "Transcribe";
}

/** What's saved, shown after a Cancel (and whenever there is saved work to resume). */
export function savedNote(status: TranscriptStatus | null, cancelled: boolean, duration_ms: number): string | null {
  if (hasSaved(status)) {
    return `Saved up to ${formatDuration(status.saved_until_ms)} of ${formatDuration(duration_ms)}. Resume continues from there.`;
  }
  return cancelled ? "Nothing saved yet: Transcribe starts from the beginning." : null;
}

/** Settings ↔ the language picker ("auto" stands for null). */
export const languageValue = (s: TranscriptionSettings) => s.language ?? "auto";
export const withLanguage = (s: TranscriptionSettings, code: string): TranscriptionSettings => ({
  ...s,
  language: code === "auto" ? null : code,
});

/** The settings as sent to the backend (and its cache): custom words trimmed, so stray
 * spaces don't throw away a finished transcript. */
export const normalized = (s: TranscriptionSettings): TranscriptionSettings => ({
  ...s,
  custom_words: s.custom_words.trim(),
});

export function wordCount(t: Transcript): number {
  return t.segments.reduce((n, s) => n + (s.words.length || s.text.split(/\s+/).filter(Boolean).length), 0);
}
