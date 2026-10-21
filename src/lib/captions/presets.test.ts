import { describe, expect, it } from "vitest";
import { DEFAULT_STYLE, PRESETS, matchingPreset } from "./presets";

describe("presets", () => {
  it("have unique ids and complete styles", () => {
    expect(new Set(PRESETS.map((p) => p.id)).size).toBe(PRESETS.length);
    const keys = Object.keys(DEFAULT_STYLE).sort();
    for (const p of PRESETS) expect(Object.keys(p.style).sort()).toEqual(keys);
  });

  it("are recognised until a value changes", () => {
    expect(matchingPreset(PRESETS[0].style)?.id).toBe("clean");
    expect(matchingPreset({ ...PRESETS[2].style, font_size: 99 })).toBeUndefined();
  });

  it("default to Bold highlight", () => {
    expect(matchingPreset(DEFAULT_STYLE)?.id).toBe("bold-highlight");
  });
});
