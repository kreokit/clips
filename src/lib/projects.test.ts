import { describe, expect, it } from "vitest";
import { describeShape, fileName, formatBytes, formatDuration, formatFps, isVideoPath, relativeTime } from "./projects";

describe("isVideoPath", () => {
  it("accepts video extensions in any case", () => {
    expect(isVideoPath("/v/Talk.MP4")).toBe(true);
    expect(isVideoPath("C:\\v\\pod.mkv")).toBe(true);
  });
  it("rejects other files", () => {
    expect(isVideoPath("/v/notes.txt")).toBe(false);
    expect(isVideoPath("/v/mp4")).toBe(false);
  });
});

describe("formatDuration", () => {
  it("shows minutes and seconds, hours when needed", () => {
    expect(formatDuration(9_458)).toBe("0:09");
    expect(formatDuration(725_000)).toBe("12:05");
    expect(formatDuration(3_723_000)).toBe("1:02:03");
    expect(formatDuration(0)).toBe("0:00");
  });
});

describe("formatFps", () => {
  it("rounds to two decimals", () => {
    expect(formatFps(30000 / 1001)).toBe("29.97 fps");
    expect(formatFps(25)).toBe("25 fps");
  });
});

describe("describeShape", () => {
  it("names common shapes", () => {
    expect(describeShape(1920, 1080)).toBe("Landscape 16:9");
    expect(describeShape(1080, 1920)).toBe("Vertical 9:16");
    expect(describeShape(1080, 1080)).toBe("Square");
    expect(describeShape(640, 480)).toBe("Landscape 4:3");
  });
  it("falls back to the ratio", () => {
    expect(describeShape(2560, 1080)).toBe("Landscape 2.37:1");
  });
});

describe("formatBytes", () => {
  it("picks a unit", () => {
    expect(formatBytes(80_000)).toBe("80 KB");
    expect(formatBytes(84_200_000)).toBe("84 MB");
    expect(formatBytes(1_230_000_000)).toBe("1.2 GB");
  });
});

describe("fileName", () => {
  it("handles both separators", () => {
    expect(fileName("/a/b/c.mp4")).toBe("c.mp4");
    expect(fileName("C:\\a\\c.mp4")).toBe("c.mp4");
  });
});

// A fixed "now": Monday 12 October 2026, 15:00 local time.
const now = new Date(2026, 9, 12, 15, 0).getTime();
const at = (...args: [number, number, number, number?, number?]) => new Date(...args).getTime();

describe("relativeTime", () => {
  it("counts minutes and hours today", () => {
    expect(relativeTime(now - 20_000, now)).toBe("just now");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relativeTime(at(2026, 9, 12, 9, 30), now)).toBe("5 h ago");
  });

  it("names yesterday and the last week's days", () => {
    expect(relativeTime(at(2026, 9, 11, 23, 0), now)).toBe("yesterday");
    expect(relativeTime(at(2026, 9, 8, 12, 0), now)).toBe("Thursday");
  });

  it("shows a date for older projects, with the year when it differs", () => {
    expect(relativeTime(at(2026, 8, 1), now)).toBe("1 Sept");
    expect(relativeTime(at(2025, 11, 24), now)).toBe("24 Dec 2025");
  });
});
