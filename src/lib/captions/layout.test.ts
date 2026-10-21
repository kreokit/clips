import { describe, expect, it } from "vitest";
import shared from "../../../fixtures/layout-cases.json";
import { layout } from "./layout";
import type { Segment } from "./types";

describe("layout (shared cases with layout.rs)", () => {
  for (const c of shared.cases) {
    it(c.name, () => {
      const segment: Segment = { id: 0, ...c.segment };
      const result = layout(segment, c.max_chars, c.max_lines);
      const pages = result.pages.map((p) => ({
        start_ms: p.start_ms,
        end_ms: p.end_ms,
        lines: p.lines.map((line) => line.map((i) => result.words[i].text)),
      }));
      expect(pages).toEqual(c.pages);
    });
  }
});
