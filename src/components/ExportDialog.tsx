import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, FolderOpen } from "lucide-react";
import {
  ExportItem,
  ExportPlan,
  ExportProgress,
  ExportRow,
  ItemResult,
  cancelExport,
  estimateBytes,
  exportPlan,
  exportRows,
  exportSummary,
  formatBytes,
  initiallyTicked,
  lastFolder,
  prepareItem,
  rememberFolder,
  revealExport,
} from "../lib/exportClips";
import { Project, formatDuration } from "../lib/projects";
import { normalized } from "../lib/transcription";
import { Button, Checkbox, Dialog, ProgressBar, toast } from "../ui";
import "../styles/export.css";

type Status =
  | { kind: "choose" }
  | { kind: "checking" }
  | { kind: "confirm"; folder: string; plan: ExportPlan }
  | { kind: "preparing"; done: number; total: number }
  | { kind: "exporting"; index: number; total: number; percent: number }
  | { kind: "finished"; results: ItemResult[] };

/** The folder part of a path (either separator). */
const folderOf = (path: string) => path.slice(0, Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);

/**
 * Export all (or one clip, `only`): the kept clips in time order, ticked unless they're
 * under 61 s with TikTok Rewards; an SRT per clip on request. After choosing the folder:
 * files already there (Replace / Keep both) and the size against the free space, then
 * each clip is prepared (faces, fitted captions) and exported with progress and Stop.
 */
export function ExportDialog({ project, only, onClose }: { project: Project; only?: number; onClose: () => void }) {
  const rows = exportRows(project, only !== undefined ? [only] : []).filter((r) => only === undefined || r.clip.id === only);
  const [ticked, setTicked] = useState<Set<number>>(() => (only !== undefined ? new Set([only]) : initiallyTicked(rows)));
  const [srt, setSrt] = useState(false);
  const [status, setStatus] = useState<Status>({ kind: "choose" });
  const [stopping, setStopping] = useState(false);
  const stopRequested = useRef(false);
  const chosen = rows.filter((r) => ticked.has(r.clip.id));
  const totalMs = chosen.reduce((sum, r) => sum + r.lengthMs, 0);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let live = true;
    void listen<ExportProgress>("export-progress", (e) => {
      setStatus((s) => (s.kind === "exporting" ? { kind: "exporting", ...e.payload } : s));
    }).then((u) => (live ? (unlisten = u) : u()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  const toggle = (id: number, on: boolean) =>
    setTicked((t) => {
      const next = new Set(t);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const pickFolder = async () => {
    // Development aid (debug builds): KREOKIT_DEV_EXPORT_DIR skips the folder picker.
    const devFolder = await invoke<string | null>("dev_export_folder").catch(() => null);
    const folder =
      devFolder ??
      (await open({ directory: true, multiple: false, defaultPath: lastFolder() ?? (folderOf(project.video_path) || undefined) }));
    if (typeof folder !== "string") return;
    rememberFolder(folder);
    setStatus({ kind: "checking" });
    try {
      const plan = await exportPlan(project.id, folder, chosen.map((r) => [r.number, r.lengthMs]), srt);
      if (plan.conflicts.length || !plan.enough_space) setStatus({ kind: "confirm", folder, plan });
      else await run(folder, false);
    } catch (e) {
      toast(String(e), "error");
      setStatus({ kind: "choose" });
    }
  };

  const run = async (folder: string, replace: boolean) => {
    stopRequested.current = false;
    setStopping(false);
    // Faces and fitted captions first, clip by clip (a clip that can't be prepared fails).
    const items: ExportItem[] = [];
    const failed: ItemResult[] = [];
    for (const [i, row] of chosen.entries()) {
      if (stopRequested.current) break;
      setStatus({ kind: "preparing", done: i, total: chosen.length });
      try {
        items.push(await prepareItem(project, row));
      } catch (e) {
        failed.push({ status: "failed", clip_id: row.clip.id, error: e instanceof Error ? e.message : String(e) });
      }
    }
    if (stopRequested.current) {
      const done = new Set([...items.map((i) => i.clip_id), ...failed.map((f) => f.clip_id)]);
      const skipped: ItemResult[] = chosen.filter((r) => !done.has(r.clip.id)).map((r) => ({ status: "skipped", clip_id: r.clip.id }));
      setStatus({ kind: "finished", results: [...failed, ...items.map((i): ItemResult => ({ status: "skipped", clip_id: i.clip_id })), ...skipped] });
      return;
    }
    setStatus({ kind: "exporting", index: 0, total: items.length, percent: 0 });
    try {
      const results = items.length
        ? await invoke<ItemResult[]>("export_clips", {
            id: project.id,
            settings: normalized(project.transcription),
            folder,
            items,
            srt,
            replace,
          })
        : [];
      setStatus({ kind: "finished", results: [...results, ...failed] });
    } catch (e) {
      toast(String(e), "error");
      setStatus({ kind: "choose" });
    }
  };

  const stop = () => {
    setStopping(true);
    stopRequested.current = true;
    void cancelExport();
  };

  const name = (id: number) => {
    const row = rows.find((r) => r.clip.id === id);
    return row ? `Clip ${String(row.number).padStart(2, "0")}` : `Clip ${id}`;
  };

  if (status.kind === "finished") {
    const saved = status.results.filter((r): r is Extract<ItemResult, { status: "saved" }> => r.status === "saved");
    const failed = status.results.filter((r): r is Extract<ItemResult, { status: "failed" }> => r.status === "failed");
    return (
      <Dialog
        title={only !== undefined ? "Export clip" : "Export all"}
        onClose={onClose}
        footer={
          <>
            {saved[0] && (
              <Button icon={FolderOpen} onClick={() => void revealExport(saved[0].path).catch((e) => toast(String(e), "error"))}>
                Show in folder
              </Button>
            )}
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </>
        }
      >
        <p>{exportSummary(status.results)}.</p>
        {saved.length > 0 && (
          <ul className="export-files">
            {saved.map((s) => (
              <li key={s.clip_id}>{s.path.slice(folderOf(s.path).length)}</li>
            ))}
          </ul>
        )}
        {failed.length > 0 && (
          <ul className="export-failures">
            {failed.map((f) => (
              <li key={f.clip_id}>
                <strong>{name(f.clip_id)}</strong>: {f.error}
              </li>
            ))}
          </ul>
        )}
      </Dialog>
    );
  }

  if (status.kind === "confirm") {
    const { plan, folder } = status;
    const shown = plan.conflicts.slice(0, 5);
    return (
      <Dialog
        title={plan.enough_space ? "Some files already exist" : "Not enough free space"}
        onClose={() => setStatus({ kind: "choose" })}
        footer={
          <>
            <Button onClick={() => setStatus({ kind: "choose" })}>Cancel</Button>
            {plan.conflicts.length > 0 ? (
              <>
                <Button onClick={() => void run(folder, false)}>Keep both</Button>
                <Button variant="primary" onClick={() => void run(folder, true)}>
                  Replace
                </Button>
              </>
            ) : (
              <Button variant="primary" onClick={() => void run(folder, false)}>
                Export anyway
              </Button>
            )}
          </>
        }
      >
        {!plan.enough_space && plan.free !== null && (
          <p className="export-space-warning">
            <AlertTriangle size={14} strokeWidth={2} aria-hidden />
            The clips need about {formatBytes(plan.bytes)}, and {formatBytes(plan.free)} is free on that drive. Keep at least 2 GB free
            so your computer keeps running well.
          </p>
        )}
        {plan.conflicts.length > 0 && (
          <>
            <p>
              {plan.conflicts.length === 1 ? "This file is" : `These ${plan.conflicts.length} files are`} already in the folder:
            </p>
            <ul className="export-files">
              {shown.map((f) => (
                <li key={f}>{f}</li>
              ))}
              {plan.conflicts.length > shown.length && <li>and {plan.conflicts.length - shown.length} more</li>}
            </ul>
            <p className="ui-field-hint">Keep both saves the new files with -2, -3… added to their names.</p>
          </>
        )}
      </Dialog>
    );
  }

  const busy = status.kind === "checking" || status.kind === "preparing" || status.kind === "exporting";
  const progress =
    status.kind === "exporting"
      ? { text: `Exporting clip ${status.index + 1} of ${status.total}`, value: Math.round(((status.index + status.percent / 100) / Math.max(status.total, 1)) * 100) }
      : status.kind === "preparing"
        ? { text: `Getting clip ${status.done + 1} of ${status.total} ready (faces and captions)`, value: null }
        : null;
  return (
    <Dialog
      title={only !== undefined ? "Export clip" : `Export all (${rows.length} kept ${rows.length === 1 ? "clip" : "clips"})`}
      width={560}
      dismissible={!busy}
      onClose={onClose}
      footer={
        busy ? (
          <Button disabled={stopping || status.kind === "checking"} onClick={stop}>
            {stopping ? "Stopping…" : "Stop"}
          </Button>
        ) : (
          <>
            <Button onClick={onClose}>Cancel</Button>
            <Button variant="primary" icon={FolderOpen} disabled={chosen.length === 0} onClick={() => void pickFolder()}>
              Choose folder…
            </Button>
          </>
        )
      }
    >
      {rows.length === 0 ? (
        <p>Keep the clips you want first (K on the board), then export them here.</p>
      ) : (
        <ul className="export-rows" aria-label="Clips to export">
          {rows.map((r: ExportRow) => (
            <li key={r.clip.id}>
              <Checkbox
                label={`Clip ${String(r.number).padStart(2, "0")} · ${formatDuration(r.clip.in_ms)}–${formatDuration(r.clip.out_ms)} · ${formatDuration(r.lengthMs)}`}
                checked={ticked.has(r.clip.id)}
                disabled={busy}
                onChange={(on) => toggle(r.clip.id, on)}
              />
              {r.note && (
                <span className="export-note">
                  <AlertTriangle size={12} strokeWidth={2} aria-hidden />
                  {r.note}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="export-options">
        <Checkbox label="Also save subtitles (.srt) for each clip" checked={srt} disabled={busy} onChange={setSrt} />
        <p className="ui-field-hint">
          {chosen.length} {chosen.length === 1 ? "clip" : "clips"}, {formatDuration(totalMs)} in all, about{" "}
          {formatBytes(chosen.reduce((sum, r) => sum + estimateBytes(r.lengthMs), 0))}. Saved as 1080 × 1920 MP4s with the captions
          burned in, named after the video (…-clip-01.mp4). Nothing is replaced without asking.
        </p>
      </div>
      {progress && (
        <div className="export-progress" aria-live="polite">
          <span>{progress.text}</span>
          <ProgressBar value={progress.value} label="Export" />
        </div>
      )}
    </Dialog>
  );
}
