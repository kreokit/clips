import { describe, expect, it } from "vitest";
import { Clip, DEFAULT_FINDER, Moment, applyScores, boardView, fixRange, countByStatus, findRequest, firstLine, manualClip, mergeFound, nextId, topReasons, untouched } from "./clips";

const moment = (in_ms: number, out_ms: number, score = 50, reasons: [string, boolean][] = []): Moment => ({
  in_ms,
  out_ms,
  score,
  text: "words",
  features: [{ kind: "hook", weight: 25, value: 0.5, reasons: reasons.map(([text, good]) => ({ text, good })) }],
});

const clip = (id: number, p: Partial<Clip> = {}): Clip => ({
  id,
  in_ms: id * 100_000,
  out_ms: id * 100_000 + 30_000,
  status: "proposed",
  source: "found",
  edited: false,
  moment: moment(id * 100_000, id * 100_000 + 30_000),
  ...p,
});

describe("Find again", () => {
  const board = [
    clip(1),
    clip(2, { status: "kept" }),
    clip(3, { status: "discarded" }),
    clip(4, { edited: true }),
    clip(5, { source: "manual", status: "kept" }),
  ];

  it("replaces only untouched proposals", () => {
    expect(board.filter(untouched).map((c) => c.id)).toEqual([1]);
    const merged = mergeFound(board, [moment(0, 25_000, 80), moment(700_000, 740_000, 60)]);
    expect(merged.map((c) => c.id)).toEqual([2, 3, 4, 5, 6, 7]);
    expect(merged[4]).toMatchObject({ in_ms: 0, out_ms: 25_000, status: "proposed", source: "found", edited: false });
  });

  it("avoids every clip that stays and counts those not discarded", () => {
    const { avoid, have } = findRequest(board);
    expect(avoid).toEqual([2, 3, 4, 5].map((i) => [i * 100_000, i * 100_000 + 30_000]));
    expect(have).toBe(3);
    expect(findRequest([])).toEqual({ avoid: [], have: 0 });
  });

  it("numbers new clips after the highest id", () => {
    expect(nextId([])).toBe(1);
    expect(nextId([clip(4), clip(2)])).toBe(5);
    expect(manualClip([clip(4)], 1_000, 9_000, null)).toEqual({
      id: 5,
      in_ms: 1_000,
      out_ms: 9_000,
      status: "kept",
      source: "manual",
      edited: false,
      moment: null,
    });
  });
});

describe("boardView", () => {
  const clips = [
    clip(1, { moment: moment(0, 1, 40), out_ms: 100_000 + 50_000 }),
    clip(2, { moment: moment(0, 1, 90), status: "kept" }),
    clip(3, { moment: moment(0, 1, 40), status: "discarded" }),
    clip(4, { moment: null }),
  ];

  it("sorts by score, time or length", () => {
    expect(boardView(clips, "all", "score").map((c) => c.id)).toEqual([2, 1, 3, 4]);
    expect(boardView(clips, "all", "time").map((c) => c.id)).toEqual([1, 2, 3, 4]);
    expect(boardView(clips, "all", "length").map((c) => c.id)).toEqual([1, 2, 3, 4]);
  });

  it("filters by status and counts them", () => {
    expect(boardView(clips, "kept", "score").map((c) => c.id)).toEqual([2]);
    expect(boardView(clips, "proposed", "time").map((c) => c.id)).toEqual([1, 4]);
    expect(countByStatus(clips)).toEqual({ all: 4, proposed: 2, kept: 1, discarded: 1 });
  });
});

describe("card text", () => {
  it("cuts the first line at a word end", () => {
    expect(firstLine("short")).toBe("short");
    expect(firstLine("one two three four", 10)).toBe("one two…");
  });

  it("shows good reasons first, skipping the routine ones", () => {
    const m = moment(0, 1, 50, [
      ["Starts at the beginning of a sentence", true],
      ["Starts with “and”", false],
      ["Opens with a question", true],
      ["Speaks to “you”", true],
    ]);
    expect(topReasons(m).map((r) => r.text)).toEqual(["Opens with a question", "Speaks to “you”"]);
    expect(topReasons(m, 3).map((r) => r.text)).toEqual(["Opens with a question", "Speaks to “you”", "Starts with “and”"]);
    expect(topReasons(null)).toEqual([]);
  });
});

describe("applyScores", () => {
  it("updates clips whose in/out are unchanged, matched by id", () => {
    const scored = [clip(1), clip(2), clip(3)];
    const latest = [clip(3), clip(2, { in_ms: 5 }), clip(4)];
    const out = applyScores(latest, scored, [moment(0, 1, 11), moment(0, 1, 22), null]);
    expect(out[0].moment).toBeNull(); // clip 3 scored to null
    expect(out[1]).toBe(latest[1]); // moved meanwhile
    expect(out[2]).toBe(latest[2]); // new
  });
});

describe("fixRange", () => {
  it("keeps shortest ≤ target ≤ longest, with 5 s between shortest and longest", () => {
    expect(fixRange(DEFAULT_FINDER)).toEqual(DEFAULT_FINDER);
    expect(fixRange({ ...DEFAULT_FINDER, min_ms: 60_000 })).toMatchObject({ min_ms: 60_000, max_ms: 65_000, target_ms: 60_000 });
    expect(fixRange({ ...DEFAULT_FINDER, target_ms: 90_000 }).target_ms).toBe(60_000);
  });
});
