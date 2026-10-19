import { ArrowLeft } from "lucide-react";
import { describeShape, fileName, formatBytes, formatDuration, formatFps } from "../lib/projects";
import { closeProject, useProject } from "../store/project";
import { Button, TextInput } from "../ui";
import { Thumbnail } from "./Thumbnail";
import { TopBar } from "./TopBar";
import "../styles/project.css";

/**
 * The open project. Milestone 1 shows what the probe found; transcription and the clips
 * board replace this in milestones 2 and 3.
 */
export function ProjectView() {
  const project = useProject((s) => s.project);
  const update = useProject((s) => s.update);
  if (!project) return null;
  const { video } = project;
  const home = () => void closeProject();

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
            <p className="project-next">Next: transcribe the video and find the best moments (coming in the next build).</p>
          </div>
        </div>
      </main>
    </div>
  );
}
