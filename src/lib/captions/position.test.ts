import { describe, expect, it } from "vitest";
import shared from "../../../fixtures/position-cases.json";
import { DEFAULT_STYLE } from "./presets";
import {
  PRESET_Y,
  anchorForY,
  anchorPoint,
  captionExtents,
  dragAnchor,
  fitAnchor,
  fittedStyle,
  placement,
  playResX,
} from "./position";
import type { Style } from "./types";

describe("placement (shared cases with ass.rs)", () => {
  for (const c of shared.cases) {
    it(c.name, () => {
      const style: Style = { ...DEFAULT_STYLE, pos_x: null, pos_y: null, ...(c.style as Partial<Style>) };
      const res = playResX(c.frame.width, c.frame.height);
      expect(res).toBe(c.play_res_x);
      expect(placement(style, res)).toEqual({ an: c.an, x: c.pos[0], y: c.pos[1] });
    });
  }
});

describe("anchors", () => {
  it("uses the preset spot until a free position is set", () => {
    expect(anchorPoint({ ...DEFAULT_STYLE, position: "bottom" })).toEqual({ x: 0.5, y: PRESET_Y.bottom });
    expect(anchorPoint({ ...DEFAULT_STYLE, pos_x: 0.2, pos_y: 0.4 })).toEqual({ x: 0.2, y: 0.4 });
  });

  it("anchors by thirds of the frame", () => {
    expect(anchorForY(0.1)).toBe("top");
    expect(anchorForY(0.5)).toBe("middle");
    expect(anchorForY(0.9)).toBe("bottom");
  });
});

describe("keeping captions inside the frame", () => {
  // A fake font: every character is 10 script pixels wide.
  const measure = (text: string) => [...text].length * 10;
  const seg = (text: string, start = 0) => ({
    id: start,
    start_ms: start,
    end_ms: start + 1000,
    text,
    words: text.split(" ").map((t, i) => ({ start_ms: start + i * 100, end_ms: start + i * 100 + 100, text: t })),
  });
  const style: Style = {
    ...DEFAULT_STYLE,
    uppercase: false,
    max_chars_per_line: 20,
    max_lines: 2,
    font_size: 60,
    outline_width: 5,
    mode: "line",
  };

  it("measures the widest line and the most lines, outline included", () => {
    const ext = captionExtents([seg("short"), seg("a much longer line here and more words", 2000)], style, measure);
    // Lines "a much longer line" (18) and "here and more words" (19): the widest is 19
    // characters; two lines of 60 px.
    expect(ext).toEqual({ width: 190 + 10, height: 120 + 10 });
  });

  it("measures single words in one_word mode", () => {
    expect(captionExtents([seg("hi wonderful")], { ...style, mode: "one_word" }, measure)).toEqual({
      width: 90 + 10,
      height: 60 + 10,
    });
  });

  it("pulls a caption back inside the frame", () => {
    const ext = { width: 400, height: 130 };
    // Dragged to the bottom-right corner of a 608-wide (9:16) frame.
    const corner = fitAnchor({ ...style, position: "bottom", pos_x: 1, pos_y: 1 }, ext, 608);
    expect(corner.x * 608).toBe(408); // 608 - 200
    expect(corner.y * 1080).toBe(1080); // bottom edge on the frame edge
    // Top-anchored at the very top stays; middle-anchored too high comes down by half.
    expect(fitAnchor({ ...style, position: "top", pos_x: 0.5, pos_y: 0 }, ext, 608).y).toBe(0);
    expect(fitAnchor({ ...style, position: "middle", pos_x: 0.5, pos_y: 0 }, ext, 608).y * 1080).toBe(65);
  });

  it("leaves captions that fit where they are, and centres oversized ones", () => {
    const placed = { ...style, position: "bottom" as const, pos_x: 0.3, pos_y: 0.8 };
    expect(fitAnchor(placed, { width: 100, height: 60 }, 1920)).toEqual({ x: 0.3, y: 0.8 });
    expect(fitAnchor(placed, { width: 5000, height: 60 }, 1920).x).toBe(0.5);
    expect(fittedStyle(placed, { width: 100, height: 60 }, 1920)).toMatchObject({ pos_x: 0.3, pos_y: 0.8 });
  });

  it("shrinks text that is wider than the frame, keeping the outline", () => {
    // 9:16 frame (608 wide): text 900 px wide at size 60 plus a 2 x 5 px outline.
    const wide = { ...style, font_size: 60, outline_width: 5, position: "bottom" as const };
    const fitted = fittedStyle(wide, { width: 910, height: 130 }, 608);
    // (608 * 0.98 - 10) / 900 = 0.6509 → size floor(60 * 0.6509) = 39.
    expect(fitted.font_size).toBe(39);
    // Centred: the shrunk box (900 * 39/60 + 10 = 595) fits; anchor stays in the middle.
    expect(fitted.pos_x).toBeCloseTo(0.5);
    // Text that fits keeps its size.
    expect(fittedStyle(wide, { width: 400, height: 130 }, 608).font_size).toBe(60);
  });
});

describe("dragging a caption", () => {
  const box = { w: 0.4, h: 0.1 };
  const bottom = { x: 0.5, y: PRESET_Y.bottom, position: "bottom" as const };
  const snap = { x: 0.01, y: 0.01 };
  const near = (a: number, b: number) => Math.abs(a - b) < 1e-9;

  it("moves with the pointer and re-anchors by thirds without jumping", () => {
    const moved = dragAnchor(bottom, -0.2, -0.2, box, null);
    // Centre was at bottom - h/2; moved up by 0.2 into the middle third.
    expect(moved.position).toBe("middle");
    expect(near(moved.x, 0.3)).toBe(true);
    expect(near(moved.y, PRESET_Y.bottom - 0.05 - 0.2)).toBe(true);
    const top = dragAnchor(bottom, 0, -0.5, box, null);
    expect(top.position).toBe("top");
    // Top anchor = centre - h/2.
    expect(near(top.y, PRESET_Y.bottom - 0.05 - 0.5 - 0.05)).toBe(true);
  });

  it("snaps to the centres and the Top / Bottom spots, with guides", () => {
    const centred = dragAnchor({ x: 0.3, y: 0.5, position: "middle" }, 0.195, 0.004, box, snap);
    expect(centred).toMatchObject({ x: 0.5, y: 0.5, position: "middle", guides: { vertical: 0.5, horizontal: 0.5 } });
    // Bottom edge lands 0.005 above the Bottom spot: snaps onto it.
    const nearBottom = dragAnchor(bottom, 0, -0.005, box, snap);
    expect(near(nearBottom.y, PRESET_Y.bottom)).toBe(true);
    expect(nearBottom.guides.horizontal).toBeCloseTo(PRESET_Y.bottom);
  });

  it("does not snap with snapping off, and keeps the box inside the frame", () => {
    expect(dragAnchor({ x: 0.3, y: 0.5, position: "middle" }, 0.195, 0, box, null).x).toBeCloseTo(0.495);
    const corner = dragAnchor(bottom, 2, 2, box, null);
    expect(corner.x).toBeCloseTo(0.8); // 1 - w/2
    expect(corner.y).toBeCloseTo(1); // bottom edge on the frame edge
  });
});
