import { describe, expect, it } from "vitest";
import type { TranscriptionSettings } from "./projects";
import {
  Progress,
  describeModelProgress,
  describeProgress,
  formatEta,
  languageName,
  languageValue,
  modelHint,
  normalized,
  startLabel,
  withLanguage,
  wordCount,
} from "./transcription";

const progress = (p: Partial<Progress>): Progress => ({
  stage: "transcribing",
  position_ms: 0,
  duration_ms: 3_723_000,
  part: 1,
  parts: 13,
  eta_ms: null,
  ...p,
});

describe("formatEta", () => {
  it("rounds to minutes and adds hours when needed", () => {
    expect(formatEta(30_000)).toBe("less than a minute left");
    expect(formatEta(8 * 60_000 + 20_000)).toBe("about 8 min left");
    expect(formatEta(65 * 60_000)).toBe("about 1 h 5 min left");
    expect(formatEta(120 * 60_000)).toBe("about 2 h left");
  });
});

describe("describeProgress", () => {
  it("shows the part, position and time left while transcribing", () => {
    const view = describeProgress(progress({ position_ms: 725_000, part: 3, eta_ms: 8 * 60_000 }));
    expect(view).toEqual({
      label: "Transcribing · part 3 of 13",
      detail: "12:05 of 1:02:03 · about 8 min left",
      percent: 19,
    });
  });
  it("says it is measuring before there is a speed", () => {
    expect(describeProgress(progress({})).detail).toBe("0:00 of 1:02:03 · measuring speed…");
  });
  it("leaves out the part for a short video", () => {
    const view = describeProgress(progress({ parts: 1, duration_ms: 60_000, position_ms: 30_000 }));
    expect(view.label).toBe("Transcribing");
    expect(view.percent).toBe(50);
  });
  it("shows the first read of the audio", () => {
    expect(describeProgress(progress({ stage: "reading-audio", position_ms: 725_000, part: 0, parts: 0 }))).toEqual({
      label: "Reading the audio…",
      detail: "12:05 of 1:02:03 · once, to plan the parts",
      percent: 19,
    });
  });
  it("covers loading and finishing", () => {
    expect(describeProgress(progress({ stage: "loading-model" })).label).toBe("Loading the model…");
    expect(describeProgress(progress({ stage: "finishing" }))).toEqual({ label: "Finishing…", detail: null, percent: 100 });
  });
  it("never goes past 100% or divides by zero", () => {
    expect(describeProgress(progress({ position_ms: 4_000_000 })).percent).toBe(100);
    expect(describeProgress(progress({ duration_ms: 0 })).percent).toBeNull();
  });
});

describe("describeModelProgress", () => {
  it("names the download and its size", () => {
    const view = describeModelProgress({ stage: "downloading", done_bytes: 50_000_000, total_bytes: 147_951_465 }, "base");
    expect(view.label).toBe("Downloading the Base model");
    expect(view.detail).toBe("50 MB of 148 MB · once, then it works offline");
    expect(view.percent).toBe(33);
  });
  it("names the check of Captions' copy", () => {
    const view = describeModelProgress({ stage: "checking", done_bytes: 1, total_bytes: 2 }, "small");
    expect(view.label).toBe("Checking the Small model from KreoKit Captions…");
    expect(view.percent).toBe(50);
  });
});

describe("modelHint", () => {
  it("says where the model is, or how big the download is", () => {
    expect(modelHint({ id: "base", size_bytes: 147_951_465, source: "ours" })).toBe("fast · ready");
    expect(modelHint({ id: "small", size_bytes: 487_601_967, source: "captions" })).toBe("balanced · from Captions");
    expect(modelHint({ id: "large-v3-turbo", size_bytes: 1_624_555_275, source: null })).toBe("most accurate · 1.6 GB");
  });
});

describe("startLabel", () => {
  it("offers to resume a cancelled run", () => {
    expect(startLabel(null)).toBe("Transcribe");
    expect(startLabel({ transcript: null, finished_parts: 0, parts: 13 })).toBe("Transcribe");
    expect(startLabel({ transcript: null, finished_parts: 3, parts: 13 })).toBe("Resume (3 of 13 parts done)");
  });
});

const settings: TranscriptionSettings = {
  model: "base",
  language: null,
  custom_words: "  KreoKit, Zylvana \n",
  hide_non_speech: true,
};

describe("settings helpers", () => {
  it("maps the language picker's auto to null", () => {
    expect(languageValue(settings)).toBe("auto");
    expect(withLanguage(settings, "de").language).toBe("de");
    expect(withLanguage({ ...settings, language: "de" }, "auto").language).toBeNull();
  });
  it("trims custom words before they reach the cache", () => {
    expect(normalized(settings).custom_words).toBe("KreoKit, Zylvana");
    expect(normalized(settings).model).toBe("base");
  });
  it("names languages", () => {
    expect(languageName("en")).toBe("English");
    expect(languageName("xx")).toBe("xx");
  });
});

describe("wordCount", () => {
  it("counts words, from the text when a segment has no word timings", () => {
    const word = (text: string) => ({ start_ms: 0, end_ms: 1, text });
    expect(
      wordCount({
        language: "en",
        segments: [
          { id: 0, start_ms: 0, end_ms: 1, text: "Hello there", words: [word("Hello"), word("there")] },
          { id: 1, start_ms: 1, end_ms: 2, text: " three more words ", words: [] },
        ],
      }),
    ).toBe(5);
  });
});
