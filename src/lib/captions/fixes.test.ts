import { describe, expect, it } from "vitest";
import { fixedText, isFixed, setFix } from "./fixes";

describe("word fixes", () => {
  const word = { start_ms: 1200, text: "wrold" };

  it("change, hide and restore a word", () => {
    const fixed = setFix([], word, "  world ");
    expect(fixed).toEqual([{ start_ms: 1200, was: "wrold", text: "world" }]);
    expect(fixedText(fixed, word)).toBe("world");
    expect(isFixed(fixed, word)).toBe(true);
    // Fixed again: replaced, not added.
    const again = setFix(fixed, word, "World");
    expect(again).toHaveLength(1);
    expect(fixedText(again, word)).toBe("World");
    // Hidden.
    expect(fixedText(setFix(again, word, " "), word)).toBe("");
    // Typed back as it was: no fix.
    expect(setFix(again, word, "wrold")).toEqual([]);
  });

  it("only touch the word they name", () => {
    const fixes = setFix(setFix([], { start_ms: 5000, text: "b" }, "B"), word, "world");
    expect(fixes.map((f) => f.start_ms)).toEqual([1200, 5000]);
    // Another transcript's word at the same time: not fixed.
    expect(fixedText(fixes, { start_ms: 1200, text: "hello" })).toBe("hello");
    expect(setFix(fixes, word, "a  b").find((f) => f.start_ms === 1200)?.text).toBe("a b");
  });
});
