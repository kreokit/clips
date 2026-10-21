import { describe, expect, it } from "vitest";
import { DEFAULT_STYLE } from "./presets";
import { PRESET_Y } from "./position";
import { FaceSamples, captionRect, coveredTimes, coversAFace, faceInFrame, overlapWarning } from "./overlap";
import type { Style } from "./types";

const style: Style = { ...DEFAULT_STYLE, font_size: 50, outline_width: 5 };

describe("caption box", () => {
  it("sits on its anchor like the export", () => {
    // Bottom spot: the box's bottom edge at 74% of the height, centred.
    const r = captionRect([300, 200], style)!;
    expect(r.y1).toBeCloseTo(PRESET_Y.bottom);
    expect(r.y1 - r.y0).toBeCloseTo((2 * 50 + 10) / 1080);
    expect((r.x0 + r.x1) / 2).toBeCloseTo(0.5);
    expect(r.x1 - r.x0).toBeCloseTo(310 / 608);
    const top = captionRect([100], { ...style, position: "top", pos_x: 0.5, pos_y: 0.2 })!;
    expect(top.y0).toBeCloseTo(0.2);
    expect(captionRect([], style)).toBeNull();
  });
});

describe("faces in the 9:16 frame", () => {
  it("follow the crop, kept inside the source like the export", () => {
    // A 16:9 source: the crop is 0.316 of the width, centred on 0.5.
    const span = 0.31640625;
    const face = faceInFrame([0.45, 0.2, 0.55, 0.4], { kind: "crop", axis: "x", span, pos: 0.5 });
    expect((face.x0 + face.x1) / 2).toBeCloseTo(0.5);
    expect(face.x1 - face.x0).toBeCloseTo(0.1 / span);
    expect(face.y0).toBe(0.2);
    // A crop asked past the left edge starts at 0.
    const edge = faceInFrame([0, 0.2, 0.1, 0.4], { kind: "crop", axis: "x", span, pos: 0 });
    expect(edge.x0).toBe(0);
  });

  it("shrink into the middle with Fit + blur", () => {
    const face = faceInFrame([0, 0, 1, 1], { kind: "fit", source: { width: 1920, height: 1080 } });
    expect(face.x0).toBe(0);
    expect(face.x1).toBe(1);
    expect(face.y0).toBeCloseTo((1920 - 607.5) / 2 / 1920);
  });
});

describe("covering a face", () => {
  const face = { x0: 0.4, y0: 0.3, x1: 0.6, y1: 0.45 };

  it("needs a real share of a visible face", () => {
    expect(coversAFace({ x0: 0.2, y0: 0.4, x1: 0.8, y1: 0.5 }, [face])).toBe(true);
    // Just touching its chin.
    expect(coversAFace({ x0: 0.2, y0: 0.44, x1: 0.8, y1: 0.6 }, [face])).toBe(false);
    // A face mostly outside the crop doesn't count.
    expect(coversAFace({ x0: 0, y0: 0, x1: 1, y1: 1 }, [{ x0: -0.3, y0: 0.3, x1: 0.05, y1: 0.45 }])).toBe(false);
  });

  it("warns when it lasts half a second", () => {
    const samples: FaceSamples = [
      [1000, [[0.45, 0.3, 0.55, 0.45]]],
      [1250, [[0.45, 0.3, 0.55, 0.45]]],
      [1500, []],
      [1750, [[0.45, 0.3, 0.55, 0.45]]],
    ];
    const view = () => ({ kind: "crop" as const, axis: "x" as const, span: 0.31640625, pos: 0.5 });
    // A caption dragged up over the face, shown from 1.2 s.
    const high = { x0: 0.1, y0: 0.3, x1: 0.9, y1: 0.45 };
    const times = coveredTimes(samples, (ms) => (ms >= 1200 ? high : null), view);
    expect(times).toEqual([1250, 1750]);
    expect(overlapWarning(times)).toBe(1250);
    expect(overlapWarning([1250])).toBeNull();
    // At the Bottom spot it's clear of the face.
    const low = captionRect([400, 400], style)!;
    expect(coveredTimes(samples, () => low, view)).toEqual([]);
  });
});
