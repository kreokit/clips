import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Dropped, FaceQueue } from "./faceQueue";
import type { CropPath } from "./reframe";

/** The app's one face-analysis queue (lib.rs `analyze_faces`). */
export const faceQueue = new FaceQueue((cmd, args) => invoke(cmd, args));

/** lib.rs `FacesProgress`: blocks of 10 s done of those this run needs. */
export interface FacesProgress {
  id: string;
  done: number;
  total: number;
}

export const reframePath = (id: string, inMs: number, outMs: number) =>
  invoke<CropPath | null>("reframe_path", { id, inMs, outMs });

export type PathState =
  | { kind: "ready"; path: CropPath }
  | { kind: "finding"; progress: number | null }
  | { kind: "waiting" }
  | { kind: "error"; message: string };

/** A trimmed clip asks again after this pause, not on every drag step. */
const SETTLE_MS = 300;

/**
 * The clip's automatic crop path: from the cache, else after analysing its faces first
 * in the queue. `retry` asks again after an error.
 */
export function useCropPath(projectId: string, inMs: number, outMs: number): [PathState, () => void] {
  const [state, setState] = useState<PathState>({ kind: "finding", progress: null });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | null = null;
    const timer = setTimeout(async () => {
      try {
        const cached = await reframePath(projectId, inMs, outMs);
        if (!live) return;
        if (cached) return setState({ kind: "ready", path: cached });
        setState({ kind: "finding", progress: null });
        unlisten = await listen<FacesProgress>("faces-progress", (e) => {
          const p = e.payload;
          if (live && p.id === projectId && p.total > 0 && faceQueue.status().running?.priority === "now") {
            setState({ kind: "finding", progress: p.done / p.total });
          }
        });
        const stop = faceQueue.subscribe((s) => {
          if (live && s.waitingForTranscription) setState({ kind: "waiting" });
        });
        try {
          await faceQueue.request(projectId, [[inMs, outMs]], "now");
        } finally {
          stop();
        }
        const path = await reframePath(projectId, inMs, outMs);
        if (live) setState(path ? { kind: "ready", path } : { kind: "error", message: "The faces couldn't be read." });
      } catch (e) {
        if (live && !(e instanceof Dropped)) setState({ kind: "error", message: String(e) });
      }
    }, SETTLE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
      unlisten?.();
    };
  }, [projectId, inMs, outMs, attempt]);

  return [state, () => setAttempt((a) => a + 1)];
}
