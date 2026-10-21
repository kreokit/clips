/**
 * Caption line layout: how a segment's words are broken into lines and on-screen pages.
 *
 * Mirrors src-tauri/src/layout.rs; both are tested against fixtures/layout-cases.json so
 * the preview and the exports always break lines identically. Keep the two in sync.
 */
import type { Segment, Word } from "./types";

/** One caption screen: up to max_lines lines of word indices into the layout's words. */
export interface Page {
  start_ms: number;
  end_ms: number;
  lines: number[][];
}

export interface Layout {
  /** The segment's words, or one word spanning the segment if it has no word timings. */
  words: Word[];
  pages: Page[];
}

/** Characters as the user sees them (code points), matching Rust's `chars().count()`. */
function charCount(text: string): number {
  return [...text].length;
}

/** Function words a line or caption shouldn't end on. */
const SHORT_WORDS = new Set([
  "a", "an", "the", "to", "of", "and", "or", "but", "in", "on", "at", "for", "with", "from",
  "by", "as", "i", "my", "your",
]);

/** The word without closing quotes or brackets after its punctuation. */
function core(word: string): string {
  return word.replace(/["'”’)\]»]+$/u, "");
}

const endsSentence = (word: string) => /[.!?…]$/u.test(core(word));
const endsClause = (word: string) => /[,;:]$/u.test(core(word));

/** Characters of the words joined by single spaces. */
function lineLen(texts: string[], line: number[]): number {
  return line.reduce((sum, i) => sum + charCount(texts[i]), 0) + Math.max(0, line.length - 1);
}

/**
 * How many of `line`'s last words to move to the next line (with word `next`) when
 * `next` doesn't fit. Same rules as layout.rs `choose_break`: each possible break, after
 * word w, scores +3 if w ends a sentence, +2 if it ends a clause, -3 if w starts a new
 * sentence, -2 if w is a short function word; twice the score minus the words moved wins,
 * ties keep more on the line. A break may not leave the line under half of maxChars, and
 * the moved words plus `next` must fit.
 */
function chooseBreak(texts: string[], line: number[], next: number, maxChars: number): number {
  let best = { total: -Infinity, moved: 0 };
  for (let moved = 0; moved < line.length; moved++) {
    const keep = line.slice(0, line.length - moved);
    if (moved > 0) {
      const movedLen = lineLen(texts, line.slice(keep.length)) + 1 + charCount(texts[next]);
      if (movedLen > maxChars || lineLen(texts, keep) * 2 < maxChars) break;
    }
    const last = texts[keep[keep.length - 1]];
    let score = 0;
    if (endsSentence(last)) score += 3;
    else if (endsClause(last)) score += 2;
    if (keep.length >= 2 && endsSentence(texts[keep[keep.length - 2]])) score -= 3;
    if (SHORT_WORDS.has(last.toLowerCase())) score -= 2;
    const total = score * 2 - moved;
    if (total > best.total) best = { total, moved };
  }
  return best.moved;
}

/**
 * Line breaking: words fill a line while it stays within maxChars characters (words plus
 * single spaces); a longer word gets a line of its own. When the next word doesn't fit,
 * the break may move back a few words to a better spot (see chooseBreak). Lines are
 * grouped into pages of maxLines, so the same rules decide where captions end. The first
 * page starts with the segment, later pages with their first word, and each page lasts
 * until the next one starts (the last until the segment ends).
 */
export function layout(segment: Segment, maxChars: number, maxLines: number): Layout {
  const text = segment.text.trim();
  const words: Word[] =
    segment.words.length > 0
      ? segment.words
      : text
        ? [{ start_ms: segment.start_ms, end_ms: segment.end_ms, text }]
        : [];
  const chars = Math.max(1, maxChars);
  const perPage = Math.max(1, maxLines);

  const texts = words.map((w) => w.text);
  const lines: number[][] = [];
  let current: number[] = [];
  texts.forEach((text, i) => {
    if (current.length === 0 || lineLen(texts, current) + 1 + charCount(text) <= chars) {
      current.push(i);
      return;
    }
    const moved = chooseBreak(texts, current, i, chars);
    const next = [...current.slice(current.length - moved), i];
    lines.push(current.slice(0, current.length - moved));
    current = next;
  });
  if (current.length > 0) lines.push(current);

  const chunks: number[][][] = [];
  for (let i = 0; i < lines.length; i += perPage) chunks.push(lines.slice(i, i + perPage));
  const starts = chunks.map((page, p) => (p === 0 ? segment.start_ms : words[page[0][0]].start_ms));
  const pages = chunks.map((lines, p) => ({
    start_ms: starts[p],
    end_ms: p + 1 < starts.length ? starts[p + 1] : segment.end_ms,
    lines,
  }));
  return { words, pages };
}
