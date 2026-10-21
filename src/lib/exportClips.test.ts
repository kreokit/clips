import { describe, expect, it } from "vitest";
import { DEFAULT_STYLE } from "./captions/presets";
import { Clip, DEFAULT_FINDER, defaultFraming, withPlatform } from "./clips";
import { ExportRow, estimateBytes, exportRows, exportSummary, formatBytes, initiallyTicked, pictureFor } from "./exportClips";
import type { Project } from "./projects";

const clip = (id: number, in_ms: number, lengthMs: number, p: Partial<Clip> = {}): Clip => ({
  id,
  in_ms,
  out_ms: in_ms + lengthMs,
  status: "kept",
  source: "found",
  edited: false,
  moment: null,
  framing: defaultFraming(),
  ...p,
});

const project = (clips: Clip[], finder = DEFAULT_FINDER): Project => ({
  version: 1,
  id: "0123456789abcdef",
  name: "talk",
  video_path: "/v/talk.mp4",
  video_size: 1,
  video_mtime_ms: 1,
  video: { duration_ms: 600_000, width: 1920, height: 1080, fps: 30, rotation: 0, has_audio: true },
  transcription: { model: "base", language: null, custom_words: "", hide_non_speech: true },
  finder,
  clips,
  style: DEFAULT_STYLE,
  word_fixes: [],
  created_at: 0,
  opened_at: 0,
  updated_at: 0,
});

describe("export list", () => {
  const clips = [
    clip(1, 300_000, 70_000),
    clip(2, 10_000, 45_000),
    clip(3, 100_000, 65_000, { status: "discarded" }),
    clip(4, 200_000, 61_000, { status: "proposed" }),
  ];

  it("numbers the kept clips by time and notes the short ones under TikTok Rewards", () => {
    const rows = exportRows(project(clips));
    expect(rows.map((r) => [r.clip.id, r.number])).toEqual([
      [2, 1],
      [1, 2],
    ]);
    expect(rows[0].note).toMatch(/not eligible/);
    expect(rows[1].note).toBeNull();
    // Decided 2026-10-21: listed, but unticked at first.
    expect([...initiallyTicked(rows)]).toEqual([1]);
  });

  it("takes the editor's clip even when it isn't kept, and no notes for Shorts", () => {
    const rows = exportRows(project(clips, withPlatform(DEFAULT_FINDER, "shorts")), [4]);
    expect(rows.map((r) => [r.clip.id, r.number])).toEqual([
      [2, 1],
      [4, 2],
      [1, 3],
    ]);
    expect(rows.every((r: ExportRow) => r.note === null)).toBe(true);
  });
});

describe("export pictures", () => {
  const source = { width: 1920, height: 1080 };
  it("follow the clip's framing like the preview", () => {
    const c = clip(1, 5_000, 61_000);
    expect(pictureFor({ ...c, framing: { mode: "fit_blur", manual: [] } }, null, source)).toEqual({ mode: "fit" });
    expect(pictureFor({ ...c, framing: { mode: "center", manual: [] } }, null, source)).toEqual({
      mode: "crop",
      axis: "x",
      keyframes: [{ t_ms: 5_000, pos: 0.5, jump: false, ease: false }],
    });
    const auto = { axis: "x" as const, crop: [0.3, 1] as [number, number], keyframes: [{ t_ms: 5_000, pos: 0.3, jump: false, ease: false }], no_face: false };
    expect(pictureFor(c, auto, source)).toEqual({ mode: "crop", axis: "x", keyframes: auto.keyframes });
    const manual = [{ t_ms: 6_000, pos: 0.7, jump: false, ease: true }];
    expect(pictureFor({ ...c, framing: { mode: "manual", manual } }, auto, source)).toMatchObject({ keyframes: manual });
  });
});

describe("export text", () => {
  it("sizes and summaries", () => {
    // Same as batch.rs: 161 s ≈ 207 MB.
    expect(estimateBytes(161_000)).toBe(207_165_140);
    expect(formatBytes(740_000_000)).toBe("740 MB");
    expect(formatBytes(1_340_000_000)).toBe("1.3 GB");
    expect(formatBytes(10)).toBe("1 MB");
    expect(
      exportSummary([
        { status: "saved", clip_id: 1, path: "a", srt: null },
        { status: "failed", clip_id: 2, error: "x" },
        { status: "skipped", clip_id: 3 },
      ]),
    ).toBe("1 exported, 1 failed, 1 not exported (stopped)");
  });
});
