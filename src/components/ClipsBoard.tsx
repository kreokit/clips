import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Check, Film, Info, Pencil, RotateCcw, SlidersHorizontal, X } from "lucide-react";
import {
  Clip,
  ClipFilter,
  ClipSort,
  FEATURE_NAMES,
  FinderSettings,
  Moment,
  boardView,
  clipLength,
  clipRange,
  clipThumbnail,
  countByStatus,
  findMoments,
  findRequest,
  firstLine,
  mergeFound,
  scoreClips,
  topReasons,
  untouched,
  withMoments,
} from "../lib/clips";
import { Project, formatDuration } from "../lib/projects";
import { limiter } from "../lib/queue";
import { normalized } from "../lib/transcription";
import { useProject } from "../store/project";
import { Button, Field, IconButton, NumberField, Popover, ProgressBar, Select, Tabs } from "../ui";
import "../styles/clips.css";

/** Two thumbnails at a time (each is an FFmpeg run). */
const thumbnails = limiter(2);

const SORTS: { value: ClipSort; label: string }[] = [
  { value: "score", label: "Best first" },
  { value: "time", label: "In video order" },
  { value: "length", label: "Longest first" },
];

const COUNTS = ["auto", "5", "10", "15", "20", "30"] as const;

/**
 * The clips board: the moment finder's proposals and the user's own clips as cards with
 * score, length, first line and reasons; keep / discard, filter, sort, "Why this clip",
 * and the finder's settings with "Find again". Finds by itself the first time.
 * Keys: arrows move, K keeps, X discards, U undoes, I explains, Enter edits.
 */
export function ClipsBoard({ project, onEdit }: { project: Project; onEdit?: (id: number) => void }) {
  const update = useProject((s) => s.update);
  const [filter, setFilter] = useState<ClipFilter>("all");
  const [sort, setSort] = useState<ClipSort>("score");
  const [selected, setSelected] = useState<number | null>(null);
  const [finding, setFinding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [why, setWhy] = useState<number | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settingsButton = useRef<HTMLButtonElement>(null);
  const grid = useRef<HTMLDivElement>(null);
  const triedFirst = useRef(false);

  const clips = project.clips;
  const shown = boardView(clips, filter, sort);
  const counts = countByStatus(clips);

  async function find() {
    setFinding(true);
    setError(null);
    try {
      const settings = normalized(project.transcription);
      const finder = project.finder;
      const { avoid, have } = findRequest(clips);
      const found = await findMoments(project.id, settings, finder, avoid, have);
      // Clips that stay are rescored too: the target length may have changed.
      const current = useProject.getState().project?.clips ?? [];
      const staying = current.filter((c) => !untouched(c));
      const scores = await scoreClips(
        project.id,
        settings,
        finder,
        staying.map((c) => [c.in_ms, c.out_ms]),
      );
      const rescored = withMoments(staying, scores);
      const latest = useProject.getState().project;
      if (latest?.id !== project.id) return;
      update({
        clips: mergeFound([...rescored, ...latest.clips.filter(untouched)], found),
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setFinding(false);
    }
  }

  // The first time the board opens on a transcript, it finds by itself.
  useEffect(() => {
    if (triedFirst.current || clips.length > 0) return;
    triedFirst.current = true;
    void find();
    // Only on opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setStatus = (id: number, status: Clip["status"]) =>
    update({
      clips: clips.map((c) => (c.id === id ? { ...c, status: c.status === status ? "proposed" : status } : c)),
    });

  function onKey(e: React.KeyboardEvent) {
    if (e.target !== grid.current && !(e.target as HTMLElement).classList.contains("clip-card")) return;
    const index = shown.findIndex((c) => c.id === selected);
    const columns = grid.current ? getComputedStyle(grid.current).gridTemplateColumns.split(" ").length : 1;
    const move = {
      ArrowRight: 1,
      ArrowLeft: -1,
      ArrowDown: columns,
      ArrowUp: -columns,
    }[e.key];
    if (move !== undefined) {
      e.preventDefault();
      const to = index < 0 ? 0 : Math.min(shown.length - 1, Math.max(0, index + move));
      if (shown[to]) {
        setSelected(shown[to].id);
        grid.current?.querySelector<HTMLElement>(`[data-clip="${shown[to].id}"]`)?.focus();
      }
      return;
    }
    if (selected === null) return;
    const key = e.key.toLowerCase();
    if (key === "k") setStatus(selected, "kept");
    else if (key === "x") setStatus(selected, "discarded");
    else if (key === "u")
      update({
        clips: clips.map((c) => (c.id === selected ? { ...c, status: "proposed" } : c)),
      });
    else if (key === "i") setWhy(selected);
    else if (e.key === "Enter" && onEdit) onEdit(selected);
    else return;
    e.preventDefault();
  }

  const tabs: { value: ClipFilter; label: string }[] = [
    { value: "all", label: `All ${counts.all}` },
    { value: "proposed", label: `To review ${counts.proposed}` },
    { value: "kept", label: `Kept ${counts.kept}` },
    { value: "discarded", label: `Discarded ${counts.discarded}` },
  ];

  return (
    <section className="board" aria-label="Clips">
      <div className="board-bar">
        <Tabs label="Show" value={filter} tabs={tabs} onChange={setFilter} />
        <div className="board-tools">
          <Select label="Sort" value={sort} onChange={setSort} options={SORTS} />
          <Button
            ref={settingsButton}
            variant="secondary"
            size="sm"
            icon={SlidersHorizontal}
            onClick={() => setSettingsOpen((o) => !o)}
            aria-expanded={settingsOpen}
          >
            Find moments
          </Button>
        </div>
      </div>
      <Popover open={settingsOpen} onClose={() => setSettingsOpen(false)} anchor={settingsButton} align="end" className="finder-popover">
        <FinderPanel
          finder={project.finder}
          onChange={(finder) => update({ finder })}
          finding={finding}
          onFind={() => {
            setSettingsOpen(false);
            void find();
          }}
        />
      </Popover>

      {finding && (
        <div className="board-status" aria-live="polite">
          <ProgressBar value={null} label="Finding moments" />
          <span>Finding the best moments…</span>
        </div>
      )}
      {error && <p className="board-error">{error}</p>}
      {!finding && clips.length === 0 && !error && (
        <p className="board-empty">
          No moments between {formatDuration(project.finder.min_ms)} and {formatDuration(project.finder.max_ms)} were found. Try a wider
          range in Find moments.
        </p>
      )}
      {clips.length > 0 && shown.length === 0 && <p className="board-empty">{emptyNote(filter)}</p>}

      <div className="clip-grid" ref={grid} role="listbox" aria-label="Clips" tabIndex={-1} onKeyDown={onKey}>
        {shown.map((c, i) => (
          <ClipCard
            key={c.id}
            clip={c}
            projectId={project.id}
            number={i + 1}
            selected={c.id === selected}
            why={why === c.id}
            onSelect={() => setSelected(c.id)}
            onStatus={(s) => setStatus(c.id, s)}
            onWhy={(open) => setWhy(open ? c.id : null)}
            onEdit={onEdit && (() => onEdit(c.id))}
          />
        ))}
      </div>
    </section>
  );
}

function emptyNote(filter: ClipFilter): string {
  switch (filter) {
    case "kept":
      return "No kept clips yet. Select a clip and press K to keep it.";
    case "discarded":
      return "Nothing discarded.";
    case "proposed":
      return "Everything has been reviewed.";
    default:
      return "";
  }
}

function ClipCard({
  clip,
  projectId,
  number,
  selected,
  why,
  onSelect,
  onStatus,
  onWhy,
  onEdit,
}: {
  clip: Clip;
  projectId: string;
  number: number;
  selected: boolean;
  why: boolean;
  onSelect: () => void;
  onStatus: (status: Clip["status"]) => void;
  onWhy: (open: boolean) => void;
  onEdit?: () => void;
}) {
  const whyButton = useRef<HTMLButtonElement>(null);
  const m = clip.moment;
  const kept = clip.status === "kept";
  const discarded = clip.status === "discarded";
  return (
    <div
      className={["clip-card", selected && "selected", kept && "kept", discarded && "discarded"].filter(Boolean).join(" ")}
      role="option"
      aria-selected={selected}
      aria-label={`Clip ${number}, ${formatDuration(clipLength(clip))}${m ? `, score ${m.score}` : ""}${kept ? ", kept" : discarded ? ", discarded" : ""}`}
      data-clip={clip.id}
      tabIndex={selected ? 0 : -1}
      onClick={onSelect}
      onFocus={onSelect}
      onDoubleClick={onEdit}
    >
      <div className="clip-thumb">
        <ClipThumb projectId={projectId} ms={clip.in_ms + 300} />
        {m && (
          <span className="clip-score tabular" title="Score out of 100">
            {m.score}
          </span>
        )}
        {kept && (
          <span className="clip-kept" title="Kept">
            <Check size={14} strokeWidth={2.5} aria-hidden />
          </span>
        )}
        <span className="clip-length tabular">{formatDuration(clipLength(clip))}</span>
      </div>
      <div className="clip-body">
        <span className="clip-time tabular">
          {clipRange(clip, formatDuration)}
          {clip.source === "manual" && " · added by you"}
          {clip.edited && clip.source === "found" && " · edited"}
        </span>
        <p className="clip-text">{m ? firstLine(m.text, 120) : "No words in this clip."}</p>
        <ul className="clip-reasons">
          {topReasons(m).map((r) => (
            <li key={r.text} className={r.good ? "good" : "bad"}>
              {r.text}
            </li>
          ))}
        </ul>
      </div>
      <div className="clip-actions" onClick={(e) => e.stopPropagation()}>
        {discarded ? (
          <IconButton icon={RotateCcw} label="Restore" shortcut="U" size="sm" onClick={() => onStatus("proposed")} />
        ) : (
          <>
            <IconButton
              icon={Check}
              label={kept ? "Kept (click to undo)" : "Keep"}
              shortcut="K"
              size="sm"
              pressed={kept}
              onClick={() => onStatus("kept")}
            />
            <IconButton icon={X} label="Discard" shortcut="X" size="sm" onClick={() => onStatus("discarded")} />
          </>
        )}
        <span className="clip-actions-gap" />
        {m && (
          <IconButton
            ref={whyButton}
            icon={Info}
            label="Why this clip"
            shortcut="I"
            size="sm"
            aria-expanded={why}
            onClick={() => onWhy(!why)}
          />
        )}
        {onEdit && <IconButton icon={Pencil} label="Edit" shortcut="Enter" size="sm" onClick={onEdit} />}
      </div>
      {m && (
        <Popover open={why} onClose={() => onWhy(false)} anchor={whyButton} align="end" className="why-popover">
          <WhyThisClip moment={m} />
        </Popover>
      )}
    </div>
  );
}

function ClipThumb({ projectId, ms }: { projectId: string; ms: number }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setSrc(null);
    thumbnails(() => (live ? clipThumbnail(projectId, ms) : Promise.reject(new Error("gone")))).then(
      (path) => live && setSrc(convertFileSrc(path)),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [projectId, ms]);
  return (
    <span className="thumb clip-thumb-image">{src ? <img src={src} alt="" /> : <Film size={22} strokeWidth={1.75} aria-hidden />}</span>
  );
}

/** "Why this clip": each feature's share of the score and its reasons. */
export function WhyThisClip({ moment }: { moment: Moment }) {
  return (
    <div className="why">
      <div className="why-head">
        <span className="why-score tabular">{moment.score}</span>
        <span className="why-title">Why this clip</span>
      </div>
      {moment.features.map((f) => (
        <div key={f.kind} className="why-feature">
          <div className="why-row">
            <span className="why-name">{FEATURE_NAMES[f.kind]}</span>
            <span className="why-bar" aria-hidden>
              <span style={{ width: `${Math.round(f.value * 100)}%` }} />
            </span>
            <span className="why-points tabular">{f.weight > 0 ? `${Math.round(f.value * f.weight)} / ${f.weight}` : "—"}</span>
          </div>
          <ul className="why-reasons">
            {f.reasons.map((r) => (
              <li key={r.text} className={r.good ? "good" : "bad"}>
                {r.text}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

/** The finder's settings and "Find again". */
function FinderPanel({
  finder,
  onChange,
  finding,
  onFind,
}: {
  finder: FinderSettings;
  onChange: (f: FinderSettings) => void;
  finding: boolean;
  onFind: () => void;
}) {
  const s = (ms: number) => ms / 1000;
  const set = (patch: Partial<FinderSettings>) => onChange(fixRange({ ...finder, ...patch }));
  return (
    <div className="finder">
      <div className="finder-lengths">
        <NumberField
          label="Shortest"
          value={s(finder.min_ms)}
          min={5}
          max={170}
          step={5}
          decimals={0}
          unit="s"
          onChange={(v) => set({ min_ms: v * 1000 })}
        />
        <NumberField
          label="Target"
          value={s(finder.target_ms)}
          min={5}
          max={180}
          step={5}
          decimals={0}
          unit="s"
          onChange={(v) => set({ target_ms: v * 1000 })}
        />
        <NumberField
          label="Longest"
          value={s(finder.max_ms)}
          min={10}
          max={180}
          step={5}
          decimals={0}
          unit="s"
          onChange={(v) => set({ max_ms: v * 1000 })}
        />
      </div>
      <Field label="How many">
        <Select
          label="How many"
          value={finder.count === null ? "auto" : (String(finder.count) as (typeof COUNTS)[number])}
          onChange={(v) => set({ count: v === "auto" ? null : Number(v) })}
          options={COUNTS.map((c) => ({
            value: c,
            label: c === "auto" ? "About one per 2 minutes" : `${c} clips`,
          }))}
        />
      </Field>
      <p className="finder-note">Find again replaces the proposals you haven't touched. Kept, discarded and edited clips stay.</p>
      <div className="panel-actions">
        <Button variant="primary" onClick={onFind} disabled={finding}>
          Find again
        </Button>
      </div>
    </div>
  );
}

/** Keeps shortest ≤ target ≤ longest, the longest at least 5 s over the shortest. */
export function fixRange(f: FinderSettings): FinderSettings {
  const min_ms = Math.min(f.min_ms, 175_000);
  const max_ms = Math.max(f.max_ms, min_ms + 5_000);
  const target_ms = Math.min(Math.max(f.target_ms, min_ms), max_ms);
  return { ...f, min_ms, max_ms, target_ms };
}
