import { useCallback, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { applyScores, scoreClips } from "../lib/clips";
import { describeShape, fileName, formatBytes, formatDuration, formatFps } from "../lib/projects";
import { closeProject, useProject } from "../store/project";
import { Transcript, cancelJob, normalized } from "../lib/transcription";
import { Button, Dialog, Tabs, TextInput, toast } from "../ui";
import { ClipsBoard } from "./ClipsBoard";
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
  const onRunningChange = useCallback((r: boolean) => setRunning(r), []);
  const onTranscript = useCallback((t: Transcript | null, fresh: boolean) => {
    setTranscript(t);
    if (t && fresh) {
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
      />
      <main className="project-main">
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
        {showBoard && <ClipsBoard project={project} />}
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
          <TranscribePanel project={project} onRunningChange={onRunningChange} onTranscript={onTranscript} />
        </div>
      </main>
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
