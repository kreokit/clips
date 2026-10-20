import { useCallback, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { describeShape, fileName, formatBytes, formatDuration, formatFps } from "../lib/projects";
import { closeProject, useProject } from "../store/project";
import { cancelJob } from "../lib/transcription";
import { Button, Dialog, TextInput } from "../ui";
import { Thumbnail } from "./Thumbnail";
import { TranscribePanel } from "./TranscribePanel";
import { TopBar } from "./TopBar";
import "../styles/project.css";

/**
 * The open project: what the probe found, then the Processing steps (transcription now;
 * the moment finder and clips board come with milestone 3).
 */
export function ProjectView() {
  const project = useProject((s) => s.project);
  const update = useProject((s) => s.update);
  const [running, setRunning] = useState(false);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const onRunningChange = useCallback((r: boolean) => setRunning(r), []);
  if (!project) return null;
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
        <TranscribePanel project={project} onRunningChange={onRunningChange} />
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
