/**
 * Word fixes (decided 2026-10-21): a corrected word applies wherever it shows, in every
 * clip. A fix names the word by its start in the transcript and its original text
 * (src-tauri/src/captions.rs `WordFix`, which applies them for preview and export).
 */
import type { WordFix } from "./types";

/** A transcript word, as the clip editor lists it. */
export interface FixableWord {
  start_ms: number;
  text: string;
}

const matches = (f: WordFix, w: FixableWord) => f.start_ms === w.start_ms && f.was === w.text;

/** The word as the captions show it: its fix, or as Whisper wrote it. */
export function fixedText(fixes: WordFix[], word: FixableWord): string {
  return fixes.find((f) => matches(f, word))?.text.trim() ?? word.text;
}

/** Whether the word has a fix. */
export function isFixed(fixes: WordFix[], word: FixableWord): boolean {
  return fixes.some((f) => matches(f, word));
}

/**
 * The fixes with `word` shown as `text` (trimmed; empty hides the word). Back to the
 * original text removes the fix. Fixes stay sorted by time.
 */
export function setFix(fixes: WordFix[], word: FixableWord, text: string): WordFix[] {
  const rest = fixes.filter((f) => !matches(f, word));
  const trimmed = text.trim().replace(/\s+/g, " ");
  if (trimmed === word.text) return rest;
  return [...rest, { start_ms: word.start_ms, was: word.text, text: trimmed }].sort((a, b) => a.start_ms - b.start_ms);
}
