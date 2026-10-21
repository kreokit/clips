import { describe, expect, it } from "vitest";
import { displayText, ghostFrame, outlineShadow, overlayFrame } from "./overlay";
import { DEFAULT_STYLE } from "./presets";
import type { CaptionMode, Segment, Style } from "./types";

// Same captions as the ASS snapshot tests in src-tauri/src/ass.rs.
const segments: Segment[] = [
  {
    id: 0,
    start_ms: 1000,
    end_ms: 2500,
    text: "Hello there friend",
    words: [
      { start_ms: 1000, end_ms: 1400, text: "Hello" },
      { start_ms: 1500, end_ms: 1800, text: "there" },
      { start_ms: 1800, end_ms: 2300, text: "friend" },
    ],
  },
];

function style(mode: CaptionMode, extra: Partial<Style> = {}): Style {
  return { ...DEFAULT_STYLE, mode, uppercase: false, max_chars_per_line: 11, max_lines: 2, ...extra };
}

/** Lines joined with " / ", the active word in [brackets]. */
const shown = (ms: number, s: Style) =>
  overlayFrame(segments, ms, s)
    ?.lines.map((line) => line.map((w) => (w.active ? `[${w.text}]` : w.text)).join(" "))
    .join(" / ") ?? null;

describe("overlayFrame", () => {
  it("shows nothing outside captions", () => {
    expect(shown(500, style("line"))).toBeNull();
    expect(shown(2500, style("word_highlight"))).toBeNull();
  });

  it("line mode: the page with the layout's line breaks", () => {
    expect(shown(1200, style("line"))).toBe("Hello there / friend");
  });

  it("word_highlight follows the ASS events", () => {
    const s = style("word_highlight");
    expect(shown(1000, s)).toBe("[Hello] there / friend");
    expect(shown(1450, s)).toBe("Hello there / friend");
    expect(shown(1500, s)).toBe("Hello [there] / friend");
    expect(shown(2300, s)).toBe("Hello there / friend");
  });

  it("one_word shows each word until the next starts", () => {
    const s = style("one_word", { uppercase: true });
    expect(shown(1000, s)).toBe("[HELLO]");
    expect(shown(1450, s)).toBe("[HELLO]");
    expect(shown(1800, s)).toBe("[FRIEND]");
    expect(shown(2400, s)).toBe("[FRIEND]");
  });

  it("changes key when a new caption appears", () => {
    const s = style("one_word");
    expect(overlayFrame(segments, 1000, s)?.key).not.toBe(overlayFrame(segments, 1500, s)?.key);
    expect(overlayFrame(segments, 1000, s)?.key).toBe(overlayFrame(segments, 1450, s)?.key);
  });

  it("uses the layout's single word for segments without timings", () => {
    const noWords: Segment[] = [{ ...segments[0], words: [] }];
    expect(overlayFrame(noWords, 1200, style("one_word"))?.lines).toEqual([
      [{ text: "Hello there friend", active: true }],
    ]);
  });
});

describe("displayText", () => {
  it("swaps ASS override characters like ass.rs", () => {
    expect(displayText("{\\b1}hi")).toBe("｛＼b1｝hi");
  });
});

describe("outlineShadow", () => {
  it("has no shadow without an outline", () => {
    expect(outlineShadow(0, "#000000")).toBeUndefined();
  });

  it("surrounds the text at the outline radius, ~1 px apart", () => {
    const shadows = outlineShadow(1, "#000000")!.split(", ");
    expect(shadows).toHaveLength(8);
    expect(shadows[0]).toBe("1.00px 0.00px 0 #000000");
  });

  it("adds an inner ring for thick outlines and caps the count", () => {
    const shadows = outlineShadow(6, "#112233")!.split(", ");
    expect(shadows).toHaveLength(38 + 19); // ceil(2π·6) + ceil(2π·3)
    expect(outlineShadow(40, "#000000")!.split(", ")).toHaveLength(48 + 48);
  });
});

describe("ghostFrame", () => {
  const two: Segment[] = [segments[0], { ...segments[0], id: 1, start_ms: 5000, end_ms: 6000, text: "Later", words: [{ start_ms: 5000, end_ms: 6000, text: "Later" }] }];
  const s = style("line");
  const text = (ms: number) => ghostFrame(two, ms, s)?.lines.map((l) => l.map((w) => w.text).join(" ")).join(" / ");

  it("shows the caption nearest in time", () => {
    expect(text(0)).toBe("Hello there / friend");
    expect(text(3000)).toBe("Hello there / friend"); // 500 ms after the first, 2000 before the second
    expect(text(4800)).toBe("Later");
    expect(text(9000)).toBe("Later");
  });

  it("has nothing to show without captions", () => {
    expect(ghostFrame([], 0, s)).toBeNull();
  });
});
