import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, RotateCcw, Trash2 } from "lucide-react";
import { applyScores, forgetSpokenText, scoreClips } from "../lib/clips";
import { faceQueue } from "../lib/faces";
import { describeShape, discardTranscript, fileName, formatBytes, formatDuration, formatFps } from "../lib/projects";
import { closeProject, deleteOpenProject, useProject } from "../store/project";
import { Transcript, cancelJob, normalized } from "../lib/transcription";
import { Button, Dialog, MenuEntry, Tabs, TextInput, toast } from "../ui";
import { ClipEditor } from "./ClipEditor";
import { ClipsBoard } from "./ClipsBoard";
import { DeleteProjectDialog } from "./Home";
import { Thumbnail } from "./Thumbnail";
import { TranscribePanel } from "./TranscribePanel";
import { TopBar } from "./TopBar";
import "../styles/project.css";

type Tab = "clips" | "transcript";

/** Scores every clip again for a new transcript (times stay, words and scores change). */
async function rescoreAll() {
  const p = useProject.getState().project;
  if (!p || p.clips.length === 0) return;
  try {
    const ranges = p.clips.map((c): [number, number] => [c.in_ms, c.out_ms]);
    const moments = await scoreClips(p.id, normalized(p.transcription), p.finder, ranges);
    const latest = useProject.getState().project;
    if (latest?.id === p.id) useProject.getState().update({ clips: applyScores(latest.clips, p.clips, moments) });
  } catch (e) {
    toast(`Couldn't score the clips again: ${String(e)}`, "error");
  }
}

/**
 * The open project. Until there is a transcript: the video's details and the Transcribe
 * step. Then two tabs: Clips (the board) and Transcript (details and Transcribe again).
 */
export function ProjectView() {
  const project = useProject((s) => s.project);
  const update = useProject((s) => s.update);
  const [running, setRunning] = useState(false);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [transcript, setTranscript] = useState<Transcript | null>(null);
  const [tab, setTab] = useState<Tab>("clips");
  // The clip being edited, and the board's order for previous / next.
  const [editing, setEditing] = useState<{ id: number; order: number[] } | null>(null);
  const [confirm, setConfirm] = useState<"delete" | "transcribe" | null>(null);
  // A new key remounts the Transcribe panel, so it reads the (now missing) transcript.
  const [panelKey, setPanelKey] = useState(0);
  const onRunningChange = useCallback((r: boolean) => setRunning(r), []);
  // Development aid: KREOKIT_DEV_CLIP opens that clip in the editor once.
  useEffect(() => {
    invoke<number | null>("dev_startup_clip").then((id) => {
      const p = useProject.getState().project;
      if (id !== null && p?.clips.some((c) => c.id === id)) setEditing({ id, order: p.clips.map((c) => c.id) });
    });
  }, []);
  // Leaving a project stops its face analysis (finished blocks stay cached).
  const projectId = project?.id;
  useEffect(
    () => () => {
      if (projectId) faceQueue.clear(projectId);
    },
    [projectId],
  );
  const onTranscript = useCallback((t: Transcript | null, fresh: boolean) => {
    setTranscript(t);
    if (t && fresh) {
      // The editor must read the new words, not the last transcript's.
      const p = useProject.getState().project;
      if (p) forgetSpokenText(p.id);
      setTab("clips");
      void rescoreAll();
    }
  }, []);
  if (!project) return null;
  const board = transcript !== null && !running;
  const showBoard = board && tab === "clips";
  const { video } = project;
  // Leaving while transcribing asks first; finished parts are kept either way.
  const home = () => (running ? setConfirmLeave(true) : void closeProject());
  const leave = async () => {
    setConfirmLeave(false);
    await cancelJob();
    void closeProject();
  };
  const projectItems: MenuEntry[] = [
    {
      label: "Transcribe again…",
      icon: RotateCcw,
      disabled: running || transcript === null,
      onSelect: () => setConfirm("transcribe"),
    },
    { label: "Delete project…", icon: Trash2, danger: true, disabled: running, onSelect: () => setConfirm("delete") },
  ];
  const remove = async () => {
    setConfirm(null);
    try {
      await deleteOpenProject();
    } catch (e) {
      toast(`Couldn't delete the project: ${String(e)}`, "error");
    }
  };
  const transcribeAgain = async () => {
    setConfirm(null);
    try {
      await discardTranscript(project.id);
    } catch (e) {
      return toast(`Couldn't remove the transcript: ${String(e)}`, "error");
    }
    forgetSpokenText(project.id);
    // Fixes name words of the old transcript; the dialog said they'd be lost.
    update({ word_fixes: [] });
    setEditing(null);
    setTranscript(null);
    setPanelKey((k) => k + 1);
  };

  return (
    <div className="screen">
      <TopBar
        title={
          <>
            <Button variant="ghost" size="sm" icon={ArrowLeft} onClick={home}>
              Home
            </Button>
            <span className="topbar-name" data-tauri-drag-region>
              {project.name}
            </span>
          </>
        }
        onHome={home}
        projectItems={projectItems}
      />
      {board && editing && (
        <ClipEditor
          project={project}
          clipId={editing.id}
          order={editing.order}
          onOpen={(id, order) => setEditing({ id, order: order ?? editing.order })}
          onClose={() => setEditing(null)}
        />
      )}
      <main className="project-main" hidden={Boolean(board && editing)}>
        {board && (
          <div className="project-tabs">
            <Tabs
              label="Project"
              value={tab}
              onChange={setTab}
              tabs={[
                { value: "clips", label: "Clips" },
                { value: "transcript", label: "Transcript" },
              ]}
            />
          </div>
        )}
        {showBoard && <ClipsBoard project={project} onEdit={(id, order) => setEditing({ id, order })} />}
        {/* Kept mounted on the Clips tab: it owns a running transcription. */}
        <div className="project-steps" hidden={showBoard}>
          <div className="project-card">
            <Thumbnail id={project.id} className="project-thumb" />
            <div className="project-details">
              <label className="project-name-field">
                <span className="section-title">Project name</span>
                <TextInput
                  value={project.name}
                  onChange={(e) => update({ name: e.target.value })}
                  onBlur={(e) => !e.target.value.trim() && update({ name: fileName(project.video_path).replace(/\.[^.]+$/, "") })}
                  spellCheck={false}
                  aria-label="Project name"
                />
              </label>
              <dl className="project-facts">
                <dt>Length</dt>
                <dd>{formatDuration(video.duration_ms)}</dd>
                <dt>Size</dt>
                <dd>
                  {video.width} × {video.height} · {describeShape(video.width, video.height)}
                </dd>
                <dt>Frame rate</dt>
                <dd>{formatFps(video.fps)}</dd>
                <dt>File</dt>
                <dd className="project-file" title={project.video_path}>
                  {fileName(project.video_path)} · {formatBytes(project.video_size)}
                </dd>
              </dl>
            </div>
          </div>
          <TranscribePanel key={panelKey} project={project} onRunningChange={onRunningChange} onTranscript={onTranscript} />
        </div>
      </main>
      {confirm === "delete" && <DeleteProjectDialog name={project.name} onCancel={() => setConfirm(null)} onDelete={() => void remove()} />}
      {confirm === "transcribe" && (
        <Dialog
          title="Transcribe this video again?"
          onClose={() => setConfirm(null)}
          width={440}
          footer={
            <>
              <Button variant="secondary" onClick={() => setConfirm(null)}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => void transcribeAgain()}>
                Transcribe again
              </Button>
            </>
          }
        >
          <p className="dialog-text">
            The current transcript is deleted and you can choose the settings for a new one. Your clips stay; they're scored again
            when the new transcript is done.
            {project.word_fixes.length > 0 && (
              <> Your {project.word_fixes.length === 1 ? "word fix" : `${project.word_fixes.length} word fixes`} in the captions will be lost.</>
            )}
          </p>
        </Dialog>
      )}
      {confirmLeave && (
        <Dialog
          title="Stop transcribing?"
          onClose={() => setConfirmLeave(false)}
          footer={
            <>
              <Button variant="secondary" onClick={() => setConfirmLeave(false)}>
                Keep going
              </Button>
              <Button variant="primary" onClick={() => void leave()}>
                Stop and go Home
              </Button>
            </>
          }
        >
          <p>The text finished so far is kept. Open this video again and choose Resume to carry on.</p>
        </Dialog>
      )}
    </div>
  );
}
