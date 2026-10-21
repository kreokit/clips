import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { clipCaptions } from "../clips";
import type { Project } from "../projects";
import { normalized } from "../transcription";
import type { FaceSamples } from "./overlap";
import type { Segment } from "./types";

/** Captions are read again this long after the in/out points or fixes change. */
const SETTLE_MS = 120;

/**
 * The clip's captions on its own clock (src-tauri/src/captions.rs), read again when the
 * clip's range, the transcript settings or the word fixes change. Null while reading.
 */
export function useClipCaptions(project: Project, inMs: number, outMs: number): Segment[] | null {
  const [segments, setSegments] = useState<Segment[] | null>(null);
  const settings = JSON.stringify(normalized(project.transcription));
  const fixes = JSON.stringify(project.word_fixes);
  useEffect(() => {
    let live = true;
    const timer = setTimeout(() => {
      clipCaptions(project.id, JSON.parse(settings), JSON.parse(fixes), inMs, outMs).then(
        (s) => live && setSegments(s),
        () => live && setSegments([]),
      );
    }, SETTLE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [project.id, settings, fixes, inMs, outMs]);
  return segments;
}

/**
 * The faces found in the clip (per face-analysis sample, src-tauri `clip_faces`), once
 * they are analysed (`ready` turns true); null before.
 */
export function useClipFaces(projectId: string, inMs: number, outMs: number, ready: boolean): FaceSamples | null {
  const [faces, setFaces] = useState<FaceSamples | null>(null);
  useEffect(() => {
    if (!ready) return setFaces(null);
    let live = true;
    invoke<FaceSamples | null>("clip_faces", { id: projectId, inMs, outMs }).then(
      (f) => live && setFaces(f),
      () => live && setFaces(null),
    );
    return () => {
      live = false;
    };
  }, [projectId, inMs, outMs, ready]);
  return faces;
}
