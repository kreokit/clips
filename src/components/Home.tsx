import { useCallback, useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Ellipsis, FolderOpen, FolderSearch, Trash2, TriangleAlert, Upload } from "lucide-react";
import { ProjectEntry, formatDuration, isVideoPath, listProjects, relativeTime, removeProject } from "../lib/projects";
import { locateAndOpen, openProject, openVideoPath } from "../store/project";
import { Button, Dialog, Menu, MenuEntry, toast, useContextMenu } from "../ui";
import { Thumbnail } from "./Thumbnail";
import { TopBar, pickVideo } from "./TopBar";
import "../styles/home.css";

/** Home: drop or choose a long video, or reopen a recent project. */
export function Home() {
  const [projects, setProjects] = useState<ProjectEntry[] | null>(null);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [removing, setRemoving] = useState<ProjectEntry | null>(null);

  const refresh = useCallback(() => {
    listProjects().then(setProjects, () => setProjects([]));
  }, []);
  useEffect(refresh, [refresh]);

  const run = useCallback(
    async (work: () => Promise<boolean>) => {
      setBusy(true);
      try {
        if (!(await work())) refresh();
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  // Dropping a video opens it; Clips works on one video at a time.
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const { type } = event.payload;
      if (type === "enter" || type === "over") return setDragging(true);
      setDragging(false);
      if (type !== "drop") return;
      const videos = event.payload.paths.filter(isVideoPath);
      if (videos.length === 0) return toast("That isn't a video file. Try MP4, MOV, MKV or WebM.", "error");
      if (videos.length > 1) toast("Opened the first video. KreoKit Clips works on one video at a time.");
      void run(() => openVideoPath(videos[0]));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [run]);

  const choose = () =>
    run(async () => {
      const path = await pickVideo();
      return path ? openVideoPath(path) : true;
    });

  const locate = (entry: ProjectEntry) =>
    run(async () => {
      const path = await pickVideo(`Locate “${entry.name}”`);
      return path ? locateAndOpen(entry.id, path) : true;
    });

  async function confirmRemove() {
    if (!removing) return;
    try {
      await removeProject(removing.id);
    } catch (e) {
      toast(String(e), "error");
    }
    setRemoving(null);
    refresh();
  }

  return (
    <div className="screen home">
      <TopBar
        title={
          <span className="home-brand" data-tauri-drag-region>
            KreoKit Clips
          </span>
        }
      />

      <main className="home-main">
        <section className={dragging ? "dropzone dragging" : "dropzone"} aria-label="Add a video">
          <div className="dropzone-icon">
            <Upload size={22} strokeWidth={1.75} aria-hidden />
          </div>
          <h1 className="dropzone-title">{dragging ? "Drop to start" : "Drop a long video to make short clips"}</h1>
          <p className="dropzone-text">
            A podcast, interview, talk or livestream. Everything runs on this computer; your video never leaves it.
          </p>
          <Button variant="primary" onClick={() => void choose()} disabled={busy}>
            Choose video
          </Button>
          <p className="dropzone-formats">MP4 · MOV · MKV · WebM</p>
        </section>

        <section className="recent" aria-labelledby="recent-title">
          <h2 id="recent-title" className="section-title">
            Recent projects
          </h2>
          {projects === null ? null : projects.length === 0 ? (
            <p className="recent-empty">Videos you open appear here. Projects save automatically.</p>
          ) : (
            <div className="recent-grid">
              {projects.map((entry) => (
                <ProjectCard
                  key={entry.id}
                  entry={entry}
                  disabled={busy}
                  onOpen={() => void run(() => openProject(entry.id))}
                  onLocate={() => void locate(entry)}
                  onDelete={() => setRemoving(entry)}
                />
              ))}
            </div>
          )}
        </section>
      </main>

      {removing && <DeleteProjectDialog name={removing.name} onCancel={() => setRemoving(null)} onDelete={() => void confirmRemove()} />}
    </div>
  );
}

/** "Delete this project?", on Home and in the project's menu. */
export function DeleteProjectDialog({ name, onCancel, onDelete }: { name: string; onCancel: () => void; onDelete: () => void }) {
  return (
    <Dialog
      title="Delete this project?"
      onClose={onCancel}
      width={440}
      footer={
        <>
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="danger" icon={Trash2} onClick={onDelete}>
            Delete project
          </Button>
        </>
      }
    >
      <p className="dialog-text">Your video file is not touched.</p>
      <p className="dialog-text">
        “{name}” and its clips are deleted. Its transcript is kept, so opening the video again doesn't transcribe it again.
      </p>
    </Dialog>
  );
}

function ProjectCard({
  entry,
  disabled,
  onOpen,
  onLocate,
  onDelete,
}: {
  entry: ProjectEntry;
  disabled: boolean;
  onOpen: () => void;
  onLocate: () => void;
  onDelete: () => void;
}) {
  const items = (): MenuEntry[] => [
    entry.video_exists
      ? { label: "Open", icon: FolderOpen, onSelect: onOpen }
      : { label: "Locate video…", icon: FolderSearch, onSelect: onLocate },
    "separator",
    { label: "Delete project…", icon: Trash2, danger: true, onSelect: onDelete },
  ];
  const { onContextMenu, menu } = useContextMenu(items, "Project actions");
  return (
    <div className="recent-item">
      <button
        type="button"
        className={entry.video_exists ? "recent-card" : "recent-card missing"}
        onClick={entry.video_exists ? onOpen : onLocate}
        onContextMenu={onContextMenu}
        disabled={disabled}
        title={entry.video_path}
      >
        <Thumbnail id={entry.id} className="recent-thumb" />
        <span className="recent-info">
          <span className="recent-name">{entry.name}</span>
          {entry.video_exists ? (
            <span className="recent-meta">
              {formatDuration(entry.video.duration_ms)} · {relativeTime(entry.opened_at, Date.now())}
            </span>
          ) : (
            <span className="recent-missing">
              <TriangleAlert size={12} strokeWidth={2} aria-hidden /> Video moved · Locate video…
            </span>
          )}
        </span>
      </button>
      <Menu
        buttonLabel={`Actions for “${entry.name}”`}
        icon={Ellipsis}
        variant="secondary"
        align="end"
        items={items()}
        className="recent-more"
      />
      {menu}
    </div>
  );
}
