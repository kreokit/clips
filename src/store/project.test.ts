import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
let finishSave: (() => void) | null = null;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: { project?: { name: string } }) => {
    calls.push(args.project ? `${cmd} ${args.project.name}` : cmd);
    if (cmd === "save_project") return new Promise<unknown>((resolve) => (finishSave = () => resolve(args.project)));
    return Promise.resolve();
  },
  convertFileSrc: (p: string) => p,
}));

const { useProject, deleteOpenProject, flushProject } = await import("./project");
import type { Project } from "../lib/projects";

const project = { id: "0123456789abcdef", name: "Talk", clips: [] } as unknown as Project;

describe("deleteOpenProject", () => {
  beforeEach(() => {
    calls.length = 0;
    finishSave = null;
    useProject.setState({ project });
  });

  it("drops an edit not saved yet, so nothing writes the project back", async () => {
    useProject.getState().update({ name: "Renamed" });
    await deleteOpenProject();
    expect(calls).toEqual(["remove_project"]);
    expect(useProject.getState().project).toBeNull();
  });

  it("deletes only after a save already under way has finished", async () => {
    useProject.getState().update({ name: "Saving" });
    const saving = flushProject();
    const deleting = deleteOpenProject();
    await Promise.resolve();
    expect(calls).toEqual(["save_project Saving"]);
    finishSave?.();
    await Promise.all([saving, deleting]);
    expect(calls).toEqual(["save_project Saving", "remove_project"]);
    expect(useProject.getState().project).toBeNull();
  });
});
