import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { forgetSpokenText, spokenText } = await import("./clips");
import type { Project } from "./projects";

const settings = { model: "small", language: "en", custom_words: "", hide_non_speech: true };
const project = (id: string) => ({ id, transcription: settings }) as unknown as Project;
const words = (start_ms: number) => ({ words: [{ start_ms, end_ms: start_ms + 300, text: "Hi." }], sentences: [], sounds: [] });

describe("spokenText", () => {
  beforeEach(() => invoke.mockReset());

  it("reads a transcript once, and again after a new transcript", async () => {
    invoke.mockResolvedValueOnce(words(1_000)).mockResolvedValueOnce(words(1_677));
    expect((await spokenText(project("a"))).words[0].start_ms).toBe(1_000);
    expect((await spokenText(project("a"))).words[0].start_ms).toBe(1_000);
    expect(invoke).toHaveBeenCalledTimes(1);
    forgetSpokenText("a");
    expect((await spokenText(project("a"))).words[0].start_ms).toBe(1_677);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("forgets only that project", async () => {
    invoke.mockResolvedValue(words(0));
    await spokenText(project("b"));
    await spokenText(project("c"));
    forgetSpokenText("b");
    await spokenText(project("b"));
    await spokenText(project("c"));
    expect(invoke.mock.calls.map((c) => (c[1] as { id: string }).id)).toEqual(["b", "c", "b"]);
  });
});
