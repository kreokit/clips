import { describe, expect, it } from "vitest";
import cases from "../../fixtures/reframe/path-cases.json";
import type { Keyframe } from "./clips";
import {
  centerPath,
  cropPlacement,
  cropSize,
  ease,
  fitPlacement,
  framingKeyframes,
  lowResolution,
  portraitBox,
  positionAt,
  removeKeyframe,
  seedManual,
  setKeyframe,
} from "./reframe";

const k = (t_ms: number, pos: number, jump = false, eased = false): Keyframe => ({ t_ms, pos, jump, ease: eased });

describe("positionAt (the same cases as reframe.rs)", () => {
  for (const c of cases.cases) {
    it(c.name, () => {
      for (const [t, want] of c.at) expect(positionAt(c.keyframes as Keyframe[], t)).toBeCloseTo(want, 6);
    });
  }
  it("eases with smootherstep", () => {
    expect(ease(0)).toBe(0);
    expect(ease(0.5)).toBe(0.5);
    expect(ease(1)).toBe(1);
    expect(ease(2)).toBe(1);
  });
});

describe("crop geometry", () => {
  it("is the full height of a landscape source, the full width of a tall one", () => {
    const [w, h] = cropSize(1920, 1080);
    expect(w).toBeCloseTo(0.31640625, 6);
    expect(h).toBe(1);
    const [w2, h2] = cropSize(1000, 2400);
    expect(w2).toBe(1);
    expect(h2).toBeCloseTo(0.7407407, 6);
    expect(centerPath(1000, 2400, 0).axis).toBe("y");
  });

  it("places the video so the crop fills the box", () => {
    const box = { width: 270, height: 480 };
    // 1920×1080 drawn 480 high is 853.3 wide; the crop at 0.25 is centred in the box.
    const p = cropPlacement({ width: 1920, height: 1080 }, box, "x", 0.25);
    expect(p.height).toBe(480);
    expect(p.width).toBeCloseTo(853.33, 2);
    expect(p.left + 0.25 * p.width).toBeCloseTo(135, 6);
    const tall = cropPlacement({ width: 1000, height: 2400 }, box, "y", 0.5);
    expect(tall.width).toBe(270);
    expect(tall.top + 0.5 * tall.height).toBeCloseTo(240, 6);
  });

  it("fits the whole frame for Fit + blur", () => {
    expect(fitPlacement({ width: 1920, height: 1080 }, { width: 270, height: 480 })).toEqual({
      width: 270,
      height: 151.875,
      left: 0,
      top: 164.0625,
    });
  });

  it("finds the largest 9:16 box", () => {
    expect(portraitBox({ width: 1000, height: 640 })).toEqual({ width: 360, height: 640 });
    expect(portraitBox({ width: 200, height: 640 })).toEqual({ width: 200, height: 355 });
  });

  it("calls sources under 1080p low resolution", () => {
    expect(lowResolution(1280, 720)).toBe(true);
    expect(lowResolution(1920, 1080)).toBe(false);
    expect(lowResolution(1080, 1920)).toBe(false);
    expect(lowResolution(720, 1280)).toBe(true);
  });
});

describe("framing modes", () => {
  const auto = { ...centerPath(1920, 1080, 0), keyframes: [k(0, 0.3), k(5000, 0.3), k(6000, 0.6, false, true)] };

  it("picks the keyframes per mode", () => {
    expect(framingKeyframes({ mode: "track", manual: [] }, auto, 0)).toBe(auto.keyframes);
    expect(framingKeyframes({ mode: "track", manual: [] }, null, 1000)).toEqual([k(1000, 0.5)]);
    expect(framingKeyframes({ mode: "center", manual: [] }, auto, 1000)).toEqual([k(1000, 0.5)]);
    expect(framingKeyframes({ mode: "fit_blur", manual: [] }, auto, 1000)).toBeNull();
    expect(framingKeyframes({ mode: "manual", manual: [k(0, 0.2)] }, auto, 1000)).toEqual([k(0, 0.2)]);
  });

  it("starts Manual from the automatic path inside the clip", () => {
    const seeded = seedManual(auto, { in_ms: 5500, out_ms: 9000 });
    expect(seeded.map((s) => s.t_ms)).toEqual([5500, 6000]);
    expect(seeded[0].pos).toBeCloseTo(0.45, 6); // halfway through the eased move
    expect(seeded[1]).toEqual(k(6000, 0.6, false, true));
    expect(seedManual(null, { in_ms: 100, out_ms: 900 })).toEqual([k(100, 0.5)]);
  });

  it("moves a keyframe in reach, else adds an eased one, keeping the crop inside", () => {
    const keys = [k(0, 0.3), k(4000, 0.5)];
    expect(setKeyframe(keys, 4100, 0.7, 0.32)).toEqual([k(0, 0.3), k(4000, 0.7)]);
    expect(setKeyframe(keys, 2000, 0.4, 0.32)).toEqual([k(0, 0.3), k(2000, 0.4, false, true), k(4000, 0.5)]);
    expect(setKeyframe(keys, 2000, 0.99, 0.32)[1].pos).toBeCloseTo(0.84, 6);
    expect(removeKeyframe(keys, 3900)).toEqual([k(0, 0.3)]);
    expect(removeKeyframe([k(0, 0.3)], 0)).toEqual([k(0, 0.3)]);
  });
});
