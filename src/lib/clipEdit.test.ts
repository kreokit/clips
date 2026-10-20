import { describe, expect, it } from "vitest";
import type { SpokenText } from "./clips";
import { DEFAULT_FINDER } from "./clips";
import {
  LEAD_MS,
  TAIL_MS,
  clipAround,
  clipFromWords,
  clipView,
  clipWords,
  contextWords,
  dragClip,
  hitEdge,
  inPointBefore,
  lengthNote,
  outPointAfter,
  sentenceMarks,
  snapEdge,
  snapTargets,
  wordIndexAt,
} from "./clipEdit";

/** Words of 400 ms with 100 ms gaps; sentences end after words 2, 5 and 8 (1 s pauses). */
function text(): SpokenText {
  const words: SpokenText["words"] = [];
  let t = 1_000;
  for (let i = 0; i < 9; i++) {
    words.push({ start_ms: t, end_ms: t + 400, text: `w${i}` });
    t += 400 + (i % 3 === 2 ? 1_000 : 100);
  }
  const sentence = (first: number, last: number) => ({
    first,
    last,
    start_ms: words[first].start_ms,
    end_ms: words[last].end_ms,
    starts: "punctuation" as const,
    ends: "punctuation" as const,
    question: false,
  });
  return { words, sentences: [sentence(0, 2), sentence(3, 5), sentence(6, 8)], sounds: [] };
}

describe("in and out points", () => {
  const { words } = text();
  it("pad like the moment finder, at most half the gap", () => {
    expect(inPointBefore(words, 0)).toBe(1_000 - LEAD_MS);
    expect(inPointBefore(words, 1)).toBe(words[1].start_ms - 50); // 100 ms gap
    expect(inPointBefore(words, 3)).toBe(words[3].start_ms - LEAD_MS); // 1 s pause
    expect(outPointAfter(words, 2, 20_000)).toBe(words[2].end_ms + TAIL_MS);
    expect(outPointAfter(words, 0, 20_000)).toBe(words[0].end_ms + 50);
    expect(outPointAfter(words, 8, 20_000)).toBe(words[8].end_ms + TAIL_MS);
    expect(outPointAfter(words, 8, words[8].end_ms + 100)).toBe(words[8].end_ms + 100);
  });
});

describe("snapping", () => {
  const t = text();
  const { ins, outs } = snapTargets(t, 20_000);

  it("prefers sentence edges, then word edges, within reach", () => {
    const sentenceIn = inPointBefore(t.words, 3);
    expect(snapEdge(sentenceIn + 150, ins, 100)).toBe(sentenceIn); // within 2 × 100
    const wordIn = inPointBefore(t.words, 4);
    expect(snapEdge(wordIn + 60, ins, 100)).toBe(wordIn);
    expect(snapEdge(wordIn + 60, ins, 0)).toBe(wordIn + 60); // snapping off
    expect(snapEdge(50_000, outs, 100)).toBe(50_000); // nothing near
  });

  it("drags edges within the video, at least a second long", () => {
    const opts = { durationMs: 20_000, ins, outs, snapMs: 0 };
    const clip = { in_ms: 2_000, out_ms: 6_000 };
    expect(dragClip(clip, "in", -5_000, opts)).toEqual({ in_ms: 0, out_ms: 6_000 });
    expect(dragClip(clip, "in", 10_000, opts)).toEqual({ in_ms: 5_000, out_ms: 6_000 });
    expect(dragClip(clip, "out", 50_000, opts)).toEqual({ in_ms: 2_000, out_ms: 20_000 });
    expect(dragClip(clip, "move", 30_000, opts)).toEqual({ in_ms: 16_000, out_ms: 20_000 });
    expect(dragClip(clip, "move", -3_000, opts)).toEqual({ in_ms: 0, out_ms: 4_000 });
  });

  it("snaps a dragged edge, and a moved clip by its closer edge", () => {
    const opts = { durationMs: 20_000, ins, outs, snapMs: 100 };
    const target = inPointBefore(t.words, 3);
    const clip = { in_ms: 1_000, out_ms: 9_000 };
    expect(dragClip(clip, "in", target - 1_000 + 120, opts).in_ms).toBe(target);
    const moved = dragClip(clip, "move", target - 1_000 + 30, opts);
    expect(moved.in_ms).toBe(target);
    expect(moved.out_ms - moved.in_ms).toBe(8_000);
  });

  it("tells edges from the body", () => {
    expect(hitEdge(102, 100, 200)).toBe("in");
    expect(hitEdge(295, 100, 200)).toBe("out");
    expect(hitEdge(200, 100, 200)).toBe("move");
    expect(hitEdge(103, 100, 12)).toBe("in"); // a narrow block splits in thirds
    expect(hitEdge(105, 100, 12)).toBe("move");
  });
});

describe("words and sentences around a clip", () => {
  const t = text();

  it("finds the spoken word and the clip's words", () => {
    expect(wordIndexAt(t.words, 0)).toBe(-1);
    expect(wordIndexAt(t.words, t.words[4].start_ms + 10)).toBe(4);
    expect(wordIndexAt(t.words, 99_999)).toBe(8);
    const clip = { in_ms: inPointBefore(t.words, 3), out_ms: outPointAfter(t.words, 5, 20_000) };
    expect(clipWords(t.words, clip)).toEqual([3, 5]);
    expect(clipWords(t.words, { in_ms: 0, out_ms: 500 })).toBeNull();
  });

  it("shows one sentence of context on each side", () => {
    const clip = { in_ms: inPointBefore(t.words, 3), out_ms: outPointAfter(t.words, 5, 20_000) };
    expect(contextWords(t, clip)).toEqual([0, 8]);
    const first = { in_ms: 0, out_ms: outPointAfter(t.words, 2, 20_000) };
    expect(contextWords(t, first)).toEqual([0, 5]);
  });

  it("marks sentence ends between sentences", () => {
    const marks = sentenceMarks(t);
    expect(marks).toHaveLength(2);
    expect(marks[0]).toEqual({ ms: (t.words[2].end_ms + t.words[3].start_ms) / 2, kind: "punctuation" });
  });
});

describe("editor view", () => {
  it("warns outside the finder's range", () => {
    expect(lengthNote(35_000, DEFAULT_FINDER)).toBeNull();
    expect(lengthNote(12_000, DEFAULT_FINDER)).toBe("Shorter than 20 s");
    expect(lengthNote(75_000, DEFAULT_FINDER)).toBe("Longer than 60 s");
  });

  it("frames the clip with room on both sides", () => {
    const view = clipView({ in_ms: 60_000, out_ms: 90_000 }, 1000, 600_000);
    expect(view.startMs).toBe(50_000);
    expect(view.msPerPx).toBe(50); // 50 s in 1000 px
    expect(clipView({ in_ms: 2_000, out_ms: 30_000 }, 1000, 35_000).startMs).toBe(0);
  });
});

describe("new clips", () => {
  const t = text(); // three sentences of ~1.4 s, 1 s apart
  const d = 20_000;

  it("start at the sentence at the playhead and grow by whole sentences towards the target", () => {
    const finder = { ...DEFAULT_FINDER, target_ms: 3_000, max_ms: 10_000 };
    const r = clipAround(t, t.words[4].start_ms, finder, d)!;
    expect(r.in_ms).toBe(inPointBefore(t.words, 3));
    expect(r.out_ms).toBe(outPointAfter(t.words, 8, d)); // one sentence is too short, two reach the target
    const short = clipAround(t, 0, { ...finder, target_ms: 1_000 }, d)!;
    expect(short.out_ms).toBe(outPointAfter(t.words, 2, d));
    // Never over the longest length.
    const capped = clipAround(t, 0, { ...finder, target_ms: 60_000, max_ms: 3_500 }, d)!;
    expect(capped.out_ms - capped.in_ms).toBeLessThanOrEqual(3_500);
    // After the last sentence: the last one.
    expect(clipAround(t, 99_000, finder, d)!.in_ms).toBe(inPointBefore(t.words, 6));
    expect(clipAround({ ...t, sentences: [] }, 0, finder, d)).toBeNull();
  });

  it("widen selected words to whole sentences, either way round", () => {
    expect(clipFromWords(t, 4, 7, d)).toEqual({ in_ms: inPointBefore(t.words, 3), out_ms: outPointAfter(t.words, 8, d) });
    expect(clipFromWords(t, 7, 4, d)).toEqual(clipFromWords(t, 4, 7, d));
    expect(clipFromWords(t, 1, 1, d)).toEqual({ in_ms: inPointBefore(t.words, 0), out_ms: outPointAfter(t.words, 2, d) });
  });
});
