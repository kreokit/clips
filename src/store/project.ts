import { create } from "zustand";
import { createAutosave } from "../lib/autosave";
import { Project, locateVideo, openVideo, saveProject, openProject as openProjectById } from "../lib/projects";
import { toast } from "../ui";

/** How long edits wait before they're saved (CLAUDE.md "Projects": auto-saved, no Save dialog). */
export const AUTOSAVE_DELAY_MS = 600;

interface ProjectState {
  /** The open project, or null on Home. */
  project: Project | null;
  /** Edits the open project; the change is saved automatically. */
  update: (patch: Partial<Omit<Project, "id" | "version">>) => void;
}

export const useProject = create<ProjectState>((set, get) => ({
  project: null,
  update: (patch) => {
    const current = get().project;
    if (!current) return;
    const project = { ...current, ...patch };
    set({ project });
    autosave.schedule(project);
  },
}));

const autosave = createAutosave<Project>(
  async (project) => {
    const saved = await saveProject(project);
    // Keep the newer timestamp unless the user moved on or edited again meanwhile.
    const current = useProject.getState().project;
    if (current?.id === saved.id) useProject.setState({ project: { ...current, updated_at: saved.updated_at } });
  },
  AUTOSAVE_DELAY_MS,
  (e) => toast(`Couldn't save the project: ${String(e)}`, "error"),
);

/** Saves any pending edit now (before closing a project or the window). */
export const flushProject = () => autosave.flush();

async function show(load: Promise<Project>): Promise<boolean> {
  try {
    const project = await load;
    await autosave.flush();
    useProject.setState({ project });
    return true;
  } catch (e) {
    toast(String(e), "error");
    return false;
  }
}

/** Imports a video (or reopens its project). Returns false and shows why on failure. */
export const openVideoPath = (path: string) => show(openVideo(path));
/** Opens a project from Home's list. */
export const openProject = (id: string) => show(openProjectById(id));
/** Points a project at its moved video, then opens it. */
export const locateAndOpen = (id: string, path: string) => show(locateVideo(id, path));

export async function closeProject() {
  await autosave.flush();
  useProject.setState({ project: null });
}

/** Forgets edits that weren't saved yet, e.g. when the open project is removed. */
export const discardPendingSave = () => autosave.cancel();
