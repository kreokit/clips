import { describe, expect, it } from "vitest";
import { SAFE_AREA, isVertical, loadShowSafeArea, saveShowSafeArea } from "./safeArea";

describe("safe area", () => {
  it("applies to vertical videos only", () => {
    expect(isVertical({ width: 1080, height: 1920 })).toBe(true);
    expect(isVertical({ width: 1920, height: 1080 })).toBe(false);
    expect(isVertical({ width: 1080, height: 1080 })).toBe(false);
    expect(isVertical(null)).toBe(false);
  });

  it("leaves most of the frame free", () => {
    expect(SAFE_AREA.top + SAFE_AREA.bottom).toBeLessThan(0.5);
    expect(SAFE_AREA.right).toBeLessThan(0.25);
  });

  it("survives missing storage", () => {
    saveShowSafeArea(true); // Node has no localStorage: must not throw
    expect(typeof loadShowSafeArea()).toBe("boolean");
  });
});
